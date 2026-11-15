//! The step panel ("Lesson coach", right column of the design): step bar, title, body,
//! formula card, choice grid, "Do the math myself" + prediction card, damage card, quiz,
//! code editor, goals, "Your measurements", Back / Next.

use super::LessonsScreen;
use crate::app::AppCtx;
use crate::theme::{font, pal};
use crate::widgets::{self, Btn, CodeEditor, Icon, markdown};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

const GREEN: Color32 = Color32::from_rgb(0x6F, 0xD4, 0x8F);
const BODY: Color32 = Color32::from_rgb(0xC7, 0xC7, 0xC7);

/// Text painted without an accessibility node (its control carries the name).
fn plain(ui: &mut Ui, text: &str, f: egui::FontId, color: Color32) {
    let g = ui.painter().layout_no_wrap(text.to_string(), f, color);
    let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(r.min, g, color);
}

fn label(ui: &mut Ui, text: &str, f: egui::FontId, color: Color32) -> egui::Response {
    ui.add(egui::Label::new(egui::RichText::new(text).font(f).color(color)).wrap())
}

impl LessonsScreen {
    pub(super) fn step_panel(&mut self, ui: &mut Ui, app: &mut AppCtx, rect: Rect) {
        let p = pal(ui);
        let Some(run) = self.run.as_mut() else { return };
        let tel = run.telemetry();
        run.evaluate(false);
        run.auto_record(tel.as_deref());
        run.poll_build();
        let step = run.session.current().clone();
        let n_steps = run.session.lesson.steps.len();
        let cur = run.session.step;
        let meter = run.meter(tel.as_deref());
        let damaged = run.damaged(tel.as_deref());
        let mut cmd: Option<Action> = None;

        let inner = rect.shrink(20.0);
        let footer_h = 40.0;
        let body_rect = Rect::from_min_max(
            inner.min,
            pos2(inner.right(), inner.bottom() - footer_h - 12.0),
        );
        let mut c = widgets::child_ui(ui, body_rect);
        egui::ScrollArea::vertical()
            .id_salt(("lesson-step", run.session.lesson.id.as_str(), cur))
            .auto_shrink([false, false])
            .show(&mut c, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 16.0;
                // ---- step bar, label, title, body
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    let w = ui.available_width();
                    let (r, _) = ui.allocate_exact_size(vec2(w, 4.0), Sense::hover());
                    let gap = 4.0;
                    let seg = (w - gap * (n_steps as f32 - 1.0)) / n_steps.max(1) as f32;
                    for i in 0..n_steps {
                        let col = if i < cur {
                            p.ok
                        } else if i == cur {
                            p.accent
                        } else {
                            p.border_strong
                        };
                        ui.painter().rect_filled(
                            Rect::from_min_size(
                                pos2(r.left() + i as f32 * (seg + gap), r.top()),
                                vec2(seg, 4.0),
                            ),
                            CornerRadius::same(2),
                            col,
                        );
                    }
                    let l = run.session.label();
                    label(ui, &l, font::ui(12.0), p.text_muted);
                    let t = ui.add(
                        egui::Label::new(
                            egui::RichText::new(&step.title)
                                .font(font::heading(22.0))
                                .color(p.text),
                        )
                        .wrap(),
                    );
                    let tt = step.title.clone();
                    t.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Heading, true, &tt));
                    if !step.body.is_empty() {
                        let text = run.session.render(&step.body, meter);
                        for para in text.split("\n\n") {
                            let mut job = markdown::inline(
                                &para.replace('\n', " "),
                                14.0,
                                BODY,
                                p.accent_soft,
                                p.bg_raised,
                            );
                            job.wrap.max_width = ui.available_width();
                            for s in job.sections.iter_mut() {
                                s.format.line_height = Some(22.4);
                            }
                            let r = ui.label(job);
                            let a = para.replace("**", "");
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Label, true, &a)
                            });
                        }
                    }
                });
                // ---- formula card
                if let Some(f) = &step.formula {
                    let fr = egui::Frame::new()
                        .fill(p.bg_card)
                        .stroke(Stroke::new(1.0, p.border_strong))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(14, 12))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.vertical_centered(|ui| {
                                label(ui, f, font::mono_medium(15.0), p.text);
                            });
                        });
                    let ft = f.clone();
                    fr.response
                        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &ft));
                }
                // ---- choice ("Pick R1")
                if let Some(ch) = &step.choice {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        label(ui, &ch.label, font::semibold(12.0), p.text_muted);
                        let sel = run.session.state().choice;
                        let burnt = damaged.is_some();
                        let cols = 3;
                        let gap = 6.0;
                        let w = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
                        for row in ch.options.chunks(cols).enumerate() {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = gap;
                                for (k, o) in row.1.iter().enumerate() {
                                    let i = row.0 * cols + k;
                                    let on = sel == Some(i) && !(burnt && !o.bypass);
                                    if rbtn(ui, &o.label, on, w).clicked() {
                                        cmd = Some(Action::Choose(i));
                                    }
                                }
                            });
                        }
                    });
                }
                // ---- damage card ("Pop. That LED is gone.")
                if damaged.is_some() && !step.damage_title.is_empty() {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(0x17, 0x0D, 0x0C))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(0x5A, 0x24, 0x20)))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(egui::Margin::same(14))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 10.0;
                            label(
                                ui,
                                &step.damage_title,
                                font::heading(15.0),
                                Color32::from_rgb(0xFF, 0x8A, 0x7A),
                            );
                            label(ui, &step.damage_text, font::ui(13.0), p.text_dim);
                            let rl = if step.repair_label.is_empty() {
                                "Repair".to_string()
                            } else {
                                step.repair_label.clone()
                            };
                            if Btn::ghost(&rl).show(ui).clicked() {
                                cmd = Some(Action::Repair);
                            }
                        });
                }
                // ---- prediction
                if let Some(pr) = &step.predict {
                    let st = run.session.state().clone();
                    if !pr.toggle_label.is_empty() {
                        egui::Frame::new()
                            .stroke(Stroke::new(1.0, p.border))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(egui::Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let mut on = st.math;
                                    let r = widgets::checkbox(ui, &mut on, "");
                                    let tl = pr.toggle_label.clone();
                                    r.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::Role::CheckBox,
                                            true,
                                            on,
                                            &tl,
                                        )
                                    });
                                    if r.changed() || on != st.math {
                                        cmd = Some(Action::Math(on));
                                    }
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        plain(ui, &pr.toggle_label, font::ui(13.0), p.text);
                                        if !pr.toggle_note.is_empty() {
                                            label(
                                                ui,
                                                &pr.toggle_note,
                                                font::ui(11.0),
                                                p.text_muted,
                                            );
                                        }
                                    });
                                });
                            });
                    }
                    let show = st.math && !run.session.bypassed() && damaged.is_none();
                    if show {
                        egui::Frame::new()
                            .fill(Color32::from_rgb(0x14, 0x11, 0x0C))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(0x4A, 0x36, 0x14)))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(egui::Margin::same(14))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 10.0;
                                let prompt = run.session.render(&pr.prompt, meter);
                                label(ui, &prompt, font::ui(13.0), p.text);
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    let field_w = ui.available_width() - 76.0;
                                    let (fr, _) =
                                        ui.allocate_exact_size(vec2(field_w, 38.0), Sense::hover());
                                    ui.painter().rect(
                                        fr,
                                        CornerRadius::same(6),
                                        p.bg_app,
                                        Stroke::new(1.0, Color32::from_rgb(0x3A, 0x3A, 0x3A)),
                                        StrokeKind::Inside,
                                    );
                                    ui.painter().text(
                                        pos2(fr.right() - 10.0, fr.center().y),
                                        Align2::RIGHT_CENTER,
                                        &pr.unit,
                                        font::mono(13.0),
                                        p.text_muted,
                                    );
                                    let mut fc = ui.new_child(
                                        egui::UiBuilder::new()
                                            .max_rect(fr.shrink2(vec2(10.0, 0.0))),
                                    );
                                    let te = egui::TextEdit::singleline(&mut run.guess)
                                        .frame(egui::Frame::NONE)
                                        .font(font::mono(14.0))
                                        .hint_text(
                                            egui::RichText::new("e.g. 8.5")
                                                .font(font::mono(14.0))
                                                .color(p.text_faint),
                                        )
                                        .desired_width(fr.width() - 50.0)
                                        .align(egui::Align2::LEFT_CENTER)
                                        .min_size(vec2(0.0, 36.0));
                                    let r = fc.add(te);
                                    let lab = format!("Prediction ({})", pr.unit);
                                    r.widget_info(|| {
                                        egui::WidgetInfo::labeled(egui::Role::TextInput, true, &lab)
                                    });
                                    let enter = r.lost_focus()
                                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                    let check = Btn::amber("Check").height(38.0).show(ui).clicked();
                                    if enter || check {
                                        cmd = Some(Action::Check);
                                    }
                                });
                                if let Some(v) = st.verdict {
                                    let (text, col) = if v {
                                        (run.session.render(&pr.correct_text, meter), GREEN)
                                    } else {
                                        (st.hint.clone().unwrap_or_default(), p.accent_soft)
                                    };
                                    label(ui, &text, font::ui(13.0), col);
                                }
                                let wl = if st.show_working {
                                    "Hide the working"
                                } else {
                                    "Show me the working"
                                };
                                if Btn::ghost(wl)
                                    .height(30.0)
                                    .font_size(12.0)
                                    .show(ui)
                                    .clicked()
                                {
                                    cmd = Some(Action::Working);
                                }
                                if st.show_working {
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 6.0;
                                        for (i, w) in run.session.working().iter().enumerate() {
                                            let mut job = egui::text::LayoutJob::default();
                                            job.append(
                                                &format!("{}  ", i + 1),
                                                0.0,
                                                egui::TextFormat {
                                                    font_id: font::mono(13.0),
                                                    color: p.text_muted,
                                                    ..Default::default()
                                                },
                                            );
                                            job.append(
                                                w,
                                                0.0,
                                                egui::TextFormat {
                                                    font_id: font::mono(13.0),
                                                    color: p.text_dim,
                                                    ..Default::default()
                                                },
                                            );
                                            let r = ui.label(job);
                                            let ww = w.clone();
                                            r.widget_info(|| {
                                                egui::WidgetInfo::labeled(
                                                    egui::Role::Label,
                                                    true,
                                                    &ww,
                                                )
                                            });
                                        }
                                    });
                                }
                            });
                    }
                }
                // ---- quiz
                if let Some(q) = &step.quiz {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        label(ui, &q.question, font::medium(14.0), p.text);
                        let ans = run.session.state().quiz;
                        for (i, o) in q.options.iter().enumerate() {
                            let chosen = ans == Some(i);
                            let right = chosen && i == q.answer;
                            let w = ui.available_width();
                            let (r, resp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
                            let (fill, stroke) = if right {
                                (
                                    Color32::from_rgb(0x1C, 0x2A, 0x1F),
                                    Color32::from_rgb(0x2F, 0x5A, 0x3A),
                                )
                            } else if chosen {
                                (p.accent_bg2, p.accent_border)
                            } else if resp.hovered() {
                                (p.bg_hover, p.border_strong)
                            } else {
                                (p.bg_card, p.border_strong)
                            };
                            ui.painter().rect(
                                r,
                                CornerRadius::same(6),
                                fill,
                                Stroke::new(1.0, stroke),
                                StrokeKind::Inside,
                            );
                            ui.painter().text(
                                pos2(r.left() + 12.0, r.center().y),
                                Align2::LEFT_CENTER,
                                o,
                                font::ui(13.0),
                                p.text_dim,
                            );
                            let oo = o.clone();
                            resp.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::Role::RadioButton,
                                    true,
                                    chosen,
                                    &oo,
                                )
                            });
                            if resp.clicked() {
                                cmd = Some(Action::Quiz(i));
                            }
                        }
                        if let Some(a) = ans {
                            let (t, c) = if a == q.answer {
                                (format!("Right. {}", q.explain), GREEN)
                            } else {
                                ("Not that one — try again.".to_string(), p.accent_soft)
                            };
                            label(ui, t.trim(), font::ui(13.0), c);
                        }
                    });
                }
                // ---- code
                if step.code.is_some() {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        label(ui, "Your sketch", font::semibold(12.0), p.text_muted);
                        if let Some(doc) = run.code.as_mut() {
                            egui::Frame::new()
                                .fill(p.bg_canvas)
                                .stroke(Stroke::new(1.0, p.border))
                                .corner_radius(CornerRadius::same(8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.set_height(260.0);
                                    CodeEditor::new("lesson-code")
                                        .breakpoints(false)
                                        .font(12.0, 18.0)
                                        .show(ui, doc, &mut run.code_view);
                                });
                        }
                        ui.horizontal(|ui| {
                            let busy = run.build.is_some();
                            if Btn::amber(if busy {
                                "Compiling…"
                            } else {
                                "Compile & upload"
                            })
                            .icon(Icon::Upload)
                            .enabled(!busy)
                            .show(ui)
                            .clicked()
                            {
                                cmd = Some(Action::Compile);
                            }
                            if run.reference_image().is_some()
                                && Btn::ghost("Load the reference build").show(ui).clicked()
                            {
                                cmd = Some(Action::Reference);
                            }
                        });
                        if !run.build_log.is_empty() {
                            let tail: Vec<&String> = run.build_log.iter().rev().take(6).collect();
                            for l in tail.into_iter().rev() {
                                let col = if l.contains("error") || l.contains("didn't") {
                                    p.danger_soft
                                } else if l.starts_with("Flashed") {
                                    p.ok
                                } else {
                                    p.text_muted
                                };
                                label(ui, l, font::mono(11.5), col);
                            }
                        }
                    });
                }
                // ---- measure: the live reading
                if let Some(m) = &step.measure
                    && step.table.is_none()
                {
                    let text = match meter {
                        Some(v) => format!("{} {}", bb_lessons::text::smart(v), m.unit),
                        None => "----".into(),
                    };
                    egui::Frame::new()
                        .fill(p.bg_card)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                label(ui, "Meter", font::ui(12.0), p.text_muted);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let r = label(ui, &text, font::mono_medium(15.0), p.text);
                                        let t2 = format!("Meter reading {text}");
                                        r.widget_info(|| {
                                            egui::WidgetInfo::labeled(egui::Role::Label, true, &t2)
                                        });
                                    },
                                );
                            });
                        });
                }
                // ---- goals (steps without a measurements table)
                if step.table.is_none() && !run.goals.is_empty() {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 6.0;
                        label(ui, "Goals", font::semibold(12.0), p.text_muted);
                        for g in &run.goals {
                            ui.horizontal(|ui| {
                                let (r, _) =
                                    ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                                if g.ok {
                                    ui.painter().circle_filled(
                                        r.center(),
                                        8.0,
                                        Color32::from_rgb(0x1C, 0x2A, 0x1F),
                                    );
                                    Icon::Check.paint(ui.painter(), r.shrink(4.0), GREEN);
                                } else {
                                    ui.painter().circle_stroke(
                                        r.center(),
                                        7.0,
                                        Stroke::new(1.0, p.border_strong),
                                    );
                                }
                                let t = if g.detail.is_empty() {
                                    g.label.clone()
                                } else {
                                    format!("{} · {}", g.label, g.detail)
                                };
                                label(
                                    ui,
                                    &t,
                                    font::ui(12.5),
                                    if g.ok { p.text_dim } else { p.text_muted },
                                );
                            });
                        }
                    });
                }
                // ---- hints
                if !step.hints.is_empty() {
                    if Btn::plain(if run.show_hint {
                        "Hide the hint"
                    } else {
                        "Need a hint?"
                    })
                    .icon(Icon::Lightbulb)
                    .show(ui)
                    .clicked()
                    {
                        run.show_hint = !run.show_hint;
                    }
                    if run.show_hint {
                        for h in &step.hints {
                            label(ui, h, font::ui(13.0), p.accent_soft);
                        }
                    }
                }
                // ---- measurements table
                if let Some(t) = &step.table {
                    let (passed, cp) = run.session.checkpoint();
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        ui.horizontal(|ui| {
                            label(ui, &t.title, font::semibold(12.0), p.text_muted);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    label(
                                        ui,
                                        &cp,
                                        font::ui(12.0),
                                        if passed { GREEN } else { p.text_muted },
                                    );
                                },
                            );
                        });
                        let rows = run.session.state().rows.clone();
                        egui::Frame::new()
                            .stroke(Stroke::new(1.0, p.border))
                            .corner_radius(CornerRadius::same(8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 0.0;
                                let head: Vec<String> = t.columns.clone();
                                table_row(ui, &head, &[p.text_muted; 3], Some(p.bg_raised), false);
                                if rows.is_empty() {
                                    let dash = vec!["—".to_string(); head.len().max(3)];
                                    table_row(ui, &dash, &[p.text_faint; 3], None, true);
                                }
                                for r in &rows {
                                    let pc = if r.predicted == "—" {
                                        p.text_faint
                                    } else {
                                        GREEN
                                    };
                                    table_row(
                                        ui,
                                        &[r.label.clone(), r.predicted.clone(), r.meter.clone()],
                                        &[p.text_dim, pc, p.text_dim],
                                        None,
                                        true,
                                    );
                                }
                            });
                    });
                }
            });
        // ---- footer: Back / Next
        let foot = Rect::from_min_max(pos2(inner.left(), inner.bottom() - footer_h), inner.max);
        let mut fc = widgets::child_ui(ui, foot);
        fc.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if Btn::ghost("Back")
                .height(footer_h)
                .enabled(cur > 0)
                .show(ui)
                .clicked()
            {
                cmd = Some(Action::Back);
            }
            let passed = run.step_passed();
            let nl = run.session.next_label();
            let w = ui.available_width();
            if primary(ui, &nl, passed, vec2(w, footer_h)).clicked() {
                cmd = Some(Action::Next);
            }
        });
        // ---- apply
        let Some(a) = cmd else { return };
        match a {
            Action::Choose(i) => {
                if let Err(e) = run.choose(i) {
                    app.toasts.error(e);
                }
                run.evaluate(true);
            }
            Action::Math(on) => {
                run.session.set_math(on);
                run.guess.clear();
            }
            Action::Check => {
                if run.check_prediction(tel.as_deref()).is_none() {
                    app.toasts.info("Type a number first.");
                }
            }
            Action::Working => {
                run.session.toggle_working();
            }
            Action::Repair => {
                run.repair();
                run.evaluate(true);
            }
            Action::Quiz(i) => {
                run.session.answer_quiz(i);
                run.evaluate(true);
            }
            Action::Compile => run.compile(app.repaint.clone()),
            Action::Reference => run.flash_reference(),
            Action::Back => {
                if let Some(p) = self.progress.as_mut() {
                    run.back(p);
                }
            }
            Action::Next => {
                let mut prog = self.progress.take().unwrap_or_default();
                let moved = run.next(&mut prog);
                self.progress = Some(prog);
                if moved {
                    run.evaluate(true);
                } else {
                    // lesson finished: on to the next lesson of the track
                    let id = run.session.lesson.id.clone();
                    let track = run.session.lesson.track.clone();
                    let title = run.session.lesson.title.clone();
                    let lessons = self.track_lessons(&track);
                    let next = lessons
                        .iter()
                        .position(|l| l.lesson.id == id)
                        .and_then(|i| lessons.get(i + 1))
                        .map(|l| l.lesson.id.clone());
                    app.toasts.success(format!("Lesson complete: {title}"));
                    if let Some(n) = next {
                        self.open_lesson(app, &n);
                    }
                }
            }
        }
    }
}

enum Action {
    Choose(usize),
    Math(bool),
    Check,
    Working,
    Repair,
    Quiz(usize),
    Compile,
    Reference,
    Back,
    Next,
}

/// The design's `.ghost.primary` button; disabled = the whole button at 45 % opacity.
fn primary(ui: &mut Ui, text: &str, enabled: bool, size: egui::Vec2) -> egui::Response {
    let p = pal(ui);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (r, resp) = ui.allocate_exact_size(size, sense);
    let k = if enabled { 1.0 } else { 0.45 };
    let fill = if enabled && resp.hovered() {
        Color32::from_rgb(0x3A, 0x2A, 0x12)
    } else {
        p.accent_bg
    };
    ui.painter().rect(
        r,
        CornerRadius::same(6),
        fill.gamma_multiply(k),
        Stroke::new(1.0, p.accent_border.gamma_multiply(k)),
        StrokeKind::Inside,
    );
    ui.painter().text(
        r.center(),
        Align2::CENTER_CENTER,
        text,
        font::medium(13.0),
        p.accent_soft.gamma_multiply(k),
    );
    let t = text.to_string();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &t));
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// `.rbtn`: 36 px mono option button, amber when selected.
fn rbtn(ui: &mut Ui, text: &str, on: bool, w: f32) -> egui::Response {
    let p = pal(ui);
    let (r, resp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
    let (fill, stroke, col) = if on {
        (p.accent_bg, p.accent, p.accent_soft)
    } else if resp.hovered() {
        (p.bg_hover, p.border_strong, p.text_dim)
    } else {
        (
            Color32::from_rgb(0x14, 0x14, 0x14),
            p.border_strong,
            p.text_dim,
        )
    };
    ui.painter().rect(
        r,
        CornerRadius::same(6),
        fill,
        Stroke::new(1.0, stroke),
        StrokeKind::Inside,
    );
    ui.painter().text(
        r.center(),
        Align2::CENTER_CENTER,
        text,
        font::mono_medium(12.0),
        col,
    );
    let t = text.to_string();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, &t));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// One `.tr` row of the measurements table (3 mono columns).
fn table_row(
    ui: &mut Ui,
    cells: &[String],
    colors: &[Color32],
    fill: Option<Color32>,
    top_border: bool,
) {
    let p = pal(ui);
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    if let Some(f) = fill {
        ui.painter().rect_filled(
            r,
            CornerRadius {
                nw: 7,
                ne: 7,
                sw: 0,
                se: 0,
            },
            f,
        );
    }
    if top_border {
        ui.painter().line_segment(
            [r.left_top(), r.right_top()],
            Stroke::new(1.0, p.border_faint),
        );
    }
    let n = cells.len().max(1);
    let cw = (w - 24.0) / n as f32;
    for (i, c) in cells.iter().enumerate() {
        ui.painter().text(
            pos2(r.left() + 12.0 + cw * i as f32, r.center().y),
            Align2::LEFT_CENTER,
            c,
            font::mono(12.5),
            colors.get(i).copied().unwrap_or(p.text_dim),
        );
    }
    let a = cells.join(" · ");
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Row, true, &a));
}
