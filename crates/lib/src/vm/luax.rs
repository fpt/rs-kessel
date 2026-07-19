//! `luax` — a small, statically-typed **Lua-flavored** language that compiles to
//! the [`super::assembler`] dialect. It replaces the earlier C/Pascal-ish
//! front-end: models have strong Lua priors (PICO-8/TIC-80/Löve), so a Lua
//! surface lets them reuse that knowledge and resist hallucinating `import` /
//! `malloc` / SDL. It is **not** real Lua — no `require`, metatables, coroutines,
//! closures, varargs, GC, or stdlib; tables are compile-time **records** and
//! arrays are fixed-length.
//!
//! ```lua
//! record Ball { x, y, vx, vy, color: byte }   -- fields default to `word`
//!
//! local ball: Ball            -- top-level local = a global (persistent state)
//! local GRAVITY = 1           -- constant-initialized local folds as a constant
//!
//! function init() ball.x = 20  ball.y = 30  ball.vx = 1  ball.vy = 1 end
//!
//! function move(b: Ball)      -- records pass by address (mutable)
//!   b.x = b.x + b.vx
//!   if b.x >= 118 or b.x <= 2 then b.vx = 0 - b.vx end
//! end
//!
//! function update() move(ball) end
//! function draw()
//!   cls(0)
//!   pset(ball.x, ball.y, ball.color)
//!   entity(ball.x, ball.y, 1)
//! end
//! ```
//!
//! Entry points (as before): `init` runs once at reset; `update` then `draw` run
//! each frame (or a single `frame`). Locals/params use static slots — **no
//! recursion**. Everything lowers by a post-order walk onto the VM data stack;
//! generated labels are `lx_`-prefixed so a function named `add` can't emit the
//! `ADD` opcode.

use std::collections::HashMap;

use super::assembler::Diagnostic;

/// Result of compiling luax source: generated assembler text plus diagnostics.
pub struct Compiled {
    pub asm: String,
    pub diagnostics: Vec<Diagnostic>,
}

impl Compiled {
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Compile luax source into assembler text.
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

// Longest-match first.
const SYMBOLS: &[&str] = &[
    "==", "~=", "<=", ">=", "<<", ">>", "+", "-", "*", "/", "%", "&", "|", "~", "<", ">", "=", "(",
    ")", "{", "}", "[", "]", ",", ":", ".",
];

fn lex(src: &str, diagnostics: &mut Vec<Diagnostic>) -> Vec<Token> {
    let b: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut line = 1usize;
    let mut out = Vec::new();

    while i < b.len() {
        let c = b[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Comments: -- line, --[[ ]] block.
        if c == '-' && i + 1 < b.len() && b[i + 1] == '-' {
            if i + 3 < b.len() && b[i + 2] == '[' && b[i + 3] == '[' {
                i += 4;
                while i + 1 < b.len() && !(b[i] == ']' && b[i + 1] == ']') {
                    if b[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 2;
            } else {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            }
            continue;
        }
        // Identifier / keyword.
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == '_') {
                i += 1;
            }
            out.push(Token {
                tok: Tok::Ident(b[start..i].iter().collect()),
                line,
            });
            continue;
        }
        // Number.
        if c.is_ascii_digit() {
            let start = i;
            if c == '0' && i + 1 < b.len() && (b[i + 1] == 'x' || b[i + 1] == 'X') {
                i += 2;
                while i < b.len() && b[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let s: String = b[start + 2..i].iter().collect();
                match i64::from_str_radix(&s, 16) {
                    Ok(v) => out.push(Token { tok: Tok::Num(v), line }),
                    Err(_) => diagnostics.push(err(line, format!("bad hex literal '0x{s}'"))),
                }
            } else {
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                let s: String = b[start..i].iter().collect();
                match s.parse::<i64>() {
                    Ok(v) => out.push(Token { tok: Tok::Num(v), line }),
                    Err(_) => diagnostics.push(err(line, format!("bad number '{s}'"))),
                }
            }
            continue;
        }
        // Symbol.
        if let Some(sym) = SYMBOLS
            .iter()
            .find(|s| b[i..].iter().collect::<String>().starts_with(**s))
        {
            i += sym.len();
            out.push(Token { tok: Tok::Sym(sym), line });
            // Raw-capture a `sprite NAME { <rows> }` body: pixel rows like
            // `..2222..` aren't lexable as normal tokens, so once we see the
            // opening `{` of a sprite block, scan whitespace-separated rows
            // verbatim (each becomes an Ident) up to the matching `}`.
            if *sym == "{"
                && out.len() >= 3
                && matches!(&out[out.len() - 3].tok, Tok::Ident(k) if k == "sprite")
            {
                while i < b.len() {
                    let cc = b[i];
                    if cc == '\n' {
                        line += 1;
                        i += 1;
                    } else if cc.is_whitespace() {
                        i += 1;
                    } else if cc == '}' {
                        i += 1;
                        out.push(Token { tok: Tok::Sym("}"), line });
                        break;
                    } else if cc == '-' && i + 1 < b.len() && b[i + 1] == '-' {
                        // Comments are valid inside a sprite body too: `--[[ ]]`
                        // block or `--` to end of line (not pixel rows).
                        if i + 3 < b.len() && b[i + 2] == '[' && b[i + 3] == '[' {
                            i += 4;
                            while i + 1 < b.len() && !(b[i] == ']' && b[i + 1] == ']') {
                                if b[i] == '\n' {
                                    line += 1;
                                }
                                i += 1;
                            }
                            i += 2;
                        } else {
                            while i < b.len() && b[i] != '\n' {
                                i += 1;
                            }
                        }
                    } else {
                        let start = i;
                        while i < b.len() && !b[i].is_whitespace() && b[i] != '}' {
                            i += 1;
                        }
                        out.push(Token {
                            tok: Tok::Ident(b[start..i].iter().collect()),
                            line,
                        });
                    }
                }
            }
            continue;
        }
        diagnostics.push(err(line, format!("unexpected character '{c}'")));
        i += 1;
    }
    out.push(Token { tok: Tok::Eof, line });
    out
}

// ======================================================================
// AST
// ======================================================================

/// A resolved scalar/aggregate type.
#[derive(Debug, Clone, PartialEq)]
enum Ty {
    Byte,
    Word,
    Int, // 16-bit signed (two's complement); comparisons are signed
    Bool,
    Record(String, u16),      // name, byte size
    Array(Box<Ty>, u16),      // element, length
}

impl Ty {
    fn size(&self) -> u16 {
        match self {
            Ty::Byte => 1,
            Ty::Word | Ty::Int | Ty::Bool => 2,
            Ty::Record(_, sz) => *sz,
            Ty::Array(e, n) => e.size() * n,
        }
    }
    fn is_scalar(&self) -> bool {
        matches!(self, Ty::Byte | Ty::Word | Ty::Int | Ty::Bool)
    }
    fn is_byte(&self) -> bool {
        matches!(self, Ty::Byte)
    }
    fn is_int(&self) -> bool {
        matches!(self, Ty::Int)
    }
}

/// A syntactic type as written (resolved to `Ty` by the compiler).
#[derive(Debug, Clone)]
enum TypeExpr {
    Scalar(Ty),
    Named(String, usize),
    Array(Box<TypeExpr>, Box<Expr>, usize),
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
enum Expr {
    Num(i64, usize),
    Var(String, usize),
    Field(Box<Expr>, String, usize),
    Index(Box<Expr>, Box<Expr>, usize),
    Unary(&'static str, Box<Expr>, usize),
    Binary(&'static str, Box<Expr>, Box<Expr>, usize),
    Call(String, Vec<Expr>, usize),
}

impl Expr {
    fn line(&self) -> usize {
        match self {
            Expr::Num(_, l)
            | Expr::Var(_, l)
            | Expr::Field(_, _, l)
            | Expr::Index(_, _, l)
            | Expr::Unary(_, _, l)
            | Expr::Binary(_, _, _, l)
            | Expr::Call(_, _, l) => *l,
        }
    }
}

#[derive(Debug, Clone)]
enum Stmt {
    Local {
        name: String,
        ty: Option<TypeExpr>,
        init: Option<Expr>,
        line: usize,
    },
    Assign {
        place: Expr,
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
    For {
        var: String,
        from: Expr,
        to: Expr,
        step: Option<Expr>,
        body: Vec<Stmt>,
        line: usize,
    },
    Break(usize),
    Return(Option<Expr>, usize),
    ExprStmt(Expr),
}

#[derive(Debug, Clone)]
enum Decl {
    Record {
        name: String,
        fields: Vec<(String, Ty)>,
        line: usize,
    },
    Global {
        name: String,
        ty: Option<TypeExpr>,
        init: Option<Expr>,
        line: usize,
    },
    Function {
        name: String,
        params: Vec<(String, TypeExpr)>,
        body: Vec<Stmt>,
        line: usize,
    },
    Sprite {
        name: String,
        rows: Vec<String>, // pixel rows, e.g. "..2222.."
        line: usize,
    },
    Tilemap {
        name: String,
        w: Expr,
        h: Expr,
        line: usize,
    },
}

// ======================================================================
// Parser
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
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(x) if x == kw)
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.advance();
            true
        } else {
            false
        }
    }
    fn expect_sym(&mut self, s: &'static str, d: &mut Vec<Diagnostic>) {
        if !self.eat_sym(s) {
            d.push(err(self.line(), format!("expected '{s}'")));
        }
    }
    fn expect_kw(&mut self, kw: &str, d: &mut Vec<Diagnostic>) {
        if !self.eat_kw(kw) {
            d.push(err(self.line(), format!("expected '{kw}'")));
        }
    }
    fn ident(&mut self, d: &mut Vec<Diagnostic>) -> String {
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
            if self.is_kw("record") {
                decls.push(self.parse_record(d));
            } else if self.is_kw("function") {
                decls.push(self.parse_function(d));
            } else if self.is_kw("local") {
                decls.push(self.parse_global(d));
            } else if self.is_kw("sprite") {
                decls.push(self.parse_sprite(d));
            } else if self.is_kw("tilemap") {
                decls.push(self.parse_tilemap(d));
            } else {
                d.push(err(
                    self.line(),
                    "expected 'record', 'function', 'local', 'sprite', or 'tilemap'",
                ));
                self.advance();
            }
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

    fn parse_record(&mut self, d: &mut Vec<Diagnostic>) -> Decl {
        let line = self.line();
        self.eat_kw("record");
        let name = self.ident(d);
        self.expect_sym("{", d);
        let mut fields = Vec::new();
        while !matches!(self.peek(), Tok::Sym("}") | Tok::Eof) {
            let fname = self.ident(d);
            let fty = if self.eat_sym(":") {
                self.parse_scalar_ty(d)
            } else {
                Ty::Word
            };
            fields.push((fname, fty));
            if !self.eat_sym(",") {
                break;
            }
        }
        self.expect_sym("}", d);
        Decl::Record { name, fields, line }
    }

    fn parse_sprite(&mut self, d: &mut Vec<Diagnostic>) -> Decl {
        let line = self.line();
        self.eat_kw("sprite");
        let name = self.ident(d);
        self.expect_sym("{", d);
        // Rows were raw-captured by the lexer as Ident tokens.
        let mut rows = Vec::new();
        while !matches!(self.peek(), Tok::Sym("}") | Tok::Eof) {
            match self.advance() {
                Tok::Ident(r) => rows.push(r),
                _ => {
                    d.push(err(self.line(), "expected a sprite pixel row"));
                    break;
                }
            }
        }
        self.expect_sym("}", d);
        Decl::Sprite { name, rows, line }
    }

    fn parse_tilemap(&mut self, d: &mut Vec<Diagnostic>) -> Decl {
        let line = self.line();
        self.eat_kw("tilemap");
        let name = self.ident(d);
        self.expect_sym("(", d);
        let w = self.parse_expr(d);
        self.expect_sym(",", d);
        let h = self.parse_expr(d);
        self.expect_sym(")", d);
        Decl::Tilemap { name, w, h, line }
    }

    fn parse_scalar_ty(&mut self, d: &mut Vec<Diagnostic>) -> Ty {
        let line = self.line();
        match self.advance() {
            Tok::Ident(k) if k == "word" => Ty::Word,
            Tok::Ident(k) if k == "byte" => Ty::Byte,
            Tok::Ident(k) if k == "int" => Ty::Int,
            Tok::Ident(k) if k == "bool" => Ty::Bool,
            _ => {
                d.push(err(line, "expected a scalar type (word, byte, int, bool)"));
                Ty::Word
            }
        }
    }

    /// Parse a type expression: `word|byte|bool`, a record name, or `array(N, T)`.
    fn parse_type(&mut self, d: &mut Vec<Diagnostic>) -> TypeExpr {
        let line = self.line();
        if self.is_kw("array") {
            self.advance();
            self.expect_sym("(", d);
            let len = self.parse_expr(d);
            self.expect_sym(",", d);
            let elem = self.parse_type(d);
            self.expect_sym(")", d);
            return TypeExpr::Array(Box::new(elem), Box::new(len), line);
        }
        match self.peek().clone() {
            Tok::Ident(k) if k == "word" => {
                self.advance();
                TypeExpr::Scalar(Ty::Word)
            }
            Tok::Ident(k) if k == "byte" => {
                self.advance();
                TypeExpr::Scalar(Ty::Byte)
            }
            Tok::Ident(k) if k == "int" => {
                self.advance();
                TypeExpr::Scalar(Ty::Int)
            }
            Tok::Ident(k) if k == "bool" => {
                self.advance();
                TypeExpr::Scalar(Ty::Bool)
            }
            Tok::Ident(name) => {
                self.advance();
                TypeExpr::Named(name, line)
            }
            _ => {
                d.push(err(line, "expected a type"));
                TypeExpr::Scalar(Ty::Word)
            }
        }
    }

    fn parse_global(&mut self, d: &mut Vec<Diagnostic>) -> Decl {
        let line = self.line();
        self.eat_kw("local");
        let name = self.ident(d);
        let ty = if self.eat_sym(":") {
            Some(self.parse_type(d))
        } else {
            None
        };
        let init = if self.eat_sym("=") {
            Some(self.parse_expr(d))
        } else {
            None
        };
        Decl::Global { name, ty, init, line }
    }

    fn parse_function(&mut self, d: &mut Vec<Diagnostic>) -> Decl {
        let line = self.line();
        self.eat_kw("function");
        let name = self.ident(d);
        self.expect_sym("(", d);
        let mut params = Vec::new();
        if !matches!(self.peek(), Tok::Sym(")")) {
            loop {
                let pname = self.ident(d);
                let pty = if self.eat_sym(":") {
                    self.parse_type(d)
                } else {
                    TypeExpr::Scalar(Ty::Word)
                };
                params.push((pname, pty));
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        self.expect_sym(")", d);
        let body = self.parse_block(d, &["end"]);
        self.expect_kw("end", d);
        Decl::Function { name, params, body, line }
    }

    /// Parse statements until one of `terminators` (a keyword) or EOF. Does not
    /// consume the terminator.
    fn parse_block(&mut self, d: &mut Vec<Diagnostic>, terminators: &[&str]) -> Vec<Stmt> {
        let mut stmts = Vec::new();
        let mut guard = 0;
        loop {
            if matches!(self.peek(), Tok::Eof) {
                break;
            }
            if let Tok::Ident(k) = self.peek() {
                if terminators.contains(&k.as_str()) {
                    break;
                }
            }
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
        stmts
    }

    fn parse_stmt(&mut self, d: &mut Vec<Diagnostic>) -> Stmt {
        let line = self.line();
        if self.eat_kw("local") {
            let name = self.ident(d);
            let ty = if self.eat_sym(":") {
                Some(self.parse_type(d))
            } else {
                None
            };
            let init = if self.eat_sym("=") {
                Some(self.parse_expr(d))
            } else {
                None
            };
            return Stmt::Local { name, ty, init, line };
        }
        if self.eat_kw("if") {
            return self.parse_if(d);
        }
        if self.eat_kw("while") {
            let cond = self.parse_expr(d);
            self.expect_kw("do", d);
            let body = self.parse_block(d, &["end"]);
            self.expect_kw("end", d);
            return Stmt::While { cond, body };
        }
        if self.eat_kw("for") {
            let var = self.ident(d);
            self.expect_sym("=", d);
            let from = self.parse_expr(d);
            self.expect_sym(",", d);
            let to = self.parse_expr(d);
            let step = if self.eat_sym(",") {
                Some(self.parse_expr(d))
            } else {
                None
            };
            self.expect_kw("do", d);
            let body = self.parse_block(d, &["end"]);
            self.expect_kw("end", d);
            return Stmt::For { var, from, to, step, body, line };
        }
        if self.eat_kw("break") {
            return Stmt::Break(line);
        }
        if self.eat_kw("return") {
            // A return value is present unless the next token ends the block.
            let has_value = !matches!(self.peek(), Tok::Eof)
                && !matches!(self.peek(), Tok::Ident(k) if ["end", "else", "elseif"].contains(&k.as_str()));
            let value = if has_value { Some(self.parse_expr(d)) } else { None };
            return Stmt::Return(value, line);
        }
        // Assignment or call: a prefix expression, optionally followed by `=`.
        let e = self.parse_prefix(d);
        if self.eat_sym("=") {
            let value = self.parse_expr(d);
            return Stmt::Assign { place: e, value, line };
        }
        Stmt::ExprStmt(e)
    }

    fn parse_if(&mut self, d: &mut Vec<Diagnostic>) -> Stmt {
        let cond = self.parse_expr(d);
        self.expect_kw("then", d);
        let then = self.parse_block(d, &["end", "else", "elseif"]);
        let els = if self.is_kw("elseif") {
            self.advance();
            Some(vec![self.parse_if(d)]) // recurse; `elseif` reuses if-parsing, no `end` yet
        } else if self.eat_kw("else") {
            let body = self.parse_block(d, &["end"]);
            self.expect_kw("end", d);
            Some(body)
        } else {
            self.expect_kw("end", d);
            None
        };
        Stmt::If { cond, then, els }
    }

    // ---- expressions ----

    fn parse_expr(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        self.parse_or(d)
    }
    fn bin_left(
        &mut self,
        d: &mut Vec<Diagnostic>,
        next: fn(&mut Self, &mut Vec<Diagnostic>) -> Expr,
        ops: &[&'static str],
    ) -> Expr {
        let mut left = next(self, d);
        loop {
            let op = match self.peek() {
                Tok::Sym(s) if ops.contains(s) => *s,
                _ => break,
            };
            let line = self.line();
            self.advance();
            let right = next(self, d);
            left = Expr::Binary(op, Box::new(left), Box::new(right), line);
        }
        left
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
        let mut left = self.parse_cmp(d);
        while self.is_kw("and") {
            let line = self.line();
            self.advance();
            let right = self.parse_cmp(d);
            left = Expr::Binary("and", Box::new(left), Box::new(right), line);
        }
        left
    }
    fn parse_cmp(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        self.bin_left(d, Self::parse_bitor, &["==", "~=", "<", "<=", ">", ">="])
    }
    fn parse_bitor(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        self.bin_left(d, Self::parse_bitxor, &["|"])
    }
    fn parse_bitxor(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        self.bin_left(d, Self::parse_bitand, &["~"])
    }
    fn parse_bitand(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        self.bin_left(d, Self::parse_shift, &["&"])
    }
    fn parse_shift(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        self.bin_left(d, Self::parse_add, &["<<", ">>"])
    }
    fn parse_add(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        self.bin_left(d, Self::parse_mul, &["+", "-"])
    }
    fn parse_mul(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        self.bin_left(d, Self::parse_unary, &["*", "/", "%"])
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
        self.parse_prefix(d)
    }

    /// Parse a primary followed by `.field` / `[index]` postfixes. A bare
    /// `name(` is a call.
    fn parse_prefix(&mut self, d: &mut Vec<Diagnostic>) -> Expr {
        let line = self.line();
        let mut e = match self.peek().clone() {
            Tok::Num(n) => {
                self.advance();
                Expr::Num(n, line)
            }
            Tok::Ident(k) if k == "true" => {
                self.advance();
                Expr::Num(1, line)
            }
            Tok::Ident(k) if k == "false" => {
                self.advance();
                Expr::Num(0, line)
            }
            Tok::Ident(name) => {
                self.advance();
                if matches!(self.peek(), Tok::Sym("(")) {
                    self.advance();
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
                } else {
                    Expr::Var(name, line)
                }
            }
            Tok::Sym("(") => {
                self.advance();
                let inner = self.parse_expr(d);
                self.expect_sym(")", d);
                inner
            }
            _ => {
                d.push(err(line, "expected an expression"));
                self.advance();
                Expr::Num(0, line)
            }
        };
        loop {
            if self.eat_sym(".") {
                let f = self.ident(d);
                e = Expr::Field(Box::new(e), f, line);
            } else if self.eat_sym("[") {
                let idx = self.parse_expr(d);
                self.expect_sym("]", d);
                e = Expr::Index(Box::new(e), Box::new(idx), line);
            } else {
                break;
            }
        }
        e
    }
}

// ======================================================================
// Compiler
// ======================================================================

#[derive(Clone)]
struct RecordLayout {
    fields: Vec<(String, Ty, u16)>, // name, type, offset
    size: u16,
}

#[derive(Clone)]
struct VarInfo {
    label: String,
    ty: Ty,
    /// A record/array parameter whose slot holds the *address* of the aggregate
    /// (pass-by-reference), rather than the aggregate itself.
    by_ref: bool,
}

struct GlobalInfo {
    label: String,
    ty: Ty,
    const_value: Option<i64>,
}

struct FuncSig {
    params: Vec<(String, Ty)>,
    has_ret: bool,
}

struct Compiler {
    records: HashMap<String, RecordLayout>,
    globals: HashMap<String, GlobalInfo>,
    funcs: HashMap<String, FuncSig>,
    locals: HashMap<String, VarInfo>,
    /// Declared sprites in id order (name, rows); each `NAME` is a constant = its id.
    sprites: Vec<(String, Vec<String>)>,
    sprite_ids: HashMap<String, u16>,
    /// The single declared tilemap: (label, width, height). `mget`/`mset`/`map`/
    /// `solid` need it.
    tilemap: Option<(String, u16, u16)>,
    data: Vec<String>,
    label_ctr: usize,
    loop_ends: Vec<String>,
    cur_func: String,
    helpers: Helpers,
}

#[derive(Default)]
struct Helpers {
    tmp: bool, // a shared @lx_tmp scratch cell (entity, mset)
    min: bool,
    max: bool,
    rect: bool,
    flags: bool, // the @lx_flags 256-byte table (fget/fset/solid)
    fget: bool,
    fset: bool,
    solid: bool,
    flagat: bool,  // @lx_flagat ( px py flag -- bit ): bounds-checked fget(mget(...))
    maprect: bool, // @lx_maprect: rect vs tilemap overlap
    touch: bool,   // @lx_touch_*: edge contact predicates
    collx: bool,   // @lx_collx: axis-resolving X movement
    colly: bool,   // @lx_colly: axis-resolving Y movement
}

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

/// Tile-flag bit indices (for `fget`/`fset`/`solid`). `SOLID` is flag 0.
const FLAG_CONSTS: &[(&str, i64)] = &[("SOLID", 0), ("FLAG1", 1), ("FLAG2", 2), ("FLAG3", 3)];

/// A predefined constant (buttons + tile flags), or `None`.
fn predefined_const(name: &str) -> Option<i64> {
    BUTTON_CONSTS
        .iter()
        .chain(FLAG_CONSTS.iter())
        .find(|(n, _)| *n == name)
        .map(|(_, v)| *v)
}

/// Builtins: name -> (arg count, returns a value).
fn builtin(name: &str) -> Option<(usize, bool)> {
    Some(match name {
        "cls" => (1, false),
        "pset" => (3, false),
        "spr" => (4, false),
        "sprn" => (6, false),
        "sspr" => (4, false),
        "entity" => (3, false),
        "camera" => (2, false),
        "poke" => (2, false),
        "poke16" => (2, false),
        "btn" => (1, true),
        "btnp" => (1, true),
        "btnr" => (1, true),
        "frame_count" => (0, true),
        "rnd" => (1, true),
        "peek" => (1, true),
        "peek16" => (1, true),
        "min" => (2, true),
        "max" => (2, true),
        "rect_overlap" => (8, true),
        "mget" => (2, true),
        "mset" => (3, false),
        "map" => (6, false),
        "fget" => (2, true),
        "fset" => (3, false),
        "solid" => (2, true),
        "map_rect_overlap" => (5, true),
        "collide_x" => (6, true),
        "collide_y" => (6, true),
        "touching_left" => (5, true),
        "touching_right" => (5, true),
        "touching_floor" => (5, true),
        "touching_ceiling" => (5, true),
        _ => return None,
    })
}

fn store_op(ty: &Ty) -> &'static str {
    if ty.is_byte() {
        "STORE8"
    } else {
        "STORE16"
    }
}
fn load_op(ty: &Ty) -> &'static str {
    if ty.is_byte() {
        "LOAD8"
    } else {
        "LOAD16"
    }
}

impl Compiler {
    fn new() -> Self {
        Compiler {
            records: HashMap::new(),
            globals: HashMap::new(),
            funcs: HashMap::new(),
            locals: HashMap::new(),
            sprites: Vec::new(),
            sprite_ids: HashMap::new(),
            tilemap: None,
            data: Vec::new(),
            label_ctr: 0,
            loop_ends: Vec::new(),
            cur_func: String::new(),
            helpers: Helpers::default(),
        }
    }

    fn new_label(&mut self) -> String {
        let l = format!("lx_L{}", self.label_ctr);
        self.label_ctr += 1;
        l
    }

    fn compile(&mut self, decls: &[Decl], d: &mut Vec<Diagnostic>) -> String {
        // Pass 1: record layouts.
        for decl in decls {
            if let Decl::Record { name, fields, line } = decl {
                let mut offset = 0u16;
                let mut laid = Vec::new();
                for (fname, fty) in fields {
                    laid.push((fname.clone(), fty.clone(), offset));
                    offset += fty.size();
                }
                if self
                    .records
                    .insert(name.clone(), RecordLayout { fields: laid, size: offset })
                    .is_some()
                {
                    d.push(err(*line, format!("duplicate record '{name}'")));
                }
            }
        }
        // Pass 1.5: sprites — assign ids (declaration order), bind each name to
        // its id (a compile-time constant).
        for decl in decls {
            if let Decl::Sprite { name, rows, line } = decl {
                let id = self.sprites.len() as u16;
                if self.sprite_ids.insert(name.clone(), id).is_some() {
                    d.push(err(*line, format!("duplicate sprite '{name}'")));
                }
                self.sprites.push((name.clone(), rows.clone()));
            }
        }
        // Pass 1.6: the tilemap (single) — reserve its tile-id grid.
        for decl in decls {
            if let Decl::Tilemap { name, w, h, line } = decl {
                // Validate before casting: dimensions in 1..=1024 and a grid that
                // fits well inside the 64 KiB space (avoids u16 truncation and
                // out-of-range addressing).
                const MAX_DIM: i64 = 1024;
                const MAX_CELLS: i64 = 0x4000; // 16 KiB of tile ids
                let wv = self.eval_const(w, &mut vec![]);
                let hv = self.eval_const(h, &mut vec![]);
                let (wv, hv) = match (wv, hv) {
                    (Some(a), Some(b))
                        if (1..=MAX_DIM).contains(&a)
                            && (1..=MAX_DIM).contains(&b)
                            && a * b <= MAX_CELLS =>
                    {
                        (a as u16, b as u16)
                    }
                    (Some(_), Some(_)) => {
                        d.push(err(*line, format!(
                            "tilemap dimensions out of range (each 1..={MAX_DIM}, w*h <= {MAX_CELLS})"
                        )));
                        continue;
                    }
                    _ => {
                        d.push(err(*line, "tilemap dimensions must be positive constants"));
                        continue;
                    }
                };
                if self.tilemap.is_some() {
                    d.push(err(*line, "only one tilemap is supported"));
                    continue;
                }
                let label = format!("lx_map_{name}");
                self.data.push(format!("@{label} .res {}", wv as u32 * hv as u32));
                self.tilemap = Some((label, wv, hv));
            }
        }
        // Pass 2: function signatures.
        for decl in decls {
            if let Decl::Function { name, params, line, .. } = decl {
                let resolved: Vec<(String, Ty)> = params
                    .iter()
                    .map(|(pn, pt)| (pn.clone(), self.resolve_type(pt, d)))
                    .collect();
                let has_ret = fn_has_return(decl);
                if self
                    .funcs
                    .insert(name.clone(), FuncSig { params: resolved, has_ret })
                    .is_some()
                {
                    d.push(err(*line, format!("duplicate function '{name}'")));
                }
            }
        }
        // Pass 3: globals (names + const values, for sizing/const-folding).
        for decl in decls {
            if let Decl::Global { name, ty, init, line } = decl {
                let gty = match ty {
                    Some(te) => self.resolve_type(te, d),
                    None => Ty::Word,
                };
                let const_value = init.as_ref().and_then(|e| self.eval_const(e, &mut vec![]));
                self.globals.insert(
                    name.clone(),
                    GlobalInfo { label: format!("lx_g_{name}"), ty: gty, const_value },
                );
                let _ = line;
            }
        }

        // Pass 4: emit global data.
        for decl in decls {
            if let Decl::Global { name, init, .. } = decl {
                self.emit_global_data(name, init.as_ref(), d);
            }
        }
        // Pass 5: compile function bodies.
        let mut body = String::new();
        for decl in decls {
            if let Decl::Function { name, params, body: fbody, line } = decl {
                body.push_str(&self.compile_function(name, params, fbody, *line, d));
            }
        }

        self.assemble_program(&body)
    }

    fn resolve_type(&self, te: &TypeExpr, d: &mut Vec<Diagnostic>) -> Ty {
        match te {
            TypeExpr::Scalar(t) => t.clone(),
            TypeExpr::Named(name, line) => match self.records.get(name) {
                Some(layout) => Ty::Record(name.clone(), layout.size),
                None => {
                    d.push(err(*line, format!("unknown type '{name}'")));
                    Ty::Word
                }
            },
            TypeExpr::Array(elem, size, line) => {
                let n = self.eval_const(size, &mut vec![]).filter(|&v| v > 0);
                let n = match n {
                    Some(v) => v as u16,
                    None => {
                        d.push(err(*line, "array length must be a positive constant"));
                        1
                    }
                };
                Ty::Array(Box::new(self.resolve_type(elem, d)), n)
            }
        }
    }

    fn emit_global_data(&mut self, name: &str, init: Option<&Expr>, d: &mut Vec<Diagnostic>) {
        let info = &self.globals[name];
        let label = info.label.clone();
        let ty = info.ty.clone();
        match (&ty, init) {
            (t, Some(_)) if t.is_scalar() => {
                let v = (info.const_value.unwrap_or(0) & 0xffff) as u16;
                if t.is_byte() {
                    self.data.push(format!("@{label} .byte {}", v & 0xff));
                } else {
                    self.data.push(format!("@{label} .word {v}"));
                }
                if info.const_value.is_none() {
                    d.push(err(0, format!("global '{name}' initializer must be constant (set it in init())")));
                }
            }
            (t, Some(_)) => {
                d.push(err(0, format!("cannot initialize aggregate global '{name}' (set fields in init())")));
                self.data.push(format!("@{label} .res {}", t.size()));
            }
            (t, None) => self.data.push(format!("@{label} .res {}", t.size())),
        }
    }

    fn compile_function(
        &mut self,
        name: &str,
        params: &[(String, TypeExpr)],
        body: &[Stmt],
        _line: usize,
        d: &mut Vec<Diagnostic>,
    ) -> String {
        self.locals.clear();
        self.cur_func = name.to_string();

        // Declare param slots. Aggregates are passed by address (word slot).
        let mut out: Vec<String> = Vec::new();
        let mut prologue: Vec<String> = Vec::new();
        for (pname, pte) in params {
            let pty = self.resolve_type(pte, d);
            let by_ref = !pty.is_scalar();
            let label = format!("lx_l_{name}_{pname}");
            let slot_size = if by_ref { 2 } else { pty.size() };
            self.data.push(format!("@{label} .res {slot_size}"));
            self.locals.insert(
                pname.clone(),
                VarInfo { label: label.clone(), ty: pty.clone(), by_ref },
            );
            // Prologue stores each arg (built in declared order; reversed below).
            let op = if by_ref { "STORE16" } else { store_op(&pty) };
            prologue.push(format!("{label} {op}"));
        }
        // Args arrive with the last on top, so pop in reverse.
        prologue.reverse();
        out.extend(prologue);

        self.gen_block(body, &mut out, d);
        out.push("RET".to_string());
        format!("@lx_p_{name}\n  {}\n", out.join(" "))
    }

    fn gen_block(&mut self, stmts: &[Stmt], out: &mut Vec<String>, d: &mut Vec<Diagnostic>) {
        for s in stmts {
            self.gen_stmt(s, out, d);
        }
    }

    fn gen_stmt(&mut self, s: &Stmt, out: &mut Vec<String>, d: &mut Vec<Diagnostic>) {
        match s {
            Stmt::Local { name, ty, init, line } => {
                let vty = match ty {
                    Some(te) => self.resolve_type(te, d),
                    None => Ty::Word,
                };
                let label = format!("lx_l_{}_{}", self.cur_func, name);
                self.data.push(format!("@{label} .res {}", vty.size()));
                self.locals.insert(
                    name.clone(),
                    VarInfo { label: label.clone(), ty: vty.clone(), by_ref: false },
                );
                if let Some(e) = init {
                    if vty.is_scalar() {
                        self.gen_expr(e, out, d);
                        out.push(format!("{label} {}", store_op(&vty)));
                    } else {
                        d.push(err(*line, "cannot initialize an aggregate local (assign fields instead)"));
                    }
                }
            }
            Stmt::Assign { place, value, line } => {
                self.gen_expr(value, out, d);
                let ty = self.gen_place_addr(place, out, d);
                if let Some(ty) = ty {
                    if ty.is_scalar() {
                        out.push(store_op(&ty).to_string());
                    } else {
                        d.push(err(*line, "cannot assign to a whole record/array"));
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
            Stmt::For { var, from, to, step, body, line } => {
                // Ascending only: step must be a positive integer literal (default 1).
                let step_val = match step {
                    None => 1i64,
                    Some(e) => match self.eval_const(e, &mut vec![]) {
                        Some(v) if v > 0 => v,
                        _ => {
                            d.push(err(*line, "for step must be a positive integer literal (use while otherwise)"));
                            1
                        }
                    },
                };
                let label = format!("lx_l_{}_{}", self.cur_func, var);
                self.data.push(format!("@{label} .res 2"));
                self.locals.insert(
                    var.clone(),
                    VarInfo { label: label.clone(), ty: Ty::Word, by_ref: false },
                );
                // i = from
                self.gen_expr(from, out, d);
                out.push(format!("{label} STORE16"));
                let top = self.new_label();
                let end = self.new_label();
                out.push(format!("@{top}"));
                // while i <= to  ->  !(i > to)
                out.push(format!("{label} LOAD16"));
                self.gen_expr(to, out, d);
                out.push("GT #00 EQ".to_string());
                out.push(format!("{end} JZ"));
                self.loop_ends.push(end.clone());
                self.gen_block(body, out, d);
                self.loop_ends.pop();
                // i = i + step
                out.push(format!("{label} LOAD16 {step_val} ADD {label} STORE16"));
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
                if self.gen_expr(e, out, d) {
                    out.push("DROP".to_string());
                }
            }
        }
    }

    /// Emit code pushing the *address* of a place, returning its type. `None`
    /// on error (diagnostic already pushed).
    fn gen_place_addr(&mut self, e: &Expr, out: &mut Vec<String>, d: &mut Vec<Diagnostic>) -> Option<Ty> {
        match e {
            Expr::Var(name, line) => {
                let info = self.resolve_var(name).or_else(|| {
                    d.push(err(*line, format!("unknown variable '{name}'")));
                    None
                })?;
                if info.by_ref {
                    out.push(format!("{} LOAD16", info.label)); // slot holds the address
                } else {
                    out.push(info.label.clone()); // storage address
                }
                Some(info.ty)
            }
            Expr::Field(base, field, line) => {
                let bt = self.gen_place_addr(base, out, d)?;
                let Ty::Record(rname, _) = &bt else {
                    d.push(err(*line, "field access on a non-record"));
                    return None;
                };
                let layout = self.records.get(rname)?;
                let Some((_, fty, off)) = layout.fields.iter().find(|(n, _, _)| n == field) else {
                    d.push(err(*line, format!("record '{rname}' has no field '{field}'")));
                    return None;
                };
                let (fty, off) = (fty.clone(), *off);
                if off != 0 {
                    out.push(format!("{off} ADD"));
                }
                Some(fty)
            }
            Expr::Index(base, idx, line) => {
                let bt = self.gen_place_addr(base, out, d)?;
                let Ty::Array(elem, _) = &bt else {
                    d.push(err(*line, "indexing a non-array"));
                    return None;
                };
                let elem = (**elem).clone();
                self.gen_expr(idx, out, d);
                let sz = elem.size();
                if sz != 1 {
                    out.push(format!("{sz} MUL"));
                }
                out.push("ADD".to_string());
                Some(elem)
            }
            _ => {
                d.push(err(e.line(), "not an assignable place"));
                None
            }
        }
    }

    /// Generate an expression, leaving its value (or, for aggregates, its
    /// address) on the stack. Returns whether a value was produced.
    fn gen_expr(&mut self, e: &Expr, out: &mut Vec<String>, d: &mut Vec<Diagnostic>) -> bool {
        match e {
            Expr::Num(n, _) => {
                out.push(((*n & 0xffff) as u16).to_string());
                true
            }
            Expr::Var(name, _) => {
                if let Some(v) = predefined_const(name) {
                    out.push(((v & 0xffff) as u16).to_string());
                    return true;
                }
                if let Some(id) = self.sprite_ids.get(name) {
                    out.push(id.to_string()); // sprite name -> its tile id
                    return true;
                }
                // A place: scalar -> load; aggregate -> its address (reference).
                let ty = match self.gen_place_addr(e, out, d) {
                    Some(t) => t,
                    None => {
                        out.push("0".to_string());
                        return true;
                    }
                };
                if ty.is_scalar() {
                    out.push(load_op(&ty).to_string());
                }
                true
            }
            Expr::Field(..) | Expr::Index(..) => {
                match self.gen_place_addr(e, out, d) {
                    Some(ty) if ty.is_scalar() => out.push(load_op(&ty).to_string()),
                    Some(_) => {} // aggregate: address left on stack
                    None => out.push("0".to_string()),
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

    fn gen_binary(&mut self, op: &str, l: &Expr, r: &Expr, out: &mut Vec<String>, d: &mut Vec<Diagnostic>) {
        if op == "and" || op == "or" {
            self.gen_expr(l, out, d);
            out.push("#00 NE".to_string());
            self.gen_expr(r, out, d);
            out.push("#00 NE".to_string());
            out.push(if op == "and" { "AND" } else { "OR" }.to_string());
            return;
        }
        // Ordering comparisons are signed if either operand is `int`. The VM's
        // LT/GT are unsigned, so bias both operands by 0x8000 (flip the sign bit)
        // — unsigned-compare-of-biased == signed compare. `==`/`~=` are bit
        // equality, unaffected by signedness.
        let signed = matches!(op, "<" | "<=" | ">" | ">=")
            && (self.type_of(l).is_int() || self.type_of(r).is_int());
        self.gen_expr(l, out, d);
        if signed {
            out.push("#8000 XOR".to_string());
        }
        self.gen_expr(r, out, d);
        if signed {
            out.push("#8000 XOR".to_string());
        }
        let ops: &[&str] = match op {
            "+" => &["ADD"],
            "-" => &["SUB"],
            "*" => &["MUL"],
            "/" => &["DIV"],
            "%" => &["MOD"],
            "&" => &["AND"],
            "|" => &["OR"],
            "~" => &["XOR"],
            "<<" => &["SHL"],
            ">>" => &["SHR"],
            "==" => &["EQ"],
            "~=" => &["NE"],
            "<" => &["LT"],
            ">" => &["GT"],
            "<=" => &["GT", "#00", "EQ"],
            ">=" => &["LT", "#00", "EQ"],
            _ => &["ADD"],
        };
        for o in ops {
            out.push(o.to_string());
        }
    }

    /// Best-effort static type of an expression — used only to decide signed vs
    /// unsigned comparisons; never emits code.
    fn type_of(&self, e: &Expr) -> Ty {
        match e {
            Expr::Num(..) => Ty::Word,
            Expr::Var(name, _) => {
                if predefined_const(name).is_some() {
                    Ty::Word
                } else {
                    self.resolve_var(name).map(|v| v.ty).unwrap_or(Ty::Word)
                }
            }
            Expr::Field(base, field, _) => match self.type_of(base) {
                Ty::Record(rname, _) => self
                    .records
                    .get(&rname)
                    .and_then(|l| l.fields.iter().find(|(n, _, _)| n == field))
                    .map(|(_, t, _)| t.clone())
                    .unwrap_or(Ty::Word),
                _ => Ty::Word,
            },
            Expr::Index(base, _, _) => match self.type_of(base) {
                Ty::Array(elem, _) => *elem,
                _ => Ty::Word,
            },
            Expr::Unary(op, inner, _) => match *op {
                "not" => Ty::Bool,
                "-" => Ty::Int,             // a negated value is signed
                _ => self.type_of(inner),   // `~` keeps the operand's type
            },
            Expr::Binary(op, l, r, _) => {
                if matches!(*op, "==" | "~=" | "<" | "<=" | ">" | ">=" | "and" | "or") {
                    Ty::Bool
                } else if self.type_of(l).is_int() || self.type_of(r).is_int() {
                    Ty::Int
                } else {
                    Ty::Word
                }
            }
            Expr::Call(..) => Ty::Word,
        }
    }

    fn gen_call(&mut self, name: &str, args: &[Expr], out: &mut Vec<String>, d: &mut Vec<Diagnostic>, line: usize) -> bool {
        // `len(arr)` is a compile-time constant = the array's declared length.
        if name == "len" {
            if let [arg] = args {
                if let Ty::Array(_, n) = self.type_of(arg) {
                    out.push(n.to_string());
                    return true;
                }
            }
            d.push(err(line, "len() takes one array argument"));
            out.push("0".to_string());
            return true;
        }
        if let Some((argc, yields)) = builtin(name) {
            // On an arity mismatch, report it and emit nothing — a partial call
            // would leave the data stack unbalanced.
            if args.len() != argc {
                d.push(err(line, format!("{name}() takes {argc} argument(s), got {}", args.len())));
                return yields;
            }
            for a in args {
                self.gen_expr(a, out, d);
            }
            self.gen_builtin(name, out, d);
            return yields;
        }
        if let Some(sig) = self.funcs.get(name) {
            let (argc, yields) = (sig.params.len(), sig.has_ret);
            if args.len() != argc {
                d.push(err(line, format!("{name}() takes {argc} argument(s), got {}", args.len())));
                return yields;
            }
            for a in args {
                self.gen_expr(a, out, d);
            }
            out.push(format!("lx_p_{name} CALL"));
            return yields;
        }
        d.push(err(line, format!("unknown function '{name}'")));
        false
    }

    /// Enable `lx_flagat` and its dependencies (the flags table + `fget`), which
    /// every tilemap-collision helper builds on.
    fn need_flagat(&mut self) {
        self.helpers.flagat = true;
        self.helpers.flags = true;
        self.helpers.fget = true;
    }

    fn gen_builtin(&mut self, name: &str, out: &mut Vec<String>, d: &mut Vec<Diagnostic>) {
        // Tilemap builtins need the single declared map (label + width). `mget`
        // computes `map + ty*W + tx` and loads the tile id. The collision helpers
        // reach the map through `lx_flagat` (which bakes in the map/width), so
        // they only need the declaration to exist.
        if matches!(
            name,
            "mget" | "mset" | "map" | "solid" | "map_rect_overlap" | "collide_x" | "collide_y"
            | "touching_left" | "touching_right" | "touching_floor" | "touching_ceiling"
        ) {
            let (map, w) = match &self.tilemap {
                Some((l, w, _)) => (l.clone(), *w),
                None => {
                    d.push(err(0, format!("{name}() needs a `tilemap` declaration")));
                    return;
                }
            };
            match name {
                "mget" => out.push(format!("{w} MUL ADD {map} ADD LOAD8")), // ( tx ty -- id )
                "mset" => {
                    self.helpers.tmp = true;
                    out.push(format!(
                        "lx_tmp STORE16 {w} MUL ADD {map} ADD lx_tmp LOAD16 SWAP STORE8"
                    )); // ( tx ty id -- )
                }
                // ( tx ty sx sy tw th -- ) set region + trigger the map draw.
                "map" => out.push(
                    "#77 DEO #76 DEO #75 DEO #74 DEO #73 DEO #72 DEO #00 #78 DEO".to_string(),
                ),
                "solid" => {
                    self.helpers.solid = true;
                    self.helpers.flags = true;
                    self.helpers.fget = true;
                    out.push("lx_solid CALL".to_string()); // ( px py -- 0/1 )
                }
                "map_rect_overlap" => {
                    self.need_flagat();
                    self.helpers.maprect = true;
                    out.push("lx_maprect CALL".to_string()); // ( x y w h flag -- bool )
                }
                "collide_x" => {
                    self.need_flagat();
                    self.helpers.collx = true;
                    out.push("lx_collx CALL".to_string()); // ( x y w h dx flag -- new_x )
                }
                "collide_y" => {
                    self.need_flagat();
                    self.helpers.colly = true;
                    out.push("lx_colly CALL".to_string()); // ( x y w h dy flag -- new_y )
                }
                "touching_left" | "touching_right" | "touching_floor" | "touching_ceiling" => {
                    self.need_flagat();
                    self.helpers.touch = true;
                    out.push(format!("lx_{name} CALL")); // ( x y w h flag -- bool )
                }
                _ => {}
            }
            return;
        }

        let seq: &str = match name {
            "cls" => "#16 DEO",
            "pset" => "#13 DEO #12 DEO #11 DEO #00 #14 DEO", // ( x y color )
            "spr" => "#19 DEO #12 DEO #11 DEO #1a DEO",      // ( id x y flags ) blit by id
            // ( id x y w h flags ) draw a w×h block of contiguous sheet tiles.
            "sprn" => "#19 DEO #a2 DEO #a1 DEO #12 DEO #11 DEO #a0 DEO #00 #a3 DEO",
            "sspr" => "#19 DEO #12 DEO #11 DEO #15 DEO",     // ( addr x y flags ) raw blit
            "camera" => "#18 DEO #17 DEO",                   // ( x y )
            "poke" => "SWAP STORE8",
            "poke16" => "SWAP STORE16",
            "btn" => "#20 DEI AND #00 NE",
            "btnp" => "#21 DEI AND #00 NE", // just-pressed this frame
            "btnr" => "#22 DEI AND #00 NE", // just-released this frame
            "frame_count" => "#80 DEI",     // frames since power-on (wraps at 65536)
            "rnd" => "#30 DEI SWAP MOD", // ( n ) -> rand % n
            "peek" => "LOAD8",
            "peek16" => "LOAD16",
            "entity" => {
                self.helpers.tmp = true;
                "lx_tmp STORE16 #51 DEO #50 DEO lx_tmp LOAD16 #52 DEO"
            }
            "fget" => {
                self.helpers.flags = true;
                self.helpers.fget = true;
                "lx_fget CALL"
            }
            "fset" => {
                self.helpers.flags = true;
                self.helpers.fset = true;
                "lx_fset CALL"
            }
            "min" => {
                self.helpers.min = true;
                "lx_min CALL"
            }
            "max" => {
                self.helpers.max = true;
                "lx_max CALL"
            }
            "rect_overlap" => {
                self.helpers.rect = true;
                "lx_rect CALL"
            }
            _ => "",
        };
        if !seq.is_empty() {
            out.push(seq.to_string());
        }
    }

    fn resolve_var(&self, name: &str) -> Option<VarInfo> {
        if let Some(l) = self.locals.get(name) {
            return Some(l.clone());
        }
        self.globals.get(name).map(|g| VarInfo {
            label: g.label.clone(),
            ty: g.ty.clone(),
            by_ref: false,
        })
    }

    fn eval_const(&self, e: &Expr, seen: &mut Vec<String>) -> Option<i64> {
        match e {
            Expr::Num(n, _) => Some(*n),
            Expr::Var(name, _) => {
                if let Some(v) = predefined_const(name) {
                    return Some(v);
                }
                if let Some(id) = self.sprite_ids.get(name) {
                    return Some(*id as i64);
                }
                if seen.contains(name) {
                    return None;
                }
                seen.push(name.clone());
                self.globals.get(name).and_then(|g| g.const_value)
            }
            Expr::Unary(op, inner, _) => {
                let v = self.eval_const(inner, seen)?;
                Some(match *op {
                    "-" => -v,
                    "~" => !v,
                    "not" => (v == 0) as i64,
                    _ => return None,
                })
            }
            Expr::Binary(op, l, r, _) => {
                let a = self.eval_const(l, seen)?;
                let b = self.eval_const(r, seen)?;
                Some(match *op {
                    "+" => a + b,
                    "-" => a - b,
                    "*" => a * b,
                    "/" if b != 0 => a / b,
                    "%" if b != 0 => a % b,
                    "&" => a & b,
                    "|" => a | b,
                    "~" => a ^ b,
                    "<<" => a << b,
                    ">>" => a >> b,
                    _ => return None,
                })
            }
            // `len(arr)` folds to the array's declared length.
            Expr::Call(name, args, _) if name == "len" => {
                if let [Expr::Var(v, _)] = args.as_slice() {
                    if let Some(Ty::Array(_, n)) = self.resolve_var(v).map(|i| i.ty) {
                        return Some(n as i64);
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn assemble_program(&mut self, funcs: &str) -> String {
        let mut out = String::new();
        out.push_str("( generated by the luax front-end )\n");
        // Point the tileset base at the sprite sheet so `spr(id, …)` works.
        if !self.sprites.is_empty() {
            out.push_str("lx_sheet #1b DEO\n");
        }
        // Point the tilemap device at the map grid + its width.
        if let Some((label, width, _)) = self.tilemap.clone() {
            out.push_str(&format!("{label} #70 DEO {width} #71 DEO\n"));
        }
        if self.funcs.contains_key("init") {
            out.push_str("lx_p_init CALL\n");
        }
        out.push_str("lx_frame #10 DEO\nRET\n\n");

        out.push_str("@lx_frame\n");
        let has_ud = self.funcs.contains_key("update") || self.funcs.contains_key("draw");
        if self.funcs.contains_key("update") {
            out.push_str("  lx_p_update CALL\n");
        }
        if self.funcs.contains_key("draw") {
            out.push_str("  lx_p_draw CALL\n");
        }
        if !has_ud && self.funcs.contains_key("frame") {
            out.push_str("  lx_p_frame CALL\n");
        }
        out.push_str("  RET\n\n");

        out.push_str(funcs);
        out.push('\n');

        // Helper subroutines.
        if self.helpers.min {
            out.push_str("@lx_min OVER OVER LT lx_min_a JNZ SWAP DROP RET @lx_min_a DROP RET\n");
        }
        if self.helpers.max {
            out.push_str("@lx_max OVER OVER GT lx_max_a JNZ SWAP DROP RET @lx_max_a DROP RET\n");
        }
        if self.helpers.rect {
            out.push_str(
                "@lx_rect\n  lx_ro7 STORE16 lx_ro6 STORE16 lx_ro5 STORE16 lx_ro4 STORE16 \
                 lx_ro3 STORE16 lx_ro2 STORE16 lx_ro1 STORE16 lx_ro0 STORE16\n  \
                 lx_ro0 LOAD16 lx_ro4 LOAD16 lx_ro6 LOAD16 ADD LT \
                 lx_ro4 LOAD16 lx_ro0 LOAD16 lx_ro2 LOAD16 ADD LT AND \
                 lx_ro1 LOAD16 lx_ro5 LOAD16 lx_ro7 LOAD16 ADD LT AND \
                 lx_ro5 LOAD16 lx_ro1 LOAD16 lx_ro3 LOAD16 ADD LT AND RET\n",
            );
        }
        // fget ( tile flag -- bit ): (flags[tile] >> flag) & 1
        if self.helpers.fget {
            out.push_str("@lx_fget SWAP lx_flags ADD LOAD8 SWAP SHR #01 AND RET\n");
        }
        // fset ( tile flag v -- ): set/clear bit `flag` of flags[tile]
        if self.helpers.fset {
            out.push_str(
                "@lx_fset\n  lx_ft_v STORE16 #01 SWAP SHL lx_ft_m STORE16 lx_flags ADD DUP LOAD8 \
                 lx_ft_v LOAD16 lx_fset_set JNZ \
                 lx_ft_m LOAD16 #ffff XOR AND lx_fset_done JMP \
                 @lx_fset_set lx_ft_m LOAD16 OR @lx_fset_done SWAP STORE8 RET\n",
            );
        }
        // solid ( px py -- 0/1 ): is the tile at pixel (px,py) SOLID (flag 0)?
        // Off-map pixels (negative — a signed value like -1 is 0xffff — or past
        // the map edge) are treated as not solid. The `LT` bounds checks are
        // unsigned, so a wrapped-negative coordinate fails them and returns 0.
        if self.helpers.solid {
            if let Some((map, w, h)) = self.tilemap.clone() {
                let pw = w as u32 * 8; // map width/height in pixels
                let ph = h as u32 * 8;
                out.push_str(&format!(
                    "@lx_solid\n  lx_sy STORE16 lx_sx STORE16 \
                     lx_sx LOAD16 {pw} LT lx_sy LOAD16 {ph} LT AND lx_solid_ok JNZ \
                     #00 RET \
                     @lx_solid_ok \
                     lx_sx LOAD16 #03 SHR lx_sy LOAD16 #03 SHR {w} MUL ADD {map} ADD LOAD8 \
                     #00 lx_fget CALL RET\n"
                ));
            }
        }
        // flagat ( px py flag -- bit ): the generalized `solid` — is bit `flag`
        // set on the tile under pixel (px,py)? Off-map pixels (unsigned bounds
        // check fails a wrapped-negative coord) read as 0. The rect/edge/collision
        // helpers below all sample the map through this.
        if self.helpers.flagat {
            if let Some((map, w, h)) = self.tilemap.clone() {
                let pw = w as u32 * 8;
                let ph = h as u32 * 8;
                out.push_str(&format!(
                    "@lx_flagat\n  lx_fa_f STORE16 lx_fa_y STORE16 lx_fa_x STORE16 \
                     lx_fa_x LOAD16 {pw} LT lx_fa_y LOAD16 {ph} LT AND lx_flagat_ok JNZ \
                     #00 RET \
                     @lx_flagat_ok \
                     lx_fa_x LOAD16 #03 SHR lx_fa_y LOAD16 #03 SHR {w} MUL ADD {map} ADD LOAD8 \
                     lx_fa_f LOAD16 lx_fget CALL RET\n"
                ));
            }
        }
        // map_rect_overlap ( x y w h flag -- bool ): OR of `flagat` at the rect's
        // four corners. Exact for rects up to one tile; larger rects sample only
        // the corners (a tile fully inside an edge is not seen).
        if self.helpers.maprect {
            out.push_str(
                "@lx_maprect\n  lx_mr_f STORE16 lx_mr_h STORE16 lx_mr_w STORE16 \
                 lx_mr_y STORE16 lx_mr_x STORE16 \
                 lx_mr_x LOAD16 lx_mr_y LOAD16 lx_mr_f LOAD16 lx_flagat CALL \
                 lx_mr_x LOAD16 lx_mr_w LOAD16 ADD #01 SUB lx_mr_y LOAD16 lx_mr_f LOAD16 lx_flagat CALL OR \
                 lx_mr_x LOAD16 lx_mr_y LOAD16 lx_mr_h LOAD16 ADD #01 SUB lx_mr_f LOAD16 lx_flagat CALL OR \
                 lx_mr_x LOAD16 lx_mr_w LOAD16 ADD #01 SUB lx_mr_y LOAD16 lx_mr_h LOAD16 ADD #01 SUB \
                 lx_mr_f LOAD16 lx_flagat CALL OR RET\n",
            );
        }
        // touching_* ( x y w h flag -- bool ): sample `flagat` at the two corners
        // one pixel OUTSIDE the named edge. Shared scratch (they never nest).
        if self.helpers.touch {
            out.push_str(
                "@lx_touching_left\n  lx_tc_f STORE16 lx_tc_h STORE16 lx_tc_w STORE16 \
                 lx_tc_y STORE16 lx_tc_x STORE16 \
                 lx_tc_x LOAD16 #01 SUB lx_tc_y LOAD16 lx_tc_f LOAD16 lx_flagat CALL \
                 lx_tc_x LOAD16 #01 SUB lx_tc_y LOAD16 lx_tc_h LOAD16 ADD #01 SUB lx_tc_f LOAD16 lx_flagat CALL OR RET\n",
            );
            out.push_str(
                "@lx_touching_right\n  lx_tc_f STORE16 lx_tc_h STORE16 lx_tc_w STORE16 \
                 lx_tc_y STORE16 lx_tc_x STORE16 \
                 lx_tc_x LOAD16 lx_tc_w LOAD16 ADD lx_tc_y LOAD16 lx_tc_f LOAD16 lx_flagat CALL \
                 lx_tc_x LOAD16 lx_tc_w LOAD16 ADD lx_tc_y LOAD16 lx_tc_h LOAD16 ADD #01 SUB lx_tc_f LOAD16 lx_flagat CALL OR RET\n",
            );
            out.push_str(
                "@lx_touching_floor\n  lx_tc_f STORE16 lx_tc_h STORE16 lx_tc_w STORE16 \
                 lx_tc_y STORE16 lx_tc_x STORE16 \
                 lx_tc_x LOAD16 lx_tc_y LOAD16 lx_tc_h LOAD16 ADD lx_tc_f LOAD16 lx_flagat CALL \
                 lx_tc_x LOAD16 lx_tc_w LOAD16 ADD #01 SUB lx_tc_y LOAD16 lx_tc_h LOAD16 ADD lx_tc_f LOAD16 lx_flagat CALL OR RET\n",
            );
            out.push_str(
                "@lx_touching_ceiling\n  lx_tc_f STORE16 lx_tc_h STORE16 lx_tc_w STORE16 \
                 lx_tc_y STORE16 lx_tc_x STORE16 \
                 lx_tc_x LOAD16 lx_tc_y LOAD16 #01 SUB lx_tc_f LOAD16 lx_flagat CALL \
                 lx_tc_x LOAD16 lx_tc_w LOAD16 ADD #01 SUB lx_tc_y LOAD16 #01 SUB lx_tc_f LOAD16 lx_flagat CALL OR RET\n",
            );
        }
        // collide_x ( x y w h dx flag -- new_x ): move by signed dx; if the
        // leading vertical edge (right corners if dx>0, left if dx<0) hits a
        // flagged tile, snap to the tile boundary. Assumes the rect starts in a
        // clear cell and dx is small (no tunneling past a tile in one step).
        if self.helpers.collx {
            out.push_str(
                "@lx_collx\n  lx_cx_f STORE16 lx_cx_dx STORE16 lx_cx_h STORE16 lx_cx_w STORE16 \
                 lx_cx_y STORE16 lx_cx_x STORE16 \
                 lx_cx_x LOAD16 lx_cx_dx LOAD16 ADD lx_cx_t STORE16 \
                 lx_cx_dx LOAD16 #8000 XOR #8000 GT lx_collx_right JNZ \
                 lx_cx_dx LOAD16 #8000 XOR #8000 LT lx_collx_left JNZ \
                 lx_cx_t LOAD16 RET \
                 @lx_collx_right \
                 lx_cx_t LOAD16 lx_cx_w LOAD16 ADD #01 SUB lx_cx_lead STORE16 \
                 lx_cx_lead LOAD16 lx_cx_y LOAD16 lx_cx_f LOAD16 lx_flagat CALL \
                 lx_cx_lead LOAD16 lx_cx_y LOAD16 lx_cx_h LOAD16 ADD #01 SUB lx_cx_f LOAD16 lx_flagat CALL OR \
                 lx_collx_rhit JNZ \
                 lx_cx_t LOAD16 RET \
                 @lx_collx_rhit lx_cx_lead LOAD16 #03 SHR #03 SHL lx_cx_w LOAD16 SUB RET \
                 @lx_collx_left \
                 lx_cx_t LOAD16 lx_cx_lead STORE16 \
                 lx_cx_lead LOAD16 lx_cx_y LOAD16 lx_cx_f LOAD16 lx_flagat CALL \
                 lx_cx_lead LOAD16 lx_cx_y LOAD16 lx_cx_h LOAD16 ADD #01 SUB lx_cx_f LOAD16 lx_flagat CALL OR \
                 lx_collx_lhit JNZ \
                 lx_cx_t LOAD16 RET \
                 @lx_collx_lhit lx_cx_lead LOAD16 #03 SHR #03 SHL #08 ADD RET\n",
            );
        }
        // collide_y ( x y w h dy flag -- new_y ): the Y-axis mirror of collide_x
        // (leading edge = bottom corners if dy>0, top if dy<0).
        if self.helpers.colly {
            out.push_str(
                "@lx_colly\n  lx_cy_f STORE16 lx_cy_dy STORE16 lx_cy_h STORE16 lx_cy_w STORE16 \
                 lx_cy_y STORE16 lx_cy_x STORE16 \
                 lx_cy_y LOAD16 lx_cy_dy LOAD16 ADD lx_cy_t STORE16 \
                 lx_cy_dy LOAD16 #8000 XOR #8000 GT lx_colly_down JNZ \
                 lx_cy_dy LOAD16 #8000 XOR #8000 LT lx_colly_up JNZ \
                 lx_cy_t LOAD16 RET \
                 @lx_colly_down \
                 lx_cy_t LOAD16 lx_cy_h LOAD16 ADD #01 SUB lx_cy_lead STORE16 \
                 lx_cy_x LOAD16 lx_cy_lead LOAD16 lx_cy_f LOAD16 lx_flagat CALL \
                 lx_cy_x LOAD16 lx_cy_w LOAD16 ADD #01 SUB lx_cy_lead LOAD16 lx_cy_f LOAD16 lx_flagat CALL OR \
                 lx_colly_dhit JNZ \
                 lx_cy_t LOAD16 RET \
                 @lx_colly_dhit lx_cy_lead LOAD16 #03 SHR #03 SHL lx_cy_h LOAD16 SUB RET \
                 @lx_colly_up \
                 lx_cy_t LOAD16 lx_cy_lead STORE16 \
                 lx_cy_x LOAD16 lx_cy_lead LOAD16 lx_cy_f LOAD16 lx_flagat CALL \
                 lx_cy_x LOAD16 lx_cy_w LOAD16 ADD #01 SUB lx_cy_lead LOAD16 lx_cy_f LOAD16 lx_flagat CALL OR \
                 lx_colly_uhit JNZ \
                 lx_cy_t LOAD16 RET \
                 @lx_colly_uhit lx_cy_lead LOAD16 #03 SHR #03 SHL #08 ADD RET\n",
            );
        }

        // Data section.
        for line in &self.data {
            out.push_str(line);
            out.push('\n');
        }
        if self.helpers.tmp {
            out.push_str("@lx_tmp .res 2\n");
        }
        if self.helpers.rect {
            for i in 0..8 {
                out.push_str(&format!("@lx_ro{i} .res 2\n"));
            }
        }
        if self.helpers.flags {
            out.push_str("@lx_flags .res 256\n");
        }
        if self.helpers.fset {
            out.push_str("@lx_ft_v .res 2\n@lx_ft_m .res 2\n");
        }
        if self.helpers.solid {
            out.push_str("@lx_sx .res 2\n@lx_sy .res 2\n");
        }
        if self.helpers.flagat {
            out.push_str("@lx_fa_x .res 2\n@lx_fa_y .res 2\n@lx_fa_f .res 2\n");
        }
        if self.helpers.maprect {
            out.push_str(
                "@lx_mr_x .res 2\n@lx_mr_y .res 2\n@lx_mr_w .res 2\n@lx_mr_h .res 2\n@lx_mr_f .res 2\n",
            );
        }
        if self.helpers.touch {
            out.push_str(
                "@lx_tc_x .res 2\n@lx_tc_y .res 2\n@lx_tc_w .res 2\n@lx_tc_h .res 2\n@lx_tc_f .res 2\n",
            );
        }
        if self.helpers.collx {
            out.push_str(
                "@lx_cx_x .res 2\n@lx_cx_y .res 2\n@lx_cx_w .res 2\n@lx_cx_h .res 2\n\
                 @lx_cx_dx .res 2\n@lx_cx_f .res 2\n@lx_cx_t .res 2\n@lx_cx_lead .res 2\n",
            );
        }
        if self.helpers.colly {
            out.push_str(
                "@lx_cy_x .res 2\n@lx_cy_y .res 2\n@lx_cy_w .res 2\n@lx_cy_h .res 2\n\
                 @lx_cy_dy .res 2\n@lx_cy_f .res 2\n@lx_cy_t .res 2\n@lx_cy_lead .res 2\n",
            );
        }
        // Sprite sheet: contiguous 32-byte tiles at `lx_sheet`, in id order.
        if !self.sprites.is_empty() {
            out.push_str("@lx_sheet\n");
            for (id, (_, rows)) in self.sprites.iter().enumerate() {
                out.push_str(&format!(".sprite lx_spr{id} {} .end\n", rows.join(" ")));
            }
        }
        out
    }
}

/// Whether a function body contains a `return <value>` (rough arity for calls).
fn fn_has_return(decl: &Decl) -> bool {
    fn scan(stmts: &[Stmt]) -> bool {
        stmts.iter().any(|s| match s {
            Stmt::Return(Some(_), _) => true,
            Stmt::If { then, els, .. } => scan(then) || els.as_ref().is_some_and(|e| scan(e)),
            Stmt::While { body, .. } | Stmt::For { body, .. } => scan(body),
            _ => false,
        })
    }
    if let Decl::Function { body, .. } = decl {
        scan(body)
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::assembler::assemble;
    use crate::vm::device::{BTN_A, BTN_LEFT, BTN_RIGHT};
    use crate::vm::VmConsole;

    fn compile_ok(src: &str) -> String {
        let c = compile(src);
        assert!(c.ok(), "luax diagnostics: {:?}", c.diagnostics);
        let built = assemble(&c.asm);
        assert!(built.ok(), "generated asm errors: {:?}\nASM:\n{}", built.diagnostics, c.asm);
        c.asm
    }

    fn load(src: &str) -> VmConsole {
        let c = compile(src);
        assert!(c.ok(), "luax diagnostics: {:?}", c.diagnostics);
        let mut console = VmConsole::new();
        console.write_source("game.lua", src);
        assert!(console.assemble("game.lua").unwrap().ok());
        console.load_rom("game.lua").unwrap();
        console
    }

    #[test]
    fn mover_with_record() {
        let src = r#"
            record Player { x, y }
            local p: Player
            function init() p.x = 32  p.y = 60 end
            function update()
              if btn(LEFT) then p.x = p.x - 1 end
              if btn(RIGHT) then p.x = p.x + 1 end
            end
            function draw() cls(0)  pset(p.x, p.y, 7)  entity(p.x, p.y, 1) end
        "#;
        compile_ok(src);
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 32);
        assert_eq!(c.run_frame(BTN_RIGHT).entities[0].x, 33);
        assert_eq!(c.run_frame(BTN_LEFT).entities[0].x, 32);
    }

    #[test]
    fn record_by_address_param_mutates() {
        let src = r#"
            record Ball { x, vx }
            local b: Ball
            function init() b.x = 10  b.vx = 3 end
            function move(o: Ball) o.x = o.x + o.vx end
            function draw() move(b)  entity(b.x, 0, 1) end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 13); // move() mutated caller's b
        assert_eq!(c.run_frame(0).entities[0].x, 16);
    }

    #[test]
    fn array_of_records() {
        let src = r#"
            record Enemy { x, hp }
            local es: array(4, Enemy)
            function init()
              es[0].x = 5   es[0].hp = 2
              es[1].x = 9   es[1].hp = 7
            end
            function draw() entity(es[0].x + es[1].hp, 0, 1) end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 12); // 5 + 7
    }

    #[test]
    fn for_loop_sums() {
        let src = r#"
            local sum: word
            function draw()
              sum = 0
              for i = 1, 5 do sum = sum + i end
              entity(sum, 0, 1)
            end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 15);
    }

    #[test]
    fn if_elseif_else() {
        let src = r#"
            local a: word
            local out: word
            function init() a = 5 end
            function draw()
              if a == 1 then out = 10 elseif a == 5 then out = 20 else out = 30 end
              entity(out, 0, 1)
            end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 20);
    }

    #[test]
    fn min_max_and_rect_overlap() {
        let src = r#"
            local out: word
            function draw()
              out = min(7, 3) + max(2, 9)   -- 3 + 9 = 12
              entity(out, 0, 1)
              if rect_overlap(0, 0, 10, 10, 5, 5, 10, 10) then entity(1, 0, 2) end
            end
        "#;
        let mut c = load(src);
        let o = c.run_frame(0);
        assert_eq!(o.entities[0].x, 12);
        assert_eq!(o.entities[1].tag, 2); // overlap reported
    }

    #[test]
    fn min_max_both_argument_orders() {
        // Pins min/max branch direction: correct in both operand orders.
        let src = r#"
            function draw()
              entity(min(7, 3), 0, 1)   -- 3
              entity(min(3, 7), 0, 2)   -- 3
              entity(max(2, 9), 0, 3)   -- 9
              entity(max(9, 2), 0, 4)   -- 9
            end
        "#;
        let mut c = load(src);
        let o = c.run_frame(0);
        assert_eq!(o.entities[0].x, 3);
        assert_eq!(o.entities[1].x, 3);
        assert_eq!(o.entities[2].x, 9);
        assert_eq!(o.entities[3].x, 9);
    }

    #[test]
    fn const_folded_array_size() {
        let src = r#"
            local N = 3 + 1
            local a: array(N, word)
            function draw() a[3] = 99  entity(a[3], 0, 1) end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 99);
    }

    #[test]
    fn lua_operators() {
        let src = r#"
            local out: word
            function draw()
              if (3 ~= 4) and not (1 == 2) then out = (5 | 2) ~ 1 end  -- (7)^1 = 6
              entity(out, 0, 1)
            end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 6);
    }

    #[test]
    fn signed_int_comparisons() {
        // vx is int and negative; `vx < 0` must be signed (true), and a large
        // unsigned word must NOT read as negative.
        let src = r#"
            local vx: int
            local w: word
            local out: word
            function init() vx = 0 - 2  w = 0xC000 end
            function draw()
              out = 0
              if vx < 0 then out = out + 1 end      -- signed: -2 < 0 true (+1; unsigned would be false)
              if w > 1 then out = out + 2 end        -- unsigned word: 0xC000 > 1 true (+2)
              if 0 - 3 < vx then out = out + 8 end   -- signed (vx is int): -3 < -2 true (+8)
              entity(out, 0, 1)
            end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 1 + 2 + 8);
    }

    #[test]
    fn int_record_field_is_signed() {
        let src = r#"
            record Mob { vy: int }
            local m: Mob
            local out: word
            function init() m.vy = 0 - 5 end
            function draw()
              if m.vy < 0 then out = 1 else out = 0 end
              entity(out, 0, 1)
            end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 1);
    }

    #[test]
    fn sprite_sheet_blits_by_id() {
        let src = r#"
            sprite a {
              12......
            }
            sprite b {
              34......
            }
            function draw()
              spr(a, 0, 0, 0)
              spr(b, 8, 0, 0)
            end
        "#;
        compile_ok(src);
        let mut c = load(src);
        c.run_frame(0);
        // sprite a = id 0 at (0,0): top row pixels 1,2
        assert_eq!(c.vm.devices.framebuffer[0], 1);
        assert_eq!(c.vm.devices.framebuffer[1], 2);
        // sprite b = id 1 at (8,0): top row pixels 3,4 (from sheet base + 32)
        assert_eq!(c.vm.devices.framebuffer[8], 3);
        assert_eq!(c.vm.devices.framebuffer[9], 4);
    }

    #[test]
    fn comments_inside_sprite_body() {
        // Comments (line and block) inside a sprite must not become pixel rows.
        let src = r#"
            sprite a {
              12......   -- top row
              --[[ the rest is blank ]]
              ........
            }
            function draw() spr(a, 0, 0, 0) end
        "#;
        compile_ok(src);
        let mut c = load(src);
        c.run_frame(0);
        assert_eq!(c.vm.devices.framebuffer[0], 1);
        assert_eq!(c.vm.devices.framebuffer[1], 2);
    }

    #[test]
    fn sprite_flip_flag() {
        // flags=1 mirrors horizontally: top row 1,2 -> columns 7,6.
        let src = r#"
            sprite a { 12...... }
            function draw() spr(a, 0, 0, 1) end
        "#;
        let mut c = load(src);
        c.run_frame(0);
        assert_eq!(c.vm.devices.framebuffer[7], 1);
        assert_eq!(c.vm.devices.framebuffer[6], 2);
    }

    #[test]
    fn sprn_draws_block_row_major() {
        // A 2×2 composite from four sheet tiles: ids advance row-major, cells are
        // 8 px apart. Each sprite's top-left pixel marks which tile landed where.
        let src = r#"
            sprite a { 1....... }
            sprite b { 2....... }
            sprite c { 3....... }
            sprite d { 4....... }
            function draw() sprn(a, 0, 0, 2, 2, 0) end
        "#;
        compile_ok(src);
        let mut c = load(src);
        c.run_frame(0);
        assert_eq!(c.vm.devices.framebuffer[0], 1);            // (0,0) id a
        assert_eq!(c.vm.devices.framebuffer[8], 2);            // (8,0) id b
        assert_eq!(c.vm.devices.framebuffer[8 * 128], 3);      // (0,8) id c
        assert_eq!(c.vm.devices.framebuffer[8 * 128 + 8], 4);  // (8,8) id d
    }

    #[test]
    fn tilemap_mget_mset() {
        let src = r#"
            tilemap level(4, 4)
            local out: word
            function init() mset(1, 2, 7) end
            function draw() out = mget(1, 2)  entity(out, 0, 1) end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 7);
    }

    #[test]
    fn tile_flags_fget_fset() {
        let src = r#"
            local out: word
            function init() fset(3, SOLID, 1)  fset(3, FLAG1, 1) end
            function draw()
              out = 0
              if fget(3, SOLID) == 1 then out = out + 1 end
              if fget(3, FLAG1) == 1 then out = out + 2 end
              if fget(3, FLAG2) == 1 then out = out + 4 end   -- not set
              fset(3, SOLID, 0)                                -- clear it
              if fget(3, SOLID) == 0 then out = out + 8 end
              entity(out, 0, 1)
            end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 1 + 2 + 8);
    }

    #[test]
    fn solid_collision_query() {
        let src = r#"
            tilemap level(4, 4)
            local out: word
            function init()
              mset(1, 1, 5)        -- tile id 5 at cell (1,1)
              fset(5, SOLID, 1)     -- tile 5 is solid
            end
            function draw()
              out = 0
              if solid(12, 12) == 1 then out = out + 1 end  -- (1,1) tile 5 -> solid
              if solid(4, 4) == 1 then out = out + 2 end      -- (0,0) tile 0 -> not
              entity(out, 0, 1)
            end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 1);
    }

    #[test]
    fn tilemap_map_draws_from_sheet() {
        let src = r#"
            sprite a { 12...... }
            tilemap level(2, 2)
            function init() mset(0, 0, a)  mset(1, 1, a) end
            function draw() cls(0)  map(0, 0, 0, 0, 2, 2) end
        "#;
        let mut c = load(src);
        c.run_frame(0);
        // cell (0,0) tile a=0 -> screen (0,0): top-left pixel 1
        assert_eq!(c.vm.devices.framebuffer[0], 1);
        // cell (1,1) tile a -> screen (8,8): top-left pixel 1
        assert_eq!(c.vm.devices.framebuffer[8 * 128 + 8], 1);
    }

    #[test]
    fn tilemap_dimensions_out_of_range() {
        let c = compile("tilemap level(65536, 1)\nfunction draw() end");
        assert!(!c.ok());
        assert!(c.diagnostics.iter().any(|d| d.message.contains("out of range")), "{:?}", c.diagnostics);
    }

    #[test]
    fn solid_off_map_is_not_solid() {
        let src = r#"
            tilemap level(4, 4)
            local out: word
            function init() mset(0, 0, 5)  fset(5, SOLID, 1) end
            function draw()
              out = 0
              if solid(0 - 1, 0) == 0 then out = out + 1 end   -- negative x
              if solid(999, 0) == 0 then out = out + 2 end       -- off the right edge
              if solid(0, 999) == 0 then out = out + 4 end       -- off the bottom edge
              if solid(2, 2) == 1 then out = out + 8 end          -- in-bounds solid cell
              entity(out, 0, 1)
            end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 1 + 2 + 4 + 8);
    }

    #[test]
    fn tilemap_required_for_mget() {
        let c = compile("function draw() local x = mget(0, 0) end");
        assert!(!c.ok());
        assert!(c.diagnostics.iter().any(|d| d.message.contains("tilemap")));
    }

    #[test]
    fn byte_field_truncates() {
        let src = r#"
            record R { v: byte }
            local r: R
            function draw() r.v = 300  entity(r.v, 0, 1) end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 44); // 300 & 0xff
    }

    #[test]
    fn btnp_fires_only_on_rising_edge() {
        // A counter that increments once per fresh A press must not run away
        // while A is held — that's the whole point of btnp vs btn.
        let src = r#"
            local n: word
            function update() if btnp(A) then n = n + 1 end end
            function draw() entity(n, 0, 1) end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(BTN_A).entities[0].x, 1);   // press -> +1
        assert_eq!(c.run_frame(BTN_A).entities[0].x, 1);   // still held -> no change
        assert_eq!(c.run_frame(0).entities[0].x, 1);        // released
        assert_eq!(c.run_frame(BTN_A).entities[0].x, 2);   // new press -> +1
    }

    #[test]
    fn btnr_fires_on_release() {
        let src = r#"
            local n: word
            function update() if btnr(A) then n = n + 1 end end
            function draw() entity(n, 0, 1) end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(BTN_A).entities[0].x, 0);   // press: no release yet
        assert_eq!(c.run_frame(BTN_A).entities[0].x, 0);   // held
        assert_eq!(c.run_frame(0).entities[0].x, 1);        // release -> +1
        assert_eq!(c.run_frame(0).entities[0].x, 1);        // stays released
    }

    #[test]
    fn frame_count_increments() {
        let src = r#"
            function draw() entity(frame_count(), 0, 1) end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 1);
        assert_eq!(c.run_frame(0).entities[0].x, 2);
        assert_eq!(c.run_frame(0).entities[0].x, 3);
    }

    #[test]
    fn len_folds_to_array_length() {
        // len(a) is a compile-time constant that drives the loop bound.
        let src = r#"
            local a: array(5, word)
            local sum: word
            function draw()
              sum = 0
              for i = 0, len(a) - 1 do a[i] = i  sum = sum + a[i] end  -- 0+1+2+3+4
              entity(sum, 0, 1)
              entity(len(a), 0, 2)
            end
        "#;
        let mut c = load(src);
        let o = c.run_frame(0);
        assert_eq!(o.entities[0].x, 10);
        assert_eq!(o.entities[1].x, 5);
    }

    #[test]
    fn len_rejects_non_array() {
        let c = compile("local x: word function draw() entity(len(x), 0, 1) end");
        assert!(!c.ok());
        assert!(c.diagnostics.iter().any(|d| d.message.contains("len()")), "{:?}", c.diagnostics);
    }

    #[test]
    fn map_rect_overlap_corners() {
        let src = r#"
            tilemap level(8, 8)
            local out: word
            function init()
              fset(5, SOLID, 1)
              mset(2, 2, 5)                 -- solid tile at cell (2,2) = pixels 16..23
            end
            function draw()
              out = 0
              if map_rect_overlap(14, 14, 8, 8, SOLID) then out = out + 1 end  -- SE corner hits
              if map_rect_overlap(0, 0, 8, 8, SOLID) then out = out + 2 end      -- clear
              entity(out, 0, 1)
            end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 1);
    }

    #[test]
    fn collide_x_stops_at_wall() {
        // A solid wall column at cell x=4 (pixels 32..). An 8-px rect starting at
        // x=20 and moving +5/frame must stop flush against the wall at x=24.
        let src = r#"
            tilemap level(8, 8)
            local px: word
            function init()
              fset(5, SOLID, 1)
              for y = 0, 7 do mset(4, y, 5) end
              px = 20
            end
            function draw()
              px = collide_x(px, 40, 8, 8, 5, SOLID)
              entity(px, 0, 1)
            end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 24); // snapped to the wall
        assert_eq!(c.run_frame(0).entities[0].x, 24); // stays pinned
    }

    #[test]
    fn collide_y_lands_on_floor() {
        // Floor row at cell y=6 (pixels 48..). A rect falling +5/frame from y=30
        // lands at y=40 (bottom edge 47, flush above the floor).
        let src = r#"
            tilemap level(8, 8)
            local py: word
            function init()
              fset(5, SOLID, 1)
              for x = 0, 7 do mset(x, 6, 5) end
              py = 30
            end
            function draw()
              py = collide_y(20, py, 8, 8, 5, SOLID)
              entity(20, py, 1)
            end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].y, 35);
        assert_eq!(c.run_frame(0).entities[0].y, 40); // reaches the floor
        assert_eq!(c.run_frame(0).entities[0].y, 40); // and stops
    }

    #[test]
    fn touching_floor_detects_ground() {
        let src = r#"
            tilemap level(8, 8)
            local out: word
            function init()
              fset(5, SOLID, 1)
              for x = 0, 7 do mset(x, 6, 5) end   -- floor row at cell y=6 (pixels 48..)
            end
            function draw()
              out = 0
              if touching_floor(20, 40, 8, 8, SOLID) then out = out + 1 end  -- bottom edge 47, floor below
              if touching_floor(20, 20, 8, 8, SOLID) then out = out + 2 end   -- airborne
              entity(out, 0, 1)
            end
        "#;
        let mut c = load(src);
        assert_eq!(c.run_frame(0).entities[0].x, 1);
    }

    #[test]
    fn collision_helpers_need_tilemap() {
        let c = compile("function draw() if collide_x(0,0,8,8,1,SOLID) > 0 then end end");
        assert!(!c.ok());
        assert!(c.diagnostics.iter().any(|d| d.message.contains("tilemap")), "{:?}", c.diagnostics);
    }

    #[test]
    fn diagnostics() {
        assert!(!compile("function draw() x = 1 end").ok()); // unknown var
        assert!(!compile("function draw() foo() end").ok()); // unknown function
        assert!(!compile("record R { x } local r: R function draw() r.y = 1 end").ok()); // no field
        assert!(!compile("function draw() break end").ok()); // break outside loop
        assert!(!compile("function draw() cls() end").ok()); // wrong arg count
        assert!(!compile("local a: array(3, Nope) function draw() end").ok()); // unknown type
    }
}
