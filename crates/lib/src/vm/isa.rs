//! Instruction set for the tiny fantasy-console stack VM.
//!
//! A 16-bit stack machine in the spirit of Uxn: one-byte opcodes, a data stack
//! and a return stack of `u16` cells, a flat 64 KiB memory, and device I/O via
//! `DEI`/`DEO`. Two immediate-carrying ops (`LIT8`/`LIT16`) read their operand
//! from the bytes following the opcode; everything else is operand-free and
//! works on the stacks.

/// A decoded opcode. The `u8` discriminants are the on-ROM encoding — keep them
/// stable, the assembler and disassembler both depend on these exact values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    Nop = 0x00,
    Halt = 0x01,
    /// Push the next 1 byte (zero-extended to u16).
    Lit8 = 0x02,
    /// Push the next 2 bytes, big-endian.
    Lit16 = 0x03,

    // ---- stack shuffling ----
    Dup = 0x10,
    Drop = 0x11,
    Swap = 0x12,
    Over = 0x13,
    Rot = 0x14,

    // ---- arithmetic (wrapping u16) ----
    Add = 0x20,
    Sub = 0x21,
    Mul = 0x22,
    Div = 0x23,
    Mod = 0x24,

    // ---- bitwise ----
    And = 0x30,
    Or = 0x31,
    Xor = 0x32,
    Shl = 0x33,
    Shr = 0x34,

    // ---- comparison (push 1/0) ----
    Eq = 0x40,
    Ne = 0x41,
    Lt = 0x42,
    Gt = 0x43,

    // ---- memory ----
    Load8 = 0x50,
    Load16 = 0x51,
    Store8 = 0x52,
    Store16 = 0x53,

    // ---- control flow ----
    Jmp = 0x60,
    Jz = 0x61,
    Jnz = 0x62,
    Call = 0x63,
    Ret = 0x64,

    // ---- device I/O ----
    Dei = 0x70,
    Deo = 0x71,
}

impl Op {
    /// Decode a byte into an opcode, or `None` for an illegal encoding.
    pub fn from_byte(b: u8) -> Option<Op> {
        use Op::*;
        Some(match b {
            0x00 => Nop,
            0x01 => Halt,
            0x02 => Lit8,
            0x03 => Lit16,
            0x10 => Dup,
            0x11 => Drop,
            0x12 => Swap,
            0x13 => Over,
            0x14 => Rot,
            0x20 => Add,
            0x21 => Sub,
            0x22 => Mul,
            0x23 => Div,
            0x24 => Mod,
            0x30 => And,
            0x31 => Or,
            0x32 => Xor,
            0x33 => Shl,
            0x34 => Shr,
            0x40 => Eq,
            0x41 => Ne,
            0x42 => Lt,
            0x43 => Gt,
            0x50 => Load8,
            0x51 => Load16,
            0x52 => Store8,
            0x53 => Store16,
            0x60 => Jmp,
            0x61 => Jz,
            0x62 => Jnz,
            0x63 => Call,
            0x64 => Ret,
            0x70 => Dei,
            0x71 => Deo,
            _ => return None,
        })
    }

    /// The canonical mnemonic (uppercase) as accepted by the assembler.
    pub fn mnemonic(self) -> &'static str {
        use Op::*;
        match self {
            Nop => "NOP",
            Halt => "HALT",
            Lit8 => "LIT8",
            Lit16 => "LIT16",
            Dup => "DUP",
            Drop => "DROP",
            Swap => "SWAP",
            Over => "OVER",
            Rot => "ROT",
            Add => "ADD",
            Sub => "SUB",
            Mul => "MUL",
            Div => "DIV",
            Mod => "MOD",
            And => "AND",
            Or => "OR",
            Xor => "XOR",
            Shl => "SHL",
            Shr => "SHR",
            Eq => "EQ",
            Ne => "NE",
            Lt => "LT",
            Gt => "GT",
            Load8 => "LOAD8",
            Load16 => "LOAD16",
            Store8 => "STORE8",
            Store16 => "STORE16",
            Jmp => "JMP",
            Jz => "JZ",
            Jnz => "JNZ",
            Call => "CALL",
            Ret => "RET",
            Dei => "DEI",
            Deo => "DEO",
        }
    }

    /// Look up an opcode by mnemonic (case-insensitive).
    pub fn from_mnemonic(s: &str) -> Option<Op> {
        let up = s.to_ascii_uppercase();
        // Linear scan over the fixed opcode table — small and only hit at assemble time.
        const ALL: &[Op] = &[
            Op::Nop, Op::Halt, Op::Lit8, Op::Lit16, Op::Dup, Op::Drop, Op::Swap, Op::Over,
            Op::Rot, Op::Add, Op::Sub, Op::Mul, Op::Div, Op::Mod, Op::And, Op::Or, Op::Xor,
            Op::Shl, Op::Shr, Op::Eq, Op::Ne, Op::Lt, Op::Gt, Op::Load8, Op::Load16,
            Op::Store8, Op::Store16, Op::Jmp, Op::Jz, Op::Jnz, Op::Call, Op::Ret, Op::Dei,
            Op::Deo,
        ];
        ALL.iter().copied().find(|op| op.mnemonic() == up)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_byte_decode() {
        // Every opcode decodes back from its own byte value.
        let ops = [
            Op::Nop, Op::Halt, Op::Lit8, Op::Lit16, Op::Dup, Op::Rot, Op::Add, Op::Div,
            Op::Xor, Op::Gt, Op::Store16, Op::Call, Op::Ret, Op::Deo,
        ];
        for op in ops {
            assert_eq!(Op::from_byte(op as u8), Some(op));
        }
    }

    #[test]
    fn illegal_opcode_is_none() {
        assert_eq!(Op::from_byte(0xAB), None);
        assert_eq!(Op::from_byte(0xFF), None);
    }

    #[test]
    fn mnemonic_roundtrip_is_case_insensitive() {
        assert_eq!(Op::from_mnemonic("add"), Some(Op::Add));
        assert_eq!(Op::from_mnemonic("Store16"), Some(Op::Store16));
        assert_eq!(Op::from_mnemonic("DEO"), Some(Op::Deo));
        assert_eq!(Op::from_mnemonic("nope"), None);
    }
}
