//! The stack machine itself: memory, stacks, the fetch/decode/execute loop, and
//! the frame-vector runner. All runtime errors are *trapped* into `fault` (and
//! halt the machine) rather than panicking, so a buggy ROM the model wrote can
//! be observed and debugged instead of taking the process down.

use super::device::Devices;
use super::isa::Op;

/// Flat memory size (64 KiB). `pc` and all addresses are `u16`, so memory
/// accesses can never be out of range.
pub const MEM_SIZE: usize = 0x1_0000;
/// Depth of each stack, in `u16` cells.
pub const STACK_SIZE: usize = 256;
/// ROMs load here and the reset vector starts here (Uxn-style).
pub const ROM_ORIGIN: u16 = 0x0100;
/// Sentinel return address pushed before calling a vector; when a matching `RET`
/// pops it back into `pc`, the runner knows the vector finished.
pub const FRAME_DONE: u16 = 0xFFFF;

/// Outcome of running a vector to completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// The vector returned normally.
    Completed,
    /// The machine halted (either `HALT`/`system.halt`, or a trapped fault).
    Halted,
    /// The per-call cycle cap was reached before the vector returned.
    CapExceeded,
}

/// A small fixed stack of `u16` cells with under/overflow reported as faults.
#[derive(Clone)]
struct Stack {
    cells: [u16; STACK_SIZE],
    sp: usize,
}

impl Stack {
    fn new() -> Self {
        Stack {
            cells: [0; STACK_SIZE],
            sp: 0,
        }
    }
}

#[derive(Clone)]
pub struct Vm {
    pub mem: Vec<u8>,
    data: Stack,
    ret: Stack,
    pub pc: u16,
    /// Total instructions executed since power-on.
    pub cycle: u64,
    pub halted: bool,
    /// Set on a trapped runtime error; also halts the machine.
    pub fault: Option<String>,
    pub devices: Devices,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    pub fn new() -> Self {
        Vm {
            mem: vec![0u8; MEM_SIZE],
            data: Stack::new(),
            ret: Stack::new(),
            pc: ROM_ORIGIN,
            cycle: 0,
            halted: false,
            fault: None,
            devices: Devices::new(),
        }
    }

    /// A snapshot of the visible data stack (bottom→top), for observation.
    pub fn data_stack(&self) -> Vec<u16> {
        self.data.cells[..self.data.sp].to_vec()
    }

    pub fn return_stack_depth(&self) -> usize {
        self.ret.sp
    }

    /// Load ROM bytes at `ROM_ORIGIN` and run the reset vector once (init).
    /// Clears any previous fault/halt but keeps the caller's chosen memory
    /// origin clean by zeroing memory first.
    pub fn load_rom(&mut self, rom: &[u8]) -> RunOutcome {
        for b in self.mem.iter_mut() {
            *b = 0;
        }
        let end = (ROM_ORIGIN as usize + rom.len()).min(MEM_SIZE);
        self.mem[ROM_ORIGIN as usize..end].copy_from_slice(&rom[..end - ROM_ORIGIN as usize]);
        self.data.sp = 0;
        self.ret.sp = 0;
        self.pc = ROM_ORIGIN;
        self.cycle = 0;
        self.halted = false;
        self.fault = None;
        self.run_vector(ROM_ORIGIN, cap())
    }

    /// Run the installed frame vector for one frame with `buttons` held.
    pub fn run_frame(&mut self, buttons: u8, cap_cycles: u64) -> RunOutcome {
        self.devices.begin_frame(buttons);
        let vector = self.devices.frame_vector;
        if vector == 0 || self.halted {
            return RunOutcome::Completed;
        }
        self.run_vector(vector, cap_cycles)
    }

    /// Push `entry` as a call, then step until the matching `RET` pops the
    /// sentinel back into `pc`, the machine halts, or the cap is hit.
    fn run_vector(&mut self, entry: u16, cap_cycles: u64) -> RunOutcome {
        if self.halted {
            return RunOutcome::Halted;
        }
        // Set up the sentinel frame.
        if !self.rpush(FRAME_DONE) {
            return RunOutcome::Halted;
        }
        self.pc = entry;
        let mut local: u64 = 0;
        loop {
            if self.halted {
                return RunOutcome::Halted;
            }
            if self.pc == FRAME_DONE {
                return RunOutcome::Completed;
            }
            if local >= cap_cycles {
                return RunOutcome::CapExceeded;
            }
            self.step();
            local += 1;
        }
    }

    /// Free-run up to `n` instructions (sub-frame debugging). Stops early on
    /// halt or when `pc` reaches the frame sentinel. Returns instructions run.
    pub fn run_cycles(&mut self, n: u64) -> u64 {
        let mut ran = 0;
        while ran < n && !self.halted && self.pc != FRAME_DONE {
            self.step();
            ran += 1;
        }
        ran
    }

    /// Execute a single instruction. No-op if halted.
    pub fn step(&mut self) {
        if self.halted {
            return;
        }
        let byte = self.mem[self.pc as usize];
        let op = match Op::from_byte(byte) {
            Some(op) => op,
            None => return self.trap(format!("illegal opcode 0x{byte:02X} at 0x{:04X}", self.pc)),
        };
        self.pc = self.pc.wrapping_add(1);
        self.cycle += 1;
        self.exec(op);
    }

    fn exec(&mut self, op: Op) {
        use Op::*;
        match op {
            Nop => {}
            Halt => {
                self.halted = true;
            }
            Lit8 => {
                let v = self.fetch8();
                self.push(v as u16);
            }
            Lit16 => {
                let hi = self.fetch8();
                let lo = self.fetch8();
                self.push(u16::from_be_bytes([hi, lo]));
            }
            Dup => {
                if let Some(a) = self.peek(0) {
                    self.push(a);
                }
            }
            Drop => {
                self.pop();
            }
            Swap => {
                if let (Some(b), Some(a)) = (self.pop(), self.pop()) {
                    self.push(b);
                    self.push(a);
                }
            }
            Over => {
                if let Some(a) = self.peek(1) {
                    self.push(a);
                }
            }
            Rot => {
                // ( a b c -- b c a )
                if let (Some(c), Some(b), Some(a)) = (self.pop(), self.pop(), self.pop()) {
                    self.push(b);
                    self.push(c);
                    self.push(a);
                }
            }
            Add => self.binop(|a, b| Some(a.wrapping_add(b))),
            Sub => self.binop(|a, b| Some(a.wrapping_sub(b))),
            Mul => self.binop(|a, b| Some(a.wrapping_mul(b))),
            Div => self.binop(|a, b| if b == 0 { None } else { Some(a / b) }),
            Mod => self.binop(|a, b| if b == 0 { None } else { Some(a % b) }),
            And => self.binop(|a, b| Some(a & b)),
            Or => self.binop(|a, b| Some(a | b)),
            Xor => self.binop(|a, b| Some(a ^ b)),
            Shl => self.binop(|a, b| Some(a.wrapping_shl(b as u32))),
            Shr => self.binop(|a, b| Some(a.wrapping_shr(b as u32))),
            Eq => self.binop(|a, b| Some((a == b) as u16)),
            Ne => self.binop(|a, b| Some((a != b) as u16)),
            Lt => self.binop(|a, b| Some((a < b) as u16)),
            Gt => self.binop(|a, b| Some((a > b) as u16)),
            Load8 => {
                if let Some(addr) = self.pop() {
                    self.push(self.mem[addr as usize] as u16);
                }
            }
            Load16 => {
                if let Some(addr) = self.pop() {
                    let hi = self.mem[addr as usize];
                    let lo = self.mem[addr.wrapping_add(1) as usize];
                    self.push(u16::from_be_bytes([hi, lo]));
                }
            }
            Store8 => {
                // ( val addr -- )
                if let (Some(addr), Some(val)) = (self.pop(), self.pop()) {
                    self.mem[addr as usize] = val as u8;
                }
            }
            Store16 => {
                if let (Some(addr), Some(val)) = (self.pop(), self.pop()) {
                    let [hi, lo] = val.to_be_bytes();
                    self.mem[addr as usize] = hi;
                    self.mem[addr.wrapping_add(1) as usize] = lo;
                }
            }
            Jmp => {
                if let Some(addr) = self.pop() {
                    self.pc = addr;
                }
            }
            Jz => {
                // ( cond addr -- )
                if let (Some(addr), Some(cond)) = (self.pop(), self.pop()) {
                    if cond == 0 {
                        self.pc = addr;
                    }
                }
            }
            Jnz => {
                if let (Some(addr), Some(cond)) = (self.pop(), self.pop()) {
                    if cond != 0 {
                        self.pc = addr;
                    }
                }
            }
            Call => {
                if let Some(addr) = self.pop() {
                    if self.rpush(self.pc) {
                        self.pc = addr;
                    }
                }
            }
            Ret => {
                if let Some(addr) = self.rpop() {
                    self.pc = addr;
                }
            }
            Dei => {
                if let Some(port) = self.pop() {
                    let v = self.devices.read(port as u8);
                    self.push(v);
                }
            }
            Deo => {
                // ( val port -- )
                if let (Some(port), Some(val)) = (self.pop(), self.pop()) {
                    self.devices.write(port as u8, val, &self.mem);
                    if self.devices.halt_requested {
                        self.halted = true;
                    }
                }
            }
        }
    }

    // ---- helpers ----

    fn fetch8(&mut self) -> u8 {
        let b = self.mem[self.pc as usize];
        self.pc = self.pc.wrapping_add(1);
        b
    }

    fn binop(&mut self, f: impl FnOnce(u16, u16) -> Option<u16>) {
        // ( a b -- f(a,b) ); b is the top of stack.
        if let (Some(b), Some(a)) = (self.pop(), self.pop()) {
            match f(a, b) {
                Some(r) => self.push(r),
                None => self.trap(format!("arithmetic error at 0x{:04X}", self.pc)),
            }
        }
    }

    fn push(&mut self, v: u16) {
        if self.data.sp >= STACK_SIZE {
            return self.trap("data stack overflow".into());
        }
        self.data.cells[self.data.sp] = v;
        self.data.sp += 1;
    }

    fn pop(&mut self) -> Option<u16> {
        if self.data.sp == 0 {
            self.trap("data stack underflow".into());
            return None;
        }
        self.data.sp -= 1;
        Some(self.data.cells[self.data.sp])
    }

    /// Peek `depth` below the top (0 = top). None (and a fault) if too shallow.
    fn peek(&mut self, depth: usize) -> Option<u16> {
        if self.data.sp <= depth {
            self.trap("data stack underflow".into());
            return None;
        }
        Some(self.data.cells[self.data.sp - 1 - depth])
    }

    fn rpush(&mut self, v: u16) -> bool {
        if self.ret.sp >= STACK_SIZE {
            self.trap("return stack overflow".into());
            return false;
        }
        self.ret.cells[self.ret.sp] = v;
        self.ret.sp += 1;
        true
    }

    fn rpop(&mut self) -> Option<u16> {
        if self.ret.sp == 0 {
            self.trap("return stack underflow".into());
            return None;
        }
        self.ret.sp -= 1;
        Some(self.ret.cells[self.ret.sp])
    }

    fn trap(&mut self, msg: String) {
        if self.fault.is_none() {
            self.fault = Some(msg);
        }
        self.halted = true;
    }
}

/// Default per-frame / per-vector instruction cap.
pub fn cap() -> u64 {
    200_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::isa::Op;

    /// Build a ROM from opcodes/bytes and run it to completion (reset vector).
    fn run(bytes: &[u8]) -> Vm {
        let mut vm = Vm::new();
        vm.load_rom(bytes);
        vm
    }

    fn lit8(v: u8) -> [u8; 2] {
        [Op::Lit8 as u8, v]
    }

    #[test]
    fn arithmetic_and_return() {
        // 2 3 ADD  -> 5 ; then RET (pops sentinel, vector completes)
        let mut rom = vec![];
        rom.extend(lit8(2));
        rom.extend(lit8(3));
        rom.push(Op::Add as u8);
        rom.push(Op::Ret as u8);
        let vm = run(&rom);
        assert_eq!(vm.data_stack(), vec![5]);
        assert!(vm.fault.is_none());
    }

    #[test]
    fn sub_order_is_a_minus_b() {
        let mut rom = vec![];
        rom.extend(lit8(10));
        rom.extend(lit8(3));
        rom.push(Op::Sub as u8);
        rom.push(Op::Ret as u8);
        assert_eq!(run(&rom).data_stack(), vec![7]);
    }

    #[test]
    fn div_by_zero_traps_not_panics() {
        let mut rom = vec![];
        rom.extend(lit8(5));
        rom.extend(lit8(0));
        rom.push(Op::Div as u8);
        rom.push(Op::Ret as u8);
        let vm = run(&rom);
        assert!(vm.halted);
        assert!(vm.fault.as_deref().unwrap().contains("arithmetic"));
    }

    #[test]
    fn stack_underflow_traps() {
        let rom = vec![Op::Add as u8]; // nothing on the stack
        let vm = run(&rom);
        assert!(vm.fault.as_deref().unwrap().contains("underflow"));
    }

    #[test]
    fn lit16_is_big_endian() {
        let rom = vec![Op::Lit16 as u8, 0x12, 0x34, Op::Ret as u8];
        assert_eq!(run(&rom).data_stack(), vec![0x1234]);
    }

    #[test]
    fn store_and_load16_roundtrip() {
        // value 0xBEEF, addr 0x40: STORE16 then LOAD16
        let mut rom = vec![];
        rom.push(Op::Lit16 as u8);
        rom.extend_from_slice(&0xBEEFu16.to_be_bytes());
        rom.extend(lit8(0x40));
        rom.push(Op::Store16 as u8);
        rom.extend(lit8(0x40));
        rom.push(Op::Load16 as u8);
        rom.push(Op::Ret as u8);
        assert_eq!(run(&rom).data_stack(), vec![0xBEEF]);
    }

    #[test]
    fn jz_taken_and_call_ret() {
        // 0 <target> JZ  ... target: 42 ; skipping the 99 in between
        // Layout (origin 0x0100):
        //   LIT8 0            @0100
        //   LIT16 <target>    @0102
        //   JZ                @0105
        //   LIT8 99           @0106   (skipped)
        //   RET               @0108
        // target:
        //   LIT8 42           @0109
        //   RET               @010B
        let target: u16 = 0x0109;
        let mut rom = vec![];
        rom.extend(lit8(0)); // cond 0
        rom.push(Op::Lit16 as u8);
        rom.extend_from_slice(&target.to_be_bytes());
        rom.push(Op::Jz as u8);
        rom.extend(lit8(99));
        rom.push(Op::Ret as u8);
        rom.extend(lit8(42));
        rom.push(Op::Ret as u8);
        assert_eq!(run(&rom).data_stack(), vec![42]);
    }

    #[test]
    fn halt_stops_execution() {
        let mut rom = vec![];
        rom.extend(lit8(1));
        rom.push(Op::Halt as u8);
        rom.extend(lit8(2)); // never runs
        let vm = run(&rom);
        assert!(vm.halted);
        assert_eq!(vm.data_stack(), vec![1]);
    }

    #[test]
    fn frame_cap_is_enforced() {
        // Infinite loop: LIT16 0x0100 JMP  — reset vector never returns.
        let mut vm = Vm::new();
        let mut rom = vec![];
        rom.push(Op::Lit16 as u8);
        rom.extend_from_slice(&0x0100u16.to_be_bytes());
        rom.push(Op::Jmp as u8);
        // load_rom runs the reset vector with the default cap; expect CapExceeded.
        let outcome = vm.load_rom(&rom);
        assert_eq!(outcome, RunOutcome::CapExceeded);
        assert_eq!(vm.cycle, cap());
    }
}
