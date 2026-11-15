//! World panel extras: movable objects (`WORLD-4`: a hand in front of the HC-SR04, a person
//! walking past the PIR, a magnet at the hall sensor, a wall) and the programmable world
//! (`WORLD-5`: expression scripts like `moisture = 45 - 0.8*t_hours` and per-variable keyframe
//! timelines). Everything edits `project.world` through the undoable editor; `AppCtx` pushes
//! the result into the running bench (`sync_world_program`).

use crate::app::AppCtx;
use crate::canvas::world_objects;
use crate::theme::{font, pal};
use crate::widgets::{self, Btn, Icon, Seg};
use bb_world::{Interp, Keyframe, ObjectKind, Script, Timeline, World, WorldConfig};
use eframe::egui::{self, Color32, Sense, Stroke, Ui, pos2, vec2};

#[derive(Default)]
pub struct WorldProgramUi {
    /// "Program the world" expanded.
    pub open: bool,
    new_script: String,
    new_error: Option<String>,
    /// Script being edited: (index, text, error).
    editing: Option<(usize, String, Option<String>)>,
    /// Selected timeline.
    sel: usize,
    /// Timeline being edited (committed when a field is released).
    draft: Option<(usize, Timeline)>,
}

/// Parts that read world objects, and which kind of object suits them.
fn reader(part: &str) -> Option<&'static [ObjectKind]> {
    match part {
        "hc_sr04" => Some(&[ObjectKind::Hand, ObjectKind::Wall, ObjectKind::Person]),
        "pir" => Some(&[ObjectKind::Hand, ObjectKind::Person]),
        "hall_sensor" => Some(&[ObjectKind::Magnet]),
        _ => None,
    }
}

fn kind_name(k: ObjectKind) -> &'static str {
    match k {
        ObjectKind::Hand => "hand",
        ObjectKind::Person => "person",
        ObjectKind::Wall => "wall",
        ObjectKind::Magnet => "magnet",
    }
}

fn commit(app: &mut AppCtx, cfg: WorldConfig) {
    if let Some(doc) = app.circuit.as_mut() {
        doc.editor.set_world(cfg);
        doc.touched();
    }
}

/// Adds an object near the sensor that reads it (the palette's Hand tile lands here too).
pub fn add_object(app: &mut AppCtx, kind: ObjectKind, at: Option<(f64, f64)>) -> Option<String> {
    let doc = app.circuit.as_ref()?;
    let p = doc.project();
    let mut cfg = p.world.clone();
    let sensor = p
        .parts
        .iter()
        .find(|x| reader(&x.part).is_some_and(|ks| ks.contains(&kind)))
        .and_then(|x| x.bench.as_ref().map(|b| (b.at.x, b.at.y)));
    let centre = {
        let pts: Vec<(f64, f64)> = p
            .parts
            .iter()
            .filter_map(|x| x.bench.as_ref().map(|b| (b.at.x, b.at.y)))
            .collect();
        if pts.is_empty() {
            (0.0, 0.0)
        } else {
            let n = pts.len() as f64;
            (
                pts.iter().map(|q| q.0).sum::<f64>() / n,
                pts.iter().map(|q| q.1).sum::<f64>() / n - 20.0,
            )
        }
    };
    let (sensor, base) = match at {
        Some(a) => (None, a),
        None => (sensor, centre),
    };
    let o = world_objects::new_object(kind, &cfg.objects, sensor, base);
    let id = o.id.clone();
    cfg.objects.push(o);
    commit(app, cfg);
    Some(id)
}

/// The "Objects" section: shown when the bench has a distance / motion / hall sensor or
/// already has objects.
pub fn objects(ui: &mut Ui, app: &mut AppCtx) {
    let p = pal(ui);
    let Some(doc) = app.circuit.as_ref() else {
        return;
    };
    let proj = doc.project();
    let cfg = proj.world.clone();
    let kinds: Vec<ObjectKind> = {
        let mut v: Vec<ObjectKind> = proj
            .parts
            .iter()
            .filter_map(|x| reader(&x.part))
            .flat_map(|ks| ks.iter().copied())
            .collect();
        v.dedup();
        v
    };
    if cfg.objects.is_empty() && kinds.is_empty() {
        return;
    }
    let tel = app.bench.sim().and_then(|s| s.telemetry());
    ui.add_space(4.0);
    widgets::section_header(ui, "Objects").on_hover_text(
        "Things the distance, motion and hall sensors see · drag them on the bench to move them",
    );
    let mut remove = None;
    for o in &cfg.objects {
        let live = tel
            .as_ref()
            .and_then(|t| t.world.objects.iter().find(|(id, _)| id == &o.id))
            .map(|(_, q)| *q)
            .unwrap_or(o.pos);
        let icon = match o.kind {
            ObjectKind::Hand | ObjectKind::Person => Icon::Pir,
            ObjectKind::Wall => Icon::Sonar,
            ObjectKind::Magnet => Icon::Inductor,
        };
        // "hand" / "hand2" say what they are; a renamed object shows its kind
        let what = if o.id.starts_with(kind_name(o.kind)) {
            String::new()
        } else {
            format!("{} · ", kind_name(o.kind))
        };
        ui.horizontal(|ui| {
            icon.show(ui, 14.0, p.text_muted);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let l = format!("Remove {}", o.id);
                if Btn::mini(&l).icon_only(Icon::Trash).show(ui).clicked() {
                    remove = Some(o.id.clone());
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(&o.id)
                            .font(font::medium(12.5))
                            .color(p.text),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("{what}{:.2}, {:.2} m", live.x, live.y))
                                .font(font::mono(11.0))
                                .color(p.text_faint),
                        )
                        .truncate(),
                    );
                });
            });
        });
    }
    let offer: Vec<ObjectKind> = if kinds.is_empty() {
        vec![
            ObjectKind::Hand,
            ObjectKind::Magnet,
            ObjectKind::Person,
            ObjectKind::Wall,
        ]
    } else {
        kinds
    };
    let mut add = None;
    ui.horizontal_wrapped(|ui| {
        for k in offer {
            let label = format!("Add {}", kind_name(k));
            let b = Btn::mini(&label).icon(Icon::Plus).show(ui);
            if b.clicked() {
                add = Some(k);
            }
        }
    });
    if let Some(id) = remove {
        let mut c = cfg.clone();
        c.objects.retain(|o| o.id != id);
        commit(app, c);
    }
    if let Some(k) = add {
        add_object(app, k, None);
    }
}

/// Time with a friendly unit (world seconds in, `90 s` / `2.5 min` / `12 h` out).
pub fn fmt_t(v: f64) -> String {
    if v.abs() >= 3600.0 {
        format!("{} h", trim(v / 3600.0))
    } else if v.abs() >= 60.0 {
        format!("{} min", trim(v / 60.0))
    } else {
        format!("{} s", trim(v))
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Parses `90`, `90 s`, `2.5min`, `12 h`, `1d` into world seconds.
pub fn parse_t(s: &str) -> Option<f64> {
    let s = s.trim().to_ascii_lowercase();
    let (num, mul) = if let Some(n) = s.strip_suffix("min") {
        (n, 60.0)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3600.0)
    } else if let Some(n) = s.strip_suffix('d') {
        (n, 86_400.0)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1.0)
    } else {
        (s.as_str(), 1.0)
    };
    num.trim().parse::<f64>().ok().map(|v| v * mul)
}

/// Variables a timeline can drive.
fn variables(cfg: &WorldConfig) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for pot in &cfg.pots {
        v.push((
            format!("{}.vwc", pot.id),
            format!("{} moisture (% VWC)", pot.name),
        ));
    }
    v.push(("light.ambient_lux".into(), "Ambient light (lx)".into()));
    v.push(("light.cloud_cover".into(), "Cloud cover (0–1)".into()));
    v.push(("air.temp_c".into(), "Air temperature (°C)".into()));
    v.push(("air.rh".into(), "Humidity (%RH)".into()));
    for o in &cfg.objects {
        v.push((format!("{}.x", o.id), format!("{} x (m)", o.id)));
        v.push((format!("{}.y", o.id), format!("{} y (m)", o.id)));
    }
    v
}

fn default_value(cfg: &WorldConfig, var: &str) -> f64 {
    match var {
        "light.ambient_lux" => cfg.light.ambient_lux,
        "light.cloud_cover" => cfg.light.cloud_cover,
        "air.temp_c" => cfg.air.temp_c,
        "air.rh" => cfg.air.rh,
        _ => {
            if let Some((id, f)) = var.split_once('.') {
                if let Some(pot) = cfg.pots.iter().find(|p| p.id == id) {
                    return pot.vwc;
                }
                if let Some(o) = cfg.objects.iter().find(|o| o.id == id) {
                    return if f == "x" { o.pos.x } else { o.pos.y };
                }
            }
            0.0
        }
    }
}

/// "Program the world": scripts + timelines.
pub fn program(ui: &mut Ui, app: &mut AppCtx, st: &mut WorldProgramUi) {
    let p = pal(ui);
    let Some(doc) = app.circuit.as_ref() else {
        return;
    };
    let cfg = doc.project().world.clone();
    let world_secs = app
        .bench
        .sim()
        .and_then(|s| s.telemetry())
        .map(|t| t.world.world_secs)
        .unwrap_or(0.0);
    let n = cfg.scripts.len() + cfg.timelines.len();
    let title = if n == 0 {
        "Program the world".to_string()
    } else {
        format!("Program the world · {n}")
    };
    let chevron = if st.open {
        Icon::ChevronDown
    } else {
        Icon::ChevronRight
    };
    if Btn::plain(&title).icon(chevron).show(ui).clicked() {
        st.open = !st.open;
    }
    if !st.open {
        return;
    }
    let mut new_cfg = cfg.clone();
    let mut changed = false;

    // ---------------------------------------------------------------- scripts
    sub_label(
        ui,
        "Scripts",
        "Each runs every step: `variable = expression`, with t, t_hours, sin() … · See Help → Workbench",
    );
    let mut remove_script = None;
    for (i, s) in cfg.scripts.iter().enumerate() {
        ui.horizontal(|ui| {
            let mut on = s.enabled;
            let cb = ui.checkbox(&mut on, "");
            let l = format!("Script {} enabled", i + 1);
            cb.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, &l));
            if on != s.enabled {
                new_cfg.scripts[i].enabled = on;
                changed = true;
            }
            let mut text = match &st.editing {
                Some((j, t, _)) if *j == i => t.clone(),
                _ => s.source.clone(),
            };
            // leave room for the remove button (mini height) + spacing
            let room = crate::theme::size::MINI_BUTTON + ui.spacing().item_spacing.x + 2.0;
            let te = ui.add(
                egui::TextEdit::singleline(&mut text)
                    .font(font::mono(12.0))
                    .desired_width(ui.available_width() - room),
            );
            let l = format!("Script {}", i + 1);
            te.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, &l));
            if te.changed() {
                st.editing = Some((i, text.clone(), None));
            }
            if te.lost_focus() && st.editing.as_ref().is_some_and(|(j, _, _)| *j == i) {
                match World::check_script(&text) {
                    Ok(()) => {
                        new_cfg.scripts[i].source = text.clone();
                        changed = true;
                        st.editing = None;
                    }
                    Err(e) => st.editing = Some((i, text.clone(), Some(e.to_string()))),
                }
            }
            let l = format!("Remove script {}", i + 1);
            if Btn::mini(&l).icon_only(Icon::Trash).show(ui).clicked() {
                remove_script = Some(i);
            }
        });
        if let Some((j, _, Some(e))) = &st.editing
            && *j == i
        {
            ui.label(egui::RichText::new(e).font(font::ui(11.5)).color(p.danger));
        }
    }
    if let Some(i) = remove_script {
        new_cfg.scripts.remove(i);
        st.editing = None;
        changed = true;
    }
    ui.horizontal(|ui| {
        let te = ui.add(
            egui::TextEdit::singleline(&mut st.new_script)
                .font(font::mono(12.0))
                .hint_text("moisture = 45 - 0.8*t_hours")
                .desired_width(ui.available_width() - 60.0),
        );
        te.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "New script"));
        let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let b = Btn::mini("Add").show(ui);
        b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Add script"));
        if (b.clicked() || enter) && !st.new_script.trim().is_empty() {
            match World::check_script(&st.new_script) {
                Ok(()) => {
                    new_cfg.scripts.push(Script {
                        source: st.new_script.trim().to_string(),
                        enabled: true,
                    });
                    st.new_script.clear();
                    st.new_error = None;
                    changed = true;
                }
                Err(e) => st.new_error = Some(e.to_string()),
            }
        }
    });
    if let Some(e) = &st.new_error {
        ui.label(egui::RichText::new(e).font(font::ui(11.5)).color(p.danger));
    }

    // ---------------------------------------------------------------- timelines
    ui.add_space(6.0);
    sub_label(
        ui,
        "Timelines",
        "Drive a variable from keyframes over world time",
    );
    let vars = variables(&cfg);
    ui.horizontal_wrapped(|ui| {
        for (i, tl) in cfg.timelines.iter().enumerate() {
            let on = st.sel == i;
            let b = Btn::mini(&tl.var).selected(on).show(ui);
            if b.clicked() {
                st.sel = i;
                st.draft = None;
            }
        }
        let (_, picked) = widgets::select(ui, "Add timeline", "Add timeline…", 140.0, |ui| {
            let mut pick = None;
            for (k, label) in &vars {
                if cfg.timelines.iter().any(|t| &t.var == k) {
                    continue;
                }
                if widgets::menu_item(ui, label, false, true) {
                    pick = Some(k.clone());
                }
            }
            pick
        });
        if let Some(Some(var)) = picked {
            let v = default_value(&cfg, &var);
            new_cfg.timelines.push(Timeline {
                var,
                keys: vec![
                    Keyframe { t: 0.0, value: v },
                    Keyframe {
                        t: 3600.0,
                        value: v,
                    },
                ],
                interp: Interp::Linear,
                repeat: None,
                enabled: true,
            });
            st.sel = new_cfg.timelines.len() - 1;
            st.draft = None;
            changed = true;
        }
    });
    if st.sel < cfg.timelines.len() && !changed {
        let i = st.sel;
        let mut tl = match &st.draft {
            Some((j, t)) if *j == i => t.clone(),
            _ => cfg.timelines[i].clone(),
        };
        let mut done = false;
        let mut remove = false;
        egui::Frame::new()
            .fill(p.bg_raised)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.horizontal(|ui| {
                    let mut on = tl.enabled;
                    if widgets::checkbox(ui, &mut on, &tl.var).changed() {
                        tl.enabled = on;
                        done = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let l = format!("Remove timeline {}", tl.var);
                        if Btn::mini(&l).icon_only(Icon::Trash).show(ui).clicked() {
                            remove = true;
                        }
                    });
                });
                let mut interp = tl.interp;
                if widgets::mini_segmented(
                    ui,
                    "Interpolation",
                    &mut interp,
                    &[
                        Seg::new(Interp::Linear, "Linear"),
                        Seg::new(Interp::Step, "Step"),
                        Seg::new(Interp::Smooth, "Smooth"),
                    ],
                ) {
                    tl.interp = interp;
                    done = true;
                }
                ui.horizontal(|ui| {
                    let mut rep = tl.repeat.is_some();
                    if widgets::checkbox(ui, &mut rep, "Repeat every").changed() {
                        tl.repeat =
                            rep.then(|| tl.keys.iter().map(|k| k.t).fold(0.0, f64::max).max(60.0));
                        done = true;
                    }
                    if let Some(r) = tl.repeat.as_mut() {
                        let dv = ui.add(
                            egui::DragValue::new(r)
                                .speed(10.0)
                                .range(1.0..=31_536_000.0)
                                .custom_formatter(|v, _| fmt_t(v))
                                .custom_parser(parse_t),
                        );
                        dv.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::SpinButton, true, "Repeat period")
                        });
                        done |= released(&dv);
                    }
                });
                // keyframes
                let mut del = None;
                for (k, key) in tl.keys.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let a = ui.add(
                            egui::DragValue::new(&mut key.t)
                                .speed(10.0)
                                .range(0.0..=31_536_000.0)
                                .custom_formatter(|v, _| fmt_t(v))
                                .custom_parser(parse_t),
                        );
                        let la = format!("Keyframe {} time", k + 1);
                        a.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::SpinButton, true, &la)
                        });
                        ui.label(egui::RichText::new("→").color(p.text_faint));
                        let b = ui.add(egui::DragValue::new(&mut key.value).speed(0.1));
                        let lb = format!("Keyframe {} value", k + 1);
                        b.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::SpinButton, true, &lb)
                        });
                        done |= released(&a) || released(&b);
                        let lx = format!("Remove keyframe {}", k + 1);
                        if Btn::mini(&lx).icon_only(Icon::Close).show(ui).clicked() {
                            del = Some(k);
                        }
                    });
                }
                if let Some(k) = del {
                    tl.keys.remove(k);
                    done = true;
                }
                let add = Btn::mini("Add keyframe").icon(Icon::Plus).show(ui);
                if add.clicked() {
                    let last = tl.keys.iter().cloned().max_by(|a, b| a.t.total_cmp(&b.t));
                    tl.keys.push(match last {
                        Some(k) => Keyframe {
                            t: k.t + 3600.0,
                            value: k.value,
                        },
                        None => Keyframe {
                            t: 0.0,
                            value: default_value(&cfg, &tl.var),
                        },
                    });
                    done = true;
                }
                preview(ui, &tl, world_secs);
            });
        if remove {
            new_cfg.timelines.remove(i);
            st.sel = 0;
            st.draft = None;
            changed = true;
        } else if done {
            tl.keys.sort_by(|a, b| a.t.total_cmp(&b.t));
            new_cfg.timelines[i] = tl;
            st.draft = None;
            changed = true;
        } else if Some(&tl) != cfg.timelines.get(i) {
            st.draft = Some((i, tl));
        }
    }
    if changed {
        commit(app, new_cfg);
    }
}

/// A small muted sub-heading (`Scripts`, `Timelines`) whose hover text explains it.
fn sub_label(ui: &mut Ui, text: &str, tip: &str) {
    let p = pal(ui);
    ui.label(
        egui::RichText::new(text)
            .font(font::semibold(11.0))
            .color(p.text_muted),
    )
    .on_hover_text(tip);
}

/// A field finished changing (drag released, typed value committed).
fn released(r: &egui::Response) -> bool {
    r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged() && !r.has_focus())
}

/// The timeline's curve over one period (or its keys + 10 %) with the world-time cursor.
fn preview(ui: &mut Ui, tl: &Timeline, world_secs: f64) {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::hover());
    let label = format!("Timeline preview {}", tl.var);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, &label));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, p.bg_sunken);
    if tl.keys.is_empty() {
        return;
    }
    let t_end = tl
        .repeat
        .unwrap_or_else(|| tl.keys.iter().map(|k| k.t).fold(0.0, f64::max) * 1.1)
        .max(1.0);
    let n = 120;
    let vals: Vec<f64> = (0..=n)
        .map(|i| bb_world::eval_timeline(tl, t_end * i as f64 / n as f64))
        .collect();
    let (lo, hi) = vals
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    let span = (hi - lo).max(1e-9);
    let inner = rect.shrink2(vec2(6.0, 8.0));
    let pt = |i: usize, v: f64| {
        pos2(
            inner.left() + inner.width() * i as f32 / n as f32,
            inner.bottom() - inner.height() * ((v - lo) / span) as f32,
        )
    };
    let pts: Vec<_> = vals.iter().enumerate().map(|(i, v)| pt(i, *v)).collect();
    painter.add(egui::Shape::line(pts, Stroke::new(1.5, p.info)));
    // world-time cursor (wrapped into the period when repeating)
    let t_now = match tl.repeat {
        Some(r) if r > 0.0 => world_secs.rem_euclid(r),
        _ => world_secs,
    };
    if t_now <= t_end {
        let x = inner.left() + inner.width() * (t_now / t_end) as f32;
        painter.line_segment(
            [pos2(x, rect.top() + 3.0), pos2(x, rect.bottom() - 3.0)],
            Stroke::new(1.0, p.accent),
        );
    }
    painter.text(
        rect.right_top() + vec2(-6.0, 4.0),
        egui::Align2::RIGHT_TOP,
        format!("{} · {}", trim(hi), fmt_t(t_end)),
        font::mono(10.0),
        p.text_faint,
    );
    painter.text(
        rect.left_bottom() + vec2(6.0, -4.0),
        egui::Align2::LEFT_BOTTOM,
        trim(lo),
        font::mono(10.0),
        Color32::from_gray(120),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_format_and_parse_with_units() {
        assert_eq!(fmt_t(90.0), "1.5 min");
        assert_eq!(fmt_t(43_200.0), "12 h");
        assert_eq!(fmt_t(12.0), "12 s");
        assert_eq!(parse_t("12 h"), Some(43_200.0));
        assert_eq!(parse_t("2.5min"), Some(150.0));
        assert_eq!(parse_t("45"), Some(45.0));
        assert_eq!(parse_t("1d"), Some(86_400.0));
        assert_eq!(parse_t("soon"), None);
    }
}
