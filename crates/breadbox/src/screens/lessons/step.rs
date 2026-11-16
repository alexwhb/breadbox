//! The step panel ("Lesson coach", right column of the design): step bar, title, body,
//! formula card, choice grid, "Do the math myself" + prediction card, damage card, quiz,
//! code editor, the step's bench controls and serial output, goals (ticking live, explained
//! by "Test circuit"), hints that escalate to "Show me" and "Do it for me", "Your
//! measurements", Back / Next — and the success state when the goals pass.

use super::LessonsScreen;
use super::run::LessonRun;
use crate::app::AppCtx;
use crate::theme::{font, pal};
use crate::widgets::{self, Btn, CodeEditor, Icon, markdown};
use bb_lessons::diagnose::{Finding, Fix};
use bb_lessons::format::Action as SolAction;
use bb_lessons::guide::control;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use std::time::Instant;

const GREEN: Color32 = Color32::from_rgb(0x6F, 0xD4, 0x8F);
const GREEN_BG: Color32 = Color32::from_rgb(0x1C, 0x2A, 0x1F);
const GREEN_BORDER: Color32 = Color32::from_rgb(0x2F, 0x5A, 0x3A);
const BODY: Color32 = Color32::from_rgb(0xC7, 0xC7, 0xC7);
const AMBER: Color32 = Color32::from_rgb(0xF2, 0xA3, 0x3A);
const RED: Color32 = Color32::from_rgb(0xF0, 0x5A, 0x4A);

/// 0…1 with a small overshoot (ticks pop in).
fn ease_out_back(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
}

/// A pulsing amber ring around a control "Show me" / "Test circuit" points at.
fn ring(ui: &Ui, r: Rect) {
    let t = ui.input(|i| i.time);
    let k = (0.5 + 0.5 * (t * 4.2).sin()) as f32;
    ui.painter().rect_stroke(
        r.expand(2.0 + 2.5 * k),
        8.0,
        Stroke::new(2.0, AMBER.gamma_multiply(0.45 + 0.5 * k)),
        StrokeKind::Outside,
    );
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
}

/// The panel controls a step's worked solution needs that aren't on the canvas.
#[derive(Debug, Clone, PartialEq)]
enum Ctl {
    World(String),
    Prop(String, String),
    Add(String),
    PhoneConnect(String),
    PhoneTerminal,
    PhoneSend,
    Publish(String),
    Repair,
}

fn controls_of(step: &bb_lessons::Step) -> Vec<Ctl> {
    let mut out: Vec<Ctl> = Vec::new();
    for a in &step.solution {
        let c = match a {
            SolAction::World { key, .. } => Ctl::World(key.clone()),
            SolAction::SetProp { part, prop, .. } => Ctl::Prop(part.clone(), prop.clone()),
            SolAction::AddPart { reference, .. } => Ctl::Add(reference.clone()),
            SolAction::PhoneConnect { name } => Ctl::PhoneConnect(name.clone()),
            SolAction::PhoneTerminal => Ctl::PhoneTerminal,
            SolAction::PhoneSend { .. } => Ctl::PhoneSend,
            SolAction::Publish { topic, .. } => Ctl::Publish(topic.clone()),
            SolAction::Repair if step.damage_title.is_empty() => Ctl::Repair,
            _ => continue,
        };
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Slider range of a world variable.
fn world_range(key: &str) -> (f64, f64, bool) {
    let k = key.to_ascii_lowercase();
    if k.ends_with("lux") {
        (1.0, 2000.0, true)
    } else if k.ends_with("temp_c") {
        (-10.0, 50.0, false)
    } else if k.ends_with(".vwc") || k.ends_with(".rh") {
        (0.0, 60.0, false)
    } else {
        (0.0, 100.0, false)
    }
}

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
        run.track_meter(tel.as_deref());
        run.resolve_pending(tel.as_deref());
        run.auto_record(tel.as_deref());
        run.poll_build();
        run.tick_player();
        run.track_success();
        if run.player.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(30));
        }
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
                // ---- step bar, label, title, payoff, body
                let passed_at = run.passed_at;
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    let w = ui.available_width();
                    let (r, _) = ui.allocate_exact_size(vec2(w, 4.0), Sense::hover());
                    let gap = 4.0;
                    let seg = (w - gap * (n_steps as f32 - 1.0)) / n_steps.max(1) as f32;
                    for i in 0..n_steps {
                        let col = if i < cur || (i == cur && passed_at.is_some()) {
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
                    // the header turns green when the goals pass, with the payoff below
                    let fade = passed_at
                        .map(|t| (t.elapsed().as_secs_f32() / 0.35).clamp(0.0, 1.0))
                        .unwrap_or(0.0);
                    if fade < 1.0 && passed_at.is_some() {
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(16));
                    }
                    let frame = egui::Frame::new()
                        .fill(GREEN_BG.gamma_multiply(fade))
                        .stroke(Stroke::new(1.0, GREEN_BORDER.gamma_multiply(fade)))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(egui::Margin::symmetric(
                            (12.0 * fade) as i8,
                            (10.0 * fade) as i8,
                        ));
                    frame.show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 6.0;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            if passed_at.is_some() {
                                let (cr, _) =
                                    ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                                let k = ease_out_back(fade);
                                ui.painter().circle_filled(cr.center(), 8.0 * k, GREEN);
                                Icon::Check.paint(
                                    ui.painter(),
                                    Rect::from_center_size(cr.center(), vec2(10.0, 10.0) * k),
                                    GREEN_BG,
                                );
                            }
                            let l = run.session.label();
                            let l = if passed_at.is_some() {
                                format!("{l} · done")
                            } else {
                                l
                            };
                            label(
                                ui,
                                &l,
                                font::ui(12.0),
                                if passed_at.is_some() {
                                    GREEN
                                } else {
                                    p.text_muted
                                },
                            );
                        });
                        let t = ui.add(
                            egui::Label::new(
                                egui::RichText::new(&step.title)
                                    .font(font::heading(22.0))
                                    .color(if passed_at.is_some() { GREEN } else { p.text }),
                            )
                            .wrap(),
                        );
                        let tt = step.title.clone();
                        t.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Heading, true, &tt));
                        if passed_at.is_some() && !run.payoff.is_empty() {
                            let r = label(ui, &run.payoff, font::medium(13.5), GREEN);
                            let pt = format!("Done: {}", run.payoff);
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Label, true, &pt)
                            });
                        }
                    });
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
                                    let r = rbtn(ui, &o.label, on, w);
                                    if run.is_marked(&control::choice(i)) {
                                        ring(ui, r.rect);
                                    }
                                    if r.clicked() {
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
                            let r = Btn::ghost(&rl).show(ui);
                            if run.is_marked(control::REPAIR) {
                                ring(ui, r.rect);
                            }
                            if r.clicked() {
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
                                    if run.is_marked(control::MATH) {
                                        ring(ui, r.rect);
                                    }
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
                                    if run.is_marked(control::PREDICT) {
                                        ring(ui, fr);
                                    }
                                    let check = Btn::amber("Check").height(38.0).show(ui).clicked();
                                    if enter || check {
                                        cmd = Some(Action::Check);
                                    }
                                });
                                if run.pending.is_some() {
                                    label(
                                        ui,
                                        "Checking against the meter…",
                                        font::ui(13.0),
                                        p.text_muted,
                                    );
                                } else if let Some(v) = st.verdict {
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
                            // long answers wrap inside the option (min one 36 px line)
                            let galley = ui.painter().layout(
                                o.clone(),
                                font::ui(13.0),
                                p.text_dim,
                                (w - 24.0).max(40.0),
                            );
                            let hgt = (galley.size().y + 18.0).max(36.0);
                            let (r, resp) = ui.allocate_exact_size(vec2(w, hgt), Sense::click());
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
                            ui.painter().galley(
                                pos2(r.left() + 12.0, r.center().y - galley.size().y / 2.0),
                                galley,
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
                            if run.is_marked(&control::quiz(i)) {
                                ring(ui, r);
                            }
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
                            let up = Btn::amber(if busy {
                                "Compiling…"
                            } else {
                                "Compile & upload"
                            })
                            .icon(Icon::Upload)
                            .enabled(!busy)
                            .show(ui);
                            if run.is_marked(control::COMPILE) {
                                ring(ui, up.rect);
                            }
                            if up.clicked() {
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
                                let col = if l.contains("error") || l.starts_with("Compile failed")
                                {
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
                // ---- the step's bench controls + what the MCU prints
                controls_section(ui, run, &step, tel.as_deref(), &mut cmd);
                serial_section(ui, run, tel.as_deref());
                // ---- goals, ticking live; "Test circuit" explains the open ones
                if step.table.is_none() {
                    goals_section(ui, run, &mut cmd);
                    help_section(ui, run, &step, &mut cmd);
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
                    help_section(ui, run, &step, &mut cmd);
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
            let since = run.passed_at.map(|t| t.elapsed().as_secs_f32());
            let r = primary(ui, &nl, passed, since, vec2(w, footer_h));
            let r = if passed {
                r
            } else {
                // what's left, on hover
                let left: Vec<String> = run
                    .goals
                    .iter()
                    .filter(|g| !g.ok)
                    .map(|g| g.label.clone())
                    .collect();
                let tip = if left.is_empty() {
                    "Finish this step first".to_string()
                } else {
                    format!("Still to do: {}", left.join("; "))
                };
                r.on_hover_text(tip)
            };
            if r.clicked() {
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
            Action::TestCircuit => run.test_circuit(),
            Action::Hint => run.next_hint(),
            Action::ShowMe => run.toggle_show_me(),
            Action::DoIt => run.do_it(),
            Action::Fix(Fix::Repair) => {
                if let Some(b) = &run.bench {
                    b.send(bb_sim::Command::RepairAll);
                }
                run.diagnosis = None;
                run.evaluate(true);
            }
            Action::Fix(Fix::Run) => {
                run.set_running(true);
                run.diagnosis = None;
            }
            Action::SetWorld(k, v) => run.set_world(&k, v),
            Action::SetProp(part, prop) => {
                let key = control::prop(&part, &prop);
                let text = run.fields.get(&key).cloned().unwrap_or_default();
                if let Err(e) = run.set_prop_text(&part, &prop, &text) {
                    app.toasts.error(format!("{text}: {e}"));
                }
                run.evaluate(true);
            }
            Action::AddPart(r) => run.add_solution_part(&r),
            Action::PhoneConnect(name) => {
                if let Err(e) = run.phone_connect(&name) {
                    app.toasts.info(e);
                }
            }
            Action::PhoneTerminal => run.radio(bb_sim::radio::RadioCommand::Phone(
                bb_sim::radio::PhoneCommand::TerminalOpen,
            )),
            Action::PhoneSend => {
                let mut t = run
                    .fields
                    .get(control::PHONE_SEND)
                    .cloned()
                    .unwrap_or_default();
                if !t.ends_with('\n') {
                    t.push('\n');
                }
                run.radio(bb_sim::radio::RadioCommand::Phone(
                    bb_sim::radio::PhoneCommand::TerminalSend(t.into_bytes()),
                ));
            }
            Action::Publish(topic) => {
                let payload = run
                    .fields
                    .get(&control::publish(&topic))
                    .cloned()
                    .unwrap_or_default();
                run.radio(bb_sim::radio::RadioCommand::ExplorerPublish {
                    topic,
                    payload: payload.into_bytes(),
                    qos: 0,
                    retain: false,
                });
            }
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
                    // lesson finished: its own small celebration, then "Next lesson"
                    self.completed = Some(run.session.lesson.id.clone());
                    self.completed_at = Some(std::time::Instant::now());
                    run.chime_due = true;
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
    TestCircuit,
    Hint,
    ShowMe,
    DoIt,
    Fix(Fix),
    SetWorld(String, f64),
    SetProp(String, String),
    AddPart(String),
    PhoneConnect(String),
    PhoneTerminal,
    PhoneSend,
    Publish(String),
}

/// A section title (`Goals`, `Controls`).
fn section_title(ui: &mut Ui, text: &str) {
    let p = pal(ui);
    label(ui, text, font::semibold(12.0), p.text_muted);
}

/// The step's bench controls: world variables, part values, parts to add, the phone, MQTT.
fn controls_section(
    ui: &mut Ui,
    run: &mut LessonRun,
    step: &bb_lessons::Step,
    tel: Option<&bb_sim::Telemetry>,
    cmd: &mut Option<Action>,
) {
    let ctls = controls_of(step);
    if ctls.is_empty()
        || run.bench.is_none()
            && !ctls
                .iter()
                .any(|c| matches!(c, Ctl::Prop(..) | Ctl::Add(_)))
    {
        return;
    }
    let p = pal(ui);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        section_title(ui, "Controls");
        for c in &ctls {
            match c {
                Ctl::World(key) => {
                    let (name, unit) = bb_lessons::check::world_name(key);
                    let id = control::world(key);
                    let cur = run
                        .world_value(tel, key)
                        .or_else(|| run.loaded.lesson.bench.world.get(key).copied())
                        .unwrap_or(0.0);
                    let (lo, hi, log) = world_range(key);
                    ui.horizontal(|ui| {
                        let title = bb_lessons::check::sentence(&name);
                        label(ui, &title, font::ui(12.5), p.text_dim);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let v = format!("{} {unit}", bb_lessons::text::smart(cur))
                                .replace(".0 ", " ");
                            label(ui, v.trim(), font::mono(12.0), p.text);
                        });
                    });
                    let mut v = cur.clamp(lo, hi);
                    ui.spacing_mut().slider_width = ui.available_width();
                    let sl = egui::Slider::new(&mut v, lo..=hi)
                        .logarithmic(log)
                        .show_value(false);
                    let r = ui.add(sl);
                    let an = format!("{} ({unit})", bb_lessons::check::sentence(&name));
                    r.widget_info(|| egui::WidgetInfo::slider(true, v, &an));
                    if run.is_marked(&id) {
                        ring(ui, r.rect);
                    }
                    if r.changed() {
                        *cmd = Some(Action::SetWorld(key.clone(), v));
                    }
                }
                Ctl::Prop(part, prop) => {
                    let id = control::prop(part, prop);
                    let unit = run
                        .lib
                        .get(
                            run.doc
                                .project()
                                .part(part)
                                .map(|x| x.part.as_str())
                                .unwrap_or(""),
                        )
                        .and_then(|d| d.prop(prop).map(|p| p.unit.clone()))
                        .unwrap_or_default();
                    let reference = run
                        .doc
                        .project()
                        .part(part)
                        .map(|x| x.reference.clone())
                        .unwrap_or_else(|| part.to_uppercase());
                    if !run.fields.contains_key(&id) {
                        let v = run.doc.project().part(part).and_then(|x| {
                            let d = run.lib.get(&x.part)?;
                            bb_circuit::Project::prop_f64(d, x, prop)
                        });
                        let text = v
                            .map(|v| bb_core::si::format(v, "").replace(' ', ""))
                            .unwrap_or_default();
                        run.fields.insert(id.clone(), text);
                    }
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let title = format!("{reference} {prop}");
                        let g =
                            ui.painter()
                                .layout_no_wrap(title.clone(), font::ui(12.5), p.text_dim);
                        let (lr, _) =
                            ui.allocate_exact_size(vec2(g.size().x, 28.0), Sense::hover());
                        ui.painter().galley(
                            pos2(lr.left(), lr.center().y - g.size().y / 2.0),
                            g,
                            p.text_dim,
                        );
                        let field_w = (ui.available_width() - 60.0).max(60.0);
                        let text = run.fields.entry(id.clone()).or_default();
                        let te = egui::TextEdit::singleline(text)
                            .font(font::mono(13.0))
                            .desired_width(field_w - 24.0);
                        let r = ui.add(te);
                        let an = title.clone();
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::TextInput, true, &an)
                        });
                        label(ui, &unit, font::mono(12.0), p.text_muted);
                        let set = format!("Set {title}");
                        let b = Btn::mini("Set").accessible_name(&set).show(ui);
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if run.is_marked(&id) {
                            ring(ui, r.rect.union(b.rect));
                        }
                        if b.clicked() || enter {
                            *cmd = Some(Action::SetProp(part.clone(), prop.clone()));
                        }
                    });
                }
                Ctl::Add(reference) => {
                    let id = control::add(reference);
                    let have = run
                        .doc
                        .project()
                        .part(&reference.to_ascii_lowercase())
                        .is_some();
                    let text = format!("Add {reference}");
                    let r = Btn::ghost(&text).icon(Icon::Plus).enabled(!have).show(ui);
                    if run.is_marked(&id) && !have {
                        ring(ui, r.rect);
                    }
                    if r.clicked() {
                        *cmd = Some(Action::AddPart(reference.clone()));
                    }
                }
                Ctl::PhoneConnect(name) => {
                    let connected = tel
                        .and_then(|t| t.radio.as_ref())
                        .and_then(|r| r.phone.connected_name.clone());
                    let an = format!("Connect phone to {name}");
                    let tip = format!("Connect the virtual phone to {name}");
                    let r = Btn::ghost(if connected.is_some() {
                        "Phone connected"
                    } else {
                        "Connect phone"
                    })
                    .icon(Icon::Phone)
                    .enabled(connected.is_none())
                    .accessible_name(&an)
                    .tooltip(&tip)
                    .show(ui);
                    if run.is_marked(control::PHONE_CONNECT) {
                        ring(ui, r.rect);
                    }
                    if r.clicked() {
                        *cmd = Some(Action::PhoneConnect(name.clone()));
                    }
                }
                Ctl::PhoneTerminal => {
                    let r = Btn::ghost("Open terminal")
                        .icon(Icon::Terminal)
                        .accessible_name("Open phone terminal")
                        .tooltip("Opens the phone's Bluetooth terminal on the connected device")
                        .show(ui);
                    if run.is_marked(control::PHONE_TERMINAL) {
                        ring(ui, r.rect);
                    }
                    if r.clicked() {
                        *cmd = Some(Action::PhoneTerminal);
                    }
                }
                Ctl::PhoneSend => {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let text = run.fields.entry(control::PHONE_SEND.into()).or_default();
                        let r = ui.add(
                            egui::TextEdit::singleline(text)
                                .font(font::mono(13.0))
                                .hint_text("message")
                                .desired_width(ui.available_width() - 70.0),
                        );
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Phone message")
                        });
                        let b = Btn::mini("Send")
                            .accessible_name("Send from phone")
                            .show(ui);
                        if run.is_marked(control::PHONE_SEND) {
                            ring(ui, r.rect.union(b.rect));
                        }
                        if b.clicked() {
                            *cmd = Some(Action::PhoneSend);
                        }
                    });
                }
                Ctl::Publish(topic) => {
                    let id = control::publish(topic);
                    label(ui, topic, font::mono(11.5), p.text_muted);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let text = run.fields.entry(id.clone()).or_default();
                        let r = ui.add(
                            egui::TextEdit::singleline(text)
                                .font(font::mono(13.0))
                                .hint_text("payload")
                                .desired_width(ui.available_width() - 80.0),
                        );
                        let an = format!("Payload for {topic}");
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::TextInput, true, &an)
                        });
                        let pn = format!("Publish to {topic}");
                        let b = Btn::mini("Publish").accessible_name(&pn).show(ui);
                        if run.is_marked(&id) {
                            ring(ui, r.rect.union(b.rect));
                        }
                        if b.clicked() {
                            *cmd = Some(Action::Publish(topic.clone()));
                        }
                    });
                }
                Ctl::Repair => {
                    let r = Btn::ghost("Repair").icon(Icon::Wrench).show(ui);
                    if run.is_marked(control::REPAIR) {
                        ring(ui, r.rect);
                    }
                    if r.clicked() {
                        *cmd = Some(Action::Fix(Fix::Repair));
                    }
                }
            }
        }
    });
}

/// What the step's goals listen to: the MCU's serial output, MQTT messages, the phone.
fn serial_section(ui: &mut Ui, run: &LessonRun, tel: Option<&bb_sim::Telemetry>) {
    use bb_lessons::format::Check;
    let step = run.session.current();
    let mcu = step.checks.iter().find_map(|c| match c {
        Check::SerialContains { mcu, .. } => Some(mcu.clone()),
        _ => None,
    });
    let topics: Vec<String> = step
        .checks
        .iter()
        .filter_map(|c| match c {
            Check::Mqtt { topic, .. } => Some(topic.clone()),
            _ => None,
        })
        .collect();
    let phone = step.checks.iter().any(|c| matches!(c, Check::Phone { .. }))
        || step
            .solution
            .iter()
            .any(|a| matches!(a, SolAction::PhoneTerminal | SolAction::PhoneSend { .. }));
    let p = pal(ui);
    let boxed = |ui: &mut Ui, title: &str, lines: &[String], prefix: &str| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            section_title(ui, title);
            egui::Frame::new()
                .fill(p.bg_canvas)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 2.0;
                    if lines.is_empty() {
                        label(ui, "Nothing yet", font::mono(11.5), p.text_faint);
                    }
                    for l in lines {
                        let r = ui.add(
                            egui::Label::new(
                                egui::RichText::new(l)
                                    .font(font::mono(11.5))
                                    .color(p.text_dim),
                            )
                            .truncate(),
                        );
                        let t = format!("{prefix}: {l}");
                        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &t));
                    }
                });
        });
    };
    if let Some(mcu) = mcu {
        let reference = run
            .doc
            .project()
            .part(&mcu)
            .map(|x| x.reference.clone())
            .unwrap_or_else(|| mcu.to_uppercase());
        boxed(
            ui,
            &format!("Serial · {reference}"),
            &run.serial_tail,
            "Serial",
        );
    }
    let radio = tel.and_then(|t| t.radio.as_ref());
    if !topics.is_empty() {
        let mut rows: Vec<String> = radio
            .map(|r| {
                r.mqtt
                    .iter()
                    .filter(|m| {
                        topics
                            .iter()
                            .any(|t| bb_lessons::check::topic_matches(t, &m.topic))
                    })
                    .map(|m| format!("{}  {}", m.topic, m.payload))
                    .collect()
            })
            .unwrap_or_default();
        let n = rows.len().saturating_sub(3);
        rows.drain(..n);
        boxed(ui, "MQTT", &rows, "MQTT");
    }
    if phone {
        let mut rows: Vec<String> = radio
            .map(|r| {
                r.phone
                    .terminal
                    .lines()
                    .map(str::trim_end)
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let n = rows.len().saturating_sub(3);
        rows.drain(..n);
        boxed(ui, "Phone", &rows, "Phone");
    }
}

/// What's wrong and the next action, under a goal (or for the bench as a whole).
fn finding_view(ui: &mut Ui, f: &Finding, cmd: &mut Option<Action>) {
    let p = pal(ui);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        let r = label(ui, &f.what, font::ui(13.0), p.text_dim);
        let w = format!("Problem: {}", f.what);
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &w));
        if !f.fix.is_empty() {
            let mut job = egui::text::LayoutJob::default();
            job.append(
                "→ ",
                0.0,
                egui::TextFormat {
                    font_id: font::medium(13.0),
                    color: AMBER,
                    ..Default::default()
                },
            );
            job.append(
                &f.fix,
                0.0,
                egui::TextFormat {
                    font_id: font::medium(13.0),
                    color: p.accent_soft,
                    ..Default::default()
                },
            );
            job.wrap.max_width = ui.available_width();
            let r = ui.label(job);
            let a = format!("Next: {}", f.fix);
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &a));
        }
        if let Some(fix) = f.action {
            let (text, icon) = match fix {
                Fix::Repair => ("Repair", Icon::Wrench),
                Fix::Run => ("Run", Icon::Play),
            };
            if Btn::mini(text).icon(icon).show(ui).clicked() {
                *cmd = Some(Action::Fix(fix));
            }
        }
    });
}

/// Goals with live ticks; after "Test circuit", each open goal says what's wrong.
fn goals_section(ui: &mut Ui, run: &mut LessonRun, cmd: &mut Option<Action>) {
    if run.goals.is_empty() {
        return;
    }
    let p = pal(ui);
    let diag = run.diagnosis.clone();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.horizontal(|ui| {
            section_title(ui, "Goals");
            if let Some(d) = &diag {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let s = d.summary();
                    label(
                        ui,
                        &s,
                        font::ui(12.0),
                        if d.passed { GREEN } else { p.accent_soft },
                    );
                });
            }
        });
        for (i, g) in run.goals.iter().enumerate() {
            let ok_at: Option<Instant> = run.goal_ok_at.get(i).copied().flatten();
            let report = diag
                .as_ref()
                .and_then(|d| d.goals.get(i))
                .filter(|dg| !dg.ok && !g.ok);
            let same = report.is_some_and(|r| r.same_as.is_some());
            let finding = report.and_then(|dg| dg.finding.clone());
            let body = |ui: &mut Ui, cmd: &mut Option<Action>| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    if g.ok {
                        let k = ok_at
                            .map(|t| t.elapsed().as_secs_f32() / 0.3)
                            .unwrap_or(1.0);
                        if k < 1.6 {
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_millis(16));
                            // a ring bursts out as the tick lands
                            let b = (k / 1.6).clamp(0.0, 1.0);
                            ui.painter().circle_stroke(
                                r.center(),
                                8.0 + 8.0 * b,
                                Stroke::new(1.5, GREEN.gamma_multiply(1.0 - b)),
                            );
                        }
                        let s = ease_out_back(k);
                        ui.painter().circle_filled(r.center(), 8.0 * s, GREEN_BG);
                        Icon::Check.paint(
                            ui.painter(),
                            Rect::from_center_size(r.center(), vec2(8.0, 8.0) * s),
                            GREEN,
                        );
                    } else if finding.is_some() {
                        ui.painter()
                            .circle_stroke(r.center(), 7.0, Stroke::new(1.5, RED));
                        ui.painter().text(
                            r.center(),
                            Align2::CENTER_CENTER,
                            "!",
                            font::mono_medium(10.0),
                            RED,
                        );
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
                if let Some(f) = &finding {
                    ui.horizontal(|ui| {
                        ui.add_space(22.0);
                        if same {
                            label(ui, "Same cause as above", font::ui(12.5), p.text_muted);
                        } else {
                            ui.vertical(|ui| finding_view(ui, f, cmd));
                        }
                    });
                }
            };
            if finding.is_some() {
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x17, 0x11, 0x0E))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(0x4A, 0x2A, 0x20)))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 6.0;
                        body(ui, cmd);
                    });
            } else {
                body(ui, cmd);
            }
        }
        // mistakes on the bench that aren't one goal's
        if let Some(d) = &diag
            && !d.extra.is_empty()
        {
            section_title(ui, "Also on the bench");
            for f in &d.extra {
                egui::Frame::new()
                    .stroke(Stroke::new(1.0, Color32::from_rgb(0x4A, 0x2A, 0x20)))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        finding_view(ui, f, cmd);
                    });
            }
        }
    });
}

/// "Test circuit", then help that escalates: hint 1, hint 2 …, Show me, Do it for me.
fn help_section(
    ui: &mut Ui,
    run: &mut LessonRun,
    step: &bb_lessons::Step,
    cmd: &mut Option<Action>,
) {
    let p = pal(ui);
    let passed = run.passed_at.is_some();
    // build / measure / experiment / code / break steps, and any step with circuit or bench
    // goals (a prediction that needs the meter set up); a measurements table's checkpoint
    // already says what's left
    let hands_on = step.table.is_none() && step.is_hands_on()
        || step.table.is_none()
            && step.checks.iter().any(|c| {
                !matches!(
                    c,
                    bb_lessons::format::Check::Prediction
                        | bb_lessons::format::Check::Quiz
                        | bb_lessons::format::Check::Measurements { .. }
                )
            });
    let has_solution = !step.solution.is_empty();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        if hands_on && !passed {
            if Btn::amber("Test circuit")
                .icon(Icon::Probe)
                .full_width()
                .tooltip("Checks every goal now and explains what's missing")
                .show(ui)
                .clicked()
            {
                *cmd = Some(Action::TestCircuit);
            }
            if run.diagnosis.as_ref().is_some_and(|d| d.passed) {
                label(ui, "Every goal holds", font::ui(12.5), GREEN);
            }
        }
        // the escalation row
        if !passed && (has_solution || !step.hints.is_empty()) {
            let n = step.hints.len();
            let more_hints = run.hints_shown < n;
            let asked = run.hints_shown > 0 || run.diagnosis.as_ref().is_some_and(|d| !d.passed);
            // the next suggestion is highlighted once help was asked for
            let suggest = if !asked {
                ""
            } else if more_hints {
                "hint"
            } else if !run.show_me_used {
                "show"
            } else {
                "do"
            };
            // one row: [Hint] [Show me] [Do it for me]; the hint button goes once every hint
            // is out, and the others share its room
            let show_hint = n > 0 && more_hints;
            let cols = show_hint as usize + 2 * has_solution as usize;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let w = ((ui.available_width() - 6.0 * (cols.max(1) as f32 - 1.0))
                    / cols.max(1) as f32)
                    .floor();
                if show_hint {
                    let label = if run.hints_shown == 0 {
                        "Hint"
                    } else {
                        "Next hint"
                    };
                    let an = format!("Hint {} of {n}", run.hints_shown + 1);
                    if Btn::ghost(label)
                        .icon(Icon::Lightbulb)
                        .height(30.0)
                        .font_size(12.0)
                        .min_width(w)
                        .selected(suggest == "hint")
                        .accessible_name(&an)
                        .show(ui)
                        .clicked()
                    {
                        *cmd = Some(Action::Hint);
                    }
                }
                if has_solution {
                    let open = run.show_me.is_some();
                    if Btn::ghost(if open { "Hide" } else { "Show me" })
                        .icon(Icon::Eye)
                        .height(30.0)
                        .font_size(12.0)
                        .min_width(w)
                        .selected(suggest == "show" || open)
                        .accessible_name(if open { "Hide Show me" } else { "Show me" })
                        .tooltip("Draws the next move on the bench and explains why")
                        .show(ui)
                        .clicked()
                    {
                        *cmd = Some(Action::ShowMe);
                    }
                    let busy = run.player.is_some();
                    if Btn::ghost(if busy { "Doing it…" } else { "Do it for me" })
                        .height(30.0)
                        .font_size(12.0)
                        .min_width(w)
                        .enabled(!busy)
                        .selected(suggest == "do")
                        .accessible_name("Do it for me")
                        .tooltip("Does this step for you (⌘Z undoes the wiring)")
                        .show(ui)
                        .clicked()
                    {
                        *cmd = Some(Action::DoIt);
                    }
                }
            });
        }
        // hints so far
        if run.hints_shown > 0 && !passed {
            for (i, h) in step.hints.iter().take(run.hints_shown).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    label(
                        ui,
                        &format!("{}", i + 1),
                        font::mono_medium(12.0),
                        p.text_faint,
                    );
                    let r = label(ui, h, font::ui(13.0), p.accent_soft);
                    let a = format!("Hint {}: {h}", i + 1);
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &a));
                });
            }
        }
        // Show me: the moves (drawn on the bench) and why
        if let Some(sm) = run.show_me.clone() {
            egui::Frame::new()
                .fill(Color32::from_rgb(0x14, 0x11, 0x0C))
                .stroke(Stroke::new(1.0, Color32::from_rgb(0x4A, 0x36, 0x14)))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 6.0;
                    let moves: Vec<_> = sm.visible().collect();
                    for (i, m) in moves.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            label(ui, &format!("{}", i + 1), font::mono_medium(12.0), AMBER);
                            let r = label(ui, &m.text, font::ui(13.0), p.text);
                            let a = format!("Show me: {}", m.text);
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Label, true, &a)
                            });
                        });
                    }
                    if !sm.why.is_empty() {
                        why_line(ui, &sm.why);
                    }
                });
        }
        if let Some(e) = &run.player_error {
            label(
                ui,
                &format!("Couldn't finish: {e}"),
                font::ui(12.5),
                p.danger_soft,
            );
        }
        // done for you: why it works
        if passed && run.did_it {
            let why = if step.why.trim().is_empty() {
                step.hints.last().cloned().unwrap_or_default()
            } else {
                step.why.trim().to_string()
            };
            if !why.is_empty() {
                egui::Frame::new()
                    .fill(GREEN_BG)
                    .stroke(Stroke::new(1.0, GREEN_BORDER))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        why_line(ui, &why);
                    });
            }
        }
    });
}

/// `Why it works` + one line.
fn why_line(ui: &mut Ui, why: &str) {
    let p = pal(ui);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (r, _) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::hover());
        Icon::Lightbulb.paint(
            ui.painter(),
            Rect::from_center_size(r.center(), vec2(14.0, 14.0)),
            AMBER,
        );
        let r = label(ui, why, font::ui(13.0), p.text_dim);
        let a = format!("Why it works: {why}");
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &a));
    });
}

/// The Next button. Disabled: the design's `.ghost.primary` at 45 % opacity. Enabled: the
/// prominent primary action, solid amber, with a glow that pulses for a moment after the
/// goals pass (`since` = seconds since then).
fn primary(
    ui: &mut Ui,
    text: &str,
    enabled: bool,
    since: Option<f32>,
    size: egui::Vec2,
) -> egui::Response {
    let p = pal(ui);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (r, resp) = ui.allocate_exact_size(size, sense);
    if enabled {
        if let Some(t) = since
            && t < 2.4
        {
            let k = (t * 2.6).sin().abs() * (1.0 - t / 2.4);
            ui.painter().rect_stroke(
                r.expand(2.0 + 5.0 * k),
                9.0,
                Stroke::new(2.0, p.accent.gamma_multiply(0.7 * k)),
                StrokeKind::Outside,
            );
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        }
        let fill = if resp.hovered() {
            p.accent_hover
        } else {
            p.accent
        };
        ui.painter().rect(
            r,
            CornerRadius::same(6),
            fill,
            Stroke::new(1.0, p.accent),
            StrokeKind::Inside,
        );
        fit_text(
            ui,
            r,
            text,
            font::semibold,
            13.5,
            Color32::from_rgb(0x1A, 0x12, 0x06),
        );
    } else {
        let k = 0.45;
        ui.painter().rect(
            r,
            CornerRadius::same(6),
            p.accent_bg.gamma_multiply(k),
            Stroke::new(1.0, p.accent_border.gamma_multiply(k)),
            StrokeKind::Inside,
        );
        fit_text(
            ui,
            r,
            text,
            font::medium,
            13.0,
            p.accent_soft.gamma_multiply(k),
        );
    }
    let t = text.to_string();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &t));
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Centred one-line text that fits `r`: a smaller size first, then an ellipsis.
fn fit_text(
    ui: &Ui,
    r: Rect,
    text: &str,
    font: fn(f32) -> egui::FontId,
    size: f32,
    color: Color32,
) {
    let max = r.width() - 20.0;
    let mut s = size;
    let mut g = ui
        .painter()
        .layout_no_wrap(text.to_string(), font(s), color);
    while g.size().x > max && s > 11.5 {
        s -= 0.5;
        g = ui
            .painter()
            .layout_no_wrap(text.to_string(), font(s), color);
    }
    if g.size().x > max {
        let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font(s), color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(max);
        g = ui.painter().layout_job(job);
    }
    let at = r.center() - g.size() / 2.0;
    ui.painter().galley(at, g, color);
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
