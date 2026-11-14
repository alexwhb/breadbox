//! Netlist text → [`Netlist`].

use super::*;
use crate::expr::{BinOp, Expr};
use crate::meas::{Edge, MeasKind, StatFunc, Trigger};
use std::path::Path;

/// Options for [`Netlist::parse_with`].
#[derive(Clone)]
pub struct ParseOptions {
    /// File name used in error spans.
    pub file_name: Option<String>,
    /// Directory for relative `.include`/`.lib` paths (default: current directory).
    pub base_dir: Option<PathBuf>,
    /// Whether the first line is the title (true for netlists, false for libraries).
    pub first_line_is_title: bool,
    /// Libraries to resolve unknown subcircuits/models.
    pub libraries: Vec<Arc<Library>>,
    /// Custom include loader (path → text). Defaults to reading the file system.
    pub loader: Option<Arc<dyn Fn(&Path) -> std::io::Result<String> + Send + Sync>>,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions {
            file_name: None,
            base_dir: None,
            first_line_is_title: true,
            libraries: Vec::new(),
            loader: None,
        }
    }
}

impl std::fmt::Debug for ParseOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParseOptions")
            .field("file_name", &self.file_name)
            .field("base_dir", &self.base_dir)
            .field("first_line_is_title", &self.first_line_is_title)
            .finish()
    }
}

// ---------------------------------------------------------------------------------------------
// Logical lines
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Line {
    text: String,
    /// (offset in `text`, source line, source column of that offset)
    segs: Vec<(usize, u32, u32)>,
    file: Option<Arc<str>>,
}

impl Line {
    fn span_at(&self, off: usize) -> Span {
        let mut seg = self.segs[0];
        for s in &self.segs {
            if s.0 <= off {
                seg = *s;
            } else {
                break;
            }
        }
        let col = seg.2 + self.text[seg.0..off.min(self.text.len())].chars().count() as u32;
        Span::new(self.file.clone(), seg.1, col)
    }
    fn span(&self) -> Span {
        self.span_at(0)
    }
}

/// Strips an inline comment (`;` anywhere, `$` preceded by whitespace) outside of braces/quotes.
fn strip_inline_comment(s: &str) -> &str {
    let b = s.as_bytes();
    let mut depth = 0i32;
    let mut quote = false;
    for i in 0..b.len() {
        match b[i] {
            b'{' if !quote => depth += 1,
            b'}' if !quote => depth -= 1,
            b'\'' => quote = !quote,
            b';' if depth == 0 && !quote => return &s[..i],
            b'$' if depth == 0 && !quote && (i == 0 || b[i - 1] == b' ' || b[i - 1] == b'\t') => {
                return &s[..i];
            }
            _ => {}
        }
    }
    s
}

fn logical_lines(text: &str, file: Option<Arc<str>>, first_is_title: bool) -> (String, Vec<Line>) {
    let mut title = String::new();
    let mut lines: Vec<Line> = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let lineno = idx as u32 + 1;
        let raw = raw.trim_end_matches('\r');
        if idx == 0 && first_is_title {
            title = raw.trim().to_string();
            continue;
        }
        let lead = raw.len() - raw.trim_start().len();
        let t = raw.trim_start();
        if t.is_empty() || t.starts_with('*') {
            continue;
        }
        let body = strip_inline_comment(t);
        if body.trim().is_empty() {
            continue;
        }
        if let Some(rest) = body.strip_prefix('+') {
            if let Some(last) = lines.last_mut() {
                let off = last.text.len() + 1;
                last.text.push(' ');
                last.segs.push((off, lineno, lead as u32 + 2));
                last.text.push_str(rest.trim_end());
                continue;
            }
        }
        lines.push(Line {
            text: body.trim_end().to_string(),
            segs: vec![(0, lineno, lead as u32 + 1)],
            file: file.clone(),
        });
    }
    (title, lines)
}

// ---------------------------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum TK {
    Word,
    /// `{…}` or `'…'` contents.
    Expr,
    LParen,
    RParen,
    Comma,
    Eq,
}

#[derive(Debug, Clone)]
struct Tok {
    kind: TK,
    text: String,
    start: usize,
}

impl Tok {
    fn lower(&self) -> String {
        self.text.to_ascii_lowercase()
    }
    fn is_word(&self, w: &str) -> bool {
        self.kind == TK::Word && self.text.eq_ignore_ascii_case(w)
    }
}

fn tokenize(line: &Line) -> Result<Vec<Tok>, ParseError> {
    let s = &line.text;
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        match c {
            b'{' => {
                let mut depth = 0;
                let start = i;
                let mut j = i;
                while j < b.len() {
                    if b[j] == b'{' {
                        depth += 1;
                    } else if b[j] == b'}' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    j += 1;
                }
                if j >= b.len() {
                    return Err(ParseError::new(line.span_at(start), "unbalanced `{`"));
                }
                out.push(Tok {
                    kind: TK::Expr,
                    text: s[start + 1..j].to_string(),
                    start,
                });
                i = j + 1;
            }
            b'\'' => {
                let start = i;
                let Some(close) = s[i + 1..].find('\'') else {
                    return Err(ParseError::new(
                        line.span_at(start),
                        "unterminated `'` expression",
                    ));
                };
                out.push(Tok {
                    kind: TK::Expr,
                    text: s[i + 1..i + 1 + close].to_string(),
                    start,
                });
                i += close + 2;
            }
            b'(' => {
                out.push(Tok {
                    kind: TK::LParen,
                    text: "(".into(),
                    start: i,
                });
                i += 1;
            }
            b')' => {
                out.push(Tok {
                    kind: TK::RParen,
                    text: ")".into(),
                    start: i,
                });
                i += 1;
            }
            b',' => {
                out.push(Tok {
                    kind: TK::Comma,
                    text: ",".into(),
                    start: i,
                });
                i += 1;
            }
            b'=' => {
                out.push(Tok {
                    kind: TK::Eq,
                    text: "=".into(),
                    start: i,
                });
                i += 1;
            }
            _ => {
                let start = i;
                while i < b.len()
                    && !b[i].is_ascii_whitespace()
                    && !matches!(b[i], b'(' | b')' | b',' | b'=' | b'{' | b'\'')
                {
                    i += 1;
                }
                // keep `}` out of words (malformed input) by letting the caller see it
                out.push(Tok {
                    kind: TK::Word,
                    text: s[start..i].to_string(),
                    start,
                });
            }
        }
    }
    Ok(out)
}

/// Cursor over a card's tokens.
struct Cur<'a> {
    line: &'a Line,
    toks: Vec<Tok>,
    i: usize,
}

impl<'a> Cur<'a> {
    fn new(line: &'a Line) -> Result<Self, ParseError> {
        Ok(Cur {
            line,
            toks: tokenize(line)?,
            i: 0,
        })
    }
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i)
    }
    fn peek_at(&self, k: usize) -> Option<&Tok> {
        self.toks.get(self.i + k)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.i).cloned();
        if t.is_some() {
            self.i += 1;
        }
        t
    }
    fn done(&self) -> bool {
        self.i >= self.toks.len()
    }
    fn span(&self) -> Span {
        match self.peek() {
            Some(t) => self.line.span_at(t.start),
            None => self.line.span_at(self.line.text.len()),
        }
    }
    fn span_of(&self, t: &Tok) -> Span {
        self.line.span_at(t.start)
    }
    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError::new(self.span(), msg)
    }
    fn err_at(&self, t: &Tok, msg: impl Into<String>) -> ParseError {
        ParseError::new(self.span_of(t), msg)
    }
    /// True if the next tokens are `word =`.
    fn at_kv(&self) -> bool {
        matches!(self.peek(), Some(t) if t.kind == TK::Word)
            && matches!(self.peek_at(1), Some(t) if t.kind == TK::Eq)
    }
    fn word(&mut self, what: &str) -> Result<Tok, ParseError> {
        match self.peek() {
            Some(t) if t.kind == TK::Word => Ok(self.next().unwrap()),
            Some(t) => Err(self.err_at(t, format!("expected {what}, found `{}`", t.text))),
            None => Err(self.err(format!("expected {what}"))),
        }
    }
    fn node(&mut self, what: &str) -> Result<String, ParseError> {
        Ok(normalize_node(&self.word(what)?.text))
    }
    fn skip_commas(&mut self) {
        while matches!(self.peek(), Some(t) if t.kind == TK::Comma) {
            self.i += 1;
        }
    }
    /// Parses a value token (number, identifier expression, or `{expr}`).
    fn value(&mut self, what: &str) -> Result<Expr, ParseError> {
        self.skip_commas();
        let Some(t) = self.next() else {
            return Err(self.err(format!("expected {what}")));
        };
        self.value_of(&t, what)
    }
    fn value_of(&self, t: &Tok, what: &str) -> Result<Expr, ParseError> {
        match t.kind {
            TK::Word | TK::Expr => parse_expr_at(self.line, t, &t.text),
            _ => Err(self.err_at(t, format!("expected {what}, found `{}`", t.text))),
        }
    }
    fn is_value_tok(t: &Tok) -> bool {
        match t.kind {
            TK::Expr => true,
            TK::Word => looks_numeric(&t.text),
            _ => false,
        }
    }
    /// `key = value` (value may be `a,b,c` list for `ic=`); returns key and values.
    fn kv(&mut self) -> Result<(String, Tok, Vec<Expr>), ParseError> {
        let k = self.next().unwrap();
        self.next(); // '='
        let mut vals = vec![self.value(&format!("value for `{}`", k.text))?];
        while matches!(self.peek(), Some(t) if t.kind == TK::Comma)
            && matches!(self.peek_at(1), Some(t) if Cur::is_value_tok(t))
        {
            self.next();
            vals.push(self.value("value")?);
        }
        Ok((k.lower(), k, vals))
    }
}

fn looks_numeric(s: &str) -> bool {
    let s = s.trim_start_matches(['+', '-']);
    s.starts_with(|c: char| c.is_ascii_digit())
        || (s.starts_with('.') && s[1..].starts_with(|c: char| c.is_ascii_digit()))
}

fn normalize_node(s: &str) -> String {
    let l = s.to_ascii_lowercase();
    if l == "gnd" || l == "0" {
        "0".to_string()
    } else {
        l
    }
}

fn parse_expr_at(line: &Line, t: &Tok, text: &str) -> Result<Expr, ParseError> {
    let base = if t.kind == TK::Expr {
        t.start + 1
    } else {
        t.start
    };
    // percentage values (tol=5%)
    let txt = text.trim();
    if let Some(p) = txt.strip_suffix('%') {
        if let Ok(e) = Expr::parse(p) {
            return Ok(Expr::Binary(BinOp::Div, Box::new(e), Box::new(Expr::Num(100.0))).fold());
        }
    }
    Expr::parse(text).map_err(|e| {
        ParseError::new(
            line.span_at(base + e.pos),
            format!("in expression `{}`: {}", text.trim(), e.msg),
        )
    })
}

// ---------------------------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------------------------

struct State<'o> {
    opts: &'o ParseOptions,
    nl: Netlist,
    /// Scope stack: index 0 = top level.
    stack: Vec<Scope>,
    depth: usize,
    /// Raw analysis/control cards evaluated after all params are known.
    deferred: Vec<Line>,
}

pub(super) fn parse_netlist(text: &str, opts: &ParseOptions) -> Result<Netlist, ParseError> {
    let file: Option<Arc<str>> = opts.file_name.as_deref().map(Arc::from);
    let (title, lines) = logical_lines(text, file, opts.first_line_is_title);
    let mut st = State {
        opts,
        nl: Netlist::default(),
        stack: vec![Scope::default()],
        depth: 0,
        deferred: Vec::new(),
    };
    st.nl.title = title;
    st.nl.libraries = opts.libraries.clone();
    st.nl.base_dir = opts.base_dir.clone();
    st.lines(&lines)?;
    if st.stack.len() > 1 {
        let s = st.stack.last().unwrap();
        return Err(ParseError::new(
            s.span.clone(),
            format!("`.subckt {}` is missing its `.ends`", s.name),
        ));
    }
    st.nl.top = st.stack.pop().unwrap();
    // Deferred cards need parameter values.
    let deferred = std::mem::take(&mut st.deferred);
    for line in &deferred {
        st.control_card(line)?;
    }
    Ok(st.nl)
}

impl<'o> State<'o> {
    fn scope(&mut self) -> &mut Scope {
        self.stack.last_mut().unwrap()
    }

    fn warn(&mut self, span: Span, msg: impl Into<String>) {
        self.nl.warnings.push(Diagnostic::warning(span, msg));
    }

    fn lines(&mut self, lines: &[Line]) -> Result<(), ParseError> {
        let mut i = 0;
        while i < lines.len() {
            let line = &lines[i];
            let first = line
                .text
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            if first == ".control" {
                // skip to .endc
                let start = line.span();
                loop {
                    i += 1;
                    if i >= lines.len() {
                        return Err(ParseError::new(start, "`.control` without `.endc`"));
                    }
                    if lines[i].text.trim().eq_ignore_ascii_case(".endc") {
                        break;
                    }
                }
                self.nl.warnings.push(Diagnostic::info(
                    start,
                    "`.control` block ignored (ngspice scripting is not supported)",
                ));
                i += 1;
                continue;
            }
            if first == ".end" {
                break;
            }
            if first == ".lib" && line.text.split_whitespace().count() == 2 && self.depth > 0 {
                // `.lib section` inside a library file being included in full: skip markers.
                i += 1;
                continue;
            }
            if first == ".endl" {
                i += 1;
                continue;
            }
            self.card(line)?;
            i += 1;
        }
        Ok(())
    }

    fn card(&mut self, line: &Line) -> Result<(), ParseError> {
        let c0 = line.text.chars().next().unwrap_or(' ').to_ascii_lowercase();
        if c0 == '.' {
            return self.dot_card(line);
        }
        let mut cur = Cur::new(line)?;
        let name_tok = cur.word("element name")?;
        let name = name_tok.lower();
        let el = match c0 {
            'r' | 'c' | 'l' => self.passive(&mut cur, name, c0)?,
            'k' => self.coupling(&mut cur, name)?,
            'v' | 'i' => self.source(&mut cur, name, c0)?,
            'e' | 'g' => self.vcxs(&mut cur, name, c0)?,
            'f' | 'h' => self.ccxs(&mut cur, name, c0)?,
            'b' => self.bsource(&mut cur, name)?,
            's' | 'w' => self.switch(&mut cur, name, c0)?,
            'd' => self.semis(&mut cur, name, 2, c0)?,
            'q' => self.bjt(&mut cur, name)?,
            'm' => self.semis(&mut cur, name, 4, c0)?,
            'j' => self.semis(&mut cur, name, 3, c0)?,
            't' => self.tline(&mut cur, name)?,
            'x' => self.subckt_inst(&mut cur, name)?,
            _ => {
                return Err(ParseError::new(
                    line.span(),
                    format!(
                        "unknown element type `{}` in `{}`",
                        name_tok.text.chars().next().unwrap_or('?'),
                        name_tok.text
                    ),
                ));
            }
        };
        let el = ElementCard {
            span: line.span(),
            ..el
        };
        if self.scope().elements.iter().any(|e| e.name == el.name) {
            return Err(ParseError::new(
                line.span(),
                format!("duplicate element name `{}`", el.name),
            ));
        }
        self.scope().elements.push(el);
        Ok(())
    }

    /// Collects trailing `key=value` params and bare flags.
    fn params(
        &mut self,
        cur: &mut Cur,
        flags: &[&str],
    ) -> Result<(ParamList, Vec<String>), ParseError> {
        let mut params = Vec::new();
        let mut seen_flags = Vec::new();
        while !cur.done() {
            cur.skip_commas();
            if cur.done() {
                break;
            }
            if cur.at_kv() {
                let (k, ktok, vals) = cur.kv()?;
                if vals.len() == 1 {
                    params.push((k, vals.into_iter().next().unwrap()));
                } else {
                    // list values (ic=1,2 ; tc=a,b) are stored as k, k.1, k.2 …
                    let _ = ktok;
                    for (i, v) in vals.into_iter().enumerate() {
                        params.push((
                            if i == 0 {
                                k.clone()
                            } else {
                                format!("{k}.{i}")
                            },
                            v,
                        ));
                    }
                }
                continue;
            }
            let t = cur.next().unwrap();
            if t.kind == TK::Word && flags.iter().any(|f| t.text.eq_ignore_ascii_case(f)) {
                seen_flags.push(t.lower());
                continue;
            }
            return Err(cur.err_at(&t, format!("unexpected `{}`", t.text)));
        }
        Ok((params, seen_flags))
    }

    fn passive(
        &mut self,
        cur: &mut Cur,
        name: String,
        c0: char,
    ) -> Result<ElementCard, ParseError> {
        let a = cur.node("node")?;
        let b = cur.node("node")?;
        let mut value = None;
        let mut model = None;
        // Up to two positional tokens: value and/or model name. A lone identifier is either a
        // model or (LTspice style) a parameter reference; elaboration decides.
        for _ in 0..2 {
            if cur.at_kv() {
                break;
            }
            match cur.peek() {
                Some(t) if t.kind == TK::Expr || (t.kind == TK::Word && looks_numeric(&t.text)) => {
                    if value.is_some() {
                        let t = t.clone();
                        return Err(cur.err_at(&t, format!("unexpected `{}`", t.text)));
                    }
                    value = Some(cur.value("value")?);
                }
                Some(t) if t.kind == TK::Word => {
                    let t = t.clone();
                    if model.is_some() {
                        return Err(cur.err_at(&t, format!("unexpected `{}`", t.text)));
                    }
                    cur.next();
                    model = Some(t.lower());
                }
                _ => break,
            }
        }
        let (mut params, _flags) = self.params(cur, &[])?;
        params.retain(|(k, v)| {
            if matches!(
                k.as_str(),
                "r" | "c" | "l" | "value" | "resistance" | "capacitance" | "inductance"
            ) {
                value = Some(v.clone());
                false
            } else {
                true
            }
        });
        if value.is_none() && model.is_none() {
            return Err(ParseError::new(
                cur.line.span(),
                format!("`{name}` needs a value"),
            ));
        }
        let kind = match c0 {
            'r' => ElemKind::Resistor { value, model },
            'c' => ElemKind::Capacitor { value, model },
            _ => ElemKind::Inductor { value, model },
        };
        Ok(ElementCard {
            name,
            nodes: vec![a, b],
            kind,
            params,
            span: Span::none(),
        })
    }

    fn coupling(&mut self, cur: &mut Cur, name: String) -> Result<ElementCard, ParseError> {
        let mut inds = Vec::new();
        while let Some(t) = cur.peek() {
            if t.kind == TK::Word && !looks_numeric(&t.text) && !cur.at_kv() {
                inds.push(cur.next().unwrap().lower());
            } else {
                break;
            }
        }
        if inds.len() < 2 {
            return Err(cur.err("K needs at least two inductor names"));
        }
        let k = if cur.at_kv() {
            let (_, _, v) = cur.kv()?;
            v.into_iter().next().unwrap()
        } else {
            cur.value("coupling coefficient")?
        };
        Ok(ElementCard {
            name,
            nodes: vec![],
            kind: ElemKind::Coupling { inductors: inds, k },
            params: vec![],
            span: Span::none(),
        })
    }

    /// Reads waveform arguments: `( a b c )` or bare values until a keyword.
    fn wave_args(&mut self, cur: &mut Cur) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        let paren = matches!(cur.peek(), Some(t) if t.kind == TK::LParen);
        if paren {
            cur.next();
        }
        loop {
            cur.skip_commas();
            match cur.peek() {
                None => {
                    if paren {
                        return Err(cur.err("missing `)` after waveform arguments"));
                    }
                    break;
                }
                Some(t) if t.kind == TK::RParen => {
                    if paren {
                        cur.next();
                    }
                    break;
                }
                Some(t) if !paren && t.kind == TK::Word && !looks_numeric(&t.text) => break,
                Some(t)
                    if paren
                        && t.kind == TK::Word
                        && !looks_numeric(&t.text)
                        && matches!(cur.peek_at(1), Some(n) if n.kind == TK::Eq) =>
                {
                    break;
                }
                Some(_) => args.push(cur.value("waveform value")?),
            }
        }
        Ok(args)
    }

    fn source(&mut self, cur: &mut Cur, name: String, c0: char) -> Result<ElementCard, ParseError> {
        let a = cur.node("node")?;
        let b = cur.node("node")?;
        let mut spec = SourceSpec::default();
        let mut params = Vec::new();
        while let Some(t) = cur.peek().cloned() {
            if t.kind == TK::Comma {
                cur.next();
                continue;
            }
            let kw = t.lower();
            if t.kind == TK::Word && (kw == "dc") && !cur.at_kv() {
                cur.next();
                spec.dc = Some(cur.value("DC value")?);
            } else if cur.at_kv() && kw == "dc" {
                cur.next();
                cur.next();
                spec.dc = Some(cur.value("DC value")?);
            } else if t.kind == TK::Word && kw == "ac" && !cur.at_kv() {
                cur.next();
                spec.ac_mag = Some(Expr::Num(1.0));
                if let Some(n) = cur.peek() {
                    if Cur::is_value_tok(n) {
                        spec.ac_mag = Some(cur.value("AC magnitude")?);
                        if let Some(n) = cur.peek() {
                            if Cur::is_value_tok(n) {
                                spec.ac_phase = Some(cur.value("AC phase")?);
                            }
                        }
                    }
                }
            } else if t.kind == TK::Word
                && matches!(
                    kw.as_str(),
                    "pulse" | "sin" | "sine" | "exp" | "pwl" | "sffm" | "am"
                )
            {
                cur.next();
                if kw == "pwl" {
                    if let Some(f) = cur.peek() {
                        if f.is_word("file") {
                            return Err(cur.err("PWL file= is not supported; inline the points"));
                        }
                    }
                }
                let args = self.wave_args(cur)?;
                let need = |n: usize, what: &str| -> Result<(), ParseError> {
                    if args.len() < n {
                        Err(ParseError::new(
                            cur.span_of(&t),
                            format!("{what} needs at least {n} values"),
                        ))
                    } else {
                        Ok(())
                    }
                };
                spec.wave = Some(match kw.as_str() {
                    "pulse" => {
                        need(2, "PULSE")?;
                        Waveform::Pulse(args)
                    }
                    "sin" | "sine" => {
                        need(2, "SIN")?;
                        Waveform::Sin(args)
                    }
                    "exp" => {
                        need(2, "EXP")?;
                        Waveform::Exp(args)
                    }
                    "sffm" => {
                        need(2, "SFFM")?;
                        Waveform::Sffm(args)
                    }
                    "am" => {
                        need(2, "AM")?;
                        Waveform::Am(args)
                    }
                    _ => {
                        if args.len() < 2 || args.len() % 2 != 0 {
                            return Err(ParseError::new(
                                cur.span_of(&t),
                                "PWL needs time/value pairs",
                            ));
                        }
                        let mut repeat = None;
                        let mut delay = None;
                        while cur.at_kv() {
                            let k = cur.peek().unwrap().lower();
                            if k == "r" || k == "td" {
                                let (k, _, v) = cur.kv()?;
                                let v = v.into_iter().next().unwrap();
                                if k == "r" {
                                    repeat = Some(v);
                                } else {
                                    delay = Some(v);
                                }
                            } else {
                                break;
                            }
                        }
                        Waveform::Pwl {
                            points: args,
                            repeat,
                            delay,
                        }
                    }
                });
            } else if t.kind == TK::Word && kw.starts_with("distof") {
                cur.next();
                let _ = self.wave_args(cur)?;
            } else if cur.at_kv() {
                let (k, _, v) = cur.kv()?;
                params.push((k, v.into_iter().next().unwrap()));
            } else if Cur::is_value_tok(&t) || (t.kind == TK::Word && spec.dc.is_none()) {
                spec.dc = Some(cur.value("source value")?);
            } else {
                return Err(cur.err_at(&t, format!("unexpected `{}` in source", t.text)));
            }
        }
        let kind = if c0 == 'v' {
            ElemKind::Vsource(spec)
        } else {
            ElemKind::Isource(spec)
        };
        Ok(ElementCard {
            name,
            nodes: vec![a, b],
            kind,
            params,
            span: Span::none(),
        })
    }

    /// `POLY(n)` → dims; returns None if the next token is not POLY.
    fn poly_dims(&mut self, cur: &mut Cur) -> Result<Option<usize>, ParseError> {
        if !matches!(cur.peek(), Some(t) if t.is_word("poly")) {
            return Ok(None);
        }
        let t = cur.next().unwrap();
        if !matches!(cur.next(), Some(p) if p.kind == TK::LParen) {
            return Err(cur.err_at(&t, "expected `POLY(n)`"));
        }
        let n = cur.word("polynomial dimension")?;
        let dims: usize = n
            .text
            .parse()
            .map_err(|_| cur.err_at(&n, "POLY dimension must be an integer"))?;
        if !matches!(cur.next(), Some(p) if p.kind == TK::RParen) {
            return Err(cur.err_at(&t, "expected `)` after POLY dimension"));
        }
        if dims == 0 {
            return Err(cur.err_at(&t, "POLY dimension must be at least 1"));
        }
        Ok(Some(dims))
    }

    fn behavioral_value(&mut self, cur: &mut Cur) -> Result<Option<Control>, ParseError> {
        // VALUE = {expr} | VALUE = expr… | vol=/cur= | TABLE {expr} = (x,y)…
        let Some(t) = cur.peek().cloned() else {
            return Ok(None);
        };
        let kw = t.lower();
        if matches!(kw.as_str(), "value" | "vol" | "cur") {
            if !matches!(cur.peek_at(1), Some(e) if e.kind == TK::Eq) {
                return Err(cur.err_at(
                    &t,
                    format!("expected `{}=` followed by an expression", t.text),
                ));
            }
            let eq = cur.peek_at(1).unwrap().start;
            cur.i = cur.toks.len();
            let rest = &cur.line.text[eq + 1..];
            let expr = rest_expr(cur.line, eq + 1, rest)?;
            return Ok(Some(Control::Value(expr)));
        }
        if kw == "table" {
            cur.next();
            let input = cur.value("table input expression")?;
            if matches!(cur.peek(), Some(e) if e.kind == TK::Eq) {
                cur.next();
            }
            let mut pts = Vec::new();
            while !cur.done() {
                cur.skip_commas();
                let paren = matches!(cur.peek(), Some(p) if p.kind == TK::LParen);
                if paren {
                    cur.next();
                }
                let x = cur.value("table x")?;
                let y = cur.value("table y")?;
                if paren {
                    match cur.next() {
                        Some(p) if p.kind == TK::RParen => {}
                        _ => return Err(cur.err("expected `)` in TABLE point")),
                    }
                }
                pts.push((x, y));
            }
            if pts.is_empty() {
                return Err(cur.err_at(&t, "TABLE needs (x, y) points"));
            }
            return Ok(Some(Control::Table { input, points: pts }));
        }
        Ok(None)
    }

    fn vcxs(&mut self, cur: &mut Cur, name: String, c0: char) -> Result<ElementCard, ParseError> {
        let a = cur.node("node")?;
        let b = cur.node("node")?;
        let ctrl = if let Some(c) = self.behavioral_value(cur)? {
            c
        } else if let Some(dims) = self.poly_dims(cur)? {
            let mut nodes = Vec::new();
            for _ in 0..dims * 2 {
                nodes.push(cur.node("controlling node")?);
            }
            let mut coeffs = Vec::new();
            while !cur.done() && !cur.at_kv() {
                coeffs.push(cur.value("coefficient")?);
                cur.skip_commas();
            }
            Control::Poly {
                dims,
                ctrl_nodes: nodes,
                ctrl_srcs: vec![],
                coeffs,
            }
        } else {
            let cp = cur.node("controlling node")?;
            let cn = cur.node("controlling node")?;
            let gain = if let Some(c) = self.behavioral_value(cur)? {
                // ngspice allows `E1 o 0 a b value={..}`? treat as behavioural
                return Ok(ElementCard {
                    name,
                    nodes: vec![a, b],
                    kind: if c0 == 'e' {
                        ElemKind::Vcvs(c)
                    } else {
                        ElemKind::Vccs(c)
                    },
                    params: vec![],
                    span: Span::none(),
                });
            } else if cur.at_kv() {
                let (_, _, v) = cur.kv()?;
                v.into_iter().next().unwrap()
            } else {
                cur.value("gain")?
            };
            Control::Linear {
                ctrl_nodes: vec![cp, cn],
                ctrl_src: None,
                gain,
            }
        };
        let (params, _) = self.params(cur, &[])?;
        let kind = if c0 == 'e' {
            ElemKind::Vcvs(ctrl)
        } else {
            ElemKind::Vccs(ctrl)
        };
        Ok(ElementCard {
            name,
            nodes: vec![a, b],
            kind,
            params,
            span: Span::none(),
        })
    }

    fn ccxs(&mut self, cur: &mut Cur, name: String, c0: char) -> Result<ElementCard, ParseError> {
        let a = cur.node("node")?;
        let b = cur.node("node")?;
        let ctrl = if let Some(dims) = self.poly_dims(cur)? {
            let mut srcs = Vec::new();
            for _ in 0..dims {
                srcs.push(cur.word("controlling source")?.lower());
            }
            let mut coeffs = Vec::new();
            while !cur.done() && !cur.at_kv() {
                coeffs.push(cur.value("coefficient")?);
                cur.skip_commas();
            }
            Control::Poly {
                dims,
                ctrl_nodes: vec![],
                ctrl_srcs: srcs,
                coeffs,
            }
        } else {
            let src = cur.word("controlling voltage source")?.lower();
            let gain = cur.value("gain")?;
            Control::Linear {
                ctrl_nodes: vec![],
                ctrl_src: Some(src),
                gain,
            }
        };
        let (params, _) = self.params(cur, &[])?;
        let kind = if c0 == 'f' {
            ElemKind::Cccs(ctrl)
        } else {
            ElemKind::Ccvs(ctrl)
        };
        Ok(ElementCard {
            name,
            nodes: vec![a, b],
            kind,
            params,
            span: Span::none(),
        })
    }

    fn bsource(&mut self, cur: &mut Cur, name: String) -> Result<ElementCard, ParseError> {
        let a = cur.node("node")?;
        let b = cur.node("node")?;
        let Some(t) = cur.peek().cloned() else {
            return Err(cur.err("B source needs `V=expr` or `I=expr`"));
        };
        let kw = t.lower();
        if !(matches!(kw.as_str(), "v" | "i")
            && matches!(cur.peek_at(1), Some(e) if e.kind == TK::Eq))
        {
            return Err(cur.err_at(&t, "B source needs `V=expr` or `I=expr`"));
        }
        let eq = cur.peek_at(1).unwrap().start;
        let rest = &cur.line.text[eq + 1..];
        let expr = rest_expr(cur.line, eq + 1, rest)?;
        Ok(ElementCard {
            name,
            nodes: vec![a, b],
            kind: ElemKind::Behavioral {
                is_current: kw == "i",
                expr,
            },
            params: vec![],
            span: Span::none(),
        })
    }

    fn switch(&mut self, cur: &mut Cur, name: String, c0: char) -> Result<ElementCard, ParseError> {
        let a = cur.node("node")?;
        let b = cur.node("node")?;
        let (nodes, ctrl) = if c0 == 's' {
            let cp = cur.node("controlling node")?;
            let cn = cur.node("controlling node")?;
            (vec![a, b, cp, cn], None)
        } else {
            (
                vec![a, b],
                Some(cur.word("controlling voltage source")?.lower()),
            )
        };
        let model = cur.word("switch model")?.lower();
        let (params, flags) = self.params(cur, &["on", "off"])?;
        let init_on = flags.last().map(|f| f == "on");
        let kind = match ctrl {
            None => ElemKind::Switch { model, init_on },
            Some(c) => ElemKind::CSwitch {
                ctrl: c,
                model,
                init_on,
            },
        };
        Ok(ElementCard {
            name,
            nodes,
            kind,
            params,
            span: Span::none(),
        })
    }

    fn semis(
        &mut self,
        cur: &mut Cur,
        name: String,
        nn: usize,
        c0: char,
    ) -> Result<ElementCard, ParseError> {
        let mut nodes = Vec::new();
        for _ in 0..nn {
            nodes.push(cur.node("node")?);
        }
        let model = cur.word("model name")?.lower();
        let mut area = None;
        if let Some(t) = cur.peek() {
            if Cur::is_value_tok(t) && c0 != 'm' {
                area = Some(cur.value("area")?);
            }
        }
        let (params, flags) = self.params(cur, &["off"])?;
        let off = !flags.is_empty();
        let kind = match c0 {
            'd' => ElemKind::Diode { model, area, off },
            'j' => ElemKind::Jfet { model, area, off },
            _ => ElemKind::Mosfet { model, off },
        };
        Ok(ElementCard {
            name,
            nodes,
            kind,
            params,
            span: Span::none(),
        })
    }

    fn bjt(&mut self, cur: &mut Cur, name: String) -> Result<ElementCard, ParseError> {
        let mut nodes = vec![
            cur.node("collector")?,
            cur.node("base")?,
            cur.node("emitter")?,
        ];
        // optional substrate node: `Q c b e s model` vs `Q c b e model [area]`
        let t4 = cur.word("model name")?;
        let t5_is_model = matches!(cur.peek(), Some(t) if t.kind == TK::Word && !looks_numeric(&t.text) && !t.is_word("off"))
            && !cur.at_kv();
        let model = if t5_is_model {
            nodes.push(normalize_node(&t4.text));
            cur.word("model name")?.lower()
        } else {
            t4.lower()
        };
        let mut area = None;
        if let Some(t) = cur.peek() {
            if Cur::is_value_tok(t) {
                area = Some(cur.value("area")?);
            }
        }
        let (params, flags) = self.params(cur, &["off"])?;
        Ok(ElementCard {
            name,
            nodes,
            kind: ElemKind::Bjt {
                model,
                area,
                off: !flags.is_empty(),
            },
            params,
            span: Span::none(),
        })
    }

    fn tline(&mut self, cur: &mut Cur, name: String) -> Result<ElementCard, ParseError> {
        let mut nodes = Vec::new();
        for _ in 0..4 {
            nodes.push(cur.node("node")?);
        }
        let (params, _) = self.params(cur, &[])?;
        if !params.iter().any(|(k, _)| k == "td" || k == "f") {
            return Err(ParseError::new(
                cur.line.span(),
                format!("`{name}` needs TD= or F= (and optionally NL=)"),
            ));
        }
        Ok(ElementCard {
            name,
            nodes,
            kind: ElemKind::Tline,
            params,
            span: Span::none(),
        })
    }

    fn subckt_inst(&mut self, cur: &mut Cur, name: String) -> Result<ElementCard, ParseError> {
        // words until `params:` or `k=v`; the last word is the subckt name.
        let mut words = Vec::new();
        while let Some(t) = cur.peek() {
            if t.kind != TK::Word || cur.at_kv() {
                break;
            }
            if t.text.eq_ignore_ascii_case("params:") {
                break;
            }
            words.push(cur.next().unwrap());
        }
        if matches!(cur.peek(), Some(t) if t.text.eq_ignore_ascii_case("params:")) {
            cur.next();
        }
        let Some(sub) = words.pop() else {
            return Err(cur.err(format!("`{name}` needs a subcircuit name")));
        };
        let nodes = words.iter().map(|w| normalize_node(&w.text)).collect();
        let (params, _) = self.params(cur, &[])?;
        Ok(ElementCard {
            name,
            nodes,
            kind: ElemKind::Subckt { name: sub.lower() },
            params,
            span: Span::none(),
        })
    }

    // -----------------------------------------------------------------------------------------
    // Dot cards
    // -----------------------------------------------------------------------------------------

    fn dot_card(&mut self, line: &Line) -> Result<(), ParseError> {
        let kw = line
            .text
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match kw.as_str() {
            ".param" | ".params" => self.dot_param(line),
            ".func" => self.dot_func(line),
            ".model" => self.dot_model(line),
            ".subckt" => self.dot_subckt(line),
            ".ends" => {
                if self.stack.len() <= 1 {
                    return Err(ParseError::new(line.span(), "`.ends` without `.subckt`"));
                }
                let s = self.stack.pop().unwrap();
                self.scope().subckts.push(Arc::new(s));
                Ok(())
            }
            ".include" | ".inc" => self.dot_include(line, None),
            ".lib" => {
                let parts: Vec<&str> = line.text.split_whitespace().collect();
                if parts.len() >= 3 {
                    self.dot_include(line, Some(parts[2].to_ascii_lowercase()))
                } else {
                    // `.lib section` outside include: ignore marker
                    Ok(())
                }
            }
            ".global" => {
                for n in line.text.split_whitespace().skip(1) {
                    self.nl.globals.push(normalize_node(n));
                }
                Ok(())
            }
            ".title" => {
                self.nl.title = line.text[6..].trim().to_string();
                Ok(())
            }
            ".end" | ".endl" | ".width" | ".backanno" | ".print" | ".plot" | ".four" | ".disto"
            | ".pz" | ".sens" | ".csparam" => {
                if matches!(kw.as_str(), ".four" | ".disto" | ".pz" | ".sens") {
                    self.warn(
                        line.span(),
                        format!("`{kw}` is not supported and was ignored"),
                    );
                }
                Ok(())
            }
            ".save" | ".probe" => {
                for t in line.text.split_whitespace().skip(1) {
                    let t = t.to_ascii_lowercase();
                    if t != "all" {
                        self.nl.saves.push(t);
                    }
                }
                Ok(())
            }
            ".options" | ".option" | ".opt" | ".opts" => self.dot_options(line),
            ".temp" | ".ic" | ".nodeset" | ".op" | ".dc" | ".tran" | ".ac" | ".noise" | ".tf"
            | ".step" | ".mc" | ".meas" | ".measure" => {
                if self.stack.len() > 1 {
                    return Err(ParseError::new(
                        line.span(),
                        format!("`{kw}` is not allowed inside `.subckt`"),
                    ));
                }
                self.deferred.push(line.clone());
                Ok(())
            }
            _ => {
                self.warn(line.span(), format!("unknown control card `{kw}` ignored"));
                Ok(())
            }
        }
    }

    fn dot_param(&mut self, line: &Line) -> Result<(), ParseError> {
        let text = &line.text;
        let mut pos = text.find(char::is_whitespace).unwrap_or(text.len());
        loop {
            // skip whitespace and commas
            while pos < text.len()
                && (text.as_bytes()[pos].is_ascii_whitespace() || text.as_bytes()[pos] == b',')
            {
                pos += 1;
            }
            if pos >= text.len() {
                break;
            }
            let start = pos;
            while pos < text.len() && {
                let c = text.as_bytes()[pos];
                c.is_ascii_alphanumeric() || c == b'_' || c == b'.'
            } {
                pos += 1;
            }
            if pos == start {
                return Err(ParseError::new(
                    line.span_at(start),
                    "expected parameter name",
                ));
            }
            let name = text[start..pos].to_ascii_lowercase();
            // .param f(x) = … (function definition)
            if text[pos..].starts_with('(') {
                let close = text[pos..]
                    .find(')')
                    .ok_or_else(|| ParseError::new(line.span_at(pos), "missing `)`"))?
                    + pos;
                let args: Vec<String> = text[pos + 1..close]
                    .split(',')
                    .map(|s| s.trim().to_ascii_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect();
                let rest = text[close + 1..].trim_start();
                let Some(body) = rest.strip_prefix('=') else {
                    return Err(ParseError::new(
                        line.span_at(close + 1),
                        "expected `=` in function definition",
                    ));
                };
                let body_off = text.len() - body.len();
                let body_expr = rest_expr(line, body_off, body)?;
                self.scope().funcs.push(FuncDef {
                    name,
                    args,
                    body: body_expr,
                    span: line.span_at(start),
                });
                return Ok(());
            }
            while pos < text.len() && text.as_bytes()[pos].is_ascii_whitespace() {
                pos += 1;
            }
            if pos >= text.len() || text.as_bytes()[pos] != b'=' {
                return Err(ParseError::new(
                    line.span_at(pos),
                    format!("expected `=` after parameter `{name}`"),
                ));
            }
            pos += 1;
            while pos < text.len() && text.as_bytes()[pos].is_ascii_whitespace() {
                pos += 1;
            }
            let (expr, len) = value_prefix(line, pos)?;
            pos += len;
            let span = line.span_at(start);
            self.scope().params.push((name, expr, span));
        }
        Ok(())
    }

    fn dot_func(&mut self, line: &Line) -> Result<(), ParseError> {
        let text = &line.text;
        let after = text[5..].trim_start();
        let off = text.len() - after.len();
        let Some(open) = after.find('(') else {
            return Err(ParseError::new(line.span_at(off), "expected `name(args)`"));
        };
        let name = after[..open].trim().to_ascii_lowercase();
        let close = after
            .find(')')
            .ok_or_else(|| ParseError::new(line.span_at(off + open), "missing `)`"))?;
        let args: Vec<String> = after[open + 1..close]
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        let mut body = after[close + 1..].trim_start();
        if let Some(b) = body.strip_prefix('=') {
            body = b.trim_start();
        }
        let boff = text.len() - body.len();
        let body = rest_expr(line, boff, body)?;
        self.scope().funcs.push(FuncDef {
            name,
            args,
            body,
            span: line.span(),
        });
        Ok(())
    }

    fn dot_model(&mut self, line: &Line) -> Result<(), ParseError> {
        let mut cur = Cur::new(line)?;
        cur.next();
        let name = cur.word("model name")?.lower();
        let tt = cur.word("model type")?;
        let kind = ModelKind::from_keyword(&tt.lower()).ok_or_else(|| {
            cur.err_at(&tt, format!("unknown model type `{}` (expected D, NPN, PNP, NMOS, PMOS, NJF, PJF, SW, CSW, R, C or L)", tt.text))
        })?;
        let mut params = Vec::new();
        while !cur.done() {
            match cur.peek() {
                Some(t) if matches!(t.kind, TK::LParen | TK::RParen | TK::Comma) => {
                    cur.next();
                }
                Some(_) if cur.at_kv() => {
                    let (k, _, v) = cur.kv()?;
                    for (i, v) in v.into_iter().enumerate() {
                        params.push((
                            if i == 0 {
                                k.clone()
                            } else {
                                format!("{k}.{i}")
                            },
                            v,
                        ));
                    }
                }
                Some(t) => {
                    // bare flag (rare) — accept as =1
                    let t = t.clone();
                    cur.next();
                    if t.kind == TK::Word && !looks_numeric(&t.text) {
                        params.push((t.lower(), Expr::Num(1.0)));
                    } else {
                        return Err(cur.err_at(&t, format!("unexpected `{}` in .model", t.text)));
                    }
                }
                None => break,
            }
        }
        self.scope().models.push(ModelCard {
            name,
            kind,
            params,
            span: line.span(),
        });
        Ok(())
    }

    fn dot_subckt(&mut self, line: &Line) -> Result<(), ParseError> {
        let mut cur = Cur::new(line)?;
        cur.next();
        let name = cur.word("subcircuit name")?.lower();
        let mut ports = Vec::new();
        while let Some(t) = cur.peek() {
            if t.kind != TK::Word || cur.at_kv() || t.text.eq_ignore_ascii_case("params:") {
                break;
            }
            ports.push(normalize_node(&cur.next().unwrap().text));
        }
        if matches!(cur.peek(), Some(t) if t.text.eq_ignore_ascii_case("params:")) {
            cur.next();
        }
        let (defaults, _) = self.params(&mut cur, &[])?;
        if self.depth > 64 {
            return Err(ParseError::new(line.span(), "subcircuit nesting too deep"));
        }
        self.stack.push(Scope {
            name,
            ports,
            defaults,
            span: line.span(),
            ..Scope::default()
        });
        Ok(())
    }

    fn dot_include(&mut self, line: &Line, section: Option<String>) -> Result<(), ParseError> {
        let parts: Vec<&str> = line.text.split_whitespace().collect();
        let Some(file) = parts.get(1) else {
            return Err(ParseError::new(line.span(), "`.include` needs a file name"));
        };
        let file = file.trim_matches(['"', '\'']);
        if self.depth >= 16 {
            return Err(ParseError::new(
                line.span(),
                "`.include` nested too deeply (recursive include?)",
            ));
        }
        let base = self
            .opts
            .base_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));
        let path = if Path::new(file).is_absolute() {
            PathBuf::from(file)
        } else {
            base.join(file)
        };
        let text = match &self.opts.loader {
            Some(l) => l(&path),
            None => std::fs::read_to_string(&path),
        }
        .map_err(|e| {
            ParseError::new(
                line.span(),
                format!("cannot read `{}`: {e}", path.display()),
            )
        })?;
        let fname: Arc<str> = Arc::from(
            path.file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| file.to_string()),
        );
        let (_, lines) = logical_lines(&text, Some(fname), false);
        let lines = if let Some(sec) = section {
            let mut out = Vec::new();
            let mut on = false;
            let mut found = false;
            for l in lines {
                let mut w = l.text.split_whitespace();
                let first = w.next().unwrap_or("").to_ascii_lowercase();
                if first == ".lib" {
                    let s = w.next().unwrap_or("").to_ascii_lowercase();
                    if w.next().is_none() {
                        on = s == sec;
                        found |= on;
                        continue;
                    }
                }
                if first == ".endl" {
                    on = false;
                    continue;
                }
                if on {
                    out.push(l);
                }
            }
            if !found {
                return Err(ParseError::new(
                    line.span(),
                    format!("section `{sec}` not found in `{}`", path.display()),
                ));
            }
            out
        } else {
            lines
        };
        self.depth += 1;
        let r = self.lines(&lines);
        self.depth -= 1;
        r
    }

    fn dot_options(&mut self, line: &Line) -> Result<(), ParseError> {
        let mut cur = Cur::new(line)?;
        cur.next();
        while !cur.done() {
            cur.skip_commas();
            if cur.done() {
                break;
            }
            if cur.at_kv() {
                let (k, ktok, v) = cur.kv()?;
                let e = v.into_iter().next().unwrap();
                // method=gear etc.: identifier values
                let val = match &e {
                    Expr::Ident(id) => {
                        self.nl
                            .options
                            .push((format!("{k}={id}"), None, cur.span_of(&ktok)));
                        None
                    }
                    _ => Some(self.eval_top(&e, &cur.span_of(&ktok))?),
                };
                self.nl.options.push((k, val, cur.span_of(&ktok)));
            } else {
                let t = cur.next().unwrap();
                self.nl.options.push((t.lower(), None, cur.span_of(&t)));
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------------------------
    // Deferred cards (need top-level parameter values)
    // -----------------------------------------------------------------------------------------

    fn eval_top(&self, e: &Expr, span: &Span) -> Result<f64, ParseError> {
        let params = top_param_values(&self.nl.top, &self.nl);
        e.eval_with(&|n| params.get(n).copied())
            .map_err(|m| ParseError::new(span.clone(), m))
    }

    fn num(&self, cur: &mut Cur, what: &str) -> Result<f64, ParseError> {
        let span = cur.span();
        let e = cur.value(what)?;
        self.eval_top(&e, &span)
    }

    fn control_card(&mut self, line: &Line) -> Result<(), ParseError> {
        let kw = line
            .text
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut cur = Cur::new(line)?;
        cur.next();
        let span = line.span();
        match kw.as_str() {
            ".temp" => {
                while !cur.done() {
                    cur.skip_commas();
                    if cur.done() {
                        break;
                    }
                    let v = self.num(&mut cur, "temperature")?;
                    self.nl.temps.push(v);
                }
            }
            ".ic" | ".nodeset" => {
                while !cur.done() {
                    cur.skip_commas();
                    if cur.done() {
                        break;
                    }
                    let v = cur.word("v(node)=value")?;
                    if !v.is_word("v") || !matches!(cur.next(), Some(t) if t.kind == TK::LParen) {
                        return Err(cur.err_at(&v, format!("expected `v(node)=value` in `{kw}`")));
                    }
                    let node = cur.node("node")?;
                    if !matches!(cur.next(), Some(t) if t.kind == TK::RParen)
                        || !matches!(cur.next(), Some(t) if t.kind == TK::Eq)
                    {
                        return Err(cur.err_at(&v, format!("expected `v(node)=value` in `{kw}`")));
                    }
                    let val = cur.value("initial value")?;
                    if kw == ".ic" {
                        self.nl.ics.push((node, val, cur.span_of(&v)));
                    } else {
                        self.nl.nodesets.push((node, val, cur.span_of(&v)));
                    }
                }
            }
            ".op" => self.nl.analyses.push((Analysis::Op, span)),
            ".tran" => {
                let mut vals = Vec::new();
                let mut uic = false;
                while !cur.done() {
                    if matches!(cur.peek(), Some(t) if t.is_word("uic")) {
                        cur.next();
                        uic = true;
                        continue;
                    }
                    vals.push(self.num(&mut cur, "time")?);
                }
                if vals.len() < 2 {
                    return Err(ParseError::new(span, "`.tran` needs tstep and tstop"));
                }
                let spec = TranSpec {
                    tstep: vals[0],
                    tstop: vals[1],
                    tstart: vals.get(2).copied().unwrap_or(0.0),
                    tmax: vals.get(3).copied(),
                    uic,
                };
                if spec.tstop <= 0.0 || spec.tstart < 0.0 || spec.tstart >= spec.tstop {
                    return Err(ParseError::new(span, "`.tran`: need 0 ≤ tstart < tstop"));
                }
                self.nl.analyses.push((Analysis::Tran(spec), span));
            }
            ".ac" => {
                let sweep = self.freq_sweep(&mut cur, &span)?;
                self.nl.analyses.push((Analysis::Ac(sweep), span));
            }
            ".dc" => {
                let mut sweeps = Vec::new();
                while !cur.done() {
                    sweeps.push(self.dc_sweep(&mut cur)?);
                }
                if sweeps.is_empty() || sweeps.len() > 2 {
                    return Err(ParseError::new(span, "`.dc` takes one or two sweeps"));
                }
                self.nl.analyses.push((Analysis::Dc(sweeps), span));
            }
            ".noise" => {
                let output = self.output_var(&mut cur)?;
                let input = cur.word("input source")?.lower();
                let sweep = self.freq_sweep(&mut cur, &span)?;
                let pts = if !cur.done() {
                    self.num(&mut cur, "points per summary")? as usize
                } else {
                    0
                };
                self.nl.analyses.push((
                    Analysis::Noise {
                        output,
                        input,
                        sweep,
                        pts_per_summary: pts,
                    },
                    span,
                ));
            }
            ".tf" => {
                let output = self.output_var(&mut cur)?;
                let input = cur.word("input source")?.lower();
                self.nl
                    .analyses
                    .push((Analysis::Tf { output, input }, span));
            }
            ".step" => {
                let s = self.step(&mut cur, &span)?;
                self.nl.steps.push(s);
            }
            ".mc" => {
                let runs = self.num(&mut cur, "number of runs")?;
                if runs < 1.0 {
                    return Err(ParseError::new(span, "`.mc` needs at least one run"));
                }
                let mut mc = McSpec {
                    runs: runs as usize,
                    tol: None,
                    seed: 1,
                    dist: Dist::Gauss,
                    analyses: vec![],
                    span: span.clone(),
                };
                while !cur.done() {
                    if !cur.at_kv() {
                        let t = cur.next().unwrap();
                        return Err(cur.err_at(&t, format!("unexpected `{}` in .mc", t.text)));
                    }
                    let k = cur.next().unwrap();
                    cur.next();
                    match k.lower().as_str() {
                        "tol" => mc.tol = Some(self.num(&mut cur, "tolerance")?),
                        "seed" => mc.seed = self.num(&mut cur, "seed")? as u64,
                        "dist" => {
                            let d = cur.word("distribution")?;
                            mc.dist = match d.lower().as_str() {
                                "gauss" | "gaussian" | "normal" => Dist::Gauss,
                                "uniform" | "unif" | "flat" => Dist::Uniform,
                                _ => return Err(cur.err_at(&d, "dist must be gauss or uniform")),
                            };
                        }
                        "analysis" | "an" => {
                            let a = cur.word("analysis")?;
                            mc.analyses.push(a.lower());
                        }
                        other => {
                            return Err(cur.err_at(&k, format!("unknown .mc option `{other}`")));
                        }
                    }
                }
                self.nl.mc = Some(mc);
            }
            ".meas" | ".measure" => {
                let m = self.meas(line)?;
                self.nl.measures.push(m);
            }
            _ => {}
        }
        Ok(())
    }

    fn output_var(&mut self, cur: &mut Cur) -> Result<OutputVar, ParseError> {
        let t = cur.word("output variable")?;
        let kind = t.lower();
        if !matches!(cur.next(), Some(p) if p.kind == TK::LParen) {
            return Err(cur.err_at(&t, "expected `v(node)` or `i(source)`"));
        }
        let a = cur.word("name")?;
        let mut b = None;
        if matches!(cur.peek(), Some(p) if p.kind == TK::Comma) {
            cur.next();
            b = Some(cur.node("reference node")?);
        }
        if !matches!(cur.next(), Some(p) if p.kind == TK::RParen) {
            return Err(cur.err_at(&t, "missing `)`"));
        }
        match kind.as_str() {
            "v" => Ok(OutputVar::Voltage(
                normalize_node(&a.text),
                b.filter(|n| n != "0"),
            )),
            "i" => Ok(OutputVar::Current(a.lower())),
            _ => Err(cur.err_at(&t, "expected `v(node)` or `i(source)`")),
        }
    }

    fn freq_sweep(&mut self, cur: &mut Cur, span: &Span) -> Result<Sweep, ParseError> {
        let t = cur.word("dec, oct, lin or list")?;
        let kind = t.lower();
        if kind == "list" {
            let mut vals = Vec::new();
            while !cur.done() && cur.peek().is_some_and(Cur::is_value_tok) {
                vals.push(self.num(cur, "frequency")?);
            }
            return Ok(Sweep {
                var: SweepVar::Name("frequency".into()),
                start: 0.0,
                stop: 0.0,
                kind: SweepKind::List(vals),
            });
        }
        let n = self.num(cur, "number of points")?;
        let f1 = self.num(cur, "start frequency")?;
        let f2 = self.num(cur, "stop frequency")?;
        if f1 <= 0.0 && kind != "lin" {
            return Err(ParseError::new(span.clone(), "start frequency must be > 0"));
        }
        let kind = match kind.as_str() {
            "dec" => SweepKind::Dec(n as usize),
            "oct" => SweepKind::Oct(n as usize),
            "lin" => SweepKind::LinPoints(n as usize),
            _ => {
                return Err(cur.err_at(
                    &t,
                    format!("expected dec, oct, lin or list, found `{}`", t.text),
                ));
            }
        };
        Ok(Sweep {
            var: SweepVar::Name("frequency".into()),
            start: f1,
            stop: f2,
            kind,
        })
    }

    /// One `.dc` sweep: `name start stop incr`, `[lin|dec|oct] name start stop n`,
    /// `name start stop dec n`, `name list a b c`, `param name …`, `temp …`.
    fn dc_sweep(&mut self, cur: &mut Cur) -> Result<Sweep, ParseError> {
        let mut pre_kind: Option<String> = None;
        let mut t = cur.word("sweep variable")?;
        if matches!(t.lower().as_str(), "lin" | "dec" | "oct") {
            pre_kind = Some(t.lower());
            t = cur.word("sweep variable")?;
        }
        let mut var = match t.lower().as_str() {
            "temp" => SweepVar::Temp,
            "param" => SweepVar::Param(cur.word("parameter name")?.lower()),
            n => SweepVar::Name(n.to_string()),
        };
        if let SweepVar::Name(n) = &var {
            if n == "temper" {
                var = SweepVar::Temp;
            }
        }
        if matches!(cur.peek(), Some(p) if p.is_word("list")) {
            cur.next();
            let mut vals = Vec::new();
            while cur.peek().is_some_and(Cur::is_value_tok) {
                vals.push(self.num(cur, "value")?);
            }
            return Ok(Sweep {
                var,
                start: 0.0,
                stop: 0.0,
                kind: SweepKind::List(vals),
            });
        }
        let start = self.num(cur, "start value")?;
        let stop = self.num(cur, "stop value")?;
        let mut post_kind = None;
        if let Some(p) = cur.peek() {
            if matches!(p.lower().as_str(), "lin" | "dec" | "oct") && p.kind == TK::Word {
                post_kind = Some(cur.next().unwrap().lower());
            }
        }
        let inc = self.num(cur, "increment")?;
        let kind = match pre_kind.or(post_kind).as_deref() {
            Some("dec") => SweepKind::Dec(inc as usize),
            Some("oct") => SweepKind::Oct(inc as usize),
            Some("lin") if pre_kind_is_points(&inc) => SweepKind::LinPoints(inc as usize),
            _ => SweepKind::Lin { step: inc },
        };
        Ok(Sweep {
            var,
            start,
            stop,
            kind,
        })
    }

    fn step(&mut self, cur: &mut Cur, span: &Span) -> Result<StepSpec, ParseError> {
        // LTspice: .step [lin|dec|oct] param x start stop inc | .step param x list … |
        //          .step temp list … | .step V1 1 5 1
        let mut kind_kw: Option<String> = None;
        let mut t = cur.word("step variable")?;
        if matches!(t.lower().as_str(), "lin" | "dec" | "oct") {
            kind_kw = Some(t.lower());
            t = cur.word("step variable")?;
        }
        let var = match t.lower().as_str() {
            "param" => SweepVar::Param(cur.word("parameter name")?.lower()),
            "temp" | "temper" => SweepVar::Temp,
            n => SweepVar::Name(n.to_string()),
        };
        let values = if matches!(cur.peek(), Some(p) if p.is_word("list")) {
            cur.next();
            let mut vals = Vec::new();
            while !cur.done() {
                cur.skip_commas();
                if cur.done() {
                    break;
                }
                vals.push(self.num(cur, "value")?);
            }
            vals
        } else {
            let start = self.num(cur, "start")?;
            let stop = self.num(cur, "stop")?;
            if let Some(p) = cur.peek() {
                if p.kind == TK::Word && matches!(p.lower().as_str(), "lin" | "dec" | "oct") {
                    kind_kw = Some(cur.next().unwrap().lower());
                }
            }
            let inc = self.num(cur, "increment")?;
            let kind = match kind_kw.as_deref() {
                Some("dec") => SweepKind::Dec(inc as usize),
                Some("oct") => SweepKind::Oct(inc as usize),
                _ => SweepKind::Lin { step: inc },
            };
            sweep_values(start, stop, &kind)
        };
        if values.is_empty() {
            return Err(ParseError::new(span.clone(), "`.step` produces no values"));
        }
        Ok(StepSpec {
            var,
            values,
            span: span.clone(),
        })
    }

    fn meas(&mut self, line: &Line) -> Result<MeasSpec, ParseError> {
        let text = line.text.as_str();
        let mut m = MeasParser { line, text, pos: 0 };
        m.word(); // .meas
        let an = m.word().to_ascii_lowercase();
        if !matches!(an.as_str(), "tran" | "ac" | "dc" | "op" | "noise") {
            return Err(ParseError::new(
                line.span(),
                format!("`.meas` analysis must be tran, ac or dc, found `{an}`"),
            ));
        }
        let name = m.word().to_ascii_lowercase();
        if name.is_empty() {
            return Err(ParseError::new(line.span(), "`.meas` needs a result name"));
        }
        let kw_pos = m.pos;
        let kw = m.word().to_ascii_lowercase();
        let span = line.span();
        let kind = match kw.as_str() {
            "avg" | "rms" | "pp" | "min" | "max" | "min_at" | "max_at" | "integ" | "integral" => {
                let func = match kw.as_str() {
                    "avg" => StatFunc::Avg,
                    "rms" => StatFunc::Rms,
                    "pp" => StatFunc::Pp,
                    "min" => StatFunc::Min,
                    "max" => StatFunc::Max,
                    "min_at" => StatFunc::MinAt,
                    "max_at" => StatFunc::MaxAt,
                    _ => StatFunc::Integ,
                };
                let expr = m.expr()?;
                let (mut from, mut to) = (None, None);
                while let Some((k, v)) = m.kv(self)? {
                    match k.as_str() {
                        "from" => from = Some(v),
                        "to" => to = Some(v),
                        _ => {
                            return Err(ParseError::new(
                                span.clone(),
                                format!("unexpected `{k}=` in .meas {kw}"),
                            ));
                        }
                    }
                }
                MeasKind::Stat {
                    func,
                    expr,
                    from,
                    to,
                }
            }
            "find" | "deriv" | "derivative" => {
                let expr = m.expr()?;
                let save = m.pos;
                let next = m.word().to_ascii_lowercase();
                if next == "when" {
                    let when = m.trigger(self, true)?;
                    if kw == "find" {
                        MeasKind::FindWhen { expr, when }
                    } else {
                        MeasKind::DerivWhen { expr, when }
                    }
                } else {
                    m.pos = save;
                    let mut at = None;
                    while let Some((k, v)) = m.kv(self)? {
                        if k == "at" {
                            at = Some(v);
                        }
                    }
                    let at = at.ok_or_else(|| {
                        ParseError::new(span.clone(), format!("`.meas {kw}` needs `at=` or `when`"))
                    })?;
                    if kw == "find" {
                        MeasKind::FindAt { expr, at }
                    } else {
                        MeasKind::DerivAt { expr, at }
                    }
                }
            }
            "when" => MeasKind::When(m.trigger(self, true)?),
            "trig" => {
                let trig = m.trigger(self, false)?;
                let t = m.word().to_ascii_lowercase();
                if t != "targ" {
                    return Err(ParseError::new(
                        span,
                        "`.meas … trig …` needs a `targ …` part",
                    ));
                }
                let targ = m.trigger(self, false)?;
                MeasKind::TrigTarg { trig, targ }
            }
            "param" => {
                m.skip_ws();
                if m.peek() == Some('=') {
                    m.pos += 1;
                }
                m.skip_ws();
                let rest = &text[m.pos..];
                MeasKind::Param(rest_expr(line, m.pos, rest)?)
            }
            "" => return Err(ParseError::new(span, "`.meas` needs a measurement type")),
            _ => {
                return Err(ParseError::new(
                    line.span_at(kw_pos),
                    format!("unknown measurement `{kw}`"),
                ));
            }
        };
        Ok(MeasSpec {
            name,
            analysis: an,
            kind,
            span,
        })
    }
}

fn pre_kind_is_points(_inc: &f64) -> bool {
    false
}

/// Parses an expression that runs to the end of the line (B sources, `.func` bodies).
fn rest_expr(line: &Line, off: usize, rest: &str) -> Result<Expr, ParseError> {
    let t = rest.trim();
    let lead = rest.len() - rest.trim_start().len();
    let (inner, extra) = if (t.starts_with('{') && t.ends_with('}'))
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
    {
        (&t[1..t.len() - 1], 1)
    } else {
        (t, 0)
    };
    Expr::parse(inner).map_err(|e| {
        ParseError::new(
            line.span_at(off + lead + extra + e.pos),
            format!("in expression `{inner}`: {}", e.msg),
        )
    })
}

/// Parses a `.param` value at `pos`: `{expr}`, `'expr'` or a bare expression prefix.
fn value_prefix(line: &Line, pos: usize) -> Result<(Expr, usize), ParseError> {
    let text = &line.text[pos..];
    if text.starts_with('{') || text.starts_with('\'') {
        let (open, close) = if text.starts_with('{') {
            ('{', '}')
        } else {
            ('\'', '\'')
        };
        let mut depth = 0;
        let mut end = None;
        for (i, c) in text.char_indices() {
            if open == '{' {
                if c == open {
                    depth += 1;
                } else if c == close {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
            } else if c == close && i > 0 {
                end = Some(i);
                break;
            }
        }
        let end =
            end.ok_or_else(|| ParseError::new(line.span_at(pos), format!("unbalanced `{open}`")))?;
        let inner = &text[1..end];
        let e = Expr::parse(inner).map_err(|e| {
            ParseError::new(
                line.span_at(pos + 1 + e.pos),
                format!("in expression `{inner}`: {}", e.msg),
            )
        })?;
        return Ok((e, end + 1));
    }
    let (e, n) =
        Expr::parse_prefix(text).map_err(|e| ParseError::new(line.span_at(pos + e.pos), e.msg))?;
    Ok((e, n))
}

/// Cursor for `.meas` lines (expression-aware).
struct MeasParser<'a> {
    line: &'a Line,
    text: &'a str,
    pos: usize,
}

impl<'a> MeasParser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.text.len() && self.text.as_bytes()[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }
    fn word(&mut self) -> &'a str {
        self.skip_ws();
        let start = self.pos;
        while self.pos < self.text.len() && {
            let c = self.text.as_bytes()[self.pos];
            !c.is_ascii_whitespace() && c != b'='
        } {
            self.pos += 1;
        }
        &self.text[start..self.pos]
    }
    fn expr(&mut self) -> Result<Expr, ParseError> {
        self.skip_ws();
        let (e, n) = value_prefix(self.line, self.pos)?;
        self.pos += n;
        Ok(e)
    }
    /// `key=value` with a numeric value; None at end or when the next token is not `k=`.
    fn kv(&mut self, st: &State) -> Result<Option<(String, f64)>, ParseError> {
        self.skip_ws();
        let save = self.pos;
        let k = self.word().to_ascii_lowercase();
        self.skip_ws();
        if k.is_empty() || self.peek() != Some('=') {
            self.pos = save;
            return Ok(None);
        }
        self.pos += 1;
        self.skip_ws();
        let vstart = self.pos;
        let w = self.word();
        let span = self.line.span_at(vstart);
        let e = Expr::parse(w).map_err(|e| ParseError::new(span.clone(), e.msg))?;
        Ok(Some((k, st.eval_top(&e, &span)?)))
    }
    /// Condition: `expr=val` / `expr val=x` / `at=t`, plus td/rise/fall/cross.
    fn trigger(&mut self, st: &State, when_form: bool) -> Result<Trigger, ParseError> {
        let mut trig = Trigger {
            at: None,
            expr: None,
            val: None,
            td: 0.0,
            edge: Edge::Cross(1),
        };
        self.skip_ws();
        // `trig at=1m`
        let save = self.pos;
        let w = self.word().to_ascii_lowercase();
        if w == "at" {
            self.skip_ws();
            if self.peek() == Some('=') {
                self.pos += 1;
                self.skip_ws();
                let vs = self.pos;
                let v = self.word();
                let span = self.line.span_at(vs);
                let e = Expr::parse(v).map_err(|e| ParseError::new(span.clone(), e.msg))?;
                trig.at = Some(st.eval_top(&e, &span)?);
                return Ok(trig);
            }
        }
        self.pos = save;
        let e = self.expr()?;
        if when_form {
            if let Expr::Binary(BinOp::Eq, l, r) = e {
                trig.expr = Some(*l);
                trig.val = Some(*r);
            } else {
                trig.expr = Some(e);
            }
        } else {
            trig.expr = Some(e);
        }
        let mut seen_edge = false;
        loop {
            self.skip_ws();
            let save = self.pos;
            let k = self.word().to_ascii_lowercase();
            self.skip_ws();
            if k.is_empty() || self.peek() != Some('=') {
                self.pos = save;
                break;
            }
            self.pos += 1;
            self.skip_ws();
            let vs = self.pos;
            let v = self.word().to_string();
            let span = self.line.span_at(vs);
            match k.as_str() {
                "val" => {
                    let e = Expr::parse(&v).map_err(|e| ParseError::new(span.clone(), e.msg))?;
                    trig.val = Some(e);
                }
                "td" => {
                    let e = Expr::parse(&v).map_err(|e| ParseError::new(span.clone(), e.msg))?;
                    trig.td = st.eval_top(&e, &span)?;
                }
                "rise" | "fall" | "cross" => {
                    let last = v.eq_ignore_ascii_case("last");
                    let n = if last {
                        0
                    } else {
                        v.parse::<usize>().map_err(|_| {
                            ParseError::new(span.clone(), format!("`{k}=` needs a count or `last`"))
                        })?
                    };
                    seen_edge = true;
                    trig.edge = match (k.as_str(), last) {
                        ("rise", false) => Edge::Rise(n),
                        ("fall", false) => Edge::Fall(n),
                        ("cross", false) => Edge::Cross(n),
                        ("rise", true) => Edge::RiseLast,
                        ("fall", true) => Edge::FallLast,
                        _ => Edge::CrossLast,
                    };
                }
                _ => {
                    self.pos = save;
                    break;
                }
            }
        }
        let _ = seen_edge;
        Ok(trig)
    }
}

/// Evaluates top-level params (in order, allowing forward references by iteration).
pub(crate) fn top_param_values(top: &Scope, nl: &Netlist) -> HashMap<String, f64> {
    let mut vals: HashMap<String, f64> = HashMap::new();
    let mut all: Vec<(String, Expr)> = nl
        .library_params()
        .into_iter()
        .map(|(n, e, _)| (n, e))
        .collect();
    all.extend(top.params.iter().map(|(n, e, _)| (n.clone(), e.clone())));
    let funcs: HashMap<String, FuncDef> = {
        let mut m = nl.library_funcs();
        for f in &top.funcs {
            m.insert(f.name.clone(), f.clone());
        }
        m
    };
    for _ in 0..4 {
        for (n, e) in &all {
            let e2 = crate::elab::inline_funcs(e, &funcs, 0).unwrap_or_else(|_| e.clone());
            if let Ok(v) = e2.eval_with(&|k| vals.get(k).copied()) {
                vals.insert(n.clone(), v);
            }
        }
    }
    vals
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Netlist {
        Netlist::parse(s).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn garden_node_netlist() {
        let nl = p("* garden-node — generated, editable
V1    VCC 0  DC 5
RLDR  VCC A1 {RL}
R1    A1  0  {R1v}
R2    D9  N1 220
D1    N1  0  LED_AMBER
R3    D8  N2 330
D2    N2  0  LED_RED
* GL5528: R = 350k · lux^-0.7, ±10%
.param lux=180 RL={350k*pwr(lux,-0.7)}
.param R1v=10k
.model LED_AMBER D(Is=1e-19 N=1.8 Rs=6)
.model LED_RED   D(Is=1e-18 N=1.9 Rs=5)

.tran 0 10m 0 1u
.dc   lux 1 10k dec 40
.step param R1v list 4.7k 10k 22k
.mc   100 tol=5%  seed=7
.op
.end
");
        assert_eq!(nl.title, "* garden-node — generated, editable");
        assert_eq!(nl.top.elements.len(), 7);
        assert_eq!(nl.top.params.len(), 3);
        assert_eq!(nl.top.models.len(), 2);
        assert_eq!(nl.analyses.len(), 3);
        match &nl.analyses[1].0 {
            Analysis::Dc(s) => {
                assert_eq!(s[0].var, SweepVar::Name("lux".into()));
                assert_eq!(s[0].kind, SweepKind::Dec(40));
                assert_eq!(s[0].values().len(), 161);
            }
            a => panic!("{a:?}"),
        }
        assert_eq!(nl.steps[0].values, vec![4700.0, 10000.0, 22000.0]);
        let mc = nl.mc.as_ref().unwrap();
        assert_eq!((mc.runs, mc.tol, mc.seed), (100, Some(0.05), 7));
        match &nl.analyses[0].0 {
            Analysis::Tran(t) => assert_eq!((t.tstep, t.tstop, t.tmax), (0.0, 10e-3, Some(1e-6))),
            a => panic!("{a:?}"),
        }
    }

    #[test]
    fn continuation_and_comments() {
        let nl = p(
            "t\nV1 1 0 PULSE(0 5\n* comment between\n+ 1n 1n 1n 5u 10u) ; trailing\nR1 1 0 1k $ dollar comment\n",
        );
        assert_eq!(nl.top.elements.len(), 2);
        match &nl.top.elements[0].kind {
            ElemKind::Vsource(s) => match &s.wave {
                Some(Waveform::Pulse(a)) => assert_eq!(a.len(), 7),
                w => panic!("{w:?}"),
            },
            k => panic!("{k:?}"),
        }
    }

    #[test]
    fn subckt_with_params_and_nesting() {
        let nl = p("t
.subckt outer a b params: r=1k
.subckt inner x y
R1 x y {r*2}
.ends inner
X1 a b inner
.ends
X1 1 0 outer params: r=2k
X2 1 0 outer r=3k
V1 1 0 1
");
        assert_eq!(nl.top.subckts.len(), 1);
        let outer = &nl.top.subckts[0];
        assert_eq!(outer.defaults, vec![("r".into(), Expr::Num(1000.0))]);
        assert_eq!(outer.subckts[0].name, "inner");
        assert_eq!(
            nl.top.elements[0].params,
            vec![("r".into(), Expr::Num(2000.0))]
        );
        assert_eq!(
            nl.top.elements[1].params,
            vec![("r".into(), Expr::Num(3000.0))]
        );
    }

    #[test]
    fn errors_point_at_columns() {
        let e = Netlist::parse("t\nR1 a b {1+*2}\n").unwrap_err();
        assert_eq!((e.span.line, e.span.col), (2, 11));
        let e = Netlist::parse("t\nZ1 a b 1\n").unwrap_err();
        assert!(e.msg.contains("unknown element type"), "{e}");
        assert_eq!(e.span.line, 2);
        let e = Netlist::parse("t\n.subckt foo a\nR1 a 0 1\n").unwrap_err();
        assert!(e.msg.contains(".ends"), "{e}");
        let e = Netlist::parse("t\n.model q1 bogus(is=1)\n").unwrap_err();
        assert!(e.msg.contains("unknown model type"), "{e}");
        let e = Netlist::parse("t\nV1 1 0 PULSE(0 5 1n\n").unwrap_err();
        assert!(e.msg.contains(")"), "{e}");
        let e = Netlist::parse("t\n.tran 1u\n").unwrap_err();
        assert!(e.msg.contains("tstop"), "{e}");
        let e = Netlist::parse("t\nR1 a b 1k\nR1 c d 1k\n").unwrap_err();
        assert!(e.msg.contains("duplicate"), "{e}");
    }

    #[test]
    fn sources_and_controlled() {
        let nl = p("t
V1 in 0 DC 0 AC 1 0 SIN(0 1 1k)
I1 0 a 1m
V2 b 0 PWL(0 0 1m 5 2m 0) r=0 td=1m
E1 o 0 in 0 10
E2 o2 0 POLY(1) in 0 0 1 0.5
G1 0 o3 VALUE={v(in)*2m}
F1 o4 0 V1 2
H1 o5 0 POLY(1) V1 0 100
B1 o6 0 V = v(in) > 0.5 ? 3 : 0
B2 0 o7 I=i(V1)*2
S1 a 0 in 0 swm OFF
W1 b 0 V1 cswm ON
.model swm sw(vt=1 vh=0.1 ron=1 roff=1meg)
.model cswm csw(it=1m ih=0.1m)
");
        assert_eq!(nl.top.elements.len(), 12);
        match &nl.top.elements[0].kind {
            ElemKind::Vsource(s) => {
                assert_eq!(s.dc, Some(Expr::Num(0.0)));
                assert_eq!(s.ac_mag, Some(Expr::Num(1.0)));
                assert!(matches!(s.wave, Some(Waveform::Sin(_))));
            }
            k => panic!("{k:?}"),
        }
        match &nl.top.elements[2].kind {
            ElemKind::Vsource(s) => match &s.wave {
                Some(Waveform::Pwl {
                    points,
                    repeat,
                    delay,
                }) => {
                    assert_eq!(points.len(), 6);
                    assert!(repeat.is_some() && delay.is_some());
                }
                w => panic!("{w:?}"),
            },
            k => panic!("{k:?}"),
        }
        assert!(matches!(
            &nl.top.elements[4].kind,
            ElemKind::Vcvs(Control::Poly { dims: 1, .. })
        ));
        assert!(matches!(
            &nl.top.elements[5].kind,
            ElemKind::Vccs(Control::Value(_))
        ));
        assert!(matches!(
            &nl.top.elements[10].kind,
            ElemKind::Switch {
                init_on: Some(false),
                ..
            }
        ));
        assert!(matches!(
            &nl.top.elements[11].kind,
            ElemKind::CSwitch {
                init_on: Some(true),
                ..
            }
        ));
    }

    #[test]
    fn bjt_substrate_disambiguation() {
        let nl = p(
            "t\nQ1 c b e qn\nQ2 c b e s qn 2\nQ3 1 2 3 4 qn off\nQ4 c b e qn 1.5 off\n.model qn npn\n",
        );
        assert_eq!(nl.top.elements[0].nodes.len(), 3);
        assert_eq!(nl.top.elements[1].nodes.len(), 4);
        assert_eq!(nl.top.elements[2].nodes, vec!["1", "2", "3", "4"]);
        assert!(matches!(
            &nl.top.elements[3].kind,
            ElemKind::Bjt {
                off: true,
                area: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn meas_cards() {
        let nl = p("t
V1 1 0 1
.meas tran vavg avg v(1) from=1m to=2m
.meas tran tcross when v(1)=1 rise=2
.meas tran tdel trig v(1) val=0.5 rise=1 targ v(2) val=0.5 fall=last
.meas tran vf find v(2) when v(1)=0.5 cross=1
.meas tran va find v(2) at=1m
.meas ac bw when vdb(2)=-3 fall=1
.meas tran r param='vavg*2'
");
        assert_eq!(nl.measures.len(), 7);
        assert!(matches!(
            &nl.measures[1].kind,
            MeasKind::When(Trigger {
                edge: Edge::Rise(2),
                ..
            })
        ));
        assert!(matches!(
            &nl.measures[2].kind,
            MeasKind::TrigTarg {
                targ: Trigger {
                    edge: Edge::FallLast,
                    ..
                },
                ..
            }
        ));
        assert!(
            matches!(&nl.measures[4].kind, MeasKind::FindAt { at, .. } if (*at - 1e-3).abs() < 1e-15)
        );
    }

    #[test]
    fn options_ic_nodeset_temp() {
        let nl = p(
            "t\n.options reltol=1e-4 method=gear noopiter\n.ic v(a)=1 v(b)=2\n.nodeset v(c)=0.5\n.temp 0 27 85\n",
        );
        assert_eq!(nl.option("reltol"), Some(1e-4));
        assert!(nl.has_option("method=gear"));
        assert!(nl.has_option("noopiter"));
        assert_eq!(nl.ics.len(), 2);
        assert_eq!(nl.nodesets.len(), 1);
        assert_eq!(nl.temps, vec![0.0, 27.0, 85.0]);
    }

    #[test]
    fn include_and_lib_sections() {
        let loader: Arc<dyn Fn(&Path) -> std::io::Result<String> + Send + Sync> =
            Arc::new(|p: &Path| {
                Ok(match p.file_name().unwrap().to_str().unwrap() {
                    "parts.inc" => {
                        ".model dfast d(is=1e-15)\n.subckt buf a b\nR1 a b 1\n.ends\n".to_string()
                    }
                    "lib.lib" => ".lib typ\n.param rv=1k\n.endl\n.lib slow\n.param rv=2k\n.endl\n"
                        .to_string(),
                    _ => return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "nope")),
                })
            });
        let opts = ParseOptions {
            loader: Some(loader),
            ..ParseOptions::default()
        };
        let nl = Netlist::parse_with(
            "t\n.include parts.inc\n.lib lib.lib slow\nR1 1 0 {rv}\n",
            &opts,
        )
        .unwrap();
        assert_eq!(nl.top.models.len(), 1);
        assert_eq!(nl.top.subckts.len(), 1);
        assert_eq!(nl.top.params[0].1, Expr::Num(2000.0));
        let e = Netlist::parse_with("t\n.include missing.inc\n", &opts).unwrap_err();
        assert!(e.msg.contains("cannot read"));
    }

    #[test]
    fn param_forms() {
        let nl =
            p("t\n.param a=1 b = {a*2}, c='b+1' d=a*3+1\n.param f(x)=x*2\n.func g(x,y) {x+y}\n");
        assert_eq!(nl.top.params.len(), 4);
        assert_eq!(nl.top.funcs.len(), 2);
        let vals = top_param_values(&nl.top, &nl);
        assert_eq!(vals["c"], 3.0);
        assert_eq!(vals["d"], 4.0);
    }
}
