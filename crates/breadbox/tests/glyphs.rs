//! Every character the app can show renders with the bundled fonts (`PLAT-2`).
//!
//! egui draws a missing glyph as a tofu box (and `egui_kittest` panics on one), so this test
//! scans everything that reaches the screen — string and char literals in the Rust sources
//! of the app, the engine crates and the first-party plugins; parts library, lesson, plugin and
//! example data; example + lesson sketches (serial output); the Plugin SDK docs the app renders
//! — and fails for any character that one of the app's font families cannot draw. Characters
//! that only egui's built-in fallback fonts (Ubuntu, Hack, Noto Emoji, emoji-icon) cover must be
//! on the [`FALLBACK_OK`] list, so new symbols get a deliberate look instead of a surprise.
//!
//! Two deliberate exceptions: the instrument symbols no font has (`⎓ ∿ ⊓ ⋀ ⎍`) are drawn as
//! shapes by `widgets::instrument::ptext` and spelled out by `glyph_safe`
//! ([`SHAPED_SYMBOLS`]); [`MONO_ONLY`] symbols are only ever drawn in JetBrains Mono.

use breadbox::theme::{self, font};
use breadbox::widgets::instrument::{GLYPH_SUBSTITUTES, SHAPED_SYMBOLS};
use eframe::egui::{self, FontFamily};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Fonts Breadbox bundles for text (`assets/fonts/`).
const BUNDLED: &[&str] = &[
    "Inter-Regular",
    "Inter-Medium",
    "Inter-SemiBold",
    "JetBrainsMono-Regular",
    "JetBrainsMono-Medium",
    "SpaceGrotesk-Medium",
    "SpaceGrotesk-SemiBold",
];

/// Symbols that are fine coming from egui's built-in fallback fonts (all families have them).
const FALLBACK_OK: &[char] = &[];

/// Symbols drawn only in the monospace fonts — they must be in JetBrains Mono.
const MONO_ONLY: &[(char, &str)] = &[
    (
        '⊢',
        "DMM dial + LCD annunciator `⊣⊢` (diode), widgets::instrument in mono",
    ),
    (
        '⊣',
        "DMM dial + LCD annunciator `⊣⊢` (diode), widgets::instrument in mono",
    ),
    (
        '█',
        "HD44780 custom-character cells in the LCD text, canvas::art::modules in mono",
    ),
];

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Where a character was seen: `path:line`.
type Seen = BTreeMap<char, Vec<String>>;

fn note(seen: &mut Seen, c: char, file: &Path, line: usize) {
    if c.is_ascii() || c.is_control() || c == '\u{feff}' {
        return;
    }
    let rel = file.strip_prefix(repo()).unwrap_or(file);
    // the substitution table itself (what `glyph_safe` replaces never reaches the screen)
    if rel.ends_with("widgets/instrument.rs") && GLYPH_SUBSTITUTES.iter().any(|(s, _)| *s == c) {
        return;
    }
    let v = seen.entry(c).or_default();
    if v.len() < 4 {
        v.push(format!("{}:{line}", rel.display()));
    }
}

/// String and char literals of a Rust source file (comments and test modules skipped):
/// `(line, text)` with escapes decoded.
fn rust_literals(src: &str) -> Vec<(usize, String)> {
    // unit tests at the end of a module never reach the screen
    let src = match src.find("\n#[cfg(test)]\nmod tests") {
        Some(i) => &src[..i],
        None => src,
    };
    let b: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    while i < b.len() {
        let c = b[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        // comments
        if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && b.get(i + 1) == Some(&'*') {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    if b[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
            continue;
        }
        let prev_ident = i > 0 && ident(b[i - 1]);
        // raw strings: r"…", r#"…"#, br"…"
        if !prev_ident && (c == 'r' || (c == 'b' && b.get(i + 1) == Some(&'r'))) {
            let mut j = if c == 'b' { i + 2 } else { i + 1 };
            let mut hashes = 0;
            while b.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if b.get(j) == Some(&'"') {
                let start_line = line;
                let mut s = String::new();
                j += 1;
                while j < b.len() {
                    if b[j] == '"' && (0..hashes).all(|k| b.get(j + 1 + k) == Some(&'#')) {
                        j += 1 + hashes;
                        break;
                    }
                    if b[j] == '\n' {
                        line += 1;
                    }
                    s.push(b[j]);
                    j += 1;
                }
                out.push((start_line, s));
                i = j;
                continue;
            }
        }
        // byte strings / chars are ASCII
        if !prev_ident && c == 'b' && matches!(b.get(i + 1), Some(&'"') | Some(&'\'')) {
            i += 1;
            let q = b[i];
            i += 1;
            while i < b.len() && b[i] != q {
                if b[i] == '\\' {
                    i += 1;
                }
                if b.get(i) == Some(&'\n') {
                    line += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            let start_line = line;
            let mut s = String::new();
            i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' {
                    let (ch, used, nl) = escape(&b[i..]);
                    if let Some(ch) = ch {
                        s.push(ch);
                    }
                    line += nl;
                    i += used;
                    continue;
                }
                if b[i] == '\n' {
                    line += 1;
                }
                s.push(b[i]);
                i += 1;
            }
            i += 1;
            out.push((start_line, s));
            continue;
        }
        if c == '\'' {
            // a char literal ('x', '\n', '\u{2192}') or a lifetime ('a)
            if b.get(i + 1) == Some(&'\\') {
                let (ch, used, _) = escape(&b[i + 1..]);
                if let Some(ch) = ch {
                    out.push((line, ch.to_string()));
                }
                i += 1 + used + 1;
                continue;
            }
            if b.get(i + 2) == Some(&'\'') {
                out.push((line, b[i + 1].to_string()));
                i += 3;
                continue;
            }
            i += 1;
            continue;
        }
        i += 1;
    }
    out
}

/// Decodes one escape at `b[0] == '\\'`: (char, chars consumed, newlines skipped).
fn escape(b: &[char]) -> (Option<char>, usize, usize) {
    match b.get(1) {
        Some('u') if b.get(2) == Some(&'{') => {
            let end = b.iter().position(|&c| c == '}').unwrap_or(b.len() - 1);
            let hex: String = b[3..end].iter().filter(|c| **c != '_').collect();
            (
                u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32),
                end + 1,
                0,
            )
        }
        Some('x') => (None, 4, 0),
        Some('\n') => {
            // line continuation: skip the newline and leading whitespace
            let mut j = 2;
            while b.get(j).is_some_and(|c| c.is_whitespace() && *c != '\n') {
                j += 1;
            }
            (None, j, 1)
        }
        Some(_) => (None, 2, 0),
        None => (None, 1, 0),
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with('.') || name == "target" {
            continue;
        }
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn files(rel: &str, exts: &[&str]) -> Vec<PathBuf> {
    let mut all = Vec::new();
    walk(&repo().join(rel), &mut all);
    all.retain(|p| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.contains(&e))
    });
    all
}

/// Every character that can reach the screen, with where it was seen.
fn scan() -> Seen {
    let mut seen = Seen::new();
    // Rust sources: the app, the engine crates (messages, part names, serial annotations), the
    // plugin SDK + first-party plugins (panel text, readouts)
    let mut rust = Vec::new();
    for dir in ["crates", "plugins"] {
        rust.extend(files(dir, &["rs"]));
    }
    rust.retain(|p| {
        let s = p.to_string_lossy();
        !s.contains("/tests/")
            && !s.contains("/benches/")
            && !s.contains("/examples/")
            && !s.ends_with("/tests.rs")
    });
    assert!(rust.len() > 100, "found only {} Rust files", rust.len());
    for f in &rust {
        let text = std::fs::read_to_string(f).unwrap();
        for (line, lit) in rust_literals(&text) {
            for c in lit.chars() {
                note(&mut seen, c, f, line);
            }
        }
    }
    // data the app shows verbatim
    let mut data = Vec::new();
    data.extend(files("crates/bb-circuit/parts", &["json"]));
    data.extend(files("lessons", &["toml", "json", "ino", "h", "cpp"]));
    data.extend(files("plugins", &["json", "svg", "toml"]));
    data.extend(files("examples", &["json", "ino", "h", "cpp", "cir"]));
    data.push(repo().join("docs/PLUGIN-SDK.md"));
    data.retain(|p| !p.to_string_lossy().contains("/target/"));
    assert!(data.len() > 50, "found only {} data files", data.len());
    for f in &data {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        for (n, l) in text.lines().enumerate() {
            for c in l.chars() {
                note(&mut seen, c, f, n + 1);
            }
        }
    }
    seen
}

fn families() -> Vec<FontFamily> {
    vec![
        FontFamily::Proportional,
        FontFamily::Monospace,
        FontFamily::Name(font::INTER_MEDIUM.into()),
        FontFamily::Name(font::INTER_SEMIBOLD.into()),
        FontFamily::Name(font::MONO_MEDIUM.into()),
        FontFamily::Name(font::GROTESK_MEDIUM.into()),
        FontFamily::Name(font::GROTESK_SEMIBOLD.into()),
    ]
}

#[test]
fn every_ui_character_renders_with_the_bundled_fonts() {
    let ctx = egui::Context::default();
    ctx.set_fonts(theme::font_definitions());
    let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    // char → fonts that have it, per family
    let coverage: Vec<(FontFamily, BTreeMap<char, Vec<String>>)> = families()
        .into_iter()
        .map(|f| {
            let m = ctx.fonts_mut(|fonts| fonts.characters(&f).clone());
            (f, m)
        })
        .collect();
    let seen = scan();
    let mut missing = Vec::new();
    let mut fallback_only = Vec::new();
    for (c, at) in &seen {
        if SHAPED_SYMBOLS.iter().any(|(s, _)| s == c) {
            continue;
        }
        if MONO_ONLY.iter().any(|(m, _)| m == c) {
            for (f, m) in &coverage {
                let mono = matches!(f, FontFamily::Monospace)
                    || *f == FontFamily::Name(font::MONO_MEDIUM.into());
                if mono
                    && !m
                        .get(c)
                        .is_some_and(|fonts| fonts.iter().any(|n| n.starts_with("JetBrainsMono")))
                {
                    missing.push(format!(
                        "  {c:?} U+{:04X} — MONO_ONLY but not in JetBrains Mono ({f:?})",
                        *c as u32
                    ));
                }
            }
            continue;
        }
        let lacking: Vec<String> = coverage
            .iter()
            .filter(|(_, m)| !m.contains_key(c))
            .map(|(f, _)| format!("{f:?}"))
            .collect();
        let where_ = at.join(", ");
        if !lacking.is_empty() {
            missing.push(format!(
                "  {c:?} U+{:04X} — no glyph in {}; used at {where_}",
                *c as u32,
                lacking.join(", ")
            ));
            continue;
        }
        let bundled = coverage.iter().any(|(_, m)| {
            m.get(c)
                .is_some_and(|fonts| fonts.iter().any(|n| BUNDLED.contains(&n.as_str())))
        });
        if !bundled && !FALLBACK_OK.contains(c) {
            let by: Vec<String> = coverage
                .iter()
                .flat_map(|(_, m)| m.get(c).cloned().unwrap_or_default())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            fallback_only.push(format!(
                "  {c:?} U+{:04X} — only in egui's fallback fonts ({}); used at {where_}",
                *c as u32,
                by.join(", ")
            ));
        }
    }
    eprintln!("{} distinct non-ASCII characters checked", seen.len());
    // the shaped symbols really are missing from the fonts (else drop them from the list)
    for (c, _) in SHAPED_SYMBOLS {
        assert!(
            coverage.iter().any(|(_, m)| !m.contains_key(c)),
            "{c:?} renders in every family now — draw it as text again"
        );
    }
    assert!(
        missing.is_empty() && fallback_only.is_empty(),
        "characters the bundled fonts cannot draw (tofu boxes in the app):\n{}\n{}\n\
         Draw the symbol as a shape, spell it out, or (for fallback-only symbols that look \
         right) add it to FALLBACK_OK.",
        missing.join("\n"),
        fallback_only.join("\n")
    );
}

#[test]
fn the_literal_scanner_reads_rust_strings_and_chars() {
    let src = r####"
// a comment with ✨ is ignored
fn f<'a>(x: &'a str) {
    let s = "ohm Ω and \u{2192} arrow";
    let r = r#"raw “quotes”"#;
    let c = '°';
    let e = '\n';
    /* block ⚠ comment */
    let b = b"bytes";
}
#[cfg(test)]
mod tests { const X: &str = "☃"; }
"####;
    let lits: Vec<String> = rust_literals(src).into_iter().map(|(_, s)| s).collect();
    assert!(lits.contains(&"ohm Ω and → arrow".to_string()), "{lits:?}");
    assert!(lits.contains(&"raw “quotes”".to_string()), "{lits:?}");
    assert!(lits.contains(&"°".to_string()), "{lits:?}");
    let all: String = lits.concat();
    assert!(!all.contains('✨') && !all.contains('⚠') && !all.contains('☃'));
}
