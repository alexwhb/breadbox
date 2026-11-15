//! "Make your own lesson" (`LES-6`): a lesson editor over `bb_lessons::editor::Draft` —
//! title / track / pack, steps (kind, title, text, formula, quiz, prediction), the start
//! circuit captured from the current bench (the Workbench's project or the open lesson's),
//! checkers suggested for a picked part or two picked pins, validation, "Save lesson pack"
//! (a lesson-pack plugin in the user plugin folder → it shows up in Lessons) and "Package
//! .bbplugin…" for sharing.

use super::run::LessonRun;
use crate::app::AppCtx;
use crate::theme::{font, pal};
use crate::widgets::{self, Btn, Icon};
use bb_circuit::Project;
use bb_lessons::editor::{Draft, PackInfo, Suggestion};
use bb_lessons::format::{Check, Predict, Quiz, STEP_KINDS, TrackDef};
use eframe::egui::{self, Color32, CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};
use std::path::PathBuf;
use std::sync::Arc;

pub struct LessonEditor {
    pub draft: Draft,
    pub title: String,
    pub track_title: String,
    pub pack_name: String,
    pub selected: usize,
    pub pick_part: Option<String>,
    pub pin_a: String,
    pub pin_b: String,
    pub problems: Option<Vec<String>>,
    pub captured_from: Option<String>,
    open_request: Option<String>,
    /// Where the last save went.
    pub saved: Option<PathBuf>,
}

impl Default for LessonEditor {
    fn default() -> Self {
        let track = track_def("My lessons");
        LessonEditor {
            draft: Draft::new("My first lesson", track),
            title: "My first lesson".into(),
            track_title: "My lessons".into(),
            pack_name: "My lessons".into(),
            selected: 0,
            pick_part: None,
            pin_a: String::new(),
            pin_b: String::new(),
            problems: None,
            captured_from: None,
            open_request: None,
            saved: None,
        }
    }
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let s = out.trim_end_matches('-').to_string();
    if s.is_empty() { "lesson".into() } else { s }
}

fn track_def(title: &str) -> TrackDef {
    TrackDef {
        id: format!("my-{}", slug(title)),
        title: title.into(),
        label: format!("Your track · {title}"),
        order: 100,
        description: "Lessons you wrote with “Make your own lesson”.".into(),
    }
}

/// The bench the editor captures: the Workbench's open project, else the open lesson's.
fn current_bench(app: &AppCtx, run: Option<&LessonRun>) -> Option<(Project, String)> {
    if let Some(doc) = &app.circuit {
        let name = app
            .project
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "the Workbench".into());
        return Some((doc.project().clone(), name));
    }
    run.map(|r| {
        (
            r.doc.project().clone(),
            format!("lesson “{}”", r.session.lesson.title),
        )
    })
}

impl LessonEditor {
    pub fn on_open(&mut self, _app: &mut AppCtx) {
        self.problems = None;
    }

    /// Lesson id to open after a save (taken by the screen).
    pub fn take_open_request(&mut self) -> Option<String> {
        self.open_request.take()
    }

    fn sync_meta(&mut self) {
        self.draft.lesson.title = self.title.clone();
        self.draft.lesson.id = slug(&self.title);
        self.draft.track = track_def(&self.track_title);
        self.draft.lesson.track = self.draft.track.id.clone();
    }

    /// "Use this bench as the start circuit".
    pub fn capture(&mut self, app: &AppCtx, run: Option<&LessonRun>) -> bool {
        match current_bench(app, run) {
            Some((p, from)) => {
                self.draft.capture_bench(&p);
                self.captured_from = Some(from);
                true
            }
            None => false,
        }
    }

    pub fn add_step(&mut self, kind: &str) -> usize {
        let title = match kind {
            "explain" => "Look at the circuit",
            "build" => "Build it",
            "predict" => "Predict the value",
            "measure" => "Measure it",
            "experiment" => "Try a different value",
            "break" => "Break it on purpose",
            "quiz" => "Quick question",
            "code" => "Write the sketch",
            _ => "New step",
        };
        let i = self.draft.add_step(kind, title);
        if let Some(s) = self.draft.step_mut(i) {
            match kind {
                "quiz" => {
                    s.quiz = Some(Quiz {
                        question: "Which one is right?".into(),
                        options: vec!["This one".into(), "That one".into()],
                        answer: 0,
                        explain: String::new(),
                    })
                }
                "predict" => {
                    s.predict = Some(Predict {
                        prompt: "What do you expect?".into(),
                        unit: "V".into(),
                        answer: "5.0 / 2".into(),
                        tolerance: 0.05,
                        optional: false,
                        toggle_label: String::new(),
                        toggle_note: String::new(),
                        meter: None,
                        correct_text: "Right. The meter agrees: {meter} V.".into(),
                        hints: Vec::new(),
                        default_hint: "Not quite — try again.".into(),
                        working: Vec::new(),
                    })
                }
                _ => {}
            }
        }
        self.selected = i;
        self.problems = None;
        i
    }

    /// Checkers for the picked part with its live readings.
    pub fn part_suggestions(&self, app: &mut AppCtx, run: Option<&LessonRun>) -> Vec<Suggestion> {
        let Some(part) = &self.pick_part else {
            return Vec::new();
        };
        let lib = app.plugins.library();
        let tel = if app.circuit.is_some() {
            app.bench.sim().and_then(|s| s.telemetry())
        } else {
            run.and_then(|r| r.telemetry())
        };
        let readings = tel
            .as_ref()
            .and_then(|t| t.parts.iter().find(|p| &p.part == part))
            .map(|p| p.readings.clone())
            .unwrap_or_default();
        self.draft.suggest_for_part(&lib, part, &readings)
    }

    /// Writes the pack into the user plugin folder and installs it.
    pub fn save(&mut self, app: &mut AppCtx) -> Result<PathBuf, String> {
        self.sync_meta();
        let lib = app.plugins.library();
        let problems = self.draft.validate(&lib);
        if !problems.is_empty() {
            self.problems = Some(problems.clone());
            return Err(format!("Fix {} problem(s) first", problems.len()));
        }
        let pack = self.pack_info();
        let dir = app.plugins.registry.user_dir().join(&pack.id);
        self.draft
            .save_pack(&dir, &pack)
            .map_err(|e| e.to_string())?;
        app.plugins.registry.rescan();
        app.plugins.changed();
        self.saved = Some(dir.clone());
        self.problems = Some(Vec::new());
        self.open_request = Some(self.draft.lesson.id.clone());
        Ok(dir)
    }

    fn pack_info(&self) -> PackInfo {
        PackInfo {
            id: format!("me.{}", slug(&self.pack_name)),
            name: self.pack_name.clone(),
            version: "1.0.0".into(),
            author: String::new(),
            summary: format!("Lessons: {}", self.track_title),
        }
    }

    /// `.bbplugin` bytes of the pack (sharing).
    pub fn package_to(&mut self, app: &mut AppCtx, path: &std::path::Path) -> Result<(), String> {
        let dir = self.save(app)?;
        let bytes = self
            .draft
            .package(&dir, &self.pack_info())
            .map_err(|e| e.to_string())?;
        std::fs::write(path, bytes).map_err(|e| e.to_string())
    }

    pub fn ui(&mut self, ui: &mut Ui, app: &mut AppCtx, rect: Rect, run: Option<&LessonRun>) {
        let p = pal(ui);
        crate::theme::paint_paper(ui.painter(), rect, p);
        let left = Rect::from_min_size(rect.min, vec2(300.0, rect.height()));
        let right = Rect::from_min_max(pos2(rect.right() - 360.0, rect.top()), rect.max);
        let mid = Rect::from_min_max(
            pos2(left.right(), rect.top()),
            pos2(right.left(), rect.bottom()),
        );
        for (r, a) in [(left, 220u8), (right, 230)] {
            ui.painter()
                .rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(17, 17, 17, a));
        }
        ui.painter().line_segment(
            [left.right_top(), left.right_bottom()],
            Stroke::new(1.0, p.border),
        );
        ui.painter().line_segment(
            [right.left_top(), right.left_bottom()],
            Stroke::new(1.0, p.border),
        );
        // ---- left: lesson + steps
        let mut lc = widgets::child_ui(ui, left.shrink(18.0));
        egui::ScrollArea::vertical()
            .id_salt("les-ed-left")
            .auto_shrink([false, false])
            .show(&mut lc, |ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.label(
                    egui::RichText::new("Make your own lesson")
                        .font(font::heading(20.0))
                        .color(p.text),
                );
                field(ui, "Lesson title", &mut self.title);
                field(ui, "Track", &mut self.track_title);
                field(ui, "Lesson pack", &mut self.pack_name);
                widgets::section_header(ui, "Steps");
                let n = self.draft.lesson.steps.len();
                let mut action: Option<(usize, i32)> = None;
                for i in 0..n {
                    let s = &self.draft.lesson.steps[i];
                    let on = i == self.selected;
                    let (r, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                    if on {
                        ui.painter()
                            .rect_filled(r, CornerRadius::same(7), p.accent_bg2);
                    } else if resp.hovered() {
                        ui.painter()
                            .rect_filled(r, CornerRadius::same(7), p.bg_hover);
                    }
                    ui.painter().text(
                        pos2(r.left() + 10.0, r.center().y),
                        egui::Align2::LEFT_CENTER,
                        format!("{}", i + 1),
                        font::mono(11.0),
                        p.text_muted,
                    );
                    ui.painter().text(
                        pos2(r.left() + 28.0, r.center().y),
                        egui::Align2::LEFT_CENTER,
                        &s.title,
                        font::ui(13.0),
                        if on { p.accent_soft } else { p.text_dim },
                    );
                    ui.painter().text(
                        pos2(r.right() - 8.0, r.center().y),
                        egui::Align2::RIGHT_CENTER,
                        s.kind.to_uppercase(),
                        font::mono_medium(9.5),
                        p.info,
                    );
                    let lab = format!("Step {}: {}", i + 1, s.title);
                    resp.widget_info(|| {
                        egui::WidgetInfo::selected(egui::Role::Button, true, on, &lab)
                    });
                    if resp.clicked() {
                        self.selected = i;
                    }
                    if on {
                        ui.horizontal(|ui| {
                            if Btn::mini("Up").enabled(i > 0).show(ui).clicked() {
                                action = Some((i, -1));
                            }
                            if Btn::mini("Down").enabled(i + 1 < n).show(ui).clicked() {
                                action = Some((i, 1));
                            }
                            if Btn::mini("Remove").show(ui).clicked() {
                                action = Some((i, 0));
                            }
                        });
                    }
                }
                if let Some((i, d)) = action {
                    if d == 0 {
                        self.draft.remove_step(i);
                        self.selected = self
                            .selected
                            .min(self.draft.lesson.steps.len().saturating_sub(1));
                    } else {
                        let j = (i as i32 + d) as usize;
                        self.draft.move_step(i, j);
                        self.selected = j;
                    }
                    self.problems = None;
                }
                let add = Btn::ghost("Add step").icon(Icon::Plus).show(ui);
                let mut kind = None;
                egui::Popup::menu(&add).show(|ui| {
                    ui.set_min_width(180.0);
                    for k in STEP_KINDS {
                        if widgets::menu_item(ui, k, false, true) {
                            kind = Some(k);
                        }
                    }
                });
                if let Some(k) = kind {
                    self.add_step(k);
                }
            });
        // ---- middle: the selected step
        let mut mc = widgets::child_ui(ui, mid.shrink(22.0));
        egui::ScrollArea::vertical().id_salt("les-ed-mid").auto_shrink([false, false]).show(&mut mc, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 12.0;
            let sel = self.selected;
            if self.draft.lesson.steps.get(sel).is_none() {
                ui.label(egui::RichText::new("Add a first step: “explain” says what the circuit does, “build” asks for wiring, “measure” reads an instrument, “quiz” asks a question.").font(font::ui(13.0)).color(p.text_muted));
                return;
            }
            let mut suggestions_part: Vec<Suggestion> = Vec::new();
            let mut suggestions_pins: Vec<Suggestion> = Vec::new();
            if self.pick_part.is_some() {
                suggestions_part = self.part_suggestions(app, run);
            }
            if self.pin_a.contains('.') && self.pin_b.contains('.') {
                let lib = app.plugins.library();
                suggestions_pins = self.draft.suggest_for_pins(&lib, self.pin_a.trim(), self.pin_b.trim(), None);
            }
            let parts: Vec<(String, String)> = self
                .draft
                .project
                .parts
                .iter()
                .map(|p| (p.id.clone(), format!("{} · {}", p.reference, p.part)))
                .collect();
            let Some(step) = self.draft.step_mut(sel) else { return };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("Step {}", sel + 1)).font(font::heading(18.0)).color(p.text));
                widgets::kind_tag(ui, &step.kind.to_uppercase());
            });
            field(ui, "Step title", &mut step.title);
            ui.label(egui::RichText::new("What to tell the learner").font(font::ui(12.0)).color(p.text_muted));
            let r = ui.add(egui::TextEdit::multiline(&mut step.body).desired_rows(5).desired_width(f32::INFINITY).font(font::ui(13.0)));
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::MultilineTextInput, true, "Step text"));
            let mut formula = step.formula.clone().unwrap_or_default();
            field(ui, "Formula (optional)", &mut formula);
            step.formula = (!formula.trim().is_empty()).then_some(formula);
            if let Some(q) = step.quiz.as_mut() {
                widgets::section_header(ui, "Question");
                field(ui, "Question", &mut q.question);
                for (i, o) in q.options.iter_mut().enumerate() {
                    field(ui, &format!("Answer {}", i + 1), o);
                }
                ui.horizontal(|ui| {
                    if Btn::mini("Add answer").show(ui).clicked() {
                        q.options.push(String::new());
                    }
                    let mut a = q.answer as f64;
                    ui.label(egui::RichText::new("Right answer #").font(font::ui(12.0)).color(p.text_muted));
                    if ui.add(egui::DragValue::new(&mut a).range(1.0..=q.options.len().max(1) as f64).custom_formatter(|v, _| format!("{}", v as usize + 1)).custom_parser(|s| s.parse::<f64>().ok().map(|v| v - 1.0))).changed() {
                        q.answer = (a.max(0.0) as usize).min(q.options.len().saturating_sub(1));
                    }
                });
            }
            if let Some(pr) = step.predict.as_mut() {
                widgets::section_header(ui, "Prediction");
                field(ui, "Prompt", &mut pr.prompt);
                field(ui, "Answer (expression)", &mut pr.answer);
                field(ui, "Unit", &mut pr.unit);
            }
            // goals
            widgets::section_header(ui, "Goals");
            let mut remove = None;
            for (i, c) in step.checks.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(check_label(c)).font(font::mono(12.0)).color(p.text_dim));
                    if Btn::mini("Remove").show(ui).clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                step.checks.remove(i);
            }
            if step.checks.is_empty() {
                ui.label(egui::RichText::new("No goals yet: the learner just reads this step and presses Next.").font(font::ui(12.0)).color(p.text_faint));
            }
            widgets::section_header(ui, "Add goals from the bench");
            ui.label(egui::RichText::new("Pick a part").font(font::ui(12.0)).color(p.text_muted));
            ui.horizontal_wrapped(|ui| {
                for (id, name) in &parts {
                    let on = self.pick_part.as_deref() == Some(id.as_str());
                    let r = widgets::filter_chip(ui, name, on);
                    if r.clicked() {
                        self.pick_part = if on { None } else { Some(id.clone()) };
                    }
                }
            });
            if parts.is_empty() {
                ui.label(egui::RichText::new("Capture a bench first (right).").font(font::ui(12.0)).color(p.text_faint));
            }
            let mut add: Option<Check> = None;
            for s in suggestions_part.iter().chain(suggestions_pins.iter()) {
                ui.horizontal(|ui| {
                    let b = Btn::mini("Add").icon(Icon::Plus).show(ui);
                    let lab = format!("Add goal: {}", s.label);
                    b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &lab));
                    if b.clicked() {
                        add = Some(s.check.clone());
                    }
                    ui.label(egui::RichText::new(&s.label).font(font::ui(12.5)).color(p.text_dim));
                });
            }
            ui.label(egui::RichText::new("…or two pins (part.pin)").font(font::ui(12.0)).color(p.text_muted));
            ui.horizontal(|ui| {
                small_field(ui, "Pin A", &mut self.pin_a, "r1.1");
                small_field(ui, "Pin B", &mut self.pin_b, "d1.a");
            });
            if let Some(c) = add {
                self.draft.add_check(sel, c);
                // goals picked from the bench hold on the captured circuit: the worked
                // solution ("Show me how") runs the bench so they settle
                if let Some(st) = self.draft.step_mut(sel)
                    && st.solution.is_empty()
                {
                    st.solution
                        .push(bb_lessons::format::Action::Run { seconds: 1.0 });
                }
                self.problems = None;
            }
        });
        // ---- right: start circuit, validation, save
        let mut rc = widgets::child_ui(ui, right.shrink(20.0));
        rc.spacing_mut().item_spacing.y = 12.0;
        widgets::section_header(&mut rc, "Start circuit");
        let pr = &self.draft.project;
        let summary = if pr.parts.is_empty() {
            "Empty bench — capture one.".to_string()
        } else {
            format!(
                "{} parts · {} wires{}",
                pr.parts.len(),
                pr.wires.len(),
                self.captured_from
                    .as_ref()
                    .map(|f| format!(" · from {f}"))
                    .unwrap_or_default()
            )
        };
        let r = rc.label(
            egui::RichText::new(&summary)
                .font(font::ui(13.0))
                .color(p.text_dim),
        );
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &summary));
        let src = current_bench(app, run).map(|(_, f)| f);
        let cap = Btn::ghost("Capture the current bench")
            .icon(Icon::Download)
            .enabled(src.is_some())
            .show(&mut rc);
        if cap.clicked() {
            self.capture(app, run);
            self.problems = None;
        }
        if let Some(f) = &src {
            rc.label(
                egui::RichText::new(format!("Captures {f}."))
                    .font(font::ui(11.5))
                    .color(p.text_faint),
            );
        }
        widgets::section_header(&mut rc, "Check & save");
        if Btn::ghost("Validate").show(&mut rc).clicked() {
            self.sync_meta();
            let lib = app.plugins.library();
            self.problems = Some(self.draft.validate(&lib));
        }
        if let Some(pb) = &self.problems {
            if pb.is_empty() {
                rc.label(
                    egui::RichText::new("Ready: the lesson loads and every step can be completed.")
                        .font(font::ui(12.5))
                        .color(p.ok),
                );
            }
            for x in pb {
                let t = format!("• {x}");
                let r = rc.label(
                    egui::RichText::new(&t)
                        .font(font::ui(12.5))
                        .color(p.accent_soft),
                );
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Label, true, format!("Problem: {x}"))
                });
            }
        }
        if Btn::amber("Save lesson pack").show(&mut rc).clicked() {
            match self.save(app) {
                Ok(dir) => app.toasts.success(format!(
                    "Saved “{}” — it's in Lessons now ({})",
                    self.title,
                    dir.display()
                )),
                Err(e) => app.toasts.error(e),
            }
        }
        if Btn::ghost("Package .bbplugin…").show(&mut rc).clicked()
            && let Some(path) = crate::platform::save_file(
                "Share the lesson pack",
                &format!("{}.bbplugin", slug(&self.pack_name)),
                "Breadbox plugin",
                "bbplugin",
            )
        {
            match self.package_to(app, &path) {
                Ok(()) => app.toasts.success(format!("Packaged {}", path.display())),
                Err(e) => app.toasts.error(e),
            }
        }
        rc.label(egui::RichText::new("A lesson pack is a plugin (kind lesson-pack): share the .bbplugin and others install it from Plugins → Load local plugin….").font(font::ui(11.5)).color(p.text_faint));
        let _ = Arc::<()>::default;
    }
}

/// Caption text without its own accessibility node (the input below carries the name).
fn caption(ui: &mut Ui, text: &str) {
    let p = pal(ui);
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), font::ui(12.0), p.text_muted);
    let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(r.min, g, p.text_muted);
}

fn field(ui: &mut Ui, label: &str, value: &mut String) {
    caption(ui, label);
    let r = ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(f32::INFINITY)
            .font(font::ui(13.0))
            .margin(egui::Margin::symmetric(8, 6)),
    );
    let l = label.to_string();
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, &l));
}

fn small_field(ui: &mut Ui, label: &str, value: &mut String, hint: &str) {
    let r = ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(110.0)
            .hint_text(hint)
            .font(font::mono(12.0)),
    );
    let l = label.to_string();
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, &l));
}

/// A check as one line (`connected r1.2 d1.a`).
pub fn check_label(c: &Check) -> String {
    serde_json::to_value(c)
        .ok()
        .and_then(|v| {
            let o = v.as_object()?;
            let ty = o.get("type")?.as_str()?.replace('_', " ");
            let rest: Vec<String> = o
                .iter()
                .filter(|(k, _)| *k != "type")
                .map(|(k, v)| match v {
                    serde_json::Value::String(s) => format!("{k}={s}"),
                    other => format!("{k}={other}"),
                })
                .collect();
            Some(format!("{ty} {}", rest.join(" ")))
        })
        .unwrap_or_else(|| format!("{c:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_track_ids() {
        assert_eq!(slug("My first lesson!"), "my-first-lesson");
        assert_eq!(track_def("My lessons").id, "my-my-lessons");
        assert_eq!(
            check_label(&Check::Connected {
                a: "r1.2".into(),
                b: "d1.a".into()
            }),
            "connected a=r1.2 b=d1.a"
        );
    }
}
