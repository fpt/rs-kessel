//! A thin Forth-ish front-end that compiles to the [`super::assembler`] dialect.
//!
//! It gives the model a higher-level, more writable language than raw stack
//! assembly while staying trivially compilable: word definitions become
//! subroutines, control flow becomes labels + branches, and primitives inline to
//! opcodes. The output is ordinary assembler text, so the existing two-pass
//! assembler resolves labels and produces the ROM.
//!
//! ## Language
//!
//! ```forth
//! variable player-x
//!
//! : init   32 player-x ! ;              \ once, at reset
//! : update
//!     buttons BTN-LEFT  and if player-x @ 1- player-x ! then
//!     buttons BTN-RIGHT and if player-x @ 1+ player-x ! then ;
//! : draw
//!     0 cls
//!     player-x @ 60 1 sprite            \ ( x y tile -- )
//!     player-x @ 60 1 entity ;          \ ( x y tag -- )
//! ```
//!
//! - Top level holds only `variable` / `constant` declarations and `: … ;` word
//!   definitions. Entry points are conventional: `init` runs once at reset;
//!   `update` then `draw` run each frame (or a single `frame` word if neither is
//!   defined).
//! - Control flow: `if … then`, `if … else … then`, `begin … until`,
//!   `begin … again`.
//! - `@`/`!` are 16-bit load/store; `c@`/`c!` are 8-bit.

use std::collections::{HashMap, HashSet};

use super::assembler::Diagnostic;

/// Result of compiling Forth source: generated assembler text plus diagnostics.
pub struct Compiled {
    pub asm: String,
    pub diagnostics: Vec<Diagnostic>,
}

impl Compiled {
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Compile Forth-ish source into assembler text.
pub fn compile(src: &str) -> Compiled {
    let tokens = tokenize(src);
    let mut diagnostics = Vec::new();
    let symbols = prescan(&tokens, &mut diagnostics);

    let mut defs = String::new();
    let mut vars = String::new();
    let mut helpers = Helpers::default();
    let mut label_ctr = 0usize;

    let mut i = 0;
    while i < tokens.len() {
        let (tok, line) = (tokens[i].0.as_str(), tokens[i].1);
        match tok {
            ":" => {
                i += 1;
                let name = match tokens.get(i) {
                    Some((n, _)) => n.clone(),
                    None => {
                        diagnostics.push(err(line, "':' with no word name".into()));
                        break;
                    }
                };
                i += 1;
                let (body, next) = compile_body(
                    &tokens,
                    i,
                    &symbols,
                    &mut label_ctr,
                    &mut helpers,
                    &mut diagnostics,
                );
                i = next;
                defs.push_str(&format!("@{name}\n  {body} RET\n"));
            }
            "variable" => {
                i += 1;
                if let Some((name, _)) = tokens.get(i) {
                    vars.push_str(&format!("@{name} .res 2\n"));
                    i += 1;
                } else {
                    diagnostics.push(err(line, "'variable' with no name".into()));
                }
            }
            // `create NAME b0 b1 …` — labelled byte data (e.g. an 8×8 sprite is
            // 32 bytes, 4bpp). Referencing NAME pushes its address.
            "create" => {
                i += 1;
                let name = match tokens.get(i) {
                    Some((n, _)) => n.clone(),
                    None => {
                        diagnostics.push(err(line, "'create' with no name".into()));
                        break;
                    }
                };
                i += 1;
                let mut bytes: Vec<String> = Vec::new();
                while let Some((t, tl)) = tokens.get(i) {
                    match parse_number(t) {
                        Some(v) if v <= 0xff => {
                            bytes.push(v.to_string());
                            i += 1;
                        }
                        Some(v) => {
                            diagnostics.push(err(*tl, format!("create byte out of range: {v}")));
                            i += 1;
                        }
                        None => break,
                    }
                }
                if bytes.is_empty() {
                    diagnostics.push(err(line, "'create' needs at least one byte value".into()));
                } else {
                    vars.push_str(&format!("@{name} .byte {}\n", bytes.join(" ")));
                }
            }
            // `<n> constant <name>` — recorded in prescan; skip the triple here.
            _ if parse_number(tok).is_some() && matches!(tokens.get(i + 1), Some((k, _)) if k == "constant") =>
            {
                i += 3;
            }
            "constant" => {
                // A bare `constant` without a preceding number.
                diagnostics.push(err(line, "'constant' must follow a number: `42 constant NAME`".into()));
                i += 1;
            }
            ";" => {
                diagnostics.push(err(line, "';' without a matching ':'".into()));
                i += 1;
            }
            other => {
                diagnostics.push(err(
                    line,
                    format!("'{other}' outside a word definition (top level allows only `:`, `variable`, `constant`)"),
                ));
                i += 1;
            }
        }
    }

    let asm = assemble_program(&symbols, &defs, &vars, &helpers);
    Compiled { asm, diagnostics }
}

/// Symbols known before compiling bodies (so words can be used before defined).
struct Symbols {
    words: HashSet<String>,
    variables: HashSet<String>,
    constants: HashMap<String, u16>,
}

/// Which emitted helper subroutines the program needs.
#[derive(Default)]
struct Helpers {
    sprite: bool,
    entity: bool,
}

/// Collect word / variable / constant names in a first pass, reporting invalid
/// or duplicate names.
fn prescan(tokens: &[(String, usize)], diagnostics: &mut Vec<Diagnostic>) -> Symbols {
    let mut words = HashSet::new();
    let mut variables = HashSet::new();
    let mut constants = HashMap::new();

    let check_name = |name: &str, line: usize, sym_exists: bool, diags: &mut Vec<Diagnostic>| -> bool {
        if !is_user_ident(name) {
            diags.push(err(line, format!("invalid name '{name}'")));
            return false;
        }
        if is_reserved(name) {
            diags.push(err(line, format!("'{name}' is a reserved word")));
            return false;
        }
        if sym_exists {
            diags.push(err(line, format!("duplicate definition of '{name}'")));
            return false;
        }
        true
    };

    let mut i = 0;
    while i < tokens.len() {
        let (tok, line) = (tokens[i].0.as_str(), tokens[i].1);
        match tok {
            ":" => {
                if let Some((name, nl)) = tokens.get(i + 1) {
                    let exists = words.contains(name) || variables.contains(name) || constants.contains_key(name);
                    if check_name(name, *nl, exists, diagnostics) {
                        words.insert(name.clone());
                    }
                }
                i += 2;
            }
            "variable" | "create" => {
                if let Some((name, nl)) = tokens.get(i + 1) {
                    let exists = words.contains(name) || variables.contains(name) || constants.contains_key(name);
                    if check_name(name, *nl, exists, diagnostics) {
                        variables.insert(name.clone());
                    }
                }
                i += 2;
            }
            "constant" => {
                let value = i.checked_sub(1).and_then(|p| tokens.get(p)).and_then(|(v, _)| parse_number(v));
                match (value, tokens.get(i + 1)) {
                    (Some(v), Some((name, nl))) => {
                        let exists = words.contains(name) || variables.contains(name) || constants.contains_key(name);
                        if check_name(name, *nl, exists, diagnostics) {
                            constants.insert(name.clone(), v);
                        }
                    }
                    _ => diagnostics.push(err(line, "'constant' needs `<number> constant <name>`".into())),
                }
                i += 2;
            }
            _ => i += 1,
        }
    }

    Symbols {
        words,
        variables,
        constants,
    }
}

/// Compile the body of a `: … ;` word starting at `start`, returning the
/// generated asm and the index just past the closing `;`.
fn compile_body(
    tokens: &[(String, usize)],
    start: usize,
    sym: &Symbols,
    label_ctr: &mut usize,
    helpers: &mut Helpers,
    diagnostics: &mut Vec<Diagnostic>,
) -> (String, usize) {
    let mut out: Vec<String> = Vec::new();
    let mut cf: Vec<Ctrl> = Vec::new();
    let mut i = start;

    while i < tokens.len() {
        let (tok, line) = (tokens[i].0.as_str(), tokens[i].1);
        i += 1;
        match tok {
            ";" => {
                if !cf.is_empty() {
                    diagnostics.push(err(line, "word ends with unbalanced if/begin".into()));
                }
                return (out.join(" "), i);
            }
            ":" => {
                diagnostics.push(err(line, "nested ':' is not allowed".into()));
                return (out.join(" "), i);
            }
            "if" => {
                let id = *label_ctr;
                *label_ctr += 1;
                out.push(format!("__L{id}a JZ"));
                cf.push(Ctrl::If { id, has_else: false });
            }
            "else" => match cf.last_mut() {
                Some(Ctrl::If { id, has_else }) if !*has_else => {
                    out.push(format!("__L{id}b JMP @__L{id}a"));
                    *has_else = true;
                }
                _ => diagnostics.push(err(line, "'else' without matching 'if'".into())),
            },
            "then" => match cf.pop() {
                Some(Ctrl::If { id, has_else }) => {
                    out.push(format!("@__L{id}{}", if has_else { "b" } else { "a" }));
                }
                _ => diagnostics.push(err(line, "'then' without matching 'if'".into())),
            },
            "begin" => {
                let id = *label_ctr;
                *label_ctr += 1;
                out.push(format!("@__B{id}"));
                cf.push(Ctrl::Begin { id });
            }
            "until" => match cf.pop() {
                Some(Ctrl::Begin { id }) => out.push(format!("__B{id} JZ")),
                _ => diagnostics.push(err(line, "'until' without matching 'begin'".into())),
            },
            "again" => match cf.pop() {
                Some(Ctrl::Begin { id }) => out.push(format!("__B{id} JMP")),
                _ => diagnostics.push(err(line, "'again' without matching 'begin'".into())),
            },
            _ => {
                if let Some(v) = parse_number(tok) {
                    out.push(v.to_string());
                } else if let Some(snip) = primitive(tok) {
                    out.push(snip.to_string());
                } else if let Some(bits) = button(tok) {
                    out.push(format!("#{bits:02x}"));
                } else if tok == "sprite" {
                    helpers.sprite = true;
                    out.push("__sprite CALL".to_string());
                } else if tok == "entity" {
                    helpers.entity = true;
                    out.push("__entity CALL".to_string());
                } else if sym.variables.contains(tok) {
                    out.push(tok.to_string()); // reference -> push address
                } else if let Some(v) = sym.constants.get(tok) {
                    out.push(v.to_string());
                } else if sym.words.contains(tok) {
                    out.push(format!("{tok} CALL"));
                } else {
                    diagnostics.push(err(line, format!("unknown word '{tok}'")));
                }
            }
        }
    }

    diagnostics.push(err(
        tokens.get(start).map(|t| t.1).unwrap_or(0),
        "word is missing its terminating ';'".into(),
    ));
    (out.join(" "), i)
}

enum Ctrl {
    If { id: usize, has_else: bool },
    Begin { id: usize },
}

/// Stitch the reset/frame wiring, word defs, variables, and helpers into a full
/// assembler program.
fn assemble_program(sym: &Symbols, defs: &str, vars: &str, helpers: &Helpers) -> String {
    let mut out = String::new();
    // Reset vector: run init once, install the frame vector.
    out.push_str("( generated by the Forth front-end )\n");
    if sym.words.contains("init") {
        out.push_str("init CALL\n");
    }
    out.push_str("__frame #10 DEO\nRET\n\n");

    // Frame vector: update then draw, or a single `frame` word.
    out.push_str("@__frame\n");
    let has_ud = sym.words.contains("update") || sym.words.contains("draw");
    if sym.words.contains("update") {
        out.push_str("  update CALL\n");
    }
    if sym.words.contains("draw") {
        out.push_str("  draw CALL\n");
    }
    if !has_ud && sym.words.contains("frame") {
        out.push_str("  frame CALL\n");
    }
    out.push_str("  RET\n\n");

    out.push_str(defs);
    out.push('\n');
    out.push_str(vars);

    if helpers.sprite {
        // ( x y tile -- ) : set screen x/y, blit sprite from tile address.
        out.push_str("@__sprite\n  __tmp STORE16 #12 DEO #11 DEO __tmp LOAD16 #15 DEO RET\n");
    }
    if helpers.entity {
        // ( x y tag -- ) : report an entity to the debug port.
        out.push_str("@__entity\n  __tmp STORE16 #51 DEO #50 DEO __tmp LOAD16 #52 DEO RET\n");
    }
    if helpers.sprite || helpers.entity {
        out.push_str("@__tmp .res 2\n");
    }

    out
}

/// Inline opcode sequence for a primitive word, or `None`.
fn primitive(tok: &str) -> Option<&'static str> {
    Some(match tok {
        "+" => "ADD",
        "-" => "SUB",
        "*" => "MUL",
        "/" => "DIV",
        "mod" => "MOD",
        "and" => "AND",
        "or" => "OR",
        "xor" => "XOR",
        "lshift" => "SHL",
        "rshift" => "SHR",
        "=" => "EQ",
        "<>" => "NE",
        "<" => "LT",
        ">" => "GT",
        "dup" => "DUP",
        "drop" => "DROP",
        "swap" => "SWAP",
        "over" => "OVER",
        "rot" => "ROT",
        "nip" => "SWAP DROP",
        "1+" => "#01 ADD",
        "1-" => "#01 SUB",
        "2*" => "#01 SHL",
        "2/" => "#01 SHR",
        "negate" => "#00 SWAP SUB",
        "@" => "LOAD16",
        "!" => "STORE16",
        "c@" => "LOAD8",
        "c!" => "STORE8",
        // Device words.
        "cls" => "#16 DEO",
        "set-x" => "#11 DEO",
        "set-y" => "#12 DEO",
        "set-color" => "#13 DEO",
        "pixel" => "#00 #14 DEO",
        "buttons" => "#20 DEI",
        "rnd" => "#30 DEI",
        "deo" => "DEO",
        "dei" => "DEI",
        _ => return None,
    })
}

/// Gamepad button constant → bitfield, or `None`.
fn button(tok: &str) -> Option<u8> {
    Some(match tok {
        "BTN-LEFT" => 0x01,
        "BTN-RIGHT" => 0x02,
        "BTN-UP" => 0x04,
        "BTN-DOWN" => 0x08,
        "BTN-A" => 0x10,
        "BTN-B" => 0x20,
        "BTN-START" => 0x40,
        "BTN-SELECT" => 0x80,
        _ => return None,
    })
}

/// Reserved words that cannot be user-defined names.
fn is_reserved(name: &str) -> bool {
    matches!(
        name,
        ":" | ";" | "if" | "else" | "then" | "begin" | "until" | "again" | "variable"
            | "constant" | "create"
    ) || primitive(name).is_some()
        || button(name).is_some()
        || name == "sprite"
        || name == "entity"
}

/// Valid user identifier: assembler-label-safe and not a `__` internal prefix.
fn is_user_ident(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with("__")
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && s.chars().next().is_some_and(|c| !c.is_ascii_digit())
}

fn parse_number(s: &str) -> Option<u16> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u16::from_str_radix(hex, 16).ok();
    }
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
        return s.parse::<u32>().ok().filter(|&v| v <= 0xffff).map(|v| v as u16);
    }
    None
}

fn err(line: usize, message: String) -> Diagnostic {
    Diagnostic { line, message }
}

/// Whitespace-separated tokens; `( … )` block and `\ …` line comments stripped.
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
            '(' if cur.is_empty() => {
                // Block comment (Forth requires `(` to be its own token).
                for cc in chars.by_ref() {
                    if cc == '\n' {
                        line += 1;
                    }
                    if cc == ')' {
                        break;
                    }
                }
            }
            '\\' if cur.is_empty() => {
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
            c if c.is_whitespace() => flush(&mut cur, cur_line, &mut tokens),
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
    use crate::vm::assembler::assemble;
    use crate::vm::VmConsole;

    fn compile_ok(src: &str) -> String {
        let c = compile(src);
        assert!(c.ok(), "forth diagnostics: {:?}", c.diagnostics);
        // The generated asm must itself assemble cleanly.
        let built = assemble(&c.asm);
        assert!(built.ok(), "generated asm errors: {:?}\nASM:\n{}", built.diagnostics, c.asm);
        c.asm
    }

    #[test]
    fn primitives_and_variable() {
        let asm = compile_ok("variable x  : init  2 3 + x ! ;");
        assert!(asm.contains("@x .res 2"));
        assert!(asm.contains("init CALL")); // wired into reset
    }

    #[test]
    fn if_then_compiles_and_runs() {
        // A full mover written in Forth, driven through the VM.
        let src = r#"
            variable player-x
            : init 32 player-x ! ;
            : update
                buttons BTN-LEFT  and if player-x @ 1- player-x ! then
                buttons BTN-RIGHT and if player-x @ 1+ player-x ! then ;
            : draw
                0 cls
                player-x @ set-x  60 set-y  7 set-color  pixel
                player-x @ 60 1 entity ;
        "#;
        let mut console = VmConsole::new();
        console.write_source("game.fth", src);
        // Assemble/load via the .fth dispatch path (Forth → asm → ROM).
        assert!(console.assemble("game.fth").unwrap().ok());
        console.load_rom("game.fth").unwrap();

        use crate::vm::device::{BTN_LEFT, BTN_RIGHT};
        let o0 = console.run_frame(0);
        assert_eq!(o0.entities[0].x, 32, "init value");
        let o1 = console.run_frame(BTN_RIGHT);
        assert_eq!(o1.entities[0].x, 33);
        let o2 = console.run_frame(BTN_LEFT);
        assert_eq!(o2.entities[0].x, 32);
        // The pixel moved between frames, so the framebuffer changed.
        assert!(o2.changed_pixels_bbox.is_some());
    }

    #[test]
    fn create_sprite_blits() {
        // An 8×8 sprite whose top row is pixels [1,2,3,4,0,0,0,0] (bytes 0x12,0x34,…);
        // drawn at (0,0) via `sprite` ( x y tile -- ).
        let src = r#"
            create tile 18 52 0 0  0 0 0 0  0 0 0 0  0 0 0 0
                        0 0 0 0    0 0 0 0  0 0 0 0  0 0 0 0
            : draw 0 0 tile sprite ;
        "#;
        let mut console = VmConsole::new();
        console.write_source("s.forth", src);
        let built = console.assemble("s.forth").unwrap();
        assert!(built.ok(), "{:?}", built.diagnostics);
        console.load_rom("s.forth").unwrap();
        console.run_frame(0);
        // Top-left pixels come from the tile's first row.
        assert_eq!(console.vm.devices.framebuffer[0], 1);
        assert_eq!(console.vm.devices.framebuffer[1], 2);
        assert_eq!(console.vm.devices.framebuffer[2], 3);
        assert_eq!(console.vm.devices.framebuffer[3], 4);
    }

    #[test]
    fn if_else_then() {
        let asm = compile_ok(": pick  if 10 else 20 then ;");
        // Both branch bodies present.
        assert!(asm.contains("10") && asm.contains("20"));
    }

    #[test]
    fn begin_until_loop() {
        // Count down from 5 to 0 storing into a var; just needs to compile+run.
        let src = r#"
            variable n
            : init 5 n !
                begin
                    n @ 1- n !
                    n @ 0 =
                until ;
        "#;
        let c = compile(src);
        assert!(c.ok(), "{:?}", c.diagnostics);
        let mut console = VmConsole::new();
        console.write_source("g.asm", &c.asm);
        assert!(console.assemble("g.asm").unwrap().ok());
        assert_eq!(
            console.load_rom("g.asm").unwrap(),
            crate::vm::vm::RunOutcome::Completed
        );
    }

    #[test]
    fn constant_folds() {
        let asm = compile_ok("100 constant SPEED  : go SPEED ;");
        assert!(asm.contains("100"));
    }

    #[test]
    fn unknown_word_is_diagnosed() {
        let c = compile(": bad  frobnicate ;");
        assert!(!c.ok());
        assert!(c.diagnostics[0].message.contains("frobnicate"));
    }

    #[test]
    fn unbalanced_if_is_diagnosed() {
        let c = compile(": bad  1 if 2 ;");
        assert!(!c.ok());
        assert!(c.diagnostics.iter().any(|d| d.message.contains("unbalanced")));
    }

    #[test]
    fn reserved_and_internal_names_rejected() {
        assert!(!compile(": dup 1 ;").ok()); // reserved primitive
        assert!(!compile("variable __tmp").ok()); // internal prefix
    }

    #[test]
    fn top_level_code_is_rejected() {
        let c = compile("1 2 +");
        assert!(!c.ok());
        assert!(c.diagnostics[0].message.contains("outside a word"));
    }
}
