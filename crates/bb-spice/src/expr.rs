//! Expression language used in `{…}` / `'…'` parameters, `.param`, `.func` and B sources.
//!
//! Grammar (lowest to highest precedence): `c ? a : b`, `||`, `&&`, `== !=`, `< <= > >=`,
//! `+ -`, `* / %`, unary `- + !`, `^` / `**` (right associative). Numbers accept SPICE SI
//! suffixes (`10k`, `4k7`, `2.2u`, `1meg`). Probes: `v(node)`, `v(a,b)`, `i(vname)`. Runtime
//! variables: `time`, `temper`/`temp`, `hertz`. External variables: `ext("name")`.
//!
//! Expressions are parsed once into [`Expr`], resolved against parameters during elaboration
//! (leaving only runtime leaves), and — when they still depend on runtime values — compiled to a
//! small stack machine ([`Compiled`]) that evaluates the value *and* exact partial derivatives
//! (forward-mode AD), which the Newton solver needs for B sources.

use std::fmt;

/// Error while lexing/parsing an expression; `pos` is a byte offset into the expression text.
#[derive(Debug, Clone, PartialEq)]
pub struct ExprError {
    pub pos: usize,
    pub msg: String,
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

/// Variables whose value is only known while simulating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RtVar {
    /// Simulation time in seconds (0 outside transient).
    Time,
    /// Circuit temperature in °C.
    Temper,
    /// AC analysis frequency in Hz (0 outside AC).
    Hertz,
}

/// Built-in functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Func {
    Abs,
    Sqrt,
    Exp,
    Ln,
    Log10,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
    Asinh,
    Acosh,
    Atanh,
    Sgn,
    /// Unit step: 1 for x>0, 0.5 at 0 (ngspice), 0 below.
    U,
    /// Ramp: max(x, 0).
    Uramp,
    Floor,
    Ceil,
    Int,
    Nint,
    Min,
    Max,
    Pow,
    /// `|x|^y` (LTspice / numparam semantics).
    Pwr,
    /// `sgn(x)·|x|^y` (ngspice B-source `pwr` semantics).
    Pwrs,
    Atan2,
    Hypot,
    Fmod,
    Limit,
    If,
    Table,
    /// Random functions (Monte Carlo); nominal value outside Monte Carlo runs.
    Agauss,
    Gauss,
    Aunif,
    Unif,
    Mc,
}

impl Func {
    /// Looks up a built-in by (lower-case) name.
    pub fn from_name(name: &str) -> Option<Func> {
        Some(match name {
            "abs" => Func::Abs,
            "sqrt" => Func::Sqrt,
            "exp" => Func::Exp,
            "ln" | "log" => Func::Ln,
            "log10" => Func::Log10,
            "sin" => Func::Sin,
            "cos" => Func::Cos,
            "tan" => Func::Tan,
            "asin" | "arcsin" => Func::Asin,
            "acos" | "arccos" => Func::Acos,
            "atan" | "arctan" => Func::Atan,
            "sinh" => Func::Sinh,
            "cosh" => Func::Cosh,
            "tanh" => Func::Tanh,
            "asinh" => Func::Asinh,
            "acosh" => Func::Acosh,
            "atanh" => Func::Atanh,
            "sgn" | "sign" => Func::Sgn,
            "u" | "stp" => Func::U,
            "uramp" => Func::Uramp,
            "floor" => Func::Floor,
            "ceil" => Func::Ceil,
            "int" => Func::Int,
            "nint" | "round" => Func::Nint,
            "min" => Func::Min,
            "max" => Func::Max,
            "pow" => Func::Pow,
            "pwr" => Func::Pwr,
            "pwrs" => Func::Pwrs,
            "atan2" => Func::Atan2,
            "hypot" => Func::Hypot,
            "fmod" | "mod" => Func::Fmod,
            "limit" => Func::Limit,
            "if" | "ternary_fcn" => Func::If,
            "table" => Func::Table,
            "agauss" => Func::Agauss,
            "gauss" => Func::Gauss,
            "aunif" => Func::Aunif,
            "unif" => Func::Unif,
            "mc" => Func::Mc,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Func::Abs => "abs",
            Func::Sqrt => "sqrt",
            Func::Exp => "exp",
            Func::Ln => "ln",
            Func::Log10 => "log10",
            Func::Sin => "sin",
            Func::Cos => "cos",
            Func::Tan => "tan",
            Func::Asin => "asin",
            Func::Acos => "acos",
            Func::Atan => "atan",
            Func::Sinh => "sinh",
            Func::Cosh => "cosh",
            Func::Tanh => "tanh",
            Func::Asinh => "asinh",
            Func::Acosh => "acosh",
            Func::Atanh => "atanh",
            Func::Sgn => "sgn",
            Func::U => "u",
            Func::Uramp => "uramp",
            Func::Floor => "floor",
            Func::Ceil => "ceil",
            Func::Int => "int",
            Func::Nint => "nint",
            Func::Min => "min",
            Func::Max => "max",
            Func::Pow => "pow",
            Func::Pwr => "pwr",
            Func::Pwrs => "pwrs",
            Func::Atan2 => "atan2",
            Func::Hypot => "hypot",
            Func::Fmod => "fmod",
            Func::Limit => "limit",
            Func::If => "if",
            Func::Table => "table",
            Func::Agauss => "agauss",
            Func::Gauss => "gauss",
            Func::Aunif => "aunif",
            Func::Unif => "unif",
            Func::Mc => "mc",
        }
    }

    /// Accepted argument counts (min, max).
    fn arity(self) -> (usize, usize) {
        match self {
            Func::Min | Func::Max => (2, usize::MAX),
            Func::Pow | Func::Pwr | Func::Pwrs | Func::Atan2 | Func::Hypot | Func::Fmod => (2, 2),
            Func::Limit | Func::If => (3, 3),
            Func::Table => (3, usize::MAX),
            Func::Agauss | Func::Gauss => (2, 3),
            Func::Aunif | Func::Unif | Func::Mc => (2, 2),
            _ => (1, 1),
        }
    }

    /// True for the Monte Carlo random functions.
    pub fn is_random(self) -> bool {
        matches!(
            self,
            Func::Agauss | Func::Gauss | Func::Aunif | Func::Unif | Func::Mc
        )
    }
}

/// Expression tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    /// A parameter name (or `time`/`temper`/`pi` before resolution).
    Ident(String),
    Rt(RtVar),
    /// `ext("name")`: an external variable set by the co-simulation host.
    Ext(String),
    /// `v(a)` or `v(a,b)` (node names, lower case).
    Volt(String, Option<String>),
    /// `i(name)`: current through an element (lower case).
    Curr(String),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
    /// Call of a user `.func` (resolved during elaboration).
    UserCall(String, Vec<Expr>),
}

// ---------------------------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Str(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
    Question,
    Colon,
    End,
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
}

const OPS: [&str; 18] = [
    "**", "<=", ">=", "==", "!=", "&&", "||", "+", "-", "*", "/", "%", "^", "<", ">", "!", "=", "&",
];

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Lexer { src, pos: 0 }
    }

    fn err(&self, pos: usize, msg: impl Into<String>) -> ExprError {
        ExprError {
            pos,
            msg: msg.into(),
        }
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.src[self.pos..].chars().next() {
            if c.is_whitespace() {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    /// Reads raw text up to (not including) the next top-level `,` or `)`.
    fn raw_arg(&mut self) -> String {
        self.skip_ws();
        let start = self.pos;
        let mut depth = 0;
        while let Some(c) = self.peek_char() {
            if depth == 0 && (c == ',' || c == ')') {
                break;
            }
            if c == '(' {
                depth += 1;
            } else if c == ')' {
                depth -= 1;
            }
            self.pos += c.len_utf8();
        }
        self.src[start..self.pos].trim().to_string()
    }

    fn next(&mut self) -> Result<(usize, Tok), ExprError> {
        self.skip_ws();
        let start = self.pos;
        let Some(c) = self.peek_char() else {
            return Ok((start, Tok::End));
        };
        let rest = &self.src[self.pos..];
        if c.is_ascii_digit() || (c == '.' && rest[1..].starts_with(|d: char| d.is_ascii_digit())) {
            return self.number().map(|n| (start, Tok::Num(n)));
        }
        if c.is_alphabetic() || c == '_' {
            let mut end = self.pos;
            for ch in rest.chars() {
                if ch.is_alphanumeric() || ch == '_' || ch == '$' || ch == '#' {
                    end += ch.len_utf8();
                } else {
                    break;
                }
            }
            let id = self.src[self.pos..end].to_lowercase();
            self.pos = end;
            return Ok((start, Tok::Ident(id)));
        }
        if c == '"' {
            let body = &rest[1..];
            let Some(close) = body.find('"') else {
                return Err(self.err(start, "unterminated string"));
            };
            let s = body[..close].to_string();
            self.pos += close + 2;
            return Ok((start, Tok::Str(s)));
        }
        self.pos += c.len_utf8();
        let t = match c {
            // braces inside an expression (ngspice numparam style `{vwc}`) group like parens
            '(' | '{' => Tok::LParen,
            ')' | '}' => Tok::RParen,
            ',' => Tok::Comma,
            '?' => Tok::Question,
            ':' => Tok::Colon,
            _ => {
                self.pos = start;
                let Some(op) = OPS.iter().find(|op| rest.starts_with(**op)) else {
                    return Err(self.err(start, format!("unexpected character `{c}`")));
                };
                self.pos += op.len();
                Tok::Op(op)
            }
        };
        Ok((start, t))
    }

    fn number(&mut self) -> Result<f64, ExprError> {
        let start = self.pos;
        let b = self.src.as_bytes();
        let mut i = self.pos;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
            i += 1;
        }
        if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
            let mut j = i + 1;
            if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                j += 1;
            }
            if j < b.len() && b[j].is_ascii_digit() {
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
            }
        }
        // Suffix letters (SI prefix + unit), optionally followed by digits ("4k7").
        let mut end = i;
        let mut letters = 0;
        for ch in self.src[i..].chars() {
            if ch.is_alphabetic() || ch == 'µ' || ch == 'Ω' {
                end += ch.len_utf8();
                letters += 1;
            } else {
                break;
            }
        }
        if letters > 0 {
            let suffix = self.src[i..end].to_ascii_lowercase();
            let is_prefix = matches!(
                suffix.as_str(),
                "t" | "g" | "k" | "m" | "u" | "n" | "p" | "f" | "a" | "meg" | "µ"
            );
            if is_prefix && !self.src[start..i].contains(['.', 'e', 'E']) {
                while end < b.len() && b[end].is_ascii_digit() {
                    end += 1;
                }
            }
        }
        let text = &self.src[start..end];
        self.pos = end;
        bb_core::si::parse(text).map_err(|_| self.err(start, format!("bad number `{text}`")))
    }
}

// ---------------------------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------------------------

struct Parser<'a> {
    lex: Lexer<'a>,
    tok: Tok,
    tok_pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Result<Self, ExprError> {
        let mut lex = Lexer::new(src);
        let (tok_pos, tok) = lex.next()?;
        Ok(Parser { lex, tok, tok_pos })
    }

    fn bump(&mut self) -> Result<Tok, ExprError> {
        let (p, t) = self.lex.next()?;
        self.tok_pos = p;
        Ok(std::mem::replace(&mut self.tok, t))
    }

    fn err(&self, msg: impl Into<String>) -> ExprError {
        ExprError {
            pos: self.tok_pos,
            msg: msg.into(),
        }
    }

    fn expect(&mut self, t: Tok, what: &str) -> Result<(), ExprError> {
        if self.tok == t {
            self.bump()?;
            Ok(())
        } else {
            Err(self.err(format!("expected {what}, found {}", describe(&self.tok))))
        }
    }

    fn is_op(&self, op: &str) -> bool {
        matches!(&self.tok, Tok::Op(o) if *o == op)
    }

    fn ternary(&mut self) -> Result<Expr, ExprError> {
        let c = self.or()?;
        if self.tok == Tok::Question {
            self.bump()?;
            let a = self.ternary()?;
            self.expect(Tok::Colon, "`:`")?;
            let b = self.ternary()?;
            return Ok(Expr::Cond(Box::new(c), Box::new(a), Box::new(b)));
        }
        Ok(c)
    }

    fn or(&mut self) -> Result<Expr, ExprError> {
        let mut l = self.and()?;
        while self.is_op("||") {
            self.bump()?;
            let r = self.and()?;
            l = Expr::Binary(BinOp::Or, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Expr, ExprError> {
        let mut l = self.equality()?;
        while self.is_op("&&") {
            self.bump()?;
            let r = self.equality()?;
            l = Expr::Binary(BinOp::And, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn equality(&mut self) -> Result<Expr, ExprError> {
        let mut l = self.relational()?;
        loop {
            let op = if self.is_op("==") || self.is_op("=") {
                BinOp::Eq
            } else if self.is_op("!=") {
                BinOp::Ne
            } else {
                break;
            };
            self.bump()?;
            let r = self.relational()?;
            l = Expr::Binary(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn relational(&mut self) -> Result<Expr, ExprError> {
        let mut l = self.additive()?;
        loop {
            let op = match &self.tok {
                Tok::Op("<") => BinOp::Lt,
                Tok::Op("<=") => BinOp::Le,
                Tok::Op(">") => BinOp::Gt,
                Tok::Op(">=") => BinOp::Ge,
                _ => break,
            };
            self.bump()?;
            let r = self.additive()?;
            l = Expr::Binary(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn additive(&mut self) -> Result<Expr, ExprError> {
        let mut l = self.multiplicative()?;
        loop {
            let op = match &self.tok {
                Tok::Op("+") => BinOp::Add,
                Tok::Op("-") => BinOp::Sub,
                _ => break,
            };
            self.bump()?;
            let r = self.multiplicative()?;
            l = Expr::Binary(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn multiplicative(&mut self) -> Result<Expr, ExprError> {
        let mut l = self.unary()?;
        loop {
            let op = match &self.tok {
                Tok::Op("*") => BinOp::Mul,
                Tok::Op("/") => BinOp::Div,
                Tok::Op("%") => BinOp::Mod,
                _ => break,
            };
            self.bump()?;
            let r = self.unary()?;
            l = Expr::Binary(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn unary(&mut self) -> Result<Expr, ExprError> {
        match &self.tok {
            Tok::Op("-") => {
                self.bump()?;
                let e = self.unary()?;
                Ok(match e {
                    Expr::Num(n) => Expr::Num(-n),
                    e => Expr::Unary(UnOp::Neg, Box::new(e)),
                })
            }
            Tok::Op("+") => {
                self.bump()?;
                self.unary()
            }
            Tok::Op("!") => {
                self.bump()?;
                let e = self.unary()?;
                Ok(Expr::Unary(UnOp::Not, Box::new(e)))
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<Expr, ExprError> {
        let base = self.primary()?;
        if self.is_op("^") || self.is_op("**") {
            self.bump()?;
            let exp = self.unary()?;
            return Ok(Expr::Binary(BinOp::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn args(&mut self) -> Result<Vec<Expr>, ExprError> {
        // current token is LParen
        self.bump()?;
        let mut args = Vec::new();
        if self.tok == Tok::RParen {
            self.bump()?;
            return Ok(args);
        }
        loop {
            args.push(self.ternary()?);
            match self.tok {
                Tok::Comma => {
                    self.bump()?;
                }
                Tok::RParen => {
                    self.bump()?;
                    return Ok(args);
                }
                _ => {
                    return Err(self.err(format!(
                        "expected `,` or `)`, found {}",
                        describe(&self.tok)
                    )));
                }
            }
        }
    }

    /// After an identifier like `v`/`i`/`ext` whose `(` has *not* been lexed yet, reads raw
    /// comma-separated arguments directly from the source.
    fn raw_args(&mut self) -> Result<Vec<String>, ExprError> {
        // self.tok is LParen and the lexer is positioned right after it.
        let mut out = Vec::new();
        loop {
            let a = self.lex.raw_arg();
            out.push(a);
            match self.lex.peek_char() {
                Some(',') => {
                    self.lex.pos += 1;
                }
                Some(')') => {
                    self.lex.pos += 1;
                    break;
                }
                _ => {
                    return Err(ExprError {
                        pos: self.lex.pos,
                        msg: "expected `)`".into(),
                    });
                }
            }
        }
        self.bump()?; // load the token after `)`
        Ok(out)
    }

    fn primary(&mut self) -> Result<Expr, ExprError> {
        let pos = self.tok_pos;
        match self.bump()? {
            Tok::Num(n) => Ok(Expr::Num(n)),
            Tok::LParen => {
                let e = self.ternary()?;
                self.expect(Tok::RParen, "`)`")?;
                Ok(e)
            }
            Tok::Ident(id) => {
                if self.tok == Tok::LParen {
                    match id.as_str() {
                        "v" => {
                            let a = self.raw_args()?;
                            return match a.len() {
                                1 if !a[0].is_empty() => Ok(Expr::Volt(a[0].to_lowercase(), None)),
                                2 if !a[0].is_empty() && !a[1].is_empty() => {
                                    Ok(Expr::Volt(a[0].to_lowercase(), Some(a[1].to_lowercase())))
                                }
                                _ => Err(ExprError {
                                    pos,
                                    msg: "v() takes one or two node names".into(),
                                }),
                            };
                        }
                        "i" => {
                            let a = self.raw_args()?;
                            return match a.len() {
                                1 if !a[0].is_empty() => Ok(Expr::Curr(a[0].to_lowercase())),
                                _ => Err(ExprError {
                                    pos,
                                    msg: "i() takes one element name".into(),
                                }),
                            };
                        }
                        "vdb" | "vm" | "vp" | "vph" | "vr" | "vi" | "idb" | "im" | "ip" | "ir"
                        | "ii" => {
                            // ngspice output-variable shorthands (only meaningful in .meas)
                            let a = self.raw_args()?;
                            let probe = if id.starts_with('v') {
                                match a.len() {
                                    1 => Expr::Volt(a[0].to_lowercase(), None),
                                    2 => Expr::Volt(a[0].to_lowercase(), Some(a[1].to_lowercase())),
                                    _ => {
                                        return Err(ExprError {
                                            pos,
                                            msg: format!("{id}() takes one or two node names"),
                                        });
                                    }
                                }
                            } else {
                                Expr::Curr(a[0].to_lowercase())
                            };
                            let f = match &id[1..] {
                                "db" => "db",
                                "m" => "mag",
                                "p" | "ph" => "ph",
                                "r" => "real",
                                _ => "imag",
                            };
                            return Ok(Expr::UserCall(f.into(), vec![probe]));
                        }
                        "ext" => {
                            let a = self.raw_args()?;
                            if a.len() != 1 || a[0].is_empty() {
                                return Err(ExprError {
                                    pos,
                                    msg: "ext() takes one name".into(),
                                });
                            }
                            let name = a[0].trim_matches('"').to_string();
                            return Ok(Expr::Ext(name));
                        }
                        _ => {}
                    }
                    let args = self.args()?;
                    if let Some(f) = Func::from_name(&id) {
                        let (lo, hi) = f.arity();
                        if args.len() < lo || args.len() > hi {
                            return Err(ExprError {
                                pos,
                                msg: format!(
                                    "{}() expects {} argument{}, got {}",
                                    f.name(),
                                    arity_text(lo, hi),
                                    if hi == 1 { "" } else { "s" },
                                    args.len()
                                ),
                            });
                        }
                        if f == Func::Table && args.len() % 2 == 0 {
                            return Err(ExprError {
                                pos,
                                msg: "table(x, x1, y1, x2, y2, …) needs x plus pairs".into(),
                            });
                        }
                        return Ok(Expr::Call(f, args));
                    }
                    return Ok(Expr::UserCall(id, args));
                }
                Ok(match id.as_str() {
                    "time" => Expr::Rt(RtVar::Time),
                    "hertz" => Expr::Rt(RtVar::Hertz),
                    "temper" => Expr::Rt(RtVar::Temper),
                    _ => Expr::Ident(id),
                })
            }
            Tok::Str(_) => Err(ExprError {
                pos,
                msg: "string literal only allowed as ext(\"name\") argument".into(),
            }),
            t => Err(ExprError {
                pos,
                msg: format!("expected a value, found {}", describe(&t)),
            }),
        }
    }
}

fn arity_text(lo: usize, hi: usize) -> String {
    if lo == hi {
        lo.to_string()
    } else if hi == usize::MAX {
        format!("at least {lo}")
    } else {
        format!("{lo}–{hi}")
    }
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Num(n) => format!("number {n}"),
        Tok::Ident(s) => format!("`{s}`"),
        Tok::Str(s) => format!("\"{s}\""),
        Tok::Op(o) => format!("`{o}`"),
        Tok::LParen => "`(`".into(),
        Tok::RParen => "`)`".into(),
        Tok::Comma => "`,`".into(),
        Tok::Question => "`?`".into(),
        Tok::Colon => "`:`".into(),
        Tok::End => "end of expression".into(),
    }
}

impl Expr {
    /// Parses an expression (without surrounding braces/quotes).
    ///
    /// ```
    /// use bb_spice::expr::Expr;
    /// let e = Expr::parse("350k*pwr(lux,-0.7)").unwrap();
    /// let v = e.eval_with(&|name| (name == "lux").then_some(180.0)).unwrap();
    /// assert!((v - 9_233.8).abs() < 1.0);
    /// ```
    pub fn parse(src: &str) -> Result<Expr, ExprError> {
        let mut p = Parser::new(src)?;
        if p.tok == Tok::End {
            return Err(p.err("empty expression"));
        }
        let e = p.ternary()?;
        if p.tok != Tok::End {
            return Err(p.err(format!("unexpected {} after expression", describe(&p.tok))));
        }
        Ok(e)
    }

    /// Parses the longest expression at the start of `src`; returns it and the byte offset where
    /// parsing stopped (the first token that cannot continue the expression).
    pub fn parse_prefix(src: &str) -> Result<(Expr, usize), ExprError> {
        let mut p = Parser::new(src)?;
        if p.tok == Tok::End {
            return Err(p.err("empty expression"));
        }
        let e = p.ternary()?;
        let stop = if p.tok == Tok::End {
            src.len()
        } else {
            p.tok_pos
        };
        Ok((e, stop))
    }

    /// Constant value if this is a literal number.
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Expr::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// Visits every sub-expression (pre-order).
    pub fn visit(&self, f: &mut dyn FnMut(&Expr)) {
        f(self);
        match self {
            Expr::Unary(_, a) => a.visit(f),
            Expr::Binary(_, a, b) => {
                a.visit(f);
                b.visit(f);
            }
            Expr::Cond(c, a, b) => {
                c.visit(f);
                a.visit(f);
                b.visit(f);
            }
            Expr::Call(_, args) | Expr::UserCall(_, args) => args.iter().for_each(|a| a.visit(f)),
            _ => {}
        }
    }

    /// Bottom-up rewrite: `f` is offered each node after its children were rewritten; returning
    /// `Some` replaces the node.
    pub fn rewrite(
        &self,
        f: &mut dyn FnMut(&Expr) -> Result<Option<Expr>, String>,
    ) -> Result<Expr, String> {
        let node = match self {
            Expr::Unary(op, a) => Expr::Unary(*op, Box::new(a.rewrite(f)?)),
            Expr::Binary(op, a, b) => {
                Expr::Binary(*op, Box::new(a.rewrite(f)?), Box::new(b.rewrite(f)?))
            }
            Expr::Cond(c, a, b) => Expr::Cond(
                Box::new(c.rewrite(f)?),
                Box::new(a.rewrite(f)?),
                Box::new(b.rewrite(f)?),
            ),
            Expr::Call(func, args) => Expr::Call(
                *func,
                args.iter()
                    .map(|a| a.rewrite(f))
                    .collect::<Result<_, _>>()?,
            ),
            Expr::UserCall(n, args) => Expr::UserCall(
                n.clone(),
                args.iter()
                    .map(|a| a.rewrite(f))
                    .collect::<Result<_, _>>()?,
            ),
            other => other.clone(),
        };
        Ok(f(&node)?.unwrap_or(node))
    }

    /// True if the expression needs values only known during simulation (node voltages,
    /// currents, time, temperature, frequency, external variables).
    pub fn is_runtime(&self) -> bool {
        let mut rt = false;
        self.visit(&mut |e| {
            if matches!(
                e,
                Expr::Rt(_) | Expr::Ext(_) | Expr::Volt(..) | Expr::Curr(_)
            ) {
                rt = true;
            }
        });
        rt
    }

    /// True if it reads node voltages or branch currents.
    pub fn has_probes(&self) -> bool {
        let mut p = false;
        self.visit(&mut |e| {
            if matches!(e, Expr::Volt(..) | Expr::Curr(_)) {
                p = true;
            }
        });
        p
    }

    /// True if it uses `ext(..)`.
    pub fn has_ext(&self) -> bool {
        let mut p = false;
        self.visit(&mut |e| {
            if matches!(e, Expr::Ext(_)) {
                p = true;
            }
        });
        p
    }

    /// Folds constant sub-expressions (random functions are kept).
    pub fn fold(&self) -> Expr {
        self.rewrite(&mut |e| Ok(fold_node(e)))
            .unwrap_or_else(|_| self.clone())
    }

    /// Evaluates a constant expression; identifiers are looked up through `lookup`.
    /// Runtime leaves evaluate as 0 (time, hertz), 27 (temper) or error.
    pub fn eval_with(&self, lookup: &dyn Fn(&str) -> Option<f64>) -> Result<f64, String> {
        Ok(match self {
            Expr::Num(n) => *n,
            Expr::Ident(id) => match lookup(id) {
                Some(v) => v,
                None if id == "pi" => std::f64::consts::PI,
                None if id == "e" => std::f64::consts::E,
                None if id == "temp" => 27.0,
                None => return Err(format!("undefined parameter `{id}`")),
            },
            Expr::Rt(RtVar::Temper) => 27.0,
            Expr::Rt(_) => 0.0,
            Expr::Ext(n) => return Err(format!("ext(\"{n}\") has no value here")),
            Expr::Volt(..) | Expr::Curr(_) => {
                return Err("node voltages/currents are not available in a parameter".into());
            }
            Expr::Unary(op, a) => un(*op, a.eval_with(lookup)?),
            Expr::Binary(op, a, b) => bin(*op, a.eval_with(lookup)?, b.eval_with(lookup)?),
            Expr::Cond(c, a, b) => {
                if c.eval_with(lookup)? != 0.0 {
                    a.eval_with(lookup)?
                } else {
                    b.eval_with(lookup)?
                }
            }
            Expr::Call(f, args) => {
                let v: Vec<f64> = args
                    .iter()
                    .map(|a| a.eval_with(lookup))
                    .collect::<Result<_, _>>()?;
                call(*f, &v)
            }
            Expr::UserCall(n, _) => return Err(format!("unknown function `{n}`")),
        })
    }
}

fn fold_node(e: &Expr) -> Option<Expr> {
    match e {
        Expr::Unary(op, a) => a.as_num().map(|x| Expr::Num(un(*op, x))),
        Expr::Binary(op, a, b) => match (a.as_num(), b.as_num()) {
            (Some(x), Some(y)) => Some(Expr::Num(bin(*op, x, y))),
            _ => None,
        },
        Expr::Cond(c, a, b) => c.as_num().map(|c| {
            if c != 0.0 {
                (**a).clone()
            } else {
                (**b).clone()
            }
        }),
        Expr::Call(f, args) if !f.is_random() => {
            let v: Option<Vec<f64>> = args.iter().map(|a| a.as_num()).collect();
            v.map(|v| Expr::Num(call(*f, &v)))
        }
        Expr::Ident(id) if id == "pi" => Some(Expr::Num(std::f64::consts::PI)),
        _ => None,
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Num(n) => write!(f, "{}", fmt_g(*n)),
            Expr::Ident(s) => f.write_str(s),
            Expr::Rt(RtVar::Time) => f.write_str("time"),
            Expr::Rt(RtVar::Temper) => f.write_str("temper"),
            Expr::Rt(RtVar::Hertz) => f.write_str("hertz"),
            Expr::Ext(n) => write!(f, "ext(\"{n}\")"),
            Expr::Volt(a, None) => write!(f, "v({a})"),
            Expr::Volt(a, Some(b)) => write!(f, "v({a},{b})"),
            Expr::Curr(a) => write!(f, "i({a})"),
            Expr::Unary(UnOp::Neg, a) => write!(f, "(-{a})"),
            Expr::Unary(UnOp::Not, a) => write!(f, "(!{a})"),
            Expr::Binary(op, a, b) => {
                let o = match op {
                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                    BinOp::Mul => "*",
                    BinOp::Div => "/",
                    BinOp::Mod => "%",
                    BinOp::Pow => "^",
                    BinOp::Lt => "<",
                    BinOp::Le => "<=",
                    BinOp::Gt => ">",
                    BinOp::Ge => ">=",
                    BinOp::Eq => "==",
                    BinOp::Ne => "!=",
                    BinOp::And => "&&",
                    BinOp::Or => "||",
                };
                write!(f, "({a}{o}{b})")
            }
            Expr::Cond(c, a, b) => write!(f, "({c} ? {a} : {b})"),
            Expr::Call(_, args) | Expr::UserCall(_, args) => {
                let name = match self {
                    Expr::UserCall(n, _) => n.as_str(),
                    _ => func_name(self),
                };
                write!(f, "{name}(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{a}")?;
                }
                f.write_str(")")
            }
        }
    }
}

fn func_name(e: &Expr) -> &'static str {
    match e {
        Expr::Call(f, _) => f.name(),
        _ => "",
    }
}

/// Formats a number compactly and losslessly (`%.17g`-like, trimmed).
pub fn fmt_g(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:e}");
    // Prefer plain notation for moderate exponents when it is not longer.
    let plain = format!("{v}");
    if plain.len() <= s.len() { plain } else { s }
}

// ---------------------------------------------------------------------------------------------
// Scalar semantics
// ---------------------------------------------------------------------------------------------

fn b2f(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

fn un(op: UnOp, x: f64) -> f64 {
    match op {
        UnOp::Neg => -x,
        UnOp::Not => b2f(x == 0.0),
    }
}

fn bin(op: BinOp, x: f64, y: f64) -> f64 {
    match op {
        BinOp::Add => x + y,
        BinOp::Sub => x - y,
        BinOp::Mul => x * y,
        BinOp::Div => x / y,
        BinOp::Mod => x % y,
        BinOp::Pow => pow_semantics(x, y),
        BinOp::Lt => b2f(x < y),
        BinOp::Le => b2f(x <= y),
        BinOp::Gt => b2f(x > y),
        BinOp::Ge => b2f(x >= y),
        BinOp::Eq => b2f(x == y),
        BinOp::Ne => b2f(x != y),
        BinOp::And => b2f(x != 0.0 && y != 0.0),
        BinOp::Or => b2f(x != 0.0 || y != 0.0),
    }
}

/// `x^y` / `pow(x, y)`: negative base with a non-integer exponent gives 0 (ngspice B-source
/// behaviour) instead of NaN.
fn pow_semantics(x: f64, y: f64) -> f64 {
    if x < 0.0 && y.fract() != 0.0 {
        0.0
    } else {
        x.powf(y)
    }
}

fn table(x: f64, pts: &[f64]) -> (f64, f64) {
    let n = pts.len() / 2;
    let xs = |i: usize| pts[2 * i];
    let ys = |i: usize| pts[2 * i + 1];
    if n == 0 {
        return (0.0, 0.0);
    }
    if x <= xs(0) {
        return (ys(0), 0.0);
    }
    if x >= xs(n - 1) {
        return (ys(n - 1), 0.0);
    }
    for i in 0..n - 1 {
        let (x0, x1) = (xs(i), xs(i + 1));
        if x >= x0 && x <= x1 {
            if x1 == x0 {
                return (ys(i + 1), 0.0);
            }
            let s = (ys(i + 1) - ys(i)) / (x1 - x0);
            return (ys(i) + s * (x - x0), s);
        }
    }
    (ys(n - 1), 0.0)
}

/// Evaluates a built-in on constant arguments (random functions return their nominal value).
pub fn call(f: Func, a: &[f64]) -> f64 {
    let x = a.first().copied().unwrap_or(0.0);
    match f {
        Func::Abs => x.abs(),
        Func::Sqrt => x.sqrt(),
        Func::Exp => x.exp(),
        Func::Ln => x.ln(),
        Func::Log10 => x.log10(),
        Func::Sin => x.sin(),
        Func::Cos => x.cos(),
        Func::Tan => x.tan(),
        Func::Asin => x.asin(),
        Func::Acos => x.acos(),
        Func::Atan => x.atan(),
        Func::Sinh => x.sinh(),
        Func::Cosh => x.cosh(),
        Func::Tanh => x.tanh(),
        Func::Asinh => x.asinh(),
        Func::Acosh => x.acosh(),
        Func::Atanh => x.atanh(),
        Func::Sgn => {
            if x > 0.0 {
                1.0
            } else if x < 0.0 {
                -1.0
            } else {
                0.0
            }
        }
        Func::U => {
            if x > 0.0 {
                1.0
            } else if x == 0.0 {
                0.5
            } else {
                0.0
            }
        }
        Func::Uramp => x.max(0.0),
        Func::Floor => x.floor(),
        Func::Ceil => x.ceil(),
        Func::Int => x.trunc(),
        Func::Nint => {
            // round half to even, like ngspice's nint
            let r = x.round();
            if (x - x.trunc()).abs() == 0.5 {
                2.0 * (x / 2.0).round()
            } else {
                r
            }
        }
        Func::Min => a.iter().copied().fold(f64::INFINITY, f64::min),
        Func::Max => a.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        Func::Pow => pow_semantics(x, a[1]),
        Func::Pwr => x.abs().powf(a[1]),
        Func::Pwrs => x.signum() * x.abs().powf(a[1]),
        Func::Atan2 => x.atan2(a[1]),
        Func::Hypot => x.hypot(a[1]),
        Func::Fmod => x % a[1],
        Func::Limit => {
            let (lo, hi) = if a[1] <= a[2] {
                (a[1], a[2])
            } else {
                (a[2], a[1])
            };
            x.clamp(lo, hi)
        }
        Func::If => {
            if x != 0.0 {
                a[1]
            } else {
                a[2]
            }
        }
        Func::Table => table(x, &a[1..]).0,
        Func::Agauss | Func::Gauss | Func::Aunif | Func::Unif | Func::Mc => x,
    }
}

// ---------------------------------------------------------------------------------------------
// Compiled runtime expressions with forward-mode derivatives
// ---------------------------------------------------------------------------------------------

/// Leaves of a runtime expression, mapped to input slots by [`Compiled::compile`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Leaf {
    Volt(String, Option<String>),
    Curr(String),
    Rt(RtVar),
    Ext(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Const(f64),
    Input(u16),
    Un(UnOp),
    Bin(BinOp),
    Select,
    Call(Func, u16),
}

/// A resolved runtime expression as a stack program. Evaluates value + gradient with respect to
/// its inputs (the leaves listed in [`Compiled::leaves`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Compiled {
    ops: Vec<Op>,
    /// Input leaves in slot order.
    pub leaves: Vec<Leaf>,
    max_stack: usize,
}

/// Reusable scratch buffers for [`Compiled::eval_grad`].
#[derive(Debug, Default, Clone)]
pub struct EvalScratch {
    vals: Vec<f64>,
    grads: Vec<f64>,
    args: Vec<f64>,
}

impl Compiled {
    /// Compiles a resolved expression (no `Ident`/`UserCall` left). Fails on unresolved names.
    pub fn compile(e: &Expr) -> Result<Compiled, String> {
        let mut c = Compiled {
            ops: Vec::new(),
            leaves: Vec::new(),
            max_stack: 0,
        };
        let mut depth = 0usize;
        c.emit(e, &mut depth)?;
        Ok(c)
    }

    fn leaf(&mut self, l: Leaf) -> u16 {
        if let Some(i) = self.leaves.iter().position(|x| *x == l) {
            return i as u16;
        }
        self.leaves.push(l);
        (self.leaves.len() - 1) as u16
    }

    fn push(&mut self, op: Op, depth: &mut usize, pops: usize) {
        self.ops.push(op);
        *depth = *depth + 1 - pops;
        self.max_stack = self.max_stack.max(*depth);
    }

    fn emit(&mut self, e: &Expr, depth: &mut usize) -> Result<(), String> {
        match e {
            Expr::Num(n) => self.push(Op::Const(*n), depth, 0),
            Expr::Ident(id) => {
                if id == "pi" {
                    self.push(Op::Const(std::f64::consts::PI), depth, 0)
                } else {
                    return Err(format!("undefined parameter `{id}`"));
                }
            }
            Expr::Rt(v) => {
                let i = self.leaf(Leaf::Rt(*v));
                self.push(Op::Input(i), depth, 0)
            }
            Expr::Ext(n) => {
                let i = self.leaf(Leaf::Ext(n.clone()));
                self.push(Op::Input(i), depth, 0)
            }
            Expr::Volt(a, b) => {
                let i = self.leaf(Leaf::Volt(a.clone(), b.clone()));
                self.push(Op::Input(i), depth, 0)
            }
            Expr::Curr(a) => {
                let i = self.leaf(Leaf::Curr(a.clone()));
                self.push(Op::Input(i), depth, 0)
            }
            Expr::Unary(op, a) => {
                self.emit(a, depth)?;
                self.push(Op::Un(*op), depth, 1)
            }
            Expr::Binary(op, a, b) => {
                self.emit(a, depth)?;
                self.emit(b, depth)?;
                self.push(Op::Bin(*op), depth, 2)
            }
            Expr::Cond(c, a, b) => {
                self.emit(c, depth)?;
                self.emit(a, depth)?;
                self.emit(b, depth)?;
                self.push(Op::Select, depth, 3)
            }
            Expr::Call(f, args) => {
                if *f == Func::If {
                    self.emit(&args[0], depth)?;
                    self.emit(&args[1], depth)?;
                    self.emit(&args[2], depth)?;
                    self.push(Op::Select, depth, 3);
                    return Ok(());
                }
                for a in args {
                    self.emit(a, depth)?;
                }
                self.push(Op::Call(*f, args.len() as u16), depth, args.len())
            }
            Expr::UserCall(n, _) => return Err(format!("unknown function `{n}`")),
        }
        Ok(())
    }

    /// Number of inputs.
    pub fn n_inputs(&self) -> usize {
        self.leaves.len()
    }

    /// Value only.
    pub fn eval(&self, inputs: &[f64], s: &mut EvalScratch) -> f64 {
        let st = &mut s.vals;
        st.clear();
        for op in &self.ops {
            match *op {
                Op::Const(c) => st.push(c),
                Op::Input(i) => st.push(inputs[i as usize]),
                Op::Un(o) => {
                    let x = st.pop().unwrap_or(0.0);
                    st.push(un(o, x));
                }
                Op::Bin(o) => {
                    let y = st.pop().unwrap_or(0.0);
                    let x = st.pop().unwrap_or(0.0);
                    st.push(bin_rt(o, x, y));
                }
                Op::Select => {
                    let b = st.pop().unwrap_or(0.0);
                    let a = st.pop().unwrap_or(0.0);
                    let c = st.pop().unwrap_or(0.0);
                    st.push(if c != 0.0 { a } else { b });
                }
                Op::Call(f, n) => {
                    let n = n as usize;
                    let at = st.len() - n;
                    let v = call_rt(f, &st[at..]).0;
                    st.truncate(at);
                    st.push(v);
                }
            }
        }
        st.pop().unwrap_or(0.0)
    }

    /// Value and gradient; `grad` is resized to `n_inputs()`.
    pub fn eval_grad(&self, inputs: &[f64], grad: &mut Vec<f64>, s: &mut EvalScratch) -> f64 {
        let k = self.leaves.len();
        let vals = &mut s.vals;
        let g = &mut s.grads;
        vals.clear();
        g.clear();
        for op in &self.ops {
            match *op {
                Op::Const(c) => {
                    vals.push(c);
                    g.extend(std::iter::repeat_n(0.0, k));
                }
                Op::Input(i) => {
                    vals.push(inputs[i as usize]);
                    let at = g.len();
                    g.extend(std::iter::repeat_n(0.0, k));
                    g[at + i as usize] = 1.0;
                }
                Op::Un(o) => {
                    let n = vals.len() - 1;
                    let x = vals[n];
                    vals[n] = un(o, x);
                    let gs = &mut g[n * k..];
                    match o {
                        UnOp::Neg => gs.iter_mut().for_each(|d| *d = -*d),
                        UnOp::Not => gs.iter_mut().for_each(|d| *d = 0.0),
                    }
                }
                Op::Bin(o) => {
                    let n = vals.len() - 2;
                    let (x, y) = (vals[n], vals[n + 1]);
                    let v = bin_rt(o, x, y);
                    let (dx, dy) = bin_partials(o, x, y, v);
                    vals.truncate(n);
                    vals.push(v);
                    let (ga, gb) = g[n * k..].split_at_mut(k);
                    for j in 0..k {
                        ga[j] = dx * ga[j] + dy * gb[j];
                    }
                    g.truncate((n + 1) * k);
                }
                Op::Select => {
                    let n = vals.len() - 3;
                    let c = vals[n];
                    let pick = if c != 0.0 { 1 } else { 2 };
                    vals[n] = vals[n + pick];
                    vals.truncate(n + 1);
                    let src = (n + pick) * k;
                    for j in 0..k {
                        g[n * k + j] = g[src + j];
                    }
                    g.truncate((n + 1) * k);
                }
                Op::Call(f, na) => {
                    let na = na as usize;
                    let n = vals.len() - na;
                    s.args.clear();
                    s.args.extend_from_slice(&vals[n..]);
                    let (v, partials) = call_rt_grad(f, &s.args);
                    vals.truncate(n);
                    vals.push(v);
                    // combine gradients
                    let mut acc = vec![0.0; k];
                    for (ai, p) in partials.iter().enumerate() {
                        if *p != 0.0 {
                            let base = (n + ai) * k;
                            for j in 0..k {
                                acc[j] += p * g[base + j];
                            }
                        }
                    }
                    g.truncate(n * k);
                    g.extend_from_slice(&acc);
                }
            }
        }
        grad.clear();
        let n = vals.len();
        if n == 0 {
            grad.resize(k, 0.0);
            return 0.0;
        }
        grad.extend_from_slice(&g[(n - 1) * k..n * k]);
        vals[n - 1]
    }
}

/// Runtime binary op: like [`bin`] but with a guarded divide (x/0 → ±1e30-ish large value
/// keeps Newton iterations finite).
fn bin_rt(op: BinOp, x: f64, y: f64) -> f64 {
    bin(op, x, y)
}

fn bin_partials(op: BinOp, x: f64, y: f64, v: f64) -> (f64, f64) {
    match op {
        BinOp::Add => (1.0, 1.0),
        BinOp::Sub => (1.0, -1.0),
        BinOp::Mul => (y, x),
        BinOp::Div => (1.0 / y, -x / (y * y)),
        BinOp::Mod => (1.0, -(x / y).trunc()),
        BinOp::Pow => {
            if x == 0.0 {
                (if y == 1.0 { 1.0 } else { 0.0 }, 0.0)
            } else if x < 0.0 {
                if y.fract() != 0.0 {
                    (0.0, 0.0)
                } else {
                    (y * x.powf(y - 1.0), 0.0)
                }
            } else {
                (y * x.powf(y - 1.0), v * x.ln())
            }
        }
        _ => (0.0, 0.0),
    }
}

/// Runtime-safe exp: exact up to 80, linear continuation above (keeps Newton finite).
fn exp_lim(x: f64) -> (f64, f64) {
    const XMAX: f64 = 80.0;
    if x > XMAX {
        let e = XMAX.exp();
        (e * (1.0 + x - XMAX), e)
    } else {
        let e = x.exp();
        (e, e)
    }
}

fn call_rt(f: Func, a: &[f64]) -> (f64, ()) {
    match f {
        Func::Exp => (exp_lim(a[0]).0, ()),
        _ => (call(f, a), ()),
    }
}

/// Value plus partial derivatives with respect to each argument.
fn call_rt_grad(f: Func, a: &[f64]) -> (f64, Vec<f64>) {
    let x = a[0];
    let one = |v: f64, d: f64| (v, vec![d]);
    match f {
        Func::Abs => one(x.abs(), if x >= 0.0 { 1.0 } else { -1.0 }),
        Func::Sqrt => {
            let s = x.sqrt();
            one(s, if s > 0.0 { 0.5 / s } else { 0.0 })
        }
        Func::Exp => {
            let (v, d) = exp_lim(x);
            one(v, d)
        }
        Func::Ln => one(x.ln(), 1.0 / x),
        Func::Log10 => one(x.log10(), 1.0 / (x * std::f64::consts::LN_10)),
        Func::Sin => one(x.sin(), x.cos()),
        Func::Cos => one(x.cos(), -x.sin()),
        Func::Tan => {
            let c = x.cos();
            one(x.tan(), 1.0 / (c * c))
        }
        Func::Asin => one(x.asin(), 1.0 / (1.0 - x * x).sqrt()),
        Func::Acos => one(x.acos(), -1.0 / (1.0 - x * x).sqrt()),
        Func::Atan => one(x.atan(), 1.0 / (1.0 + x * x)),
        Func::Sinh => one(x.sinh(), x.cosh()),
        Func::Cosh => one(x.cosh(), x.sinh()),
        Func::Tanh => {
            let t = x.tanh();
            one(t, 1.0 - t * t)
        }
        Func::Asinh => one(x.asinh(), 1.0 / (x * x + 1.0).sqrt()),
        Func::Acosh => one(x.acosh(), 1.0 / (x * x - 1.0).sqrt()),
        Func::Atanh => one(x.atanh(), 1.0 / (1.0 - x * x)),
        Func::Uramp => one(x.max(0.0), if x > 0.0 { 1.0 } else { 0.0 }),
        Func::Sgn | Func::U | Func::Floor | Func::Ceil | Func::Int | Func::Nint => {
            one(call(f, a), 0.0)
        }
        Func::Min | Func::Max => {
            let mut best = 0;
            for i in 1..a.len() {
                let better = if f == Func::Min {
                    a[i] < a[best]
                } else {
                    a[i] > a[best]
                };
                if better {
                    best = i;
                }
            }
            let mut d = vec![0.0; a.len()];
            d[best] = 1.0;
            (a[best], d)
        }
        Func::Pow => {
            let v = pow_semantics(x, a[1]);
            let (dx, dy) = bin_partials(BinOp::Pow, x, a[1], v);
            (v, vec![dx, dy])
        }
        Func::Pwr | Func::Pwrs => {
            let y = a[1];
            let ax = x.abs();
            let m = ax.powf(y);
            let s = if f == Func::Pwrs { x.signum() } else { 1.0 };
            let v = s * m;
            let dx = if ax > 0.0 {
                let d = y * ax.powf(y - 1.0);
                if f == Func::Pwrs { d } else { d * x.signum() }
            } else {
                0.0
            };
            let dy = if ax > 0.0 { v * ax.ln() } else { 0.0 };
            (v, vec![dx, dy])
        }
        Func::Atan2 => {
            let (y, xx) = (a[0], a[1]);
            let r2 = y * y + xx * xx;
            let (dy, dx) = if r2 > 0.0 {
                (xx / r2, -y / r2)
            } else {
                (0.0, 0.0)
            };
            (y.atan2(xx), vec![dy, dx])
        }
        Func::Hypot => {
            let h = x.hypot(a[1]);
            if h > 0.0 {
                (h, vec![x / h, a[1] / h])
            } else {
                (0.0, vec![0.0, 0.0])
            }
        }
        Func::Fmod => (x % a[1], vec![1.0, -(x / a[1]).trunc()]),
        Func::Limit => {
            let (lo, hi) = if a[1] <= a[2] {
                (a[1], a[2])
            } else {
                (a[2], a[1])
            };
            let v = x.clamp(lo, hi);
            let mut d = vec![0.0; 3];
            if x < lo {
                d[if a[1] <= a[2] { 1 } else { 2 }] = 1.0;
            } else if x > hi {
                d[if a[1] <= a[2] { 2 } else { 1 }] = 1.0;
            } else {
                d[0] = 1.0;
            }
            (v, d)
        }
        Func::If => {
            let mut d = vec![0.0; 3];
            if x != 0.0 {
                d[1] = 1.0;
                (a[1], d)
            } else {
                d[2] = 1.0;
                (a[2], d)
            }
        }
        Func::Table => {
            let (v, s) = table(x, &a[1..]);
            let mut d = vec![0.0; a.len()];
            d[0] = s;
            (v, d)
        }
        Func::Agauss | Func::Gauss | Func::Aunif | Func::Unif | Func::Mc => {
            let mut d = vec![0.0; a.len()];
            d[0] = 1.0;
            (x, d)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> f64 {
        Expr::parse(s).unwrap().eval_with(&|_| None).unwrap()
    }

    #[test]
    fn precedence_and_numbers() {
        assert_eq!(ev("1+2*3"), 7.0);
        assert_eq!(ev("-2^2"), -4.0);
        assert_eq!(ev("2^3^2"), 512.0);
        assert_eq!(ev("2**-1"), 0.5);
        assert!((ev("4k7") - 4700.0).abs() < 1e-9);
        assert!((ev("10meg/2") - 5e6).abs() < 1e-6);
        assert!((ev("2.2u*1e6") - 2.2).abs() < 1e-12);
        assert!((ev(".5m") - 5e-4).abs() < 1e-15);
        assert_eq!(ev("1e3"), 1000.0);
        assert_eq!(ev("7%3"), 1.0);
        assert_eq!(ev("1 < 2 && 3 >= 3"), 1.0);
        assert_eq!(ev("1 ? 4 : 5"), 4.0);
        assert_eq!(ev("0 ? 4 : 2 > 1 ? 6 : 7"), 6.0);
        assert_eq!(ev("!0"), 1.0);
    }

    #[test]
    fn functions_match_ngspice() {
        assert_eq!(ev("pwr(-2,2)"), 4.0);
        assert!((ev("pwr(-2,0.5)") - 2f64.sqrt()).abs() < 1e-12);
        assert_eq!(ev("pwrs(-2,3)"), -8.0);
        assert_eq!(ev("pow(-2,3)"), -8.0);
        assert_eq!(ev("pow(-2,0.5)"), 0.0);
        assert!((ev("log(10)") - 10f64.ln()).abs() < 1e-12);
        assert!((ev("log10(1000)") - 3.0).abs() < 1e-12);
        assert_eq!(ev("u(0)"), 0.5);
        assert_eq!(ev("u(-1)"), 0.0);
        assert_eq!(ev("uramp(-1)"), 0.0);
        assert_eq!(ev("int(2.7)"), 2.0);
        assert_eq!(ev("nint(2.5)"), 2.0);
        assert_eq!(ev("nint(3.5)"), 4.0);
        assert_eq!(ev("floor(-2.5)"), -3.0);
        assert_eq!(ev("limit(5,0,3)"), 3.0);
        assert_eq!(ev("limit(-5,3,0)"), 0.0);
        assert_eq!(ev("min(3,1,2)"), 1.0);
        assert_eq!(ev("max(3,1,2)"), 3.0);
        assert_eq!(ev("table(1.5, 1,10, 2,20)"), 15.0);
        assert_eq!(ev("table(5, 1,10, 2,20)"), 20.0);
        assert_eq!(ev("if(1, 2, 3)"), 2.0);
        assert_eq!(ev("sgn(-3)"), -1.0);
        assert!((ev("2*pi") - std::f64::consts::TAU).abs() < 1e-12);
        assert!((ev("atan2(1,1)") - std::f64::consts::FRAC_PI_4).abs() < 1e-12);
    }

    #[test]
    fn probes_and_ext() {
        let e = Expr::parse("V(out, x1.mid)*2 + i(V1) + ext(\"J2.lux\") + time").unwrap();
        assert!(e.is_runtime());
        assert!(e.has_ext());
        let c = Compiled::compile(&e).unwrap();
        assert_eq!(c.leaves.len(), 4);
        assert_eq!(c.leaves[0], Leaf::Volt("out".into(), Some("x1.mid".into())));
        assert_eq!(c.leaves[2], Leaf::Ext("J2.lux".into()));
    }

    #[test]
    fn errors_have_positions() {
        let e = Expr::parse("1 + * 2").unwrap_err();
        assert_eq!(e.pos, 4);
        let e = Expr::parse("max(1)").unwrap_err();
        assert!(e.msg.contains("at least 2"), "{}", e.msg);
        let e = Expr::parse("(1+2").unwrap_err();
        assert!(e.msg.contains("`)`"), "{}", e.msg);
        assert!(Expr::parse("").is_err());
        assert!(Expr::parse("1 2").is_err());
    }

    fn check_grad(src: &str, x: &[f64]) {
        let e = Expr::parse(src).unwrap();
        let c = Compiled::compile(&e).unwrap();
        let mut s = EvalScratch::default();
        let mut g = Vec::new();
        let v = c.eval_grad(x, &mut g, &mut s);
        assert!(
            (v - c.eval(x, &mut s)).abs() <= 1e-12 * v.abs().max(1.0),
            "{src}"
        );
        for j in 0..x.len() {
            let h = 1e-6 * x[j].abs().max(1.0);
            let mut xp = x.to_vec();
            xp[j] += h;
            let mut xm = x.to_vec();
            xm[j] -= h;
            let fd = (c.eval(&xp, &mut s) - c.eval(&xm, &mut s)) / (2.0 * h);
            assert!(
                (fd - g[j]).abs() <= 1e-5 * fd.abs().max(1.0),
                "{src}: d/dx{j} ad={} fd={fd}",
                g[j]
            );
        }
    }

    #[test]
    fn gradients_match_finite_differences() {
        check_grad("v(a)*v(b) + sin(v(a))/v(b)", &[0.7, 1.3]);
        check_grad("exp(v(a)) - ln(v(b)) + sqrt(v(a))", &[0.7, 1.3]);
        check_grad("pwr(v(a),1.7) * pow(v(b), 2.5)", &[0.7, 1.3]);
        check_grad("v(a)^v(b)", &[0.7, 1.3]);
        check_grad("max(v(a), v(b)) + min(v(a)*2, v(b))", &[0.7, 1.3]);
        check_grad("v(a) > 0.5 ? v(b)*3 : v(a)", &[0.7, 1.3]);
        check_grad(
            "tanh(v(a)-v(b)) + atan2(v(a), v(b)) + table(v(a), 0,0, 1,2)",
            &[0.7, 1.3],
        );
        check_grad("limit(v(a)*4, 0, v(b))", &[0.2, 1.3]);
    }
}
