//! A tiny two-pass assembler for the stack VM.
//!
//! Grammar (whitespace-separated tokens; `( … )` block comments and `; …`
//! line comments are stripped):
//!
//! ```text
//!   ADD SUB DEO …     bare mnemonic            -> one opcode byte
//!   #ff  #1234        hex literal push         -> LIT8 / LIT16
//!   42   0x20         decimal / hex literal    -> LIT8 if <256 else LIT16
//!   @name             label definition here    -> (no bytes)
//!   name              label reference          -> LIT16 <addr of name>
//!   .byte 1 2 3       raw bytes                -> greedily consumes numbers
//!   .word 0x1234      one raw 16-bit word
//!   .res 2            reserve N zero bytes (RAM variables)
//! ```
//!
//! Code originates at [`ROM_ORIGIN`](super::vm::ROM_ORIGIN); label addresses are
//! absolute, matching where the ROM is mapped at load time.

use std::collections::BTreeMap;

use super::isa::Op;
use super::vm::ROM_ORIGIN;

/// One assembler error, tied to a source line (1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    pub message: String,
}

/// Result of assembling: the ROM bytes, any diagnostics, and the resolved label
/// table (useful for the observation/debug tools).
pub struct Assembled {
    pub rom: Vec<u8>,
    pub diagnostics: Vec<Diagnostic>,
    pub labels: BTreeMap<String, u16>,
}

impl Assembled {
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// An emitted element before label resolution.
enum Item {
    Byte(u8),
    Lit8(u8),
    Lit16(u16),
    /// Unresolved `LIT16 <addr of label>`.
    Lit16Label(String, usize),
    Bytes(Vec<u8>),
}

impl Item {
    fn size(&self) -> usize {
        match self {
            Item::Byte(_) => 1,
            Item::Lit8(_) => 2,
            Item::Lit16(_) | Item::Lit16Label(..) => 3,
            Item::Bytes(b) => b.len(),
        }
    }
}

/// Assemble source text into a ROM plus diagnostics.
pub fn assemble(src: &str) -> Assembled {
    let tokens = tokenize(src);
    let mut diagnostics = Vec::new();
    let mut labels: BTreeMap<String, u16> = BTreeMap::new();
    let mut items: Vec<Item> = Vec::new();
    let mut addr: usize = ROM_ORIGIN as usize;

    let mut i = 0;
    while i < tokens.len() {
        let (tok, line) = &tokens[i];
        let tok = tok.as_str();
        let line = *line;
        i += 1;

        // Label definition.
        if let Some(name) = tok.strip_prefix('@') {
            if name.is_empty() || !is_ident(name) {
                diagnostics.push(err(line, format!("invalid label name '@{name}'")));
                continue;
            }
            if labels.insert(name.to_string(), addr as u16).is_some() {
                diagnostics.push(err(line, format!("duplicate label '{name}'")));
            }
            continue;
        }

        // Directives.
        if let Some(dir) = tok.strip_prefix('.') {
            match dir {
                "byte" => {
                    // Greedily consume following numeric tokens as bytes.
                    let mut bytes = Vec::new();
                    while i < tokens.len() {
                        if let Some(v) = parse_number(&tokens[i].0) {
                            if v > 0xff {
                                diagnostics.push(err(tokens[i].1, format!("byte out of range: {v}")));
                            }
                            bytes.push(v as u8);
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    if bytes.is_empty() {
                        diagnostics.push(err(line, ".byte needs at least one value".into()));
                    } else {
                        addr += bytes.len();
                        items.push(Item::Bytes(bytes));
                    }
                }
                "word" => match tokens.get(i) {
                    Some(t) if parse_number(&t.0).is_some() => {
                        let v = parse_number(&t.0).unwrap();
                        i += 1;
                        addr += 2;
                        items.push(Item::Bytes(v.to_be_bytes().to_vec()));
                    }
                    // Present and numeric-looking but out of range — report it as
                    // such rather than silently truncating or "missing".
                    Some(t) if looks_numeric(&t.0) => {
                        diagnostics.push(err(
                            t.1,
                            format!(".word value out of range (0..=0xffff): '{}'", t.0),
                        ));
                        i += 1;
                    }
                    _ => diagnostics.push(err(line, ".word needs a 16-bit value".into())),
                },
                "res" => match tokens.get(i) {
                    Some(t) if parse_number(&t.0).is_some() => {
                        let n = parse_number(&t.0).unwrap() as usize;
                        i += 1;
                        // Guard the allocation: reject a reservation that would
                        // run past the 64 KiB space *before* materializing it, so
                        // an oversized `.res` can't consume memory or yield a ROM.
                        if addr + n > 0x1_0000 {
                            diagnostics.push(err(
                                t.1,
                                format!(
                                    ".res {n} exceeds the 64 KiB address space at 0x{addr:04X}"
                                ),
                            ));
                        } else {
                            addr += n;
                            items.push(Item::Bytes(vec![0u8; n]));
                        }
                    }
                    Some(t) if looks_numeric(&t.0) => {
                        diagnostics.push(err(t.1, format!(".res length out of range: '{}'", t.0)));
                        i += 1;
                    }
                    _ => diagnostics.push(err(line, ".res needs a length".into())),
                },
                other => diagnostics.push(err(line, format!("unknown directive '.{other}'"))),
            }
            continue;
        }

        // Hex literal push: #ff or #1234.
        if let Some(hex) = tok.strip_prefix('#') {
            match hex.len() {
                2 => match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        items.push(Item::Lit8(v));
                        addr += 2;
                    }
                    Err(_) => diagnostics.push(err(line, format!("bad hex literal '#{hex}'"))),
                },
                4 => match u16::from_str_radix(hex, 16) {
                    Ok(v) => {
                        items.push(Item::Lit16(v));
                        addr += 3;
                    }
                    Err(_) => diagnostics.push(err(line, format!("bad hex literal '#{hex}'"))),
                },
                _ => diagnostics.push(err(
                    line,
                    format!("hex literal '#{hex}' must be 2 or 4 digits"),
                )),
            }
            continue;
        }

        // Mnemonic.
        if let Some(op) = Op::from_mnemonic(tok) {
            items.push(Item::Byte(op as u8));
            addr += 1;
            continue;
        }

        // Bare numeric literal.
        if let Some(v) = parse_number(tok) {
            if v <= 0xff {
                items.push(Item::Lit8(v as u8));
                addr += 2;
            } else {
                items.push(Item::Lit16(v));
                addr += 3;
            }
            continue;
        }

        // Otherwise a label reference (push its address).
        if is_ident(tok) {
            items.push(Item::Lit16Label(tok.to_string(), line));
            addr += 3;
            continue;
        }

        diagnostics.push(err(line, format!("unexpected token '{tok}'")));
    }

    if addr > 0x1_0000 {
        diagnostics.push(err(0, "program exceeds 64 KiB address space".into()));
    }

    // Pass 2: emit bytes, resolving label references.
    let mut rom = Vec::new();
    for item in &items {
        match item {
            Item::Byte(b) => rom.push(*b),
            Item::Lit8(v) => {
                rom.push(Op::Lit8 as u8);
                rom.push(*v);
            }
            Item::Lit16(v) => {
                rom.push(Op::Lit16 as u8);
                rom.extend_from_slice(&v.to_be_bytes());
            }
            Item::Lit16Label(name, line) => {
                rom.push(Op::Lit16 as u8);
                match labels.get(name) {
                    Some(a) => rom.extend_from_slice(&a.to_be_bytes()),
                    None => {
                        diagnostics.push(err(*line, format!("undefined label '{name}'")));
                        rom.extend_from_slice(&[0, 0]);
                    }
                }
            }
            Item::Bytes(b) => rom.extend_from_slice(b),
        }
    }

    // Debug assertion: emitted length must match the addresses we computed.
    debug_assert_eq!(
        rom.len(),
        items.iter().map(Item::size).sum::<usize>(),
        "assembler size accounting drifted"
    );

    Assembled {
        rom,
        diagnostics,
        labels,
    }
}

fn err(line: usize, message: String) -> Diagnostic {
    Diagnostic { line, message }
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && s.chars().next().is_some_and(|c| !c.is_ascii_digit())
}

/// True if `s` is written as a number (decimal or `0x`-hex) regardless of
/// magnitude — used to tell "out of range" from "not a number" in diagnostics.
fn looks_numeric(s: &str) -> bool {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// Parse a decimal or `0x`-hex number in `0..=0xffff`.
fn parse_number(s: &str) -> Option<u16> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u16::from_str_radix(hex, 16).ok();
    }
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
        return s.parse::<u32>().ok().filter(|&v| v <= 0xffff).map(|v| v as u16);
    }
    None
}

/// Split source into `(token, line)` pairs, stripping comments.
fn tokenize(src: &str) -> Vec<(String, usize)> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut line = 1usize;
    let mut cur_line = 1usize;
    let mut chars = src.chars().peekable();

    let flush = |cur: &mut String, cur_line: usize, tokens: &mut Vec<(String, usize)>| {
        if !cur.is_empty() {
            tokens.push((std::mem::take(cur), cur_line));
        }
    };

    while let Some(c) = chars.next() {
        match c {
            '(' => {
                flush(&mut cur, cur_line, &mut tokens);
                // Skip to matching ')', counting newlines.
                for cc in chars.by_ref() {
                    if cc == '\n' {
                        line += 1;
                    }
                    if cc == ')' {
                        break;
                    }
                }
            }
            ';' => {
                flush(&mut cur, cur_line, &mut tokens);
                for cc in chars.by_ref() {
                    if cc == '\n' {
                        line += 1;
                        break;
                    }
                }
            }
            '\n' => {
                flush(&mut cur, cur_line, &mut tokens);
                line += 1;
            }
            c if c.is_whitespace() => {
                flush(&mut cur, cur_line, &mut tokens);
            }
            c => {
                if cur.is_empty() {
                    cur_line = line;
                }
                cur.push(c);
            }
        }
    }
    flush(&mut cur, cur_line, &mut tokens);
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_and_add() {
        let a = assemble("2 3 ADD RET");
        assert!(a.ok(), "{:?}", a.diagnostics);
        // LIT8 2, LIT8 3, ADD, RET
        assert_eq!(a.rom, vec![Op::Lit8 as u8, 2, Op::Lit8 as u8, 3, Op::Add as u8, Op::Ret as u8]);
    }

    #[test]
    fn big_literal_promotes_to_lit16() {
        let a = assemble("300 RET");
        assert!(a.ok());
        assert_eq!(a.rom, vec![Op::Lit16 as u8, 0x01, 0x2C, Op::Ret as u8]);
    }

    #[test]
    fn hex_literals() {
        let a = assemble("#ff #1234 RET");
        assert!(a.ok(), "{:?}", a.diagnostics);
        assert_eq!(
            a.rom,
            vec![Op::Lit8 as u8, 0xff, Op::Lit16 as u8, 0x12, 0x34, Op::Ret as u8]
        );
    }

    #[test]
    fn label_definition_and_reference() {
        // `here` references a forward label; RET; @here is a data cell.
        let a = assemble("here JMP @here .res 2");
        assert!(a.ok(), "{:?}", a.diagnostics);
        // LIT16 <addr>, JMP, then 2 reserved bytes.
        // origin 0x0100: LIT16(3) + JMP(1) = 4 bytes, so @here = 0x0104.
        assert_eq!(*a.labels.get("here").unwrap(), 0x0104);
        assert_eq!(a.rom[0], Op::Lit16 as u8);
        assert_eq!(&a.rom[1..3], &0x0104u16.to_be_bytes());
        assert_eq!(a.rom[3], Op::Jmp as u8);
        assert_eq!(&a.rom[4..6], &[0, 0]);
    }

    #[test]
    fn comments_are_stripped() {
        let a = assemble("( push two ) 2 ; line comment\n 3 ADD RET");
        assert!(a.ok(), "{:?}", a.diagnostics);
        assert_eq!(a.rom, vec![Op::Lit8 as u8, 2, Op::Lit8 as u8, 3, Op::Add as u8, Op::Ret as u8]);
    }

    #[test]
    fn undefined_label_is_diagnosed() {
        let a = assemble("missing JMP");
        assert!(!a.ok());
        assert!(a.diagnostics[0].message.contains("undefined label"));
    }

    #[test]
    fn bad_mnemonic_reports_line() {
        let a = assemble("2 3\nFLOOB");
        assert!(!a.ok());
        assert_eq!(a.diagnostics[0].line, 2);
        assert!(a.diagnostics[0].message.contains("FLOOB"));
    }

    #[test]
    fn directives_emit_data() {
        let a = assemble(".byte 1 2 3 .word 0x0102");
        assert!(a.ok(), "{:?}", a.diagnostics);
        assert_eq!(a.rom, vec![1, 2, 3, 0x01, 0x02]);
    }

    #[test]
    fn word_out_of_range_is_diagnosed() {
        let a = assemble(".word 65536");
        assert!(!a.ok());
        assert!(a.diagnostics[0].message.contains("out of range"), "{:?}", a.diagnostics);
    }

    #[test]
    fn oversized_res_is_rejected_without_allocating() {
        // .res 65535 from origin 0x0100 runs past 64 KiB — must be a diagnostic,
        // and must NOT materialize a giant ROM.
        let a = assemble(".res 65535");
        assert!(!a.ok());
        assert!(
            a.diagnostics[0].message.contains("address space"),
            "{:?}",
            a.diagnostics
        );
        assert!(a.rom.len() < 1024, "oversized .res was materialized: {} bytes", a.rom.len());
    }

    #[test]
    fn res_length_out_of_range_is_diagnosed() {
        let a = assemble(".res 70000");
        assert!(!a.ok());
        assert!(a.diagnostics[0].message.contains("out of range"), "{:?}", a.diagnostics);
    }

    #[test]
    fn small_res_still_works() {
        let a = assemble("@v .res 4");
        assert!(a.ok(), "{:?}", a.diagnostics);
        assert_eq!(a.rom, vec![0, 0, 0, 0]);
        assert_eq!(*a.labels.get("v").unwrap(), 0x0100);
    }
}
