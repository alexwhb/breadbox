//! Lessons (design screen 5, `LES-2`, `LES-5`, `LES-6`): the track nav with progress
//! checkmarks and "More tracks", the lesson's live bench (the Workbench's `BenchCanvas`
//! embedded over the lesson circuit and its own simulator), the step panel driven by
//! `bb_lessons::Session` (step bar, explanation, formula card, "Pick R1" choices, "Do the math
//! myself" → prediction + Check + hints + "Show me the working", "Your measurements" filled
//! from the meter, the damage card with Repair, quizzes, code steps with an embedded editor +
//! compile / flash, goals), `Lesson 3 of 8` progress, "Open in free play", and the lesson
//! editor ("Make your own lesson").

pub mod editor;
pub mod run;
mod step;

use crate::app::AppCtx;
use crate::router::{Route, Screen};
use crate::theme::{font, pal};
use crate::widgets::{self, Btn};
use bb_lessons::{Catalog, LoadedLesson, Progress};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use run::LessonRun;
use serde::{Deserialize, Serialize};

/// What the screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LessonsView {
    #[default]
    Lesson,
    /// "Make your own lesson".
    Editor,
}

/// Persisted state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LessonsState {
    pub track: Option<String>,
    pub lesson: Option<String>,
}

pub struct LessonsScreen {
    pub catalog: Option<Catalog>,
    catalog_gen: u64,
    pub progress: Option<Progress>,
    pub track: Option<String>,
    pub run: Option<LessonRun>,
    pub view: LessonsView,
    pub editor: editor::LessonEditor,
    saved_lesson: Option<String>,
    generation: u64,
    pub error: Option<String>,
}

impl Default for LessonsScreen {
    fn default() -> Self {
        LessonsScreen {
            catalog: None,
            catalog_gen: 0,
            progress: None,
            track: None,
            run: None,
            view: LessonsView::Lesson,
            editor: editor::LessonEditor::default(),
            saved_lesson: None,
            generation: 1000,
            error: None,
        }
    }
}

impl LessonsScreen {
    /// Built-in lessons + the active lesson packs (reloaded when the plugins change).
    pub fn ensure_catalog(&mut self, app: &mut AppCtx) {
        if self.catalog.is_some() && self.catalog_gen == app.plugins.generation {
            return;
        }
        let mut c = Catalog::builtin();
        let packs = app.plugins.host().lesson_dirs();
        c.add_packs(&packs);
        for e in &c.errors {
            log::warn!("lessons: {e}");
        }
        self.catalog = Some(c);
        self.catalog_gen = app.plugins.generation;
        if self.progress.is_none() {
            self.progress = Some(Progress::load(&app.progress_path).unwrap_or_else(|e| {
                log::warn!("lesson progress: {e}");
                Progress::default()
            }));
        }
    }

    fn lesson(&self, id: &str) -> Option<LoadedLesson> {
        self.catalog.as_ref()?.lesson(id).cloned()
    }

    /// Lessons of a track in order.
    fn track_lessons(&self, track: &str) -> Vec<LoadedLesson> {
        self.catalog
            .as_ref()
            .and_then(|c| c.track(track))
            .map(|t| t.lessons.clone())
            .unwrap_or_default()
    }

    /// Opens a lesson (resuming at the saved step).
    pub fn open_lesson(&mut self, app: &mut AppCtx, id: &str) {
        self.ensure_catalog(app);
        let Some(l) = self.lesson(id) else {
            self.error = Some(format!("No lesson `{id}`"));
            return;
        };
        let resume = self
            .progress
            .as_ref()
            .filter(|p| !p.is_done(id))
            .map(|p| p.resume(id))
            .unwrap_or(0)
            .min(l.lesson.steps.len().saturating_sub(1));
        self.open_at(app, &l, resume);
    }

    /// Opens a lesson at a step.
    pub fn open_at(&mut self, app: &mut AppCtx, l: &LoadedLesson, step: usize) {
        self.run = None; // stops the previous bench
        let bp = app.plugins.bench_plugins();
        self.generation += 1;
        match LessonRun::open(
            l,
            bp.lib,
            bp.hook,
            app.resources.fixtures(),
            app.repaint.clone(),
            app.bench.speed,
            step,
            self.generation,
        ) {
            Ok(r) => {
                self.track = Some(l.lesson.track.clone());
                self.saved_lesson = Some(l.lesson.id.clone());
                if let Some(p) = self.progress.as_mut() {
                    p.set_current(&l.lesson.id, r.session.step);
                    let _ = p.save();
                }
                self.run = Some(r);
                self.view = LessonsView::Lesson;
                self.error = None;
            }
            Err(e) => {
                self.error = Some(format!("{}: {e}", l.lesson.title));
                app.toasts.error(format!("Couldn't open the lesson: {e}"));
            }
        }
    }

    /// The lesson to show when nothing is open: the saved one, else the first unfinished
    /// lesson of the track.
    fn default_lesson(&self, track: Option<&str>) -> Option<String> {
        let c = self.catalog.as_ref()?;
        if track.is_none()
            && let Some(id) = &self.saved_lesson
            && c.lesson(id).is_some()
        {
            return Some(id.clone());
        }
        let t = match track {
            Some(t) => c.track(t)?,
            None => c.tracks.first()?,
        };
        let done = |id: &str| self.progress.as_ref().is_some_and(|p| p.is_done(id));
        t.lessons
            .iter()
            .find(|l| !done(&l.lesson.id))
            .or(t.lessons.first())
            .map(|l| l.lesson.id.clone())
    }

    fn current_track(&self) -> Option<bb_lessons::Track> {
        let c = self.catalog.as_ref()?;
        let id = self
            .run
            .as_ref()
            .map(|r| r.session.lesson.track.clone())
            .or_else(|| self.track.clone())?;
        c.track(&id).cloned()
    }

    /// `(n, of)` of the open lesson within its track.
    fn position(&self) -> Option<(usize, usize)> {
        let run = self.run.as_ref()?;
        let t = self.current_track()?;
        let i = t
            .lessons
            .iter()
            .position(|l| l.lesson.id == run.session.lesson.id)?;
        Some((i + 1, t.lessons.len()))
    }

    /// "Open in free play" (LES-5).
    pub fn free_play(&mut self, app: &mut AppCtx) -> Option<std::path::PathBuf> {
        let run = self.run.as_mut()?;
        let parent = crate::project::default_projects_dir();
        if let Err(e) = std::fs::create_dir_all(&parent) {
            app.toasts.error(format!("{}: {e}", parent.display()));
            return None;
        }
        match run.free_play(&parent) {
            Ok(dir) => match app.open_project(&dir) {
                Ok(()) => {
                    app.navigate(Route::Workbench);
                    app.toasts
                        .success(format!("Free play copy in {}", dir.display()));
                    Some(dir)
                }
                Err(e) => {
                    app.toasts.error(format!("{e:#}"));
                    None
                }
            },
            Err(e) => {
                app.toasts.error(e);
                None
            }
        }
    }

    // ------------------------------------------------------------------ track nav

    fn nav(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        let Some(track) = self.current_track() else {
            return;
        };
        let open = self.run.as_ref().map(|r| r.session.lesson.id.clone());
        ui.spacing_mut().item_spacing.y = 4.0;
        let label = if track.def.label.is_empty() {
            track.def.title.clone()
        } else {
            track.def.label.clone()
        };
        let g = ui.painter().layout(
            label.to_uppercase(),
            font::semibold(11.0),
            p.text_muted,
            ui.available_width() - 20.0,
        );
        let (r, resp) = ui.allocate_exact_size(
            vec2(ui.available_width(), g.size().y + 10.0),
            Sense::hover(),
        );
        ui.painter()
            .galley(pos2(r.left() + 10.0, r.top()), g, p.text_muted);
        let lab = label.clone();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Heading, true, &lab));
        let mut go = None;
        for (i, l) in track.lessons.iter().enumerate() {
            let id = &l.lesson.id;
            let done = self.progress.as_ref().is_some_and(|p| p.is_done(id));
            let current = open.as_deref() == Some(id.as_str()) && self.view == LessonsView::Lesson;
            if lesson_row(ui, Some(i + 1), &l.lesson.title, done, current).clicked() {
                go = Some(id.clone());
            }
        }
        section(ui, "More tracks", 20.0);
        let others: Vec<(String, String)> = self
            .catalog
            .as_ref()
            .map(|c| {
                c.tracks
                    .iter()
                    .filter(|t| t.def.id != track.def.id)
                    .map(|t| (t.def.id.clone(), t.def.title.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let mut switch = None;
        for (id, title) in others {
            if lesson_row(ui, None, &title, false, false).clicked() {
                switch = Some(id);
            }
        }
        if lesson_row(
            ui,
            None,
            "Make your own lesson",
            false,
            self.view == LessonsView::Editor,
        )
        .clicked()
        {
            self.view = LessonsView::Editor;
            self.editor.on_open(app);
        }
        if let Some(id) = go {
            self.open_lesson(app, &id);
        }
        if let Some(t) = switch
            && let Some(id) = self.default_lesson(Some(&t))
        {
            self.open_lesson(app, &id);
        }
    }

    // ------------------------------------------------------------------ bench

    fn bench(&mut self, ui: &mut Ui, app: &mut AppCtx, rect: Rect) {
        let p = pal(ui);
        let Some(run) = self.run.as_mut() else {
            return;
        };
        crate::theme::paint_dots(ui.painter(), rect, p);
        run.sync();
        let tel = run.telemetry();
        // callouts under the bench (the design's "Meter is wired in series — …")
        let callouts: Vec<(String, String)> = run
            .session
            .current()
            .callouts
            .iter()
            .map(|c| (c.title.clone(), c.text.clone()))
            .collect();
        let foot = if callouts.is_empty() { 0.0 } else { 44.0 };
        let canvas = Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - foot));
        // the meter waits for the prediction
        let masked: Vec<String> = if run.session.meter_hidden() {
            run.meter_part().into_iter().collect()
        } else {
            Vec::new()
        };
        crate::canvas::plugin_parts::set_masked(masked);
        let gen_ = run.generation;
        if run.bench.is_some() {
            let out = run
                .canvas
                .show(ui, canvas, &mut run.doc, tel.as_deref(), gen_);
            if !run.zoomed {
                // a lesson bench is small: fill the canvas (up to 2.4× the Workbench zoom)
                run.zoomed = true;
                let e = run.canvas.scene(&mut run.doc).extent;
                run.canvas.view = crate::canvas::pen::View::fit(
                    canvas.shrink(30.0),
                    (e.x0 - 2.0, e.y0 - 3.0, e.x1 + 2.0, e.y1 + 3.0),
                    crate::canvas::pen::BASE_SCALE * 2.4,
                );
                ui.ctx().request_repaint();
            }
            if let Some(b) = run.bench.as_ref() {
                for c in out.sim {
                    b.send(c);
                }
            }
            if out.live_edit
                && let Some(b) = run.bench.as_mut()
            {
                b.adopt_project(run.doc.project().clone());
            }
        } else {
            run.schematic
                .show(ui, canvas, &mut run.doc, None, gen_, app.theme);
        }
        crate::canvas::plugin_parts::set_masked(Vec::new());
        if !callouts.is_empty() {
            let r = Rect::from_min_max(pos2(rect.left(), canvas.bottom()), rect.max);
            let mut job = egui::text::LayoutJob::default();
            for (i, (title, text)) in callouts.iter().enumerate() {
                if i > 0 {
                    job.append("   ·   ", 0.0, fmt(12.0, p.text_faint));
                }
                job.append(title, 0.0, fmt(12.0, p.text));
                if !text.is_empty() {
                    job.append(&format!(" — {text}"), 0.0, fmt(12.0, p.text_muted));
                }
            }
            let g = ui.painter().layout_job(job);
            let text: String = callouts
                .iter()
                .map(|(a, b)| format!("{a} — {b}"))
                .collect::<Vec<_>>()
                .join(" · ");
            let gr = Rect::from_center_size(r.center() - vec2(0.0, 8.0), g.size());
            ui.painter().galley(gr.min, g, p.text);
            let resp = ui.interact(gr, egui::Id::unique("lesson-callout"), Sense::hover());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
        }
        if run.is_running() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}

fn fmt(size: f32, color: Color32) -> egui::TextFormat {
    egui::TextFormat {
        font_id: font::ui(size),
        color,
        ..Default::default()
    }
}

fn section(ui: &mut Ui, text: &str, top: f32) {
    let p = pal(ui);
    ui.add_space(top);
    // wraps inside the nav like the design (`TRACK 1 · ELECTRICITY YOU CAN / SEE`)
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &text.to_uppercase(),
        0.0,
        egui::TextFormat {
            font_id: font::semibold(11.0),
            color: p.text_muted,
            extra_letter_spacing: 0.9,
            ..Default::default()
        },
    );
    job.wrap.max_width = ui.available_width() - 20.0;
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), g.size().y + 10.0),
        Sense::hover(),
    );
    ui.painter()
        .galley(pos2(r.left() + 10.0, r.top()), g, p.text_muted);
}

/// A `.les` row: number circle (✓ when done, amber when current) + title.
fn lesson_row(
    ui: &mut Ui,
    n: Option<usize>,
    title: &str,
    done: bool,
    current: bool,
) -> egui::Response {
    let p = pal(ui);
    let h = if n.is_some() { 38.0 } else { 34.0 };
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let bg = if current {
        Some(p.accent_bg2)
    } else if resp.hovered() {
        Some(p.bg_hover)
    } else {
        None
    };
    if let Some(bg) = bg {
        ui.painter().rect_filled(r, CornerRadius::same(7), bg);
    }
    let mut x = r.left() + 10.0;
    if let Some(n) = n {
        let c = pos2(x + 11.0, r.center().y);
        let (fill, stroke, col) = if done {
            (
                Color32::from_rgb(0x1C, 0x2A, 0x1F),
                Color32::from_rgb(0x2F, 0x5A, 0x3A),
                Color32::from_rgb(0x6F, 0xD4, 0x8F),
            )
        } else if current {
            (Color32::TRANSPARENT, p.accent, p.accent_soft)
        } else {
            (
                Color32::TRANSPARENT,
                Color32::from_rgb(0x3A, 0x3A, 0x3A),
                p.text_muted,
            )
        };
        ui.painter().circle(c, 11.0, fill, Stroke::new(1.0, stroke));
        if done {
            widgets::Icon::Check.paint(
                ui.painter(),
                Rect::from_center_size(c, vec2(10.0, 10.0)),
                col,
            );
        } else {
            ui.painter().text(
                c,
                Align2::CENTER_CENTER,
                n.to_string(),
                font::mono_medium(11.0),
                col,
            );
        }
        x += 32.0;
    }
    let color = if current { p.accent_soft } else { p.text_dim };
    // one line, cut with an ellipsis at the nav's edge
    let mut job =
        egui::text::LayoutJob::simple_singleline(title.to_string(), font::ui(13.0), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width((r.right() - 8.0 - x).max(20.0));
    let g = ui.painter().layout_job(job);
    let elided = g.elided;
    ui.painter()
        .galley(pos2(x, r.center().y - g.size().y / 2.0), g, color);
    let label = match (n, done) {
        (Some(n), true) => format!("Lesson {n}: {title} (done)"),
        (Some(n), false) => format!("Lesson {n}: {title}"),
        (None, _) => title.to_string(),
    };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, current, &label));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if elided {
        resp.on_hover_text(title)
    } else {
        resp
    }
}

impl Screen for LessonsScreen {
    fn crumb(&self, _app: &AppCtx) -> Option<String> {
        Some("Lessons".into())
    }

    fn header(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        if self.view == LessonsView::Lesson
            && let Some(t) = self.current_track()
        {
            ui.label(
                egui::RichText::new("/")
                    .font(font::ui(13.0))
                    .color(p.text_ghost),
            );
            ui.label(
                egui::RichText::new(&t.def.title)
                    .font(font::ui(13.0))
                    .color(p.text_dim),
            );
        } else if self.view == LessonsView::Editor {
            ui.label(
                egui::RichText::new("/")
                    .font(font::ui(13.0))
                    .color(p.text_ghost),
            );
            ui.label(
                egui::RichText::new("Make your own lesson")
                    .font(font::ui(13.0))
                    .color(p.text_dim),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            if self.view == LessonsView::Editor {
                return;
            }
            if self.run.is_some() && Btn::ghost("Open in free play").show(ui).clicked() {
                self.free_play(app);
            }
            if let Some((n, of)) = self.position() {
                let (r, _) = ui.allocate_exact_size(vec2(140.0, 6.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::same(3), p.border);
                let frac = ((n as f32 - 0.5) / of.max(1) as f32).clamp(0.0, 1.0);
                ui.painter().rect_filled(
                    Rect::from_min_size(r.min, vec2(r.width() * frac, r.height())),
                    CornerRadius::same(3),
                    p.accent,
                );
                let t = format!("Lesson {n} of {of}");
                let resp = ui.label(
                    egui::RichText::new(&t)
                        .font(font::ui(12.0))
                        .color(p.text_muted),
                );
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &t));
            }
        });
    }

    fn on_enter(&mut self, app: &mut AppCtx) {
        self.ensure_catalog(app);
        // resume the lesson bench paused by `on_leave`
        if let Some(r) = self.run.as_mut() {
            r.set_running(true);
        }
    }

    /// The lesson's own bench doesn't run behind other screens.
    fn on_leave(&mut self, _app: &mut AppCtx) {
        if let Some(r) = self.run.as_mut() {
            r.set_running(false);
        }
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn ui(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        self.ensure_catalog(app);
        if self.run.is_none()
            && self.view == LessonsView::Lesson
            && self.error.is_none()
            && let Some(id) = self.default_lesson(None)
        {
            self.open_lesson(app, &id);
        }
        let p = pal(ui);
        let full = ui.max_rect();
        let left = Rect::from_min_size(full.min, vec2(260.0, full.height()));
        ui.painter()
            .rect_filled(left, 0.0, Color32::from_rgba_unmultiplied(17, 17, 17, 214));
        ui.painter().line_segment(
            [left.right_top(), left.right_bottom()],
            Stroke::new(1.0, p.border),
        );
        let mut nav = widgets::child_ui(ui, left.shrink2(vec2(12.0, 16.0)));
        egui::ScrollArea::vertical()
            .id_salt("lessons-nav")
            .auto_shrink([false, false])
            .show(&mut nav, |ui| self.nav(ui, app));
        let rest = Rect::from_min_max(pos2(left.right(), full.top()), full.max);
        if self.view == LessonsView::Editor {
            self.editor.ui(ui, app, rest, self.run.as_ref());
            if let Some(id) = self.editor.take_open_request() {
                // a saved pack: reload the catalog and open its first lesson
                self.catalog = None;
                self.ensure_catalog(app);
                self.open_lesson(app, &id);
            }
            return;
        }
        let right_w = 360.0;
        let right = Rect::from_min_max(pos2(full.right() - right_w, full.top()), full.max);
        let main = Rect::from_min_max(
            pos2(left.right(), full.top()),
            pos2(right.left(), full.bottom()),
        );
        if let Some(err) = &self.error
            && self.run.is_none()
        {
            ui.painter().text(
                main.center(),
                Align2::CENTER_CENTER,
                err,
                font::ui(13.0),
                p.danger_soft,
            );
        }
        self.bench(ui, app, main);
        ui.painter()
            .rect_filled(right, 0.0, Color32::from_rgba_unmultiplied(17, 17, 17, 230));
        ui.painter().line_segment(
            [right.left_top(), right.left_bottom()],
            Stroke::new(1.0, p.border),
        );
        if self.run.is_some() {
            self.step_panel(ui, app, right);
        }
        let _ = StrokeKind::Inside;
    }

    fn save_state(&self) -> Option<serde_json::Value> {
        serde_json::to_value(LessonsState {
            track: self.track.clone(),
            lesson: self
                .run
                .as_ref()
                .map(|r| r.session.lesson.id.clone())
                .or_else(|| self.saved_lesson.clone()),
        })
        .ok()
    }

    fn load_state(&mut self, state: &serde_json::Value) {
        if let Ok(s) = serde_json::from_value::<LessonsState>(state.clone()) {
            self.track = s.track;
            self.saved_lesson = s.lesson;
        }
    }
}
