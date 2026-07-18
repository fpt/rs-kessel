//! `uxlang` — a small, LL(1), typed Pascal/C-ish language that compiles to the
//! [`super::assembler`] dialect. A more conventional structured alternative to
//! the Forth front-end: the model writes ordinary `if`/`while`/`proc` code with
//! `byte`/`word` types, and we lower it to the stack VM.
//!
//! ```text
//! const SCREEN_W = 128;
//! var player_x: word = 32;
//! var vx: word = 1;
//!
//! proc update() {
//!     if button(LEFT)  { player_x = player_x - 1; }
//!     if button(RIGHT) { player_x = player_x + 1; }
//! }
//! proc draw() {
//!     clear(0);
//!     pixel(player_x, 60, 7);
//!     entity(player_x, 60, 1);
//! }
//! ```
//!
//! Entry points follow the same convention as the Forth front-end (the VM is
//! vector-driven, so there is no `main(){ loop … }`): `init` runs once at reset;
//! `update` then `draw` run each frame (or a single `frame` proc). Locals and
//! params are allocated to static slots — simple, but **recursion is not
//! supported**.
//!
//! Everything is evaluated onto the VM data stack, so codegen is a direct
//! post-order walk. Generated labels are `ux_`-prefixed to avoid colliding with
//! assembler mnemonics (a proc named `add` must not emit the `ADD` opcode).

use std::collections::HashMap;

use super::assembler::Diagnostic;

/// Result of compiling uxlang source: generated assembler text plus diagnostics.
pub struct Compiled {
    pub asm: String,
    pub diagnostics: Vec<Diagnostic>,
}

impl Compiled {
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Compile uxlang source into assembler text.
pub fn compile(src: &str) -> Compiled {
    let mut diagnostics = Vec::new();
    let tokens = lex(src, &mut diagnostics);
    let mut parser = Parser::new(tokens);
    let decls = parser.parse_program(&mut diagnostics);
    if !diagnostics.is_empty() {
        return Compiled {
            asm: String::new(),
            diagnostics,
        };
    }
    let asm = Compiler::new().compile(&decls, &mut diagnostics);
    Compiled { asm, diagnostics }
}

fn err(line: usize, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        line,
        message: message.into(),
    }
}

// ======================================================================
// Lexer
// ======================================================================

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Num(i64),
    Sym(&'static str),
    Eof,
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    line: usize,
}

const SYMBOLS: &[&str] = &[
    "<<", ">>", "==", "!=", "<=", ">=", "+", "-", "*", "/", "%", "&", "|", "^", "<", ">", "=", "(",
    ")", "{", "}", "[", "]", ",", ";", ":", "~",
];

fn lex(src: &str, diagnostics: &mut Vec<Diagnostic>) -> Vec<Token> {
    let bytes: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut line = 1usize;
    let mut out = Vec::new();

    while i < bytes.len() {
        let c = bytes[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Comments: // to EOL, and /* ... */
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == '/' {
            while i < bytes.len() && bytes[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == '*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == '*' && bytes[i + 1] == '/') {
                if bytes[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
            continue;
        }
        // Identifiers / keywords
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == '_') {
                i += 1;
            }
            let s: String = bytes[start..i].iter().collect();
            out.push(Token {
                tok: Tok::Ident(s),
                line,
            });
            continue;
        }
        // Numbers (decimal or 0x hex)
        if c.is_ascii_digit() {
            let start = i;
            if c == '0' && i + 1 < bytes.len() && (bytes[i + 1] == 'x' || bytes[i + 1] == 'X') {
                i += 2;
                while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let s: String = bytes[start + 2..i].iter().collect();
                match i64::from_str_radix(&s, 16) {
                    Ok(v) => out.push(Token {
                        tok: Tok::Num(v),
                        line,
                    }),
                    Err(_) => diagnostics.push(err(line, format!("bad hex literal '0x{s}'"))),
                }
            } else {
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let s: String = bytes[start..i].iter().collect();
                match s.parse::<i64>() {
                    Ok(v) => out.push(Token {
                        tok: Tok::Num(v),
                        line,
                    }),
                    Err(_) => diagnostics.push(err(line, format!("bad number '{s}'"))),
                }
            }
            continue;
        }
        // Symbols (longest match first)
        if let Some(sym) = SYMBOLS
            .iter()
            .find(|s| bytes[i..].iter().collect::<String>().starts_with(**s))
        {
            i += sym.len();
            out.push(Token {
                tok: Tok::Sym(sym),
                line,
            });
            continue;
        }
        diagnostics.push(err(line, format!("unexpected character '{c}'")));
        i += 1;
    }
    out.push(Token {
        tok: Tok::Eof,
        line,
    });
    out
}

// ======================================================================
// AST
// ======================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    Byte,
    Word,
    Bool,
}

impl Ty {
    fn size(self) -> u16 {
        match self {
            Ty::Byte => 1,
            Ty::Word | Ty::Bool => 2,
        }
    }
    fn is_byte(self) -> bool {
        self == Ty::Byte
    }
}

// Each node carries its source line for diagnostics. Not every variant reports
// errors today, so a couple of the line fields are currently unread.
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum Expr {
    Num(i64, usize),
    Var(String, usize),
    Index(String, Box<Expr>, usize),
    Unary(&'static str, Box<Expr>, usize),
    Binary(&'static str, Box<Expr>, Box<Expr>, usize),
    Call(String, Vec<Expr>, usize),
}

#[derive(Debug, Clone)]
enum Stmt {
    VarDecl {
        name: String,
        ty: Ty,
        len: Option<Expr>,
        init: Option<Expr>,
        line: usize,
    },
    Assign {
        name: String,
        index: Option<Expr>,
        value: Expr,
        line: usize,
    },
    If {
        cond: Expr,
        then: Vec<Stmt>,
        els: Option<Vec<Stmt>>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    Loop {
        body: Vec<Stmt>,
    },
    Break(usize),
    Return(Option<Expr>, usize),
    ExprStmt(Expr),
}

#[derive(Debug, Clone)]
enum Decl {
    Const {
        name: String,
        value: Expr,
        line: usize,
    },
    Var {
        name: String,
        ty: Ty,
        len: Option<Expr>,
        init: Option<Expr>,
        line: usize,
    },
    Proc {
        name: String,
        params: Vec<(String, Ty)>,
        ret: Option<Ty>,
        body: Vec<Stmt>,
        line: usize,
    },
}

// ======================================================================
// Parser (recursive descent, LL(1) + precedence climbing for expressions)
// ======================================================================

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }
    fn peek2(&self) -> &Tok {
        &self.tokens[(self.pos + 1).min(self.tokens.len() - 1)].tok
    }
    fn line(&self) -> usize {
        self.tokens[self.pos].line
    }
    fn advance(&mut self) -> Tok {
        let t = self.tokens[self.pos].tok.clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn eat_sym(&mut self, s: &str) -> bool {
        if matches!(self.peek(), Tok::Sym(x) if *x == s) {
            self.advance();
            true
        } else {
            false
        }
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        if matches!(self.peek(), Tok::Ident(x) if x == kw) {
            self.advance();
            true
        } else {
            false
        }
    }
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(x) if x == kw)
    }
    fn expect_sym(&mut self, s: &'static str, d: &mut Vec<Diagnostic>) {
        if !self.eat_sym(s) {
            d.push(err(self.line(), format!("expected '{s}'")));
        }
    }
    fn expect_ident(&mut self, d: &mut Vec<Diagnostic>) -> String {
        match self.advance() {
            Tok::Ident(s) => s,
            _ => {
                d.push(err(self.line(), "expected an identifier"));
                String::new()
            }
        }
    }

    fn parse_program(&mut self, d: &mut Vec<Diagnostic>) -> Vec<Decl> {
        let mut decls = Vec::new();
        let mut guard = 0;
        while !matches!(self.peek(), Tok::Eof) {
            let before = self.pos;
            if self.is_kw("const") {
                decls.push(self.parse_const(d));
            } else if self.is_kw("var") {
                decls.push(self.parse_global_var(d));
            } else if self.is_kw("proc") {
                decls.push(self.parse_proc(d));
            } else {
                d.push(err(self.line(), "expected 'const', 'var', or 'proc'"));
                self.advance();
            }
            // Avoid infinite loops on malformed input.
            if self.pos == before {
                self.advance();
            }
            guard += 1;
            if guard > 100_000 {
                break;
            }
        }
        decls
    }

    fn parse_type(&mut self, d: &mut Vec<Diagnostic>) -> (Ty, Option<Expr>) {
        // Array: "[" expr "]" type
        if self.eat_sym("[") {
            let len = self.parse_expr(d);
            self.expect_sym("]", d);
            let (ty, _) = self.parse_type(d);
            (ty, Some(len))
        } else {
            let line = self.line();
            let ty = match self.advance() {
                Tok::Ident(k) if k == "byte" => Ty::Byte,
                Tok::Ident(k) if k == "word" => Ty::Word,
                Tok::Ident(k) if k == "bool" => Ty::Bool,
                _ => {
                    d.push(err(line, "expected a type (byte, word, bool)"));
                    Ty::Word
                }
            };
            (ty, None)
        }
    }

    fn parse_const(&mut self, d: &mut Vec<Diagnostic>) -> Decl {
        let line = self.line();
        self.eat_kw("const");
        let name = self.expect_ident(d);
        self.expect_sym("=", d);
        let value = self.parse_expr(d);
        self.expect_sym(";", d);
        Decl::Const { name, value, line }
    }

    fn parse_global_var(&mut self, d: &mut Vec<Diagnostic>) -> Decl {
        let line = self.line();
        self.eat_kw("var");
        let name = self.expect_ident(d);
        self.expect_sym(":", d);
        let (ty, len) = self.parse_type(d);
        let init = if self.eat_sym("=") {
            Some(self.parse_expr(d))
        } else {
            None
        };
        self.expect_sym(";", d);
        Decl::Var {
            name,
            ty,
            len,
            init,
            line,
        }
    }

    fn parse_proc(&mut self, d: &mut Vec<Diagnostic>) -> Decl {
        let line = self.line();
        self.eat_kw("proc");
        let name = self.expect_ident(d);
        self.expect_sym("(", d);
        let mut params = Vec::new();
        if !matches!(self.peek(), Tok::Sym(")")) {
            loop {
                let pname = self.expect_ident(d);
                self.expect_sym(":", d);
                let (pty, _) = self.parse_type(d);
                params.push((pname, pty));
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        self.expect_sym(")", d);
        let ret = if self.eat_sym(":") {
            Some(self.parse_type(d).0)
        } else {
            None
        };
        let body = self.parse_block(d);
        Decl::Proc {
            name,
            params,
            ret,
            body,
            line,
        }
    }

    fn parse_block(&mut self, d: &mut Vec<Diagnostic>) -> Vec<Stmt> {
        self.expect_sym("{", d);
        let mut stmts = Vec::new();
        let mut guard = 0;
        while !matches!(self.peek(), Tok::Sym("}") | Tok::Eof) {
            let before = self.pos;
            stmts.push(self.parse_stmt(d));
            if self.pos == before {
                self.advance();
            }
            guard += 1;
            if guard > 100_000 {
                break;
            }
        }
        self.expect_sym("}", d);
        stmts
    }

    fn parse_stmt(&mut self, d: &mut Vec<Diagnostic>) -> Stmt {
        let line = self.line();
        if self.is_kw("var") {
            self.eat_kw("var");
            let name = self.expect_ident(d);
            self.expect_sym(":", d);
            let (ty, len) = self.parse_type(d);
            let init = if self.eat_sym("=") {
                Some(self.parse_expr(d))
            } else {
                None
            };
            self.expect_sym(";", d);
            return Stmt::VarDecl {
                name,
                ty,
                len,
                init,
                line,
            };
        }
        if self.eat_kw("if") {
            let cond = self.parse_expr(d);
            let then = self.parse_block(d);
            let els = if self.eat_kw("else") {
                if self.is_kw("if") {
                    Some(vec![self.parse_stmt(d)])
                } else {
                    Some(self.parse_block(d))
                }
            } else {
                None
            };
            return Stmt::If { cond, then, els };
        }
        if self.eat_kw("while") {
            let cond = self.parse_expr(d);
            let body = self.parse_block(d);
            return Stmt::While { cond, body };
        }
        if self.eat_kw("loop") {
            let body = self.parse_block(d);
            return Stmt::Loop { body };
        }
        if self.eat_kw("break") {
            self.expect_sym(";", d);
            return Stmt::Break(line);
        }
        if self.eat_kw("return") {
            let value = if matches!(self.peek(), Tok::Sym(";")) {
                None
            } else {
                Some(self.parse_expr(d))
            };
            self.expect_sym(";", d);
            return Stmt::Return(value, line);
        }
        if matches!(self.peek(), Tok::Sym("{")) {
            // A bare block. Flatten by wrapping in a Loop-free grouping: reuse If-less.
            let body = self.parse_block(d);
            return Stmt::If {
                cond: Expr::Num(1, line),
                then: body,
                els: None,
            };
        }
        // Assignment or call statement, both starting with IDENT.
        if let Tok::Ident(name) = self.peek().clone() {
            // call: IDENT "("
            if matches!(self.peek2(), Tok::Sym("(")) {
                let e = self.parse_expr(d);
                self.expect_sym(";", d);
                return Stmt::ExprStmt(e);
            }
            // indexed assign: IDENT "["
            self.advance(); // consume ident
            if self.eat_sym("[") {
                let idx = self.parse_expr(d);
                self.expect_sym("]", d);
                self.expect_sym("=", d);
                let value = self.parse_expr(d);
                self.expect_sym(";", d);
                return Stmt::Assign {
                    name,
                    index: Some(idx),
                    value,
                    line,
                };
            }
            // scalar assign: IDENT "="
            self.expect_sym("=", d);
            let value = self.parse_expr(d);
            self.expect_sym(";", d);
            return Stmt::Assign {
                name,
                index: None,
                value,
                line,
            };
        }
        d.push(err(line, "expected a statement"));
        self.advance();
        Stmt::Break(line) // placeholder; diagnostics already recorded
    }

    // ---- expressions (precedence climbing) ----

    fn parse_expr(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        self.parse_or(d)
    }
    fn parse_or(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        let mut left = self.parse_and(d);
        while self.is_kw("or") {
            let line = self.line();
            self.advance();
            let right = self.parse_and(d);
            left = Expr::Binary("or", Box::new(left), Box::new(right), line);
        }
        left
    }
    fn parse_and(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        let mut left = self.parse_equality(d);
        while self.is_kw("and") {
            let line = self.line();
            self.advance();
            let right = self.parse_equality(d);
            left = Expr::Binary("and", Box::new(left), Box::new(right), line);
        }
        left
    }
    fn parse_equality(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        let mut left = self.parse_comparison(d);
        while let Tok::Sym(op @ ("==" | "!=")) = self.peek() {
            let op = *op;
            let line = self.line();
            self.advance();
            let right = self.parse_comparison(d);
            left = Expr::Binary(op, Box::new(left), Box::new(right), line);
        }
        left
    }
    fn parse_comparison(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        let mut left = self.parse_bitor(d);
        while let Tok::Sym(op @ ("<" | "<=" | ">" | ">=")) = self.peek() {
            let op = *op;
            let line = self.line();
            self.advance();
            let right = self.parse_bitor(d);
            left = Expr::Binary(op, Box::new(left), Box::new(right), line);
        }
        left
    }
    fn parse_bitor(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        let mut left = self.parse_bitand(d);
        while let Tok::Sym(op @ ("|" | "^")) = self.peek() {
            let op = *op;
            let line = self.line();
            self.advance();
            let right = self.parse_bitand(d);
            left = Expr::Binary(op, Box::new(left), Box::new(right), line);
        }
        left
    }
    fn parse_bitand(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        let mut left = self.parse_shift(d);
        while let Tok::Sym("&") = self.peek() {
            let line = self.line();
            self.advance();
            let right = self.parse_shift(d);
            left = Expr::Binary("&", Box::new(left), Box::new(right), line);
        }
        left
    }
    fn parse_shift(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        let mut left = self.parse_term(d);
        while let Tok::Sym(op @ ("<<" | ">>")) = self.peek() {
            let op = *op;
            let line = self.line();
            self.advance();
            let right = self.parse_term(d);
            left = Expr::Binary(op, Box::new(left), Box::new(right), line);
        }
        left
    }
    fn parse_term(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        let mut left = self.parse_factor(d);
        while let Tok::Sym(op @ ("+" | "-")) = self.peek() {
            let op = *op;
            let line = self.line();
            self.advance();
            let right = self.parse_factor(d);
            left = Expr::Binary(op, Box::new(left), Box::new(right), line);
        }
        left
    }
    fn parse_factor(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        let mut left = self.parse_unary(d);
        while let Tok::Sym(op @ ("*" | "/" | "%")) = self.peek() {
            let op = *op;
            let line = self.line();
            self.advance();
            let right = self.parse_unary(d);
            left = Expr::Binary(op, Box::new(left), Box::new(right), line);
        }
        left
    }
    fn parse_unary(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        let line = self.line();
        if self.eat_sym("-") {
            return Expr::Unary("-", Box::new(self.parse_unary(d)), line);
        }
        if self.eat_sym("~") {
            return Expr::Unary("~", Box::new(self.parse_unary(d)), line);
        }
        if self.is_kw("not") {
            self.advance();
            return Expr::Unary("not", Box::new(self.parse_unary(d)), line);
        }
        self.parse_primary(d)
    }
    fn parse_primary(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        let line = self.line();
        match self.peek().clone() {
            Tok::Num(n) => {
                self.advance();
                Expr::Num(n, line)
            }
            Tok::Ident(kw) if kw == "true" => {
                self.advance();
                Expr::Num(1, line)
            }
            Tok::Ident(kw) if kw == "false" => {
                self.advance();
                Expr::Num(0, line)
            }
            Tok::Ident(name) => {
                self.advance();
                if self.eat_sym("(") {
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Tok::Sym(")")) {
                        loop {
                            args.push(self.parse_expr(d));
                            if !self.eat_sym(",") {
                                break;
                            }
                        }
                    }
                    self.expect_sym(")", d);
                    Expr::Call(name, args, line)
                } else if self.eat_sym("[") {
                    let idx = self.parse_expr(d);
                    self.expect_sym("]", d);
                    Expr::Index(name, Box::new(idx), line)
                } else {
                    Expr::Var(name, line)
                }
            }
            Tok::Sym("(") => {
                self.advance();
                let e = self.parse_expr(d);
                self.expect_sym(")", d);
                e
            }
            _ => {
                d.push(err(line, "expected an expression"));
                self.advance();
                Expr::Num(0, line)
            }
        }
    }
}

// ======================================================================
// Compiler / codegen
// ======================================================================

#[derive(Clone)]
struct VarInfo {
    label: String,
    ty: Ty,
    len: Option<u16>, // Some => array
}

struct ProcInfo {
    params: Vec<(String, Ty)>,
    ret: Option<Ty>,
}

struct Compiler {
    consts: HashMap<String, i64>,
    globals: HashMap<String, VarInfo>,
    procs: HashMap<String, ProcInfo>,
    // per-proc scope
    locals: HashMap<String, VarInfo>,
    data: Vec<String>,   // .res/.byte/.word declarations
    label_ctr: usize,
    loop_ends: Vec<String>,
    cur_proc: String,
    need_tmp: bool,
}

/// Builtins: name -> (arg count, returns a value).
fn builtin(name: &str) -> Option<(usize, bool)> {
    Some(match name {
        "clear" => (1, false),
        "pixel" => (3, false),
        "sprite" => (3, false),
        "entity" => (3, false),
        "poke8" => (2, false),
        "poke16" => (2, false),
        "button" => (1, true),
        "buttons" => (0, true),
        "rnd" => (0, true),
        "peek8" => (1, true),
        "peek16" => (1, true),
        _ => return None,
    })
}

/// Predefined gamepad button constants (usable as `button(LEFT)` etc.).
const BUTTON_CONSTS: &[(&str, i64)] = &[
    ("LEFT", 0x01),
    ("RIGHT", 0x02),
    ("UP", 0x04),
    ("DOWN", 0x08),
    ("A", 0x10),
    ("B", 0x20),
    ("START", 0x40),
    ("SELECT", 0x80),
];

impl Compiler {
    fn new() -> Self {
        let consts = BUTTON_CONSTS
            .iter()
            .map(|(n, v)| (n.to_string(), *v))
            .collect();
        Compiler {
            consts,
            globals: HashMap::new(),
            procs: HashMap::new(),
            locals: HashMap::new(),
            data: Vec::new(),
            label_ctr: 0,
            loop_ends: Vec::new(),
            cur_proc: String::new(),
            need_tmp: false,
        }
    }

    fn new_label(&mut self) -> String {
        let l = format!("ux_L{}", self.label_ctr);
        self.label_ctr += 1;
        l
    }

    fn compile(&mut self, decls: &[Decl], d: &mut Vec<Diagnostic>) -> String {
        // Pass 1: collect consts, globals, proc signatures.
        for decl in decls {
            match decl {
                Decl::Const { name, value, line } => {
                    match self.eval_const(value, d) {
                        Some(v) => {
                            self.consts.insert(name.clone(), v);
                        }
                        None => d.push(err(*line, format!("const '{name}' is not a constant expression"))),
                    }
                }
                Decl::Var {
                    name, ty, len, line, ..
                } => {
                    self.declare_var(name, *ty, len.as_ref(), &format!("ux_g_{name}"), *line, d);
                }
                Decl::Proc {
                    name, params, ret, ..
                } => {
                    if self.procs.contains_key(name) {
                        // duplicate reported below via redefinition
                    }
                    self.procs.insert(
                        name.clone(),
                        ProcInfo {
                            params: params.clone(),
                            ret: *ret,
                        },
                    );
                }
            }
        }

        // Pass 2: emit global data + proc bodies.
        let mut body = String::new();
        for decl in decls {
            if let Decl::Var {
                name, init, ty, len, line,
            } = decl
            {
                self.emit_global_data(name, *ty, len.as_ref(), init.as_ref(), *line, d);
            }
        }
        for decl in decls {
            if let Decl::Proc {
                name,
                params,
                body: pbody,
                ret,
                line,
            } = decl
            {
                body.push_str(&self.compile_proc(name, params, pbody, *ret, *line, d));
            }
        }

        self.assemble_program(&body)
    }

    fn declare_var(
        &mut self,
        name: &str,
        ty: Ty,
        len: Option<&Expr>,
        label: &str,
        line: usize,
        d: &mut Vec<Diagnostic>,
    ) {
        let len_val = match len {
            Some(e) => match self.eval_const(e, d) {
                Some(v) if v > 0 => Some(v as u16),
                _ => {
                    d.push(err(line, "array length must be a positive constant"));
                    Some(1)
                }
            },
            None => None,
        };
        let info = VarInfo {
            label: label.to_string(),
            ty,
            len: len_val,
        };
        if label.starts_with("ux_g_") {
            self.globals.insert(name.to_string(), info);
        } else {
            self.locals.insert(name.to_string(), info);
        }
    }

    fn emit_global_data(
        &mut self,
        name: &str,
        ty: Ty,
        len: Option<&Expr>,
        init: Option<&Expr>,
        line: usize,
        d: &mut Vec<Diagnostic>,
    ) {
        let label = format!("ux_g_{name}");
        let len_val = len.and_then(|e| self.eval_const(e, d)).map(|v| v as u16);
        if let Some(n) = len_val {
            if init.is_some() {
                d.push(err(line, "array initializers are not supported"));
            }
            self.data.push(format!("@{label} .res {}", n * ty.size()));
        } else if let Some(e) = init {
            match self.eval_const(e, d) {
                Some(v) => {
                    let v = (v as i64 & 0xffff) as u16;
                    if ty.is_byte() {
                        self.data.push(format!("@{label} .byte {}", v & 0xff));
                    } else {
                        self.data.push(format!("@{label} .word {v}"));
                    }
                }
                None => {
                    d.push(err(line, format!("global '{name}' initializer must be constant (use init() for computed values)")));
                    self.data.push(format!("@{label} .res {}", ty.size()));
                }
            }
        } else {
            self.data.push(format!("@{label} .res {}", ty.size()));
        }
    }

    fn compile_proc(
        &mut self,
        name: &str,
        params: &[(String, Ty)],
        body: &[Stmt],
        _ret: Option<Ty>,
        _line: usize,
        d: &mut Vec<Diagnostic>,
    ) -> String {
        self.locals.clear();
        self.cur_proc = name.to_string();

        // Declare params as static slots.
        for (pname, pty) in params {
            let label = format!("ux_l_{name}_{pname}");
            self.locals.insert(
                pname.clone(),
                VarInfo {
                    label: label.clone(),
                    ty: *pty,
                    len: None,
                },
            );
            self.data.push(format!("@{label} .res {}", pty.size()));
        }

        let mut out: Vec<String> = Vec::new();
        // Prologue: pop args (reverse order) into slots.
        for (pname, pty) in params.iter().rev() {
            let label = format!("ux_l_{name}_{pname}");
            out.push(format!("{label} {}", store_op(*pty)));
            let _ = pname;
        }

        self.gen_block(body, &mut out, d);
        out.push("RET".to_string());

        format!("@ux_p_{name}\n  {}\n", out.join(" "))
    }

    fn gen_block(&mut self, stmts: &[Stmt], out: &mut Vec<String>, d: &mut Vec<Diagnostic>) {
        for s in stmts {
            self.gen_stmt(s, out, d);
        }
    }

    fn gen_stmt(&mut self, s: &Stmt, out: &mut Vec<String>, d: &mut Vec<Diagnostic>) {
        match s {
            Stmt::VarDecl {
                name, ty, len, init, line,
            } => {
                let label = format!("ux_l_{}_{}", self.cur_proc, name);
                self.declare_var(name, *ty, len.as_ref(), &label, *line, d);
                let len_val = self.locals.get(name).and_then(|v| v.len);
                let total = len_val.map(|n| n * ty.size()).unwrap_or_else(|| ty.size());
                self.data.push(format!("@{label} .res {total}"));
                if let Some(e) = init {
                    if len_val.is_some() {
                        d.push(err(*line, "array initializers are not supported"));
                    } else {
                        self.gen_expr(e, out, d);
                        out.push(format!("{label} {}", store_op(*ty)));
                    }
                }
            }
            Stmt::Assign {
                name, index, value, line,
            } => {
                let info = match self.resolve_var(name) {
                    Some(v) => v,
                    None => {
                        d.push(err(*line, format!("unknown variable '{name}'")));
                        return;
                    }
                };
                match index {
                    Some(idx) => {
                        // value, then element address, then store.
                        self.gen_expr(value, out, d);
                        self.gen_elem_addr(&info, idx, out, d);
                        out.push(store_op(info.ty).to_string());
                    }
                    None => {
                        self.gen_expr(value, out, d);
                        out.push(format!("{} {}", info.label, store_op(info.ty)));
                    }
                }
            }
            Stmt::If { cond, then, els } => {
                let else_l = self.new_label();
                self.gen_expr(cond, out, d);
                out.push(format!("{else_l} JZ"));
                self.gen_block(then, out, d);
                if let Some(else_body) = els {
                    let end_l = self.new_label();
                    out.push(format!("{end_l} JMP @{else_l}"));
                    self.gen_block(else_body, out, d);
                    out.push(format!("@{end_l}"));
                } else {
                    out.push(format!("@{else_l}"));
                }
            }
            Stmt::While { cond, body } => {
                let top = self.new_label();
                let end = self.new_label();
                out.push(format!("@{top}"));
                self.gen_expr(cond, out, d);
                out.push(format!("{end} JZ"));
                self.loop_ends.push(end.clone());
                self.gen_block(body, out, d);
                self.loop_ends.pop();
                out.push(format!("{top} JMP @{end}"));
            }
            Stmt::Loop { body } => {
                let top = self.new_label();
                let end = self.new_label();
                out.push(format!("@{top}"));
                self.loop_ends.push(end.clone());
                self.gen_block(body, out, d);
                self.loop_ends.pop();
                out.push(format!("{top} JMP @{end}"));
            }
            Stmt::Break(line) => match self.loop_ends.last() {
                Some(end) => out.push(format!("{end} JMP")),
                None => d.push(err(*line, "'break' outside a loop")),
            },
            Stmt::Return(value, _line) => {
                if let Some(e) = value {
                    self.gen_expr(e, out, d);
                }
                out.push("RET".to_string());
            }
            Stmt::ExprStmt(e) => {
                let yields = self.gen_expr(e, out, d);
                if yields {
                    out.push("DROP".to_string()); // discard unused value
                }
            }
        }
    }

    /// Emit code that pushes the address of `info[index]`.
    fn gen_elem_addr(
        &mut self,
        info: &VarInfo,
        index: &Expr,
        out: &mut Vec<String>,
        d: &mut Vec<Diagnostic>,
    ) {
        out.push(info.label.clone()); // base address
        self.gen_expr(index, out, d);
        let sz = info.ty.size();
        if sz != 1 {
            out.push(format!("#{sz:02x} MUL"));
        }
        out.push("ADD".to_string());
    }

    /// Generate code for an expression, leaving its value on the stack.
    /// Returns whether a value was produced (false for void calls).
    fn gen_expr(&mut self, e: &Expr, out: &mut Vec<String>, d: &mut Vec<Diagnostic>) -> bool {
        match e {
            Expr::Num(n, _) => {
                out.push(((*n as i64 & 0xffff) as u16).to_string());
                true
            }
            Expr::Var(name, line) => {
                if let Some(v) = self.consts.get(name) {
                    out.push(((*v & 0xffff) as u16).to_string());
                    return true;
                }
                match self.resolve_var(name) {
                    Some(info) if info.len.is_some() => {
                        // Bare array name -> its base address.
                        out.push(info.label);
                        true
                    }
                    Some(info) => {
                        out.push(format!("{} {}", info.label, load_op(info.ty)));
                        true
                    }
                    None => {
                        d.push(err(*line, format!("unknown identifier '{name}'")));
                        out.push("0".to_string());
                        true
                    }
                }
            }
            Expr::Index(name, idx, line) => {
                match self.resolve_var(name) {
                    Some(info) if info.len.is_some() => {
                        self.gen_elem_addr(&info, idx, out, d);
                        out.push(load_op(info.ty).to_string());
                    }
                    Some(_) => d.push(err(*line, format!("'{name}' is not an array"))),
                    None => d.push(err(*line, format!("unknown array '{name}'"))),
                }
                true
            }
            Expr::Unary(op, inner, _) => {
                match *op {
                    "-" => {
                        out.push("0".to_string());
                        self.gen_expr(inner, out, d);
                        out.push("SUB".to_string());
                    }
                    "~" => {
                        self.gen_expr(inner, out, d);
                        out.push("#ffff XOR".to_string());
                    }
                    "not" => {
                        self.gen_expr(inner, out, d);
                        out.push("#00 EQ".to_string());
                    }
                    _ => {}
                }
                true
            }
            Expr::Binary(op, l, r, _) => {
                self.gen_binary(op, l, r, out, d);
                true
            }
            Expr::Call(name, args, line) => self.gen_call(name, args, out, d, *line),
        }
    }

    fn gen_binary(
        &mut self,
        op: &str,
        l: &Expr,
        r: &Expr,
        out: &mut Vec<String>,
        d: &mut Vec<Diagnostic>,
    ) {
        // Logical and/or normalize both sides to 0/1 first.
        if op == "and" || op == "or" {
            self.gen_expr(l, out, d);
            out.push("#00 NE".to_string());
            self.gen_expr(r, out, d);
            out.push("#00 NE".to_string());
            out.push(if op == "and" { "AND" } else { "OR" }.to_string());
            return;
        }
        self.gen_expr(l, out, d);
        self.gen_expr(r, out, d);
        let ops: &[&str] = match op {
            "+" => &["ADD"],
            "-" => &["SUB"],
            "*" => &["MUL"],
            "/" => &["DIV"],
            "%" => &["MOD"],
            "&" => &["AND"],
            "|" => &["OR"],
            "^" => &["XOR"],
            "<<" => &["SHL"],
            ">>" => &["SHR"],
            "==" => &["EQ"],
            "!=" => &["NE"],
            "<" => &["LT"],
            ">" => &["GT"],
            "<=" => &["GT", "#00", "EQ"], // !(a>b)
            ">=" => &["LT", "#00", "EQ"], // !(a<b)
            _ => &["ADD"],
        };
        for o in ops {
            out.push(o.to_string());
        }
    }

    fn gen_call(
        &mut self,
        name: &str,
        args: &[Expr],
        out: &mut Vec<String>,
        d: &mut Vec<Diagnostic>,
        line: usize,
    ) -> bool {
        if let Some((argc, yields)) = builtin(name) {
            if args.len() != argc {
                d.push(err(line, format!("{name}() takes {argc} argument(s), got {}", args.len())));
            }
            for a in args {
                self.gen_expr(a, out, d);
            }
            self.gen_builtin(name, out);
            return yields;
        }
        if let Some(sig) = self.procs.get(name) {
            let (argc, yields) = (sig.params.len(), sig.ret.is_some());
            if args.len() != argc {
                d.push(err(line, format!("{name}() takes {argc} argument(s), got {}", args.len())));
            }
            for a in args {
                self.gen_expr(a, out, d);
            }
            out.push(format!("ux_p_{name} CALL"));
            return yields;
        }
        d.push(err(line, format!("unknown procedure '{name}'")));
        false
    }

    fn gen_builtin(&mut self, name: &str, out: &mut Vec<String>) {
        // Args are already on the stack (left-to-right).
        let seq: &str = match name {
            "clear" => "#16 DEO",                        // ( color )
            "pixel" => "#13 DEO #12 DEO #11 DEO #00 #14 DEO", // ( x y color )
            "sprite" => "#12 DEO #11 DEO #15 DEO",       // ( tile x y )
            "poke8" => "SWAP STORE8",                    // ( addr val )
            "poke16" => "SWAP STORE16",                  // ( addr val )
            "button" => "#20 DEI AND #00 NE",            // ( mask ) -> 0/1
            "buttons" => "#20 DEI",
            "rnd" => "#30 DEI",
            "peek8" => "LOAD8",                          // ( addr )
            "peek16" => "LOAD16",                        // ( addr )
            "entity" => {
                self.need_tmp = true;
                "ux_tmp STORE16 #51 DEO #50 DEO ux_tmp LOAD16 #52 DEO" // ( x y tag )
            }
            _ => "",
        };
        if !seq.is_empty() {
            out.push(seq.to_string());
        }
    }

    fn resolve_var(&self, name: &str) -> Option<VarInfo> {
        self.locals
            .get(name)
            .or_else(|| self.globals.get(name))
            .cloned()
    }

    /// Evaluate a constant expression to an i64, or None if not constant.
    fn eval_const(&self, e: &Expr, d: &mut Vec<Diagnostic>) -> Option<i64> {
        match e {
            Expr::Num(n, _) => Some(*n),
            Expr::Var(name, _) => self.consts.get(name).copied(),
            Expr::Unary(op, inner, _) => {
                let v = self.eval_const(inner, d)?;
                Some(match *op {
                    "-" => -v,
                    "~" => !v,
                    "not" => (v == 0) as i64,
                    _ => return None,
                })
            }
            Expr::Binary(op, l, r, line) => {
                let a = self.eval_const(l, d)?;
                let b = self.eval_const(r, d)?;
                Some(match *op {
                    "+" => a + b,
                    "-" => a - b,
                    "*" => a * b,
                    "/" => {
                        if b == 0 {
                            d.push(err(*line, "constant division by zero"));
                            return None;
                        }
                        a / b
                    }
                    "%" => {
                        if b == 0 {
                            d.push(err(*line, "constant modulo by zero"));
                            return None;
                        }
                        a % b
                    }
                    "&" => a & b,
                    "|" => a | b,
                    "^" => a ^ b,
                    "<<" => a << b,
                    ">>" => a >> b,
                    "==" => (a == b) as i64,
                    "!=" => (a != b) as i64,
                    "<" => (a < b) as i64,
                    "<=" => (a <= b) as i64,
                    ">" => (a > b) as i64,
                    ">=" => (a >= b) as i64,
                    "and" => ((a != 0) && (b != 0)) as i64,
                    "or" => ((a != 0) || (b != 0)) as i64,
                    _ => return None,
                })
            }
            _ => None,
        }
    }

    /// Stitch reset/frame wiring, proc bodies, globals, and helpers together.
    fn assemble_program(&mut self, procs: &str) -> String {
        let mut out = String::new();
        out.push_str("( generated by the uxlang front-end )\n");
        if self.procs.contains_key("init") {
            out.push_str("ux_p_init CALL\n");
        }
        out.push_str("ux_frame #10 DEO\nRET\n\n");

        out.push_str("@ux_frame\n");
        let has_ud = self.procs.contains_key("update") || self.procs.contains_key("draw");
        if self.procs.contains_key("update") {
            out.push_str("  ux_p_update CALL\n");
        }
        if self.procs.contains_key("draw") {
            out.push_str("  ux_p_draw CALL\n");
        }
        if !has_ud && self.procs.contains_key("frame") {
            out.push_str("  ux_p_frame CALL\n");
        }
        out.push_str("  RET\n\n");

        out.push_str(procs);
        out.push('\n');
        for line in &self.data {
            out.push_str(line);
            out.push('\n');
        }
        if self.need_tmp {
            out.push_str("@ux_tmp .res 2\n");
        }
        out
    }
}

fn load_op(ty: Ty) -> &'static str {
    if ty.is_byte() {
        "LOAD8"
    } else {
        "LOAD16"
    }
}
fn store_op(ty: Ty) -> &'static str {
    if ty.is_byte() {
        "STORE8"
    } else {
        "STORE16"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::assembler::assemble;
    use crate::vm::device::{BTN_LEFT, BTN_RIGHT};
    use crate::vm::VmConsole;

    fn compile_ok(src: &str) -> String {
        let c = compile(src);
        assert!(c.ok(), "uxlang diagnostics: {:?}", c.diagnostics);
        let built = assemble(&c.asm);
        assert!(built.ok(), "generated asm errors: {:?}\nASM:\n{}", built.diagnostics, c.asm);
        c.asm
    }

    fn load(src: &str) -> VmConsole {
        let c = compile(src);
        assert!(c.ok(), "uxlang diagnostics: {:?}", c.diagnostics);
        let mut console = VmConsole::new();
        console.write_source("game.ux", src);
        assert!(console.assemble("game.ux").unwrap().ok());
        console.load_rom("game.ux").unwrap();
        console
    }

    #[test]
    fn mover_compiles_and_runs() {
        let src = r#"
            var player_x: word = 32;

            proc update() {
                if button(LEFT)  { player_x = player_x - 1; }
                if button(RIGHT) { player_x = player_x + 1; }
            }
            proc draw() {
                clear(0);
                pixel(player_x, 60, 7);
                entity(player_x, 60, 1);
            }
        "#;
        compile_ok(src);
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 32);
        assert_eq!(c.run_frame(BTN_RIGHT).entities[0].x, 33);
        assert_eq!(c.run_frame(BTN_RIGHT).entities[0].x, 34);
        assert_eq!(c.run_frame(BTN_LEFT).entities[0].x, 33);
    }

    #[test]
    fn init_runs_once_at_reset() {
        let src = r#"
            var n: word;
            proc init() { n = 100; }
            proc draw() { entity(n, 0, 1); }
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 100);
    }

    #[test]
    fn if_else_and_comparisons() {
        // pick(a): if a >= 10 -> report 1 else 0
        let src = r#"
            var out: word;
            var a: word = 12;
            proc draw() {
                if a >= 10 { out = 1; } else { out = 0; }
                entity(out, 0, 1);
            }
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 1);
    }

    #[test]
    fn while_loop_sums() {
        // sum 1..=5 = 15
        let src = r#"
            var i: word;
            var sum: word;
            proc draw() {
                i = 1; sum = 0;
                while i <= 5 { sum = sum + i; i = i + 1; }
                entity(sum, 0, 1);
            }
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 15);
    }

    #[test]
    fn loop_with_break() {
        let src = r#"
            var i: word;
            proc draw() {
                i = 0;
                loop {
                    i = i + 1;
                    if i == 7 { break; }
                }
                entity(i, 0, 1);
            }
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 7);
    }

    #[test]
    fn proc_with_params_and_return() {
        let src = r#"
            var out: word;
            proc add3(a: word, b: word, c: word): word {
                return a + b + c;
            }
            proc draw() { out = add3(10, 20, 5); entity(out, 0, 1); }
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 35);
    }

    #[test]
    fn const_and_arithmetic() {
        let src = r#"
            const W = 128;
            var x: word;
            proc draw() { x = W - 8; entity(x, 0, 1); }
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 120);
    }

    #[test]
    fn array_indexing() {
        let src = r#"
            var a: [4]word;
            var out: word;
            proc draw() {
                a[0] = 11; a[1] = 22; a[2] = 33;
                out = a[1] + a[2];
                entity(out, 0, 1);
            }
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 55);
    }

    #[test]
    fn byte_type_stores_one_byte() {
        // A byte truncates to 8 bits: 300 & 0xff = 44.
        let src = r#"
            var b: byte;
            proc draw() { b = 300; entity(b, 0, 1); }
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 44);
    }

    #[test]
    fn logical_and_normalizes() {
        // button() returns 0/1; test and/or with non-1 truthy values via & mask.
        let src = r#"
            var out: word;
            proc draw() {
                if (2 != 0) and (4 != 0) { out = 9; } else { out = 0; }
                entity(out, 0, 1);
            }
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 9);
    }

    #[test]
    fn sprite_blits_via_builtin() {
        let src = r#"
            var tile: [32]byte;
            proc init() {
                tile[0] = 18;   /* pixels 1,2 */
                tile[1] = 52;   /* pixels 3,4 */
            }
            proc draw() { sprite(tile, 0, 0); }
        "#;
        let mut c = load(src);
        c.run_frame(0);
        assert_eq!(c.vm.devices.framebuffer[0], 1);
        assert_eq!(c.vm.devices.framebuffer[1], 2);
        assert_eq!(c.vm.devices.framebuffer[2], 3);
        assert_eq!(c.vm.devices.framebuffer[3], 4);
    }

    #[test]
    fn diagnostics() {
        assert!(!compile("proc draw() { x = 1; }").ok()); // unknown var
        assert!(!compile("proc draw() { foo(); }").ok()); // unknown proc
        assert!(!compile("var x: word; proc draw() { x = 1 }").ok()); // missing ;
        assert!(!compile("proc draw() { break; }").ok()); // break outside loop
        assert!(!compile("proc draw() { clear(); }").ok()); // wrong arg count
    }
}
