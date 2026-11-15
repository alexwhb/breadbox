//! Syntax highlighting for Arduino C++ and SPICE netlists. Pure functions over text → byte
//! spans, so they are unit-tested independently of the editor.
//!
//! Colours follow the design's CSS classes: `.kw` keywords, `.ty` types, `.fn` calls, `.num`,
//! `.str`, `.cm` comments, `.pp` preprocessor; for netlists `.d` dot-cards, `.n` values, `.m`
//! model / subcircuit / parameter names, `.c` comments.

use std::ops::Range;

/// Token class of a span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tok {
    Text,
    Keyword,
    Type,
    Function,
    Number,
    String,
    Comment,
    Preproc,
    /// SPICE dot-card (`.tran`, `.param`).
    Directive,
    /// SPICE model / subcircuit / parameter reference (`LED_AMBER`, `CAPSOIL`, `{RL}`).
    Model,
}

/// Language of a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Lang {
    Cpp,
    Spice,
    Plain,
}

impl Lang {
    /// Picks the language from a file name.
    pub fn for_file(name: &str) -> Lang {
        let l = name.to_ascii_lowercase();
        if [".ino", ".cpp", ".c", ".h", ".hpp", ".cc"]
            .iter()
            .any(|e| l.ends_with(e))
        {
            Lang::Cpp
        } else if [".cir", ".sp", ".spice", ".net", ".lib", ".sub", ".mod"]
            .iter()
            .any(|e| l.ends_with(e))
        {
            Lang::Spice
        } else {
            Lang::Plain
        }
    }

    /// Status-bar name (`C++`, `SPICE`).
    pub fn label(self) -> &'static str {
        match self {
            Lang::Cpp => "C++",
            Lang::Spice => "SPICE",
            Lang::Plain => "Text",
        }
    }
}

/// Highlights `text`. Spans are sorted, non-overlapping and cover only coloured tokens.
pub fn highlight(lang: Lang, text: &str) -> Vec<(Range<usize>, Tok)> {
    match lang {
        Lang::Cpp => cpp(text),
        Lang::Spice => spice(text),
        Lang::Plain => Vec::new(),
    }
}

const CPP_KEYWORDS: &[&str] = &[
    "alignas", "alignof", "asm", "auto", "break", "case", "catch", "class", "const", "constexpr",
    "continue", "default", "delete", "do", "else", "enum", "explicit", "extern", "false", "for",
    "friend", "goto", "if", "inline", "namespace", "new", "noexcept", "nullptr", "operator",
    "override", "private", "protected", "public", "register", "return", "sizeof", "static",
    "static_assert", "struct", "switch", "template", "this", "throw", "true", "try", "typedef",
    "typename", "union", "using", "virtual", "volatile", "while", "PROGMEM",
];

const CPP_TYPES: &[&str] = &[
    "void", "bool", "boolean", "char", "short", "int", "long", "float", "double", "signed",
    "unsigned", "byte", "word", "size_t", "int8_t", "int16_t", "int32_t", "int64_t", "uint8_t",
    "uint16_t", "uint32_t", "uint64_t", "String",
];

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}
fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn cpp(text: &str) -> Vec<(Range<usize>, Tok)> {
    let b = text.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line_start = true; // only whitespace so far on this line
    let mut after_class_type = false; // `SoftwareSerial esp(3, 2)` declares, not calls
    while i < n {
        let c = b[i];
        if c == b'\n' {
            line_start = true;
            i += 1;
            continue;
        }
        if c == b' ' || c == b'\t' || c == b'\r' {
            i += 1;
            continue;
        }
        // comments
        if c == b'/' && i + 1 < n && b[i + 1] == b'/' {
            let e = text[i..].find('\n').map(|k| i + k).unwrap_or(n);
            out.push((i..e, Tok::Comment));
            i = e;
            continue;
        }
        if c == b'/' && i + 1 < n && b[i + 1] == b'*' {
            let e = text[i + 2..].find("*/").map(|k| i + 2 + k + 2).unwrap_or(n);
            out.push((i..e, Tok::Comment));
            i = e;
            line_start = false;
            continue;
        }
        // preprocessor
        if c == b'#' && line_start {
            let mut e = i + 1;
            while e < n && (b[e] == b' ' || b[e] == b'\t') {
                e += 1;
            }
            while e < n && is_ident(b[e]) {
                e += 1;
            }
            out.push((i..e, Tok::Preproc));
            let directive = text[i + 1..e].trim();
            i = e;
            line_start = false;
            if directive == "include" {
                // <header> as a string
                let mut k = i;
                while k < n && (b[k] == b' ' || b[k] == b'\t') {
                    k += 1;
                }
                if k < n && b[k] == b'<' {
                    let e2 = text[k..]
                        .find(['>', '\n'])
                        .map(|x| k + x + usize::from(b[k + x] == b'>'))
                        .unwrap_or(n);
                    out.push((k..e2, Tok::String));
                    i = e2;
                }
            }
            continue;
        }
        line_start = false;
        // strings / chars
        if c == b'"' || c == b'\'' {
            let q = c;
            let mut e = i + 1;
            while e < n && b[e] != q && b[e] != b'\n' {
                if b[e] == b'\\' {
                    e += 1;
                }
                e += 1;
            }
            let e = (e + 1).min(n);
            out.push((i..e, Tok::String));
            i = e;
            continue;
        }
        // numbers
        if c.is_ascii_digit() || (c == b'.' && i + 1 < n && b[i + 1].is_ascii_digit()) {
            let mut e = i;
            if c == b'0' && i + 1 < n && (b[i + 1] == b'x' || b[i + 1] == b'X' || b[i + 1] == b'b' || b[i + 1] == b'B') {
                e += 2;
                while e < n && b[e].is_ascii_hexdigit() {
                    e += 1;
                }
            } else {
                while e < n && (b[e].is_ascii_digit() || b[e] == b'.') {
                    e += 1;
                }
                if e < n && (b[e] == b'e' || b[e] == b'E') {
                    e += 1;
                    if e < n && (b[e] == b'+' || b[e] == b'-') {
                        e += 1;
                    }
                    while e < n && b[e].is_ascii_digit() {
                        e += 1;
                    }
                }
            }
            while e < n && matches!(b[e], b'u' | b'U' | b'l' | b'L' | b'f' | b'F') {
                e += 1;
            }
            out.push((i..e, Tok::Number));
            i = e;
            continue;
        }
        // identifiers
        if is_ident_start(c) {
            let mut e = i;
            while e < n && is_ident(b[e]) {
                e += 1;
            }
            let word = &text[i..e];
            let mut k = e;
            while k < n && (b[k] == b' ' || b[k] == b'\t') {
                k += 1;
            }
            let next = b.get(k).copied();
            let declared = std::mem::take(&mut after_class_type);
            let tok = if CPP_TYPES.contains(&word) {
                Some(Tok::Type)
            } else if CPP_KEYWORDS.contains(&word) {
                Some(Tok::Keyword)
            } else if declared {
                None
            } else if next == Some(b'(') {
                Some(Tok::Function)
            } else if word.as_bytes()[0].is_ascii_uppercase()
                && word.bytes().any(|x| x.is_ascii_lowercase())
                && next.is_some_and(|x| is_ident_start(x) || x == b'&' || x == b'*')
            {
                // `SoftwareSerial esp(3, 2);`, `WifiAT wifi(esp);` — a class name declaring
                after_class_type = true;
                Some(Tok::Type)
            } else {
                None
            };
            if let Some(t) = tok {
                out.push((i..e, t));
            }
            i = e;
            continue;
        }
        i += 1;
    }
    out
}

/// True for a SPICE numeric literal: `5`, `220`, `10k`, `4.7k`, `1u`, `1e-19`, `2meg`, `-0.7`.
pub fn is_spice_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
        i += 1;
    }
    if i == digits_start || !b[digits_start..i].iter().any(|c| c.is_ascii_digit()) {
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') && i + 1 < b.len() {
        let mut j = i + 1;
        if b[j] == b'+' || b[j] == b'-' {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            i = j;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    // SI suffix + unit letters (`10kohm`, `1uF`), or % for tolerances
    b[i..]
        .iter()
        .all(|c| c.is_ascii_alphabetic() || *c == b'%' || *c == 0xC2 || *c == 0xB5)
}

fn spice(text: &str) -> Vec<(Range<usize>, Tok)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    let mut prev_kind: Option<u8> = None; // element letter of the previous logical line
    for (lineno, line) in text.split_inclusive('\n').enumerate() {
        let body = line.trim_end_matches(['\n', '\r']);
        let base = offset;
        offset += line.len();
        if lineno == 0 && !body.trim_start().starts_with(['.', '*']) && body.trim().is_empty() {
            continue;
        }
        let trimmed = body.trim_start();
        let lead = body.len() - trimmed.len();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('*') {
            out.push((base + lead..base + body.len(), Tok::Comment));
            continue;
        }
        // inline comments: `;` anywhere, `$` after whitespace
        let mut code_end = body.len();
        let bb = body.as_bytes();
        for (k, &c) in bb.iter().enumerate() {
            if c == b';' || (c == b'$' && (k == 0 || bb[k - 1] == b' ' || bb[k - 1] == b'\t')) {
                code_end = k;
                break;
            }
        }
        if code_end < body.len() {
            out.push((base + code_end..base + body.len(), Tok::Comment));
        }
        let code = &body[..code_end];
        // tokens with byte offsets
        let toks = spice_tokens(code);
        if toks.is_empty() {
            continue;
        }
        let first = &code[toks[0].clone()];
        let first_c = first.as_bytes()[0].to_ascii_uppercase();
        let mut spans: Vec<(Range<usize>, Tok)> = Vec::new();
        if first_c == b'.' {
            spans.push((toks[0].clone(), Tok::Directive));
            prev_kind = Some(b'.');
            if first.eq_ignore_ascii_case(".param") {
                for t in &toks[1..] {
                    spans.extend(key_value(code, t.clone()));
                }
            }
        } else {
            let kind = if first_c == b'+' {
                prev_kind.unwrap_or(b'+')
            } else {
                prev_kind = Some(first_c);
                first_c
            };
            if kind == b'.' {
                // continuation of a dot-card: plain
            } else {
                let start = usize::from(first_c != b'+');
                let mut positional = Vec::new();
                for t in &toks[start..] {
                    let s = &code[t.clone()];
                    if s.contains('=') {
                        spans.extend(key_value(code, t.clone()));
                    } else if s.starts_with('{') {
                        spans.extend(brace_idents(code, t.clone()));
                    } else {
                        positional.push(t.clone());
                    }
                }
                let model_last = matches!(kind, b'D' | b'Q' | b'M' | b'J' | b'X');
                if first_c != b'+' {
                    for (pi, t) in positional.iter().enumerate() {
                        let s = &code[t.clone()];
                        let is_last = pi + 1 == positional.len();
                        if model_last && is_last && !is_spice_number(s) && pi >= 2 {
                            spans.push((t.clone(), Tok::Model));
                        } else if matches!(kind, b'R' | b'C' | b'L') && pi == 2 && is_spice_number(s) {
                            spans.push((t.clone(), Tok::Number));
                        } else if matches!(kind, b'V' | b'I' | b'E' | b'F' | b'G' | b'H' | b'B')
                            && pi >= 2
                            && is_spice_number(s)
                        {
                            spans.push((t.clone(), Tok::Number));
                        }
                    }
                }
            }
        }
        spans.sort_by_key(|s| s.0.start);
        for (r, t) in spans {
            out.push((base + r.start..base + r.end, t));
        }
    }
    out.sort_by_key(|s| s.0.start);
    out
}

/// Whitespace-separated tokens; a `{…}` group and `name(…)` stay one token.
fn spice_tokens(code: &str) -> Vec<Range<usize>> {
    let b = code.as_bytes();
    let mut v = Vec::new();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t' || b[i] == b',') {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let s = i;
        let mut depth = 0i32;
        while i < b.len() {
            match b[i] {
                b'{' | b'(' => depth += 1,
                b'}' | b')' => depth -= 1,
                b' ' | b'\t' if depth <= 0 => break,
                _ => {}
            }
            i += 1;
        }
        v.push(s..i);
    }
    v
}

/// `vwc=42` → value span as a number; `RL={…}` → names inside the braces stay plain.
fn key_value(code: &str, t: Range<usize>) -> Vec<(Range<usize>, Tok)> {
    let s = &code[t.clone()];
    let Some(eq) = s.find('=') else {
        return Vec::new();
    };
    let v = &s[eq + 1..];
    let vs = t.start + eq + 1;
    if is_spice_number(v) {
        vec![(vs..t.end, Tok::Number)]
    } else {
        Vec::new()
    }
}

/// `{RL}` → `RL` as a model/parameter reference.
fn brace_idents(code: &str, t: Range<usize>) -> Vec<(Range<usize>, Tok)> {
    let s = &code[t.clone()];
    let inner = s.trim_start_matches('{').trim_end_matches('}');
    let off = t.start + (s.len() - s.trim_start_matches('{').len());
    if !inner.is_empty() && inner.bytes().all(is_ident) {
        vec![(off..off + inner.len(), Tok::Model)]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(lang: Lang, text: &str) -> Vec<(&str, Tok)> {
        highlight(lang, text)
            .into_iter()
            .map(|(r, t)| (&text[r], t))
            .collect()
    }

    #[test]
    fn cpp_matches_the_design_editor() {
        let src = "#include <SoftwareSerial.h>\n#include \"wifi_at.h\"\n\nconst int SOIL_PIN  = A0;   // capacitive probe J1\nSoftwareSerial esp(3, 2);  // RX, TX\nvoid setup() {\n  Serial.begin(115200);\n  if (every(1000)) wifi.publish(\"garden/pot1\", pct, light);\n}\n";
        let t = tokens(Lang::Cpp, src);
        assert!(t.contains(&("#include", Tok::Preproc)));
        assert!(t.contains(&("<SoftwareSerial.h>", Tok::String)));
        assert!(t.contains(&("\"wifi_at.h\"", Tok::String)));
        assert!(t.contains(&("const", Tok::Keyword)));
        assert!(t.contains(&("int", Tok::Type)));
        assert!(t.contains(&("// capacitive probe J1", Tok::Comment)));
        assert!(t.contains(&("SoftwareSerial", Tok::Type)));
        assert!(!t.iter().any(|(s, _)| *s == "esp"), "a declaration, not a call");
        assert!(t.contains(&("3", Tok::Number)));
        assert!(t.contains(&("void", Tok::Type)));
        assert!(t.contains(&("setup", Tok::Function)));
        assert!(t.contains(&("begin", Tok::Function)));
        assert!(t.contains(&("115200", Tok::Number)));
        assert!(t.contains(&("if", Tok::Keyword)));
        assert!(t.contains(&("\"garden/pot1\"", Tok::String)));
        // identifiers that are neither keywords nor calls stay plain
        assert!(!t.iter().any(|(s, _)| *s == "SOIL_PIN" || *s == "A0" || *s == "Serial"));
    }

    #[test]
    fn cpp_block_comments_and_numbers() {
        let t = tokens(Lang::Cpp, "/* a\n b */ x = 0x1F + 2.5f; y = 'c';");
        assert_eq!(t[0], ("/* a\n b */", Tok::Comment));
        assert!(t.contains(&("0x1F", Tok::Number)));
        assert!(t.contains(&("2.5f", Tok::Number)));
        assert!(t.contains(&("'c'", Tok::String)));
    }

    #[test]
    fn spice_matches_the_design_netlist() {
        let src = "* garden-node — generated, editable\nV1    VCC 0  DC 5\nRLDR  VCC A1 {RL}\nXJ1   VCC 0  A0 CAPSOIL vwc=42\nR2    D9  N1 220\nD1    N1  0  LED_AMBER\nXU1   VCC 0 A0 A1 D9 D8 ATMEGA328P\n+     fw=garden-node.hex  sync=1u\n.param lux=180 RL={350k*pwr(lux,-0.7)}\n.param R1v=10k\n.tran 0 10m 0 1u\n.end\n";
        let t = tokens(Lang::Spice, src);
        assert_eq!(t[0], ("* garden-node — generated, editable", Tok::Comment));
        assert!(t.contains(&("5", Tok::Number)));
        assert!(!t.contains(&("0", Tok::Number)), "node 0 stays plain: {t:?}");
        assert!(t.contains(&("RL", Tok::Model)));
        assert!(t.contains(&("CAPSOIL", Tok::Model)));
        assert!(t.contains(&("42", Tok::Number)));
        assert!(t.contains(&("220", Tok::Number)));
        assert!(t.contains(&("LED_AMBER", Tok::Model)));
        assert!(t.contains(&("ATMEGA328P", Tok::Model)));
        assert!(t.contains(&("1u", Tok::Number)));
        assert!(t.contains(&(".param", Tok::Directive)));
        assert!(t.contains(&("180", Tok::Number)));
        assert!(t.contains(&("10k", Tok::Number)));
        assert!(t.contains(&(".tran", Tok::Directive)));
        assert!(!t.contains(&("10m", Tok::Number)), "analysis cards stay plain");
        assert!(t.contains(&(".end", Tok::Directive)));
    }

    #[test]
    fn spice_numbers() {
        for s in ["5", "220", "10k", "4.7k", "1u", "1e-19", "2meg", "-0.7", "5%", "100nF"] {
            assert!(is_spice_number(s), "{s}");
        }
        for s in ["A0", "VCC", "N1", "", "-", "lux"] {
            assert!(!is_spice_number(s), "{s}");
        }
    }

    #[test]
    fn lang_from_file_names() {
        assert_eq!(Lang::for_file("garden-node.ino"), Lang::Cpp);
        assert_eq!(Lang::for_file("wifi_at.h"), Lang::Cpp);
        assert_eq!(Lang::for_file("garden-node.cir"), Lang::Spice);
        assert_eq!(Lang::for_file("notes.txt"), Lang::Plain);
    }
}
