//! Widget kit tests driven through AccessKit (`TEST-5`).

use breadbox::widgets::{self, Seg, Stop, StopSlider};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

fn styled(ui: &mut egui::Ui) {
    breadbox::theme::install(ui.ctx());
}

#[test]
fn segmented_control_switches_value() {
    let mut h = Harness::builder()
        .with_size(egui::vec2(400.0, 80.0))
        .build_ui_state(
            |ui, v: &mut u8| {
                styled(ui);
                widgets::segmented(
                    ui,
                    "View",
                    v,
                    &[Seg::new(0, "Breadboard"), Seg::new(1, "Schematic")],
                );
            },
            0u8,
        );
    h.run();
    h.get_by_label("Schematic").click();
    h.run();
    assert_eq!(*h.state(), 1);
    h.get_by_label("Breadboard").click();
    h.run();
    assert_eq!(*h.state(), 0);
}

#[test]
fn mini_segmented_and_checkbox_and_tabs() {
    #[derive(Default)]
    struct S {
        lib: u8,
        evap: bool,
        tab: usize,
    }
    let mut h = Harness::builder()
        .with_size(egui::vec2(500.0, 200.0))
        .build_ui_state(
            |ui, s: &mut S| {
                styled(ui);
                widgets::mini_segmented(
                    ui,
                    "Library mode",
                    &mut s.lib,
                    &[Seg::new(0, "Simple"), Seg::new(1, "Advanced")],
                );
                widgets::checkbox(ui, &mut s.evap, "Evaporation");
                widgets::tab_strip(ui, &mut s.tab, &["Serial monitor", "Probes & power"]);
            },
            S::default(),
        );
    h.run();
    h.get_by_label("Advanced").click();
    h.get_by_label("Evaporation").click();
    h.get_by_label("Probes & power").click();
    h.run();
    assert_eq!(h.state().lib, 1);
    assert!(h.state().evap);
    assert_eq!(h.state().tab, 1);
}

#[test]
fn stop_slider_reports_value_and_label() {
    let mut h = Harness::builder()
        .with_size(egui::vec2(300.0, 120.0))
        .build_ui_state(
            |ui, v: &mut f64| {
                styled(ui);
                let fmt = |v: f64| format!("{v:.0} % VWC");
                let stops = [
                    Stop {
                        value: 0.0,
                        label: "Bone dry",
                    },
                    Stop {
                        value: 30.0,
                        label: "Field capacity",
                    },
                    Stop {
                        value: 60.0,
                        label: "Saturated",
                    },
                ];
                StopSlider::new("Soil moisture", 0.0..=60.0, &fmt)
                    .stops(&stops)
                    .step(1.0)
                    .show(ui, v);
            },
            42.0,
        );
    h.run();
    assert!(
        h.query_by_label_contains("Soil moisture 42 % VWC")
            .is_some()
    );
    // click near the left end → snaps toward the low end
    let r = h.get_by_label_contains("Soil moisture").rect();
    h.hover_at(egui::pos2(r.left() + 8.0, r.center().y));
    h.drag_at(egui::pos2(r.left() + 8.0, r.center().y));
    h.drop_at(egui::pos2(r.left() + 8.0, r.center().y));
    h.run();
    assert!(*h.state() < 5.0, "{}", h.state());
}

#[test]
fn ghost_and_amber_buttons_click() {
    let mut h = Harness::builder()
        .with_size(egui::vec2(400.0, 80.0))
        .build_ui_state(
            |ui, n: &mut u32| {
                styled(ui);
                ui.horizontal(|ui| {
                    if widgets::Btn::ghost("Verify").show(ui).clicked() {
                        *n += 1;
                    }
                    if widgets::Btn::amber("Upload to sim").show(ui).clicked() {
                        *n += 10;
                    }
                    widgets::Btn::ghost("Disabled").enabled(false).show(ui);
                });
            },
            0u32,
        );
    h.run();
    h.get_by_label("Verify").click();
    h.get_by_label("Upload to sim").click();
    h.run();
    assert_eq!(*h.state(), 11);
}

#[test]
fn code_editor_toggles_breakpoints_and_highlights() {
    let doc = widgets::CodeDoc::new("blink.ino", "void setup() {\n  pinMode(13, OUTPUT);\n}\n");
    let spans = widgets::highlight::highlight(doc.lang, &doc.text);
    assert!(
        spans
            .iter()
            .any(|(r, t)| &doc.text[r.clone()] == "void" && *t == widgets::Tok::Type)
    );
    assert!(
        spans
            .iter()
            .any(|(r, t)| &doc.text[r.clone()] == "pinMode" && *t == widgets::Tok::Function)
    );
    let mut h = Harness::builder()
        .with_size(egui::vec2(600.0, 300.0))
        .build_ui_state(
            |ui, s: &mut (widgets::CodeDoc, widgets::EditorView)| {
                styled(ui);
                widgets::CodeEditor::new("t")
                    .breakpoints(true)
                    .exec_line(Some(2))
                    .show(ui, &mut s.0, &mut s.1);
            },
            (doc, widgets::EditorView::default()),
        );
    h.run();
    h.get_by_label("Breakpoint line 2").click();
    h.run();
    assert!(h.state().0.breakpoints.contains(&2));
    h.get_by_label("Breakpoint line 2").click();
    h.run();
    assert!(h.state().0.breakpoints.is_empty());
}
