# Kessel Fantasy-Console VM

A tiny 16-bit stack VM (Uxn-inspired) that lets the model **write a small game,
assemble it, run it, observe the result, and debug it** — and the game is
playable by a human. Lives in `crates/lib/src/vm/`. Pure Rust, deterministic,
snapshotable.

The `vm_*` tools are registered **only** in `agent_new` (the standalone `kessel`
app), so they are present in the mac/Windows voice app alongside
screenshot/STT/TTS but **absent** from `kessel-cli`/app-server.

## Machine

- 16-bit stack machine. Data stack + return stack, 256 `u16` cells each.
- Flat 64 KiB memory (`u16` addresses — no out-of-range accesses).
- ROMs load at `0x0100`; the **reset vector** runs once (init).
- Each frame calls the installed **frame vector**; it runs until `RET` (to the
  top), `HALT`, or the per-frame cap (200,000 instructions).
- Runtime errors (div-by-zero, stack under/overflow, illegal opcode) are
  *trapped*: they set a `fault` string and halt the machine — never a crash.

## Instruction set (34 opcodes)

Immediates: `LIT8` reads 1 following byte, `LIT16` reads 2 (big-endian). In
assembly you rarely write these directly — use `#ff` / `#1234` / decimal.

```
NOP HALT LIT8 LIT16
DUP DROP SWAP OVER ROT
ADD SUB MUL DIV MOD           ( wrapping u16; DIV/MOD by 0 -> fault )
AND OR XOR SHL SHR
EQ NE LT GT                   ( push 1/0; unsigned )
LOAD8 LOAD16 STORE8 STORE16
JMP JZ JNZ CALL RET
DEI DEO                       ( device in / out )
HALT
```

Stack effects worth memorizing (top of stack is the **rightmost**):

```
SUB      ( a b -- a-b )
LT       ( a b -- a<b )
STORE16  ( val addr -- )        LOAD16  ( addr -- val )
JZ       ( cond addr -- )       jump to addr if cond == 0
JNZ      ( cond addr -- )       jump to addr if cond != 0
CALL     ( addr -- )            RET ( -- )
DEI      ( port -- val )        DEO ( val port -- )
```

## Assembly syntax

```
ADD SUB DEO       bare mnemonic
#ff  #1234        hex literal push (LIT8 / LIT16)
42   0x20         decimal / hex literal (LIT8 if <256, else LIT16)
@name             define a label at the current address
name              reference a label -> pushes its 16-bit address
.byte 1 2 3       raw bytes
.word 0x1234      one raw 16-bit word
.res 2            reserve N zero bytes (RAM variables)
( ... )           block comment      ; ... line comment
```

Referencing a label pushes its **address**. For a variable, define it with
`@player-x .res 2` and use `player-x LOAD16` / `player-x STORE16`.

## Devices (via `DEI`/`DEO`)

Port byte = `(device << 4) | register`.

| Port | Dir | Meaning |
|------|-----|---------|
| `0x00` | out | system/halt (non-zero halts the machine) |
| `0x01..0x04` | out | palette: index, r, g, b (writing **b** commits the entry) |
| `0x10` | out | screen/vector — install the frame vector (address) |
| `0x11` `0x12` | out | screen x, y |
| `0x13` | out | screen colour (0–15; 0 = transparent for sprites) |
| `0x14` | out | draw pixel at (x,y) |
| `0x15` | out | draw 8×8 sprite from `mem[addr]` (32 bytes, 4bpp, hi-nibble = left) |
| `0x16` | out | clear screen to colour |
| `0x20` | in  | gamepad buttons bitfield |
| `0x30` | in/out | rng: read next `u16` / write to set the seed |
| `0x40` `0x41` `0x42` | out/in/out | storage addr / read / write (256 bytes) |
| `0x50` `0x51` `0x52` | out | debug entity: x, y, commit(tag) — reported in the observation |
| `0x60` | out | console: write a byte to the text buffer |

Gamepad bits: `LEFT 0x01 RIGHT 0x02 UP 0x04 DOWN 0x08 A 0x10 B 0x20 START 0x40
SELECT 0x80`. Screen is 128×128, 16-colour (default PICO-8 palette).

## The tools (agent-facing loop)

`vm_write_source(path, source)` → `vm_assemble(path)` → `vm_load_rom(path)` →
`vm_run_frame(buttons)` / `vm_run_cycles(n)` → `vm_inspect_memory`,
`vm_inspect_stacks`, `vm_get_framebuffer` (PNG) → `vm_snapshot`/`vm_restore`,
`vm_reset`.

`vm_run_frame` returns the observation record (screen hash + changed bbox for
"look at the screen", `vm.*` internals for white-box debugging, and
game-reported `entities` for black-box tasks):

```json
{ "frame": 2, "cycles": 130, "buttons": ["LEFT"],
  "framebuffer_hash": "…", "changed_pixels_bbox": [31,60,31,60],
  "console": "", "fault": null, "halted": false,
  "vm": { "pc": 65535, "data_stack": [], "return_stack_depth": 0 },
  "entities": [ {"tag": 1, "x": 31, "y": 60} ] }
```

## Example: move a pixel with LEFT / RIGHT

```
( reset: install the frame vector, put the player at x=32 )
on-frame #10 DEO
#20 player-x STORE16
RET

@on-frame
    ( LEFT held? decrement x )
    #20 DEI #01 AND  skip-left JZ
    player-x LOAD16 #01 SUB player-x STORE16
    @skip-left

    ( RIGHT held? increment x )
    #20 DEI #02 AND  skip-right JZ
    player-x LOAD16 #01 ADD player-x STORE16
    @skip-right

    ( draw the player pixel at (player-x, 60) in white )
    player-x LOAD16 #11 DEO
    60 #12 DEO
    #07 #13 DEO
    #00 #14 DEO

    ( report the player entity for observation )
    player-x LOAD16 #50 DEO
    60 #51 DEO
    #01 #52 DEO
    RET

@player-x .res 2
```

Note: the example wraps `@skip-left`/`@skip-right` as labels **after** the branch
so `JZ` skips the movement block — labels mark addresses, no jump is needed to
"fall through" into them.

## uxlang dialect (`.ux`)

A small, LL(1), typed **Pascal/C-ish** language that **compiles to the assembler
above** — a more conventional, readable alternative to raw stack code. Give the
source a `.ux` path and `vm_assemble` compiles it to assembly, then assembles.
Everything downstream (load, run, observe) is identical.

Structure: the top level holds `const` / `var` declarations and `proc`
definitions. Entry points are conventional (the VM is vector-driven, so there is
no `main(){ loop … }`): `init` runs once at reset; `update` then `draw` run each
frame (or a single `frame` proc). Locals/params use static slots, so **recursion
is not supported**.

```c
const SCREEN_W = 128;
var player_x: word = 32;
var vx: word = 1;

var ball: [32]byte;              // 8x8 sprite, 4bpp (2 px/byte, hi-nibble = left)

proc init() {
    ball[0] = 0x11; ball[1] = 0x11;   // fill a couple of rows...
}

proc update() {
    if button(LEFT)  { player_x = player_x - 1; }
    if button(RIGHT) { player_x = player_x + 1; }
    if player_x >= SCREEN_W - 8 { vx = 0 - vx; }
}

proc draw() {
    clear(0);
    sprite(ball, player_x, 60);   // ( tile x y )
    entity(player_x, 60, 1);      // ( x y tag ) — report for observation
}
```

- **Types**: `byte` (8-bit), `word`/`bool` (16-bit); fixed arrays `[N]byte` /
  `[N]word`. A bare array name is its base address.
- **Declarations**: `const NAME = <const-expr>;`, `var name: type [= const];`,
  `proc name(a: type, …)[: type] { … }`.
- **Statements**: `var`, assignment (`x = e;`, `a[i] = e;`), `if/else`, `while`,
  `loop`, `break`, `return`, and calls.
- **Operators**: `+ - * / %`, `& | ^ << >>`, `== != < <= > >=`, `and or not`,
  unary `-` `~`. Assignment is a statement (no `a = b = c`).
- **Builtins**: `clear(c)`, `pixel(x,y,c)`, `sprite(tile,x,y)`,
  `entity(x,y,tag)`, `button(mask)→0/1`, `buttons()`, `rnd()`,
  `peek8/peek16(addr)`, `poke8/poke16(addr,val)`.
- **Button constants**: `LEFT RIGHT UP DOWN A B START SELECT`.
- Comments: `//` to end of line, `/* … */` block.

## Phase 2 (planned)

A macOS AppKit window drives the same `VmConsole` at 60 Hz (framebuffer → image,
keyboard → gamepad) so the AI-authored ROM is human-playable. Windows C# mirrors
it.
