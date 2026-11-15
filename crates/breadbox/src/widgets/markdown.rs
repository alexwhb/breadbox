//! A small Markdown renderer for in-app docs (`docs/PLUGIN-SDK.md` behind "Plugin SDK
//! docs"): headings, paragraphs with `inline code` / **bold** / *italic* / [links](…),
//! bullet and numbered lists, block quotes, fenced code blocks (with SPICE / C++ / JSON-ish
//! highlighting) and pipe tables (monospace grid).

use crate::theme::{font, pal};
use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextFormat, Ui, text::LayoutJob};

/// Block structure of a Markdown document.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading(u8, String),
    Para(String),
    Bullet(u8, String),
    Numbered(String, String),
    Quote(String),
    Code(String, String),
    Table(Vec<Vec<String>>),
    Rule,
}

/// Parses Markdown into blocks.
pub fn parse(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let mut lines = text.lines().peekable();
    let flush = |para: &mut Vec<String>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Para(para.join(" ")));
            para.clear();
        }
    };
    while let Some(line) = lines.next() {
        let t = line.trim_end();
        let trimmed = t.trim_start();
        if let Some(lang) = trimmed.strip_prefix("```") {
            flush(&mut para, &mut out);
            let mut code = Vec::new();
            for l in lines.by_ref() {
                if l.trim_start().starts_with("```") {
                    break;
                }
                code.push(l.to_string());
            }
            out.push(Block::Code(lang.trim().to_string(), code.join("\n")));
        } else if trimmed.is_empty() {
            flush(&mut para, &mut out);
        } else if let Some(h) = heading(trimmed) {
            flush(&mut para, &mut out);
            out.push(h);
        } else if trimmed == "---" || trimmed == "***" {
            flush(&mut para, &mut out);
            out.push(Block::Rule);
        } else if trimmed.starts_with('|') {
            flush(&mut para, &mut out);
            let mut rows = vec![cells(trimmed)];
            while let Some(n) = lines.peek() {
                let n = n.trim();
                if !n.starts_with('|') {
                    break;
                }
                let r = cells(n);
                if !r
                    .iter()
                    .all(|c| c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' '))
                {
                    rows.push(r);
                }
                lines.next();
            }
            out.push(Block::Table(rows));
        } else if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            flush(&mut para, &mut out);
            let indent = ((t.len() - trimmed.len()) / 2).min(3) as u8;
            let mut item = rest.to_string();
            // continuation lines
            while let Some(n) = lines.peek() {
                let nt = n.trim();
                let ind = n.len() - n.trim_start().len();
                if nt.is_empty()
                    || nt.starts_with("- ")
                    || nt.starts_with("* ")
                    || nt.starts_with("```")
                    || ind < 2
                {
                    break;
                }
                item.push(' ');
                item.push_str(nt);
                lines.next();
            }
            out.push(Block::Bullet(indent, item));
        } else if let Some((num, rest)) = numbered(trimmed) {
            flush(&mut para, &mut out);
            out.push(Block::Numbered(num, rest));
        } else if let Some(q) = trimmed.strip_prefix('>') {
            flush(&mut para, &mut out);
            out.push(Block::Quote(q.trim().to_string()));
        } else {
            para.push(trimmed.to_string());
        }
    }
    flush(&mut para, &mut out);
    out
}

fn heading(t: &str) -> Option<Block> {
    let n = t.chars().take_while(|c| *c == '#').count();
    (1..=6)
        .contains(&n)
        .then(|| Block::Heading(n as u8, t[n..].trim().to_string()))
        .filter(|_| t.as_bytes().get(n) == Some(&b' '))
}

fn numbered(t: &str) -> Option<(String, String)> {
    let d: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if d.is_empty() {
        return None;
    }
    let rest = t[d.len()..].strip_prefix(". ")?;
    Some((d, rest.to_string()))
}

fn cells(row: &str) -> Vec<String> {
    row.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|c| c.trim().to_string())
        .collect()
}

/// Inline spans → a layout job (`code`, **bold**, *italic*, [text](url)).
pub fn inline(
    text: &str,
    size: f32,
    color: Color32,
    code_color: Color32,
    code_bg: Color32,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let plain = TextFormat {
        font_id: font::ui(size),
        color,
        ..Default::default()
    };
    let mut buf = String::new();
    let mut chars = text.chars().peekable();
    let push = |job: &mut LayoutJob, s: &mut String, f: &TextFormat| {
        if !s.is_empty() {
            job.append(s, 0.0, f.clone());
            s.clear();
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '`' => {
                push(&mut job, &mut buf, &plain);
                let mut code = String::new();
                for n in chars.by_ref() {
                    if n == '`' {
                        break;
                    }
                    code.push(n);
                }
                job.append(
                    &code,
                    0.0,
                    TextFormat {
                        font_id: font::mono(size - 1.0),
                        color: code_color,
                        background: code_bg,
                        ..Default::default()
                    },
                );
            }
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                push(&mut job, &mut buf, &plain);
                let mut b = String::new();
                while let Some(n) = chars.next() {
                    if n == '*' && chars.peek() == Some(&'*') {
                        chars.next();
                        break;
                    }
                    b.push(n);
                }
                job.append(
                    &b,
                    0.0,
                    TextFormat {
                        font_id: font::semibold(size),
                        color,
                        ..Default::default()
                    },
                );
            }
            '[' => {
                // [text](url) → text in link colour
                let rest: String = chars.clone().collect();
                if let Some(close) = rest.find("](")
                    && let Some(end) = rest[close..].find(')')
                {
                    push(&mut job, &mut buf, &plain);
                    let label = &rest[..close];
                    job.append(
                        label,
                        0.0,
                        TextFormat {
                            font_id: font::ui(size),
                            color: Color32::from_rgb(0x4C, 0xC3, 0xF0),
                            ..Default::default()
                        },
                    );
                    for _ in 0..close + end + 1 {
                        chars.next();
                    }
                } else {
                    buf.push(c);
                }
            }
            _ => buf.push(c),
        }
    }
    push(&mut job, &mut buf, &plain);
    job
}

/// A document made safe for the bundled fonts: instrument symbols spelled out
/// (`instrument::glyph_safe`, `V⎓` → `V DC`) and any other character the text or code font
/// can't draw written as `\u{…}` instead of a tofu box.
pub fn displayable(ctx: &egui::Context, text: &str) -> String {
    let text = crate::widgets::instrument::glyph_safe(text);
    let (ui_font, code) = (font::ui(13.0), font::mono(12.0));
    ctx.fonts_mut(|f| {
        text.chars()
            .map(|c| {
                if c.is_ascii() || (f.has_glyph(&ui_font, c) && f.has_glyph(&code, c)) {
                    c.to_string()
                } else {
                    format!("\\u{{{:04x}}}", c as u32)
                }
            })
            .collect()
    })
}

/// Renders a Markdown document into `ui` (inside a scroll area of the caller).
pub fn show(ui: &mut Ui, text: &str) {
    let p = pal(ui);
    let width = ui.available_width();
    for b in parse(text) {
        match b {
            Block::Heading(n, t) => {
                let (size, fid): (f32, fn(f32) -> FontId) = match n {
                    1 => (26.0, font::heading),
                    2 => (19.0, font::heading),
                    3 => (15.0, font::semibold),
                    _ => (13.0, font::semibold),
                };
                ui.add_space(if n <= 2 { 14.0 } else { 8.0 });
                ui.label(
                    egui::RichText::new(strip_inline(&t))
                        .font(fid(size))
                        .color(p.text_strong),
                );
                if n <= 2 {
                    let (r, _) =
                        ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 0.0, p.border);
                }
                ui.add_space(4.0);
            }
            Block::Para(t) => {
                let mut job = inline(&t, 13.0, p.text_dim, p.accent_soft, p.bg_raised);
                job.wrap.max_width = width;
                ui.label(job);
                ui.add_space(6.0);
            }
            Block::Bullet(level, t) => {
                ui.horizontal_top(|ui| {
                    ui.add_space(8.0 + level as f32 * 16.0);
                    ui.label(
                        egui::RichText::new("•")
                            .font(font::ui(13.0))
                            .color(p.text_muted),
                    );
                    let mut job = inline(&t, 13.0, p.text_dim, p.accent_soft, p.bg_raised);
                    job.wrap.max_width = ui.available_width();
                    // a label in a horizontal layout extends instead of wrapping
                    ui.add(egui::Label::new(job).wrap());
                });
            }
            Block::Numbered(n, t) => {
                ui.horizontal_top(|ui| {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!("{n}."))
                            .font(font::mono(12.0))
                            .color(p.text_muted),
                    );
                    let mut job = inline(&t, 13.0, p.text_dim, p.accent_soft, p.bg_raised);
                    job.wrap.max_width = ui.available_width();
                    // a label in a horizontal layout extends instead of wrapping
                    ui.add(egui::Label::new(job).wrap());
                });
            }
            Block::Quote(t) => {
                egui::Frame::new()
                    .fill(p.accent_bg2)
                    .stroke(Stroke::new(1.0, p.accent_border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        let mut job = inline(&t, 13.0, p.accent_soft, p.accent_soft, p.bg_raised);
                        job.wrap.max_width = ui.available_width();
                        ui.label(job);
                    });
                ui.add_space(6.0);
            }
            Block::Code(lang, code) => {
                egui::Frame::new()
                    .fill(p.bg_card)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(width - 22.0);
                        // long lines scroll sideways instead of widening the page
                        egui::ScrollArea::horizontal()
                            .id_salt(ui.next_auto_id())
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                let l = lang.to_ascii_lowercase();
                                if l == "json" || l == "jsonc" {
                                    ui.add(egui::Label::new(json_job(&code, p)).extend());
                                } else {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(code)
                                                .font(font::mono(12.0))
                                                .color(p.code_text),
                                        )
                                        .extend(),
                                    );
                                }
                            });
                    });
                ui.add_space(8.0);
            }
            Block::Table(rows) => {
                let ncol = rows.iter().map(|r| r.len()).max().unwrap_or(0);
                egui::Frame::new()
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        // a wide table scrolls sideways instead of widening the page
                        egui::ScrollArea::horizontal()
                            .id_salt(ui.next_auto_id())
                            .max_width(width - 18.0)
                            .show(ui, |ui| {
                                egui::Grid::new(ui.next_auto_id())
                                    .spacing(egui::vec2(16.0, 4.0))
                                    .show(ui, |ui| {
                                        for (i, r) in rows.iter().enumerate() {
                                            for c in 0..ncol {
                                                let t = r
                                                    .get(c)
                                                    .map(|s| strip_inline(s))
                                                    .unwrap_or_default();
                                                ui.label(
                                                    egui::RichText::new(t)
                                                        .font(if i == 0 {
                                                            font::medium(12.0)
                                                        } else {
                                                            font::mono(12.0)
                                                        })
                                                        .color(if i == 0 {
                                                            p.text_muted
                                                        } else {
                                                            p.text_dim
                                                        }),
                                                );
                                            }
                                            ui.end_row();
                                        }
                                    });
                            });
                    });
                ui.add_space(8.0);
            }
            Block::Rule => {
                ui.add_space(6.0);
                let (r, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 0.0, p.border);
                ui.add_space(6.0);
            }
        }
    }
}

fn strip_inline(t: &str) -> String {
    t.replace("**", "").replace('`', "")
}

/// JSON with the design's colours (keys blue, strings green, numbers amber).
pub fn json_job(text: &str, p: &crate::theme::Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    let fmt = |c: Color32| TextFormat {
        font_id: font::mono(12.0),
        color: c,
        line_height: Some(18.0),
        ..Default::default()
    };
    let key = Color32::from_rgb(0x82, 0xAA, 0xFF);
    let string = Color32::from_rgb(0xB5, 0xD8, 0x8A);
    let num = Color32::from_rgb(0xF7, 0xC7, 0x7A);
    let punct = p.text_dim;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut plain = String::new();
    let flush = |job: &mut LayoutJob, s: &mut String| {
        if !s.is_empty() {
            job.append(s, 0.0, fmt(punct));
            s.clear();
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            flush(&mut job, &mut plain);
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(chars.len());
            let s: String = chars[start..i].iter().collect();
            // a key is followed by ':'
            let mut j = i;
            while j < chars.len() && chars[j] == ' ' {
                j += 1;
            }
            let is_key = j < chars.len() && chars[j] == ':';
            job.append(&s, 0.0, fmt(if is_key { key } else { string }));
        } else if c == '-' || c.is_ascii_digit() {
            flush(&mut job, &mut plain);
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || ".eE+-".contains(chars[i])) {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            job.append(&s, 0.0, fmt(num));
        } else if text[char_offset(&chars, i)..].starts_with("true")
            || text[char_offset(&chars, i)..].starts_with("false")
            || text[char_offset(&chars, i)..].starts_with("null")
        {
            flush(&mut job, &mut plain);
            let w = if c == 'f' { 5 } else { 4 };
            let s: String = chars[i..(i + w).min(chars.len())].iter().collect();
            job.append(&s, 0.0, fmt(num));
            i += w;
        } else {
            plain.push(c);
            i += 1;
        }
    }
    flush(&mut job, &mut plain);
    job
}

fn char_offset(chars: &[char], i: usize) -> usize {
    chars[..i].iter().map(|c| c.len_utf8()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_sdk_doc_shapes() {
        let b = parse(
            "# Title\n\nSome `code` and **bold**.\n\n- one\n- two\n\n```rust\nfn x() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        );
        assert_eq!(b[0], Block::Heading(1, "Title".into()));
        assert!(matches!(&b[1], Block::Para(t) if t.contains("`code`")));
        assert_eq!(b[2], Block::Bullet(0, "one".into()));
        assert_eq!(b[4], Block::Code("rust".into(), "fn x() {}".into()));
        assert_eq!(
            b[5],
            Block::Table(vec![
                vec!["a".into(), "b".into()],
                vec!["1".into(), "2".into()]
            ])
        );
        let sdk = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/PLUGIN-SDK.md"),
        )
        .unwrap();
        let blocks = parse(&sdk);
        assert!(blocks.iter().any(|b| matches!(b, Block::Code(..))));
        assert!(blocks.len() > 20);
    }

    #[test]
    fn json_highlight_keeps_the_text() {
        let t = "{\n  \"id\": \"community.dmm-handheld\",\n  \"probes\": 2,\n  \"ok\": true\n}";
        let job = json_job(t, &crate::theme::DARK);
        assert_eq!(job.text, t);
        assert!(job.sections.len() > 6);
    }
}
