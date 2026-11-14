//! AT command line parsing shared by the UART modems.

/// One argument of `AT+CMD=a,"b",c`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtArg {
    /// Quoted string (escapes `\"`, `\,`, `\\` resolved).
    Str(String),
    /// Unquoted token.
    Raw(String),
    /// Omitted (`a,,c`).
    Empty,
}

impl AtArg {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            AtArg::Str(s) | AtArg::Raw(s) => Some(s),
            AtArg::Empty => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            AtArg::Raw(s) | AtArg::Str(s) => s.trim().parse().ok(),
            AtArg::Empty => None,
        }
    }
    pub fn is_quoted(&self) -> bool {
        matches!(self, AtArg::Str(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtKind {
    /// `AT+CMD`
    Exec,
    /// `AT+CMD?`
    Query,
    /// `AT+CMD=?`
    Test,
    /// `AT+CMD=args`
    Set(Vec<AtArg>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtCommand {
    /// Upper-cased name after `AT`: `""` for plain `AT`, `"E0"`, `"+CWJAP"`, …
    pub name: String,
    pub kind: AtKind,
}

impl AtCommand {
    pub fn args(&self) -> &[AtArg] {
        match &self.kind {
            AtKind::Set(a) => a,
            _ => &[],
        }
    }
    pub fn arg_str(&self, i: usize) -> Option<&str> {
        self.args().get(i).and_then(|a| a.as_str())
    }
    pub fn arg_int(&self, i: usize) -> Option<i64> {
        self.args().get(i).and_then(|a| a.as_int())
    }
}

/// Splits `a,"b,c",d` into arguments.
pub fn split_args(s: &str) -> Vec<AtArg> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut in_q = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if in_q => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            '"' => {
                in_q = !in_q;
                quoted = true;
            }
            ',' if !in_q => {
                out.push(finish(&mut cur, &mut quoted));
            }
            c => cur.push(c),
        }
    }
    out.push(finish(&mut cur, &mut quoted));
    out
}

fn finish(cur: &mut String, quoted: &mut bool) -> AtArg {
    let v = std::mem::take(cur);
    let q = std::mem::replace(quoted, false);
    if q {
        AtArg::Str(v)
    } else if v.trim().is_empty() {
        AtArg::Empty
    } else {
        AtArg::Raw(v.trim().to_string())
    }
}

/// Parses an `AT…` line (without CR/LF). `None` if it doesn't start with `AT`.
pub fn parse(line: &str) -> Option<AtCommand> {
    let t = line.trim_matches(|c: char| c == '\r' || c == '\n' || c == ' ');
    if t.len() < 2 || !t[..2].eq_ignore_ascii_case("AT") {
        return None;
    }
    let rest = &t[2..];
    let (name, kind) = if let Some(i) = rest.find('=') {
        let (n, a) = (&rest[..i], &rest[i + 1..]);
        if a == "?" { (n, AtKind::Test) } else { (n, AtKind::Set(split_args(a))) }
    } else if let Some(n) = rest.strip_suffix('?') {
        (n, AtKind::Query)
    } else {
        (rest, AtKind::Exec)
    };
    Some(AtCommand { name: name.to_ascii_uppercase(), kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_forms() {
        assert_eq!(parse("AT").unwrap(), AtCommand { name: "".into(), kind: AtKind::Exec });
        assert_eq!(parse("ATE0\r").unwrap().name, "E0");
        assert_eq!(parse("AT+CWMODE?").unwrap().kind, AtKind::Query);
        assert_eq!(parse("AT+CWMODE=?").unwrap().kind, AtKind::Test);
        let c = parse(r#"AT+CWJAP="SIM-AP","bread\"box,1""#).unwrap();
        assert_eq!(c.name, "+CWJAP");
        assert_eq!(c.args(), &[AtArg::Str("SIM-AP".into()), AtArg::Str("bread\"box,1".into())]);
        let c = parse(r#"AT+CIPSTART=0,"TCP","10.0.0.1",1883"#).unwrap();
        assert_eq!(c.arg_int(0), Some(0));
        assert_eq!(c.arg_int(3), Some(1883));
        assert_eq!(parse("AT+X=1,,3").unwrap().args()[1], AtArg::Empty);
        assert!(parse("hello").is_none());
        assert_eq!(parse("at+gmr").unwrap().name, "+GMR");
    }
}
