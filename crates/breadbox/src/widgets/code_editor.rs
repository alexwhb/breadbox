//! Code editor: line-number gutter, breakpoint gutter dots, executing-line highlight (amber
//! band + 3 px bar, design line 30), cursor-line tint, syntax highlighting (Arduino C++,
//! SPICE), diagnostics with squiggly underlines + hover messages, find bar, and a tab header
//! for multi-file sketches. Built on `egui::TextEdit` with a custom layouter, so editing,
//! selection, clipboard and undo are egui's.

use super::controls::{Btn, galley, tab_sized};
use super::highlight::{Lang, Tok, highlight};
use super::icons::Icon;
use crate::theme::{Palette, font, pal};
use eframe::egui::{
    self, Color32, CornerRadius, FontId, Id, Margin, Rect, Sense, Stroke, TextFormat, Ui,
    UiBuilder, pos2, text::LayoutJob, vec2,
};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

/// Severity of an editor diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagSeverity {
    Error,
    Warning,
    Note,
}

/// A compiler / parser message attached to a line (1-based line and column).
#[derive(Debug, Clone, PartialEq)]
pub struct EditorDiagnostic {
    pub line: u32,
    pub col: Option<u32>,
    pub severity: DiagSeverity,
    pub message: String,
}

/// One open file.
#[derive(Debug, Clone)]
pub struct CodeDoc {
    /// Tab name (`garden-node.ino`).
    pub name: String,
    pub path: Option<PathBuf>,
    pub text: String,
    pub lang: Lang,
    pub dirty: bool,
    pub read_only: bool,
    /// 1-based lines with a breakpoint.
    pub breakpoints: BTreeSet<u32>,
    pub diagnostics: Vec<EditorDiagnostic>,
    /// Bumped on every edit (for debounced autosave / re-parse).
    pub version: u64,
}

impl CodeDoc {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        let name = name.into();
        CodeDoc {
            lang: Lang::for_file(&name),
            name,
            path: None,
            text: text.into(),
            dirty: false,
            read_only: false,
            breakpoints: BTreeSet::new(),
            diagnostics: Vec::new(),
            version: 0,
        }
    }

    pub fn line_count(&self) -> usize {
        self.text.split('\n').count()
    }

    /// Toggles a breakpoint on a 1-based line; returns the new state.
    pub fn toggle_breakpoint(&mut self, line: u32) -> bool {
        if !self.breakpoints.remove(&line) {
            self.breakpoints.insert(line);
            true
        } else {
            false
        }
    }
}

/// Find bar state.
#[derive(Debug, Clone, Default)]
pub struct FindState {
    pub open: bool,
    pub query: String,
    /// Index of the current match.
    pub current: usize,
    pub match_count: usize,
    pub focus: bool,
    pending_jump: Option<i32>,
}

/// Per-editor view state owned by the screen (shared by its tabs).
#[derive(Debug, Clone, Default)]
pub struct EditorView {
    pub find: FindState,
    /// Scroll so this 1-based line is visible (consumed).
    pub scroll_to_line: Option<u32>,
    /// Cursor (1-based line, 1-based column) after the last frame.
    pub cursor: (u32, u32),
}

impl EditorView {
    pub fn open_find(&mut self) {
        self.find.open = true;
        self.find.focus = true;
    }
}

/// What happened this frame.
#[derive(Debug, Clone, Default)]
pub struct EditorOutput {
    pub changed: bool,
    /// A gutter click toggled a breakpoint on this 1-based line.
    pub toggled_breakpoint: Option<u32>,
    pub cursor: (u32, u32),
    pub has_focus: bool,
}

/// Editor options for one `show` call.
pub struct CodeEditor<'a> {
    pub id_salt: &'a str,
    /// Clicking the gutter toggles breakpoints.
    pub breakpoint_gutter: bool,
    /// Executing line (1-based) — amber band.
    pub exec_line: Option<u32>,
    pub font_size: f32,
    pub line_height: f32,
}

impl<'a> CodeEditor<'a> {
    pub fn new(id_salt: &'a str) -> Self {
        CodeEditor {
            id_salt,
            breakpoint_gutter: false,
            exec_line: None,
            font_size: 13.0,
            line_height: 20.0,
        }
    }
    pub fn breakpoints(mut self, on: bool) -> Self {
        self.breakpoint_gutter = on;
        self
    }
    pub fn exec_line(mut self, line: Option<u32>) -> Self {
        self.exec_line = line;
        self
    }
    pub fn font(mut self, size: f32, line_height: f32) -> Self {
        self.font_size = size;
        self.line_height = line_height;
        self
    }

    /// Shows the editor filling the available rect.
    pub fn show(self, ui: &mut Ui, doc: &mut CodeDoc, view: &mut EditorView) -> EditorOutput {
        let p = pal(ui);
        let mut out = EditorOutput::default();
        let outer = ui.available_rect_before_wrap();
        ui.painter().rect_filled(outer, 0.0, p.bg_canvas);
        let id = Id::unique(("code-editor", self.id_salt, &doc.name));

        // shortcuts while this editor's text has focus
        let te_focused = ui.memory(|m| m.has_focus(id));
        if te_focused || view.find.open {
            let (cmd_f, esc) = ui.input(|i| {
                (
                    i.modifiers.command && i.key_pressed(egui::Key::F),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if cmd_f {
                view.open_find();
            }
            if esc && view.find.open {
                view.find.open = false;
                ui.memory_mut(|m| m.request_focus(id));
            }
        }

        let mut rect = outer;
        if view.find.open {
            let bar = Rect::from_min_size(rect.min, vec2(rect.width(), 36.0));
            rect.min.y += 36.0;
            let mut child = super::controls::child_ui(ui, bar.shrink2(vec2(12.0, 4.0)));
            find_bar(&mut child, p, &mut view.find);
            ui.painter().line_segment(
                [pos2(bar.left(), bar.bottom()), pos2(bar.right(), bar.bottom())],
                Stroke::new(1.0, p.border),
            );
        }

        let font = font::mono(self.font_size);
        let lh = self.line_height;
        let char_w = galley(ui, "0000000000", font.clone(), p.text).size().x / 10.0;
        let line_starts = line_starts(&doc.text);
        let n_lines = line_starts.len();
        let digits = n_lines.to_string().len().max(2) as f32;
        let dot_lane = if self.breakpoint_gutter { 20.0 } else { 16.0 };
        let gutter_w = dot_lane + digits * char_w + 12.0;
        let top_pad = 14.0;
        let max_cols = doc
            .text
            .split('\n')
            .map(|l| l.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum::<usize>())
            .max()
            .unwrap_or(0) as f32;

        // find matches (byte ranges)
        let matches: Vec<std::ops::Range<usize>> = if view.find.open && !view.find.query.is_empty() {
            find_all(&doc.text, &view.find.query)
        } else {
            Vec::new()
        };
        view.find.match_count = matches.len();
        if view.find.current >= matches.len() {
            view.find.current = 0;
        }

        let mut child = super::controls::child_ui(ui, rect);
        let scroll = egui::ScrollArea::both()
            .id_salt(id.with("scroll"))
            .auto_shrink([false, false]);
        let lang = doc.lang;
        let text_color = p.code_text;
        let colors = TokColors::from(p);
        let font_c = font.clone();
        let mut layouter = move |ui: &Ui, buf: &dyn egui::TextBuffer, _wrap: f32| -> Arc<egui::Galley> {
            let text = buf.as_str();
            let job = layout_job(text, lang, &font_c, lh, text_color, &colors);
            ui.painter().layout_job(job)
        };

        let exec_line = self.exec_line;
        let bp_gutter = self.breakpoint_gutter;
        let scroll_to = view.scroll_to_line.take();
        let pending_jump = view.find.pending_jump.take();
        let mut jump_to_match: Option<usize> = None;
        if let Some(d) = pending_jump
            && !matches.is_empty()
        {
            let n = matches.len() as i32;
            let cur = view.find.current as i32;
            let next = ((cur + d) % n + n) % n;
            view.find.current = next as usize;
            jump_to_match = Some(next as usize);
        }
        let current_match = view.find.current;
        let read_only = doc.read_only;

        scroll.show_viewport(&mut child, |ui, viewport| {
            let origin = ui.cursor().min;
            let content_w = (gutter_w + 32.0 + max_cols * char_w).max(viewport.width());
            let content_h = top_pad * 2.0 + n_lines as f32 * lh;
            let (all, _) = ui.allocate_exact_size(vec2(content_w, content_h.max(viewport.height())), Sense::hover());
            let code_left = origin.x + gutter_w;
            let painter = ui.painter().clone();
            // backgrounds go into this slot, under the text painted by the TextEdit below
            let bg_slot = painter.add(egui::Shape::Noop);

            // ---- text
            let text_rect = Rect::from_min_max(pos2(code_left, origin.y), pos2(all.right(), origin.y + content_h.max(viewport.height())));
            let te = egui::TextEdit::multiline(&mut doc.text)
                .id(id)
                .font(font.clone())
                .code_editor()
                .frame(egui::Frame::NONE)
                .background_color(Color32::TRANSPARENT)
                .margin(Margin {
                    left: 16,
                    right: 16,
                    top: top_pad as i8,
                    bottom: top_pad as i8,
                })
                .desired_width(text_rect.width())
                .min_size(text_rect.size())
                .lock_focus(true)
                .interactive(!read_only)
                .layouter(&mut layouter);
            let output = ui
                .scope_builder(UiBuilder::new().max_rect(text_rect), |ui| te.show(ui))
                .inner;
            // geometry from the laid-out galley (exact, whatever margins egui applied)
            let text_x = output.galley_pos.x;
            let text_y = output.galley_pos.y;
            let row_top = |line0: usize| text_y + line0 as f32 * lh;
            let first = ((viewport.top() - (text_y - origin.y)) / lh).floor().max(0.0) as usize;
            let last = (((viewport.bottom() - (text_y - origin.y)) / lh).ceil() as usize + 1).min(n_lines);

            // ---- backgrounds
            let mut bg: Vec<egui::Shape> = Vec::new();
            let cursor_line0 = view.cursor.0.saturating_sub(1) as usize;
            if te_focused && cursor_line0 < n_lines && exec_line != Some(view.cursor.0) {
                let y = row_top(cursor_line0);
                bg.push(egui::Shape::rect_filled(
                    Rect::from_min_max(pos2(code_left, y), pos2(all.right(), y + lh)),
                    0.0,
                    p.code_cursor_line,
                ));
            }
            if let Some(l) = exec_line {
                let l0 = l.saturating_sub(1) as usize;
                if l0 < n_lines {
                    let y = row_top(l0);
                    let band = Rect::from_min_max(pos2(code_left, y), pos2(all.right(), y + lh));
                    bg.push(egui::Shape::rect_filled(band, 0.0, p.code_current));
                    bg.push(egui::Shape::rect_filled(
                        Rect::from_min_max(band.min, pos2(band.left() + 3.0, band.bottom())),
                        0.0,
                        p.accent,
                    ));
                }
            }
            for (k, m) in matches.iter().enumerate() {
                let (l0, c0) = line_col(&doc.text, &line_starts, m.start);
                if l0 < first || l0 > last {
                    continue;
                }
                let cols = visual_cols(&doc.text[m.clone()]);
                let y = row_top(l0);
                let x = text_x + c0 as f32 * char_w;
                let r = Rect::from_min_size(pos2(x - 1.0, y + 1.0), vec2(cols as f32 * char_w + 2.0, lh - 2.0));
                let on = k == current_match;
                bg.push(egui::Shape::Rect(egui::epaint::RectShape::new(
                    r,
                    CornerRadius::same(3),
                    if on { p.accent_bg } else { p.bg_selected },
                    if on { Stroke::new(1.0, p.accent) } else { Stroke::NONE },
                    egui::StrokeKind::Inside,
                )));
            }
            painter.set(bg_slot, egui::Shape::Vec(bg));

            // ---- gutter: one accessible target per visible line ("Breakpoint line 8")
            let mut hover_line = None;
            for l0 in first..last {
                let y = row_top(l0);
                let line = l0 as u32 + 1;
                let lr = Rect::from_min_max(pos2(origin.x, y), pos2(code_left - 2.0, y + lh));
                let lresp = ui.interact(lr, id.with(("gutter", l0)), if bp_gutter { Sense::click() } else { Sense::hover() });
                if bp_gutter {
                    let label = format!("Breakpoint line {line}");
                    let on = doc.breakpoints.contains(&line);
                    lresp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, &label));
                }
                if lresp.hovered() {
                    hover_line = Some(line);
                }
                if bp_gutter && lresp.clicked() {
                    doc.toggle_breakpoint(line);
                    out.toggled_breakpoint = Some(line);
                }
                if bp_gutter {
                    lresp.on_hover_cursor(egui::CursorIcon::PointingHand);
                }
            }
            for l0 in first..last {
                let y = row_top(l0);
                let line = l0 as u32 + 1;
                let is_exec = exec_line == Some(line);
                let num_color = if is_exec { p.accent_soft } else { p.text_ghost };
                let g = galley(ui, line.to_string(), font.clone(), num_color);
                painter.galley(
                    pos2(code_left - 12.0 - g.size().x, y + (lh - g.size().y) / 2.0),
                    g,
                    num_color,
                );
                let cy = y + lh / 2.0;
                let cx = origin.x + 10.0;
                if doc.breakpoints.contains(&line) {
                    painter.circle_filled(pos2(cx, cy), 4.5, p.danger_soft);
                } else if bp_gutter && hover_line == Some(line) {
                    painter.circle_filled(pos2(cx, cy), 4.5, p.danger_soft.gamma_multiply(0.35));
                }
                if let Some(d) = doc.diagnostics.iter().find(|d| d.line == line) {
                    let c = diag_color(p, d.severity);
                    painter.rect_filled(
                        Rect::from_min_size(pos2(code_left - 4.0, y + 3.0), vec2(2.0, lh - 6.0)),
                        0.0,
                        c,
                    );
                }
            }
            let resp = &output.response;
            if resp.changed() {
                out.changed = true;
            }
            out.has_focus = resp.has_focus();
            // cursor
            if let Some(r) = output.cursor_range {
                let ci = r.primary.index.0;
                let byte = char_to_byte(&doc.text, ci);
                let (l0, c0) = line_col(&doc.text, &line_starts_of(&doc.text), byte);
                out.cursor = (l0 as u32 + 1, c0 as u32 + 1);
            }
            // jump to a find match
            if let Some(k) = jump_to_match
                && let Some(m) = matches.get(k)
            {
                let a = doc.text[..m.start].chars().count();
                let b = a + doc.text[m.clone()].chars().count();
                let mut st = output.state.clone();
                st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(a),
                    egui::text::CCursor::new(b),
                )));
                st.store(ui.ctx(), id);
                let (l0, _) = line_col(&doc.text, &line_starts, m.start);
                let y = row_top(l0);
                ui.scroll_to_rect(Rect::from_min_size(pos2(code_left, y - lh), vec2(10.0, lh * 3.0)), None);
            }
            if let Some(l) = scroll_to {
                let y = row_top(l.saturating_sub(1) as usize);
                ui.scroll_to_rect(
                    Rect::from_min_size(pos2(code_left, y - lh * 2.0), vec2(10.0, lh * 5.0)),
                    Some(egui::Align::Center),
                );
            }

            // ---- diagnostics squiggles + hover
            for (k, d) in doc.diagnostics.iter().enumerate() {
                let l0 = d.line.saturating_sub(1) as usize;
                if l0 >= n_lines || l0 < first || l0 > last {
                    continue;
                }
                let ls = line_starts[l0];
                let le = line_starts.get(l0 + 1).map(|e| e - 1).unwrap_or(doc.text.len());
                let line_text = &doc.text[ls..le.max(ls)];
                let (c_from, c_to) = squiggle_cols(line_text, d.col);
                let y = row_top(l0) + lh - 3.0;
                let x0 = text_x + c_from as f32 * char_w;
                let x1 = text_x + c_to.max(c_from + 1) as f32 * char_w;
                let c = diag_color(p, d.severity);
                let mut pts = Vec::new();
                let mut x = x0;
                let mut up = true;
                while x <= x1 {
                    pts.push(pos2(x, if up { y } else { y + 2.0 }));
                    x += 2.0;
                    up = !up;
                }
                if pts.len() > 1 {
                    painter.add(egui::Shape::line(pts, Stroke::new(1.0, c)));
                }
                let hit = Rect::from_min_max(pos2(x0, row_top(l0)), pos2(x1, row_top(l0) + lh));
                let sev = match d.severity {
                    DiagSeverity::Error => "error",
                    DiagSeverity::Warning => "warning",
                    DiagSeverity::Note => "note",
                };
                let msg = format!("{sev}: {}", d.message);
                ui.interact(hit, id.with(("diag", k)), Sense::hover())
                    .on_hover_text(msg);
            }
        });
        if !out.changed {
            out.cursor = if out.cursor == (0, 0) { view.cursor } else { out.cursor };
        } else {
            doc.dirty = true;
            doc.version += 1;
        }
        if out.cursor != (0, 0) {
            view.cursor = out.cursor;
        }
        out
    }
}

/// Colour table for tokens.
#[derive(Clone, Copy)]
struct TokColors {
    kw: Color32,
    ty: Color32,
    fun: Color32,
    num: Color32,
    str: Color32,
    cm: Color32,
    pp: Color32,
    model: Color32,
}

impl From<&Palette> for TokColors {
    fn from(p: &Palette) -> Self {
        TokColors {
            kw: p.code_kw,
            ty: p.code_ty,
            fun: p.code_fn,
            num: p.code_num,
            str: p.code_str,
            cm: p.code_cm,
            pp: p.code_pp,
            model: p.info,
        }
    }
}

fn layout_job(text: &str, lang: Lang, font: &FontId, lh: f32, plain: Color32, c: &TokColors) -> LayoutJob {
    let mut job = LayoutJob::default();
    let fmt = |color: Color32, italics: bool| TextFormat {
        font_id: font.clone(),
        color,
        italics,
        line_height: Some(lh),
        ..Default::default()
    };
    let mut pos = 0;
    for (r, t) in highlight(lang, text) {
        if r.start > pos {
            job.append(&text[pos..r.start], 0.0, fmt(plain, false));
        }
        let (color, it) = match t {
            Tok::Keyword | Tok::Directive => (c.kw, false),
            Tok::Type => (c.ty, false),
            Tok::Function => (c.fun, false),
            Tok::Number => (c.num, false),
            Tok::String => (c.str, false),
            Tok::Comment => (c.cm, true),
            Tok::Preproc => (c.pp, false),
            Tok::Model => (c.model, false),
            Tok::Text => (plain, false),
        };
        job.append(&text[r.clone()], 0.0, fmt(color, it));
        pos = r.end;
    }
    if pos < text.len() {
        job.append(&text[pos..], 0.0, fmt(plain, false));
    }
    if text.is_empty() {
        job.append("", 0.0, fmt(plain, false));
    }
    job.wrap.max_width = f32::INFINITY;
    job
}

fn diag_color(p: &Palette, s: DiagSeverity) -> Color32 {
    match s {
        DiagSeverity::Error => p.danger_soft,
        DiagSeverity::Warning => p.accent,
        DiagSeverity::Note => p.info,
    }
}

/// Byte offsets of line starts.
pub fn line_starts(text: &str) -> Vec<usize> {
    line_starts_of(text)
}

fn line_starts_of(text: &str) -> Vec<usize> {
    let mut v = vec![0];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            v.push(i + 1);
        }
    }
    v
}

/// (0-based line, 0-based visual column) of a byte offset.
fn line_col(text: &str, starts: &[usize], byte: usize) -> (usize, usize) {
    let l = match starts.binary_search(&byte) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    };
    let s = starts[l].min(text.len());
    let e = byte.min(text.len()).max(s);
    (l, visual_cols(&text[s..e]))
}

fn visual_cols(s: &str) -> usize {
    s.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

fn char_to_byte(text: &str, ci: usize) -> usize {
    text.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(text.len())
}

/// Columns to underline: from the reported column to the end of the token there, or the whole
/// trimmed line when no column is known.
fn squiggle_cols(line: &str, col: Option<u32>) -> (usize, usize) {
    let chars: Vec<char> = line.chars().collect();
    match col {
        Some(c) if c >= 1 && (c as usize) <= chars.len().max(1) => {
            let s = (c - 1) as usize;
            let mut e = s;
            while e < chars.len() && (chars[e].is_alphanumeric() || chars[e] == '_') {
                e += 1;
            }
            if e == s {
                e = (s + 1).min(chars.len().max(s + 1));
            }
            (s, e)
        }
        _ => {
            let s = chars.iter().take_while(|c| c.is_whitespace()).count();
            (s, chars.len().max(s + 1))
        }
    }
}

/// Case-insensitive occurrences of `q` (byte ranges).
pub fn find_all(text: &str, q: &str) -> Vec<std::ops::Range<usize>> {
    if q.is_empty() {
        return Vec::new();
    }
    let lt = text.to_lowercase();
    let lq = q.to_lowercase();
    if lt.len() != text.len() {
        // non-ASCII case mapping changed lengths: fall back to exact search
        return text.match_indices(q).map(|(i, m)| i..i + m.len()).collect();
    }
    lt.match_indices(&lq).map(|(i, m)| i..i + m.len()).collect()
}

fn find_bar(ui: &mut Ui, p: &Palette, f: &mut FindState) {
    ui.horizontal_centered(|ui| {
        Icon::Search.show(ui, 14.0, p.text_muted);
        let r = ui.add(
            egui::TextEdit::singleline(&mut f.query)
                .id_salt("find-query")
                .hint_text("Find")
                .font(font::mono(12.5))
                .desired_width(220.0),
        );
        if f.focus {
            r.request_focus();
            f.focus = false;
        }
        if r.changed() {
            f.current = 0;
            f.pending_jump = Some(0);
        }
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if enter {
            let shift = ui.input(|i| i.modifiers.shift);
            f.pending_jump = Some(if shift { -1 } else { 1 });
            r.request_focus();
        }
        let label = if f.query.is_empty() {
            String::new()
        } else if f.match_count == 0 {
            "no matches".to_string()
        } else {
            format!("{} of {}", f.current + 1, f.match_count)
        };
        ui.label(egui::RichText::new(label).font(font::mono(12.0)).color(p.text_muted));
        if Btn::plain("Previous match").icon_only(Icon::ChevronLeft).height(24.0).show(ui).clicked() {
            f.pending_jump = Some(-1);
        }
        if Btn::plain("Next match").icon_only(Icon::ChevronRight).height(24.0).show(ui).clicked() {
            f.pending_jump = Some(1);
        }
        if Btn::plain("Close find").icon_only(Icon::Close).height(24.0).show(ui).clicked() {
            f.open = false;
        }
    });
}

/// The 38 px editor header: file tabs on the left, status (`Ln 30, Col 9 · C++`) on the
/// right. Returns the clicked tab index.
pub fn editor_tabs(ui: &mut Ui, names: &[(String, bool)], active: usize, status: &str) -> Option<usize> {
    let p = pal(ui);
    let rect = ui.available_rect_before_wrap();
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), 38.0));
    ui.painter().rect_filled(bar, 0.0, p.bg_header);
    ui.painter().line_segment(
        [pos2(bar.left(), bar.bottom() - 0.5), pos2(bar.right(), bar.bottom() - 0.5)],
        Stroke::new(1.0, p.border),
    );
    let mut clicked = None;
    let mut child = super::controls::child_ui(ui, bar.shrink2(vec2(14.0, 0.0)));
    child.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        for (i, (name, dirty)) in names.iter().enumerate() {
            let label = if *dirty { format!("{name} •") } else { name.clone() };
            if tab_sized(ui, &label, i == active, 37.0, 12.0).clicked() {
                clicked = Some(i);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(status).font(font::mono(12.0)).color(p.text_muted));
        });
    });
    ui.allocate_rect(bar, Sense::hover());
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column_math() {
        let t = "ab\n\tcd\nxyz";
        let s = line_starts(t);
        assert_eq!(s, vec![0, 3, 7]);
        assert_eq!(line_col(t, &s, 0), (0, 0));
        assert_eq!(line_col(t, &s, 5), (1, 5)); // tab counts as 4 columns
        assert_eq!(line_col(t, &s, 9), (2, 2));
    }

    #[test]
    fn find_is_case_insensitive() {
        let m = find_all("Serial.begin(115200);\nserial.print", "serial");
        assert_eq!(m, vec![0..6, 22..28]);
        assert!(find_all("abc", "").is_empty());
    }

    #[test]
    fn squiggles_cover_the_token_at_the_column() {
        assert_eq!(squiggle_cols("  digitalWrit(13, HIGH);", Some(3)), (2, 13));
        assert_eq!(squiggle_cols("  foo();", None), (2, 8));
    }

    #[test]
    fn breakpoints_toggle() {
        let mut d = CodeDoc::new("blink.ino", "a\nb\n");
        assert!(d.toggle_breakpoint(2));
        assert!(d.breakpoints.contains(&2));
        assert!(!d.toggle_breakpoint(2));
        assert!(d.breakpoints.is_empty());
        assert_eq!(d.lang, Lang::Cpp);
    }
}
