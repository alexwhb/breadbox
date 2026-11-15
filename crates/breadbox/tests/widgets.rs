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

/// Renders the whole widget kit (reference for later screens) and compares it against the
/// checked-in snapshot (`tests/snapshots/widget-gallery.png`; `UPDATE_SNAPSHOTS=1` to refresh).
#[test]
fn widget_gallery_snapshot() {
    use breadbox::theme::{font, pal};
    use breadbox::widgets::{Cell, Col, Icon, Row, TableStyle, Tone};
    #[derive(Default)]
    struct G {
        view: u8,
        lib: u8,
        moist: f64,
        lux: f64,
        temp: f64,
        evap: bool,
        noise: bool,
        tab: usize,
        chip: bool,
    }
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_ui_state(
            |ui, g: &mut G| {
                styled(ui);
                let p = pal(ui);
                let r = ui.max_rect();
                breadbox::theme::paint_paper(ui.painter(), r, p);
                egui::Frame::new().inner_margin(24).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(12.0, 14.0);
                    ui.horizontal(|ui| {
                        let (lr, _) =
                            ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
                        widgets::paint_logo(ui.painter(), lr);
                        ui.label(
                            egui::RichText::new("Breadbox widget kit")
                                .font(font::brand(16.0))
                                .color(p.text),
                        );
                        widgets::tag(ui, "Playground", Tone::Neutral);
                        widgets::tag(ui, "Pro", Tone::Info);
                        widgets::tag(ui, "connected", Tone::Ok);
                        widgets::kind_tag(ui, "INSTRUMENT · installed");
                    });
                    ui.horizontal(|ui| {
                        widgets::segmented(
                            ui,
                            "View",
                            &mut g.view,
                            &[
                                Seg::new(0, "Breadboard").icon(Icon::SegBreadboard),
                                Seg::new(1, "Schematic").icon(Icon::SegSchematic),
                            ],
                        );
                        widgets::mini_segmented(
                            ui,
                            "Library mode",
                            &mut g.lib,
                            &[Seg::new(0, "Simple"), Seg::new(1, "Advanced")],
                        );
                        widgets::Btn::amber("Pause").icon(Icon::Pause).show(ui);
                        widgets::Btn::ghost("Verify").show(ui);
                        widgets::Btn::amber("Upload to sim").show(ui);
                        widgets::Btn::ghost("Share").show(ui);
                        widgets::Btn::ghost("Disabled").enabled(false).show(ui);
                        widgets::Btn::danger("Repair all").show(ui);
                        widgets::Btn::plain("Plain").icon(Icon::Folder).show(ui);
                        widgets::filter_chip(ui, "All", true);
                        if widgets::filter_chip(ui, "Has SPICE model", g.chip).clicked() {
                            g.chip = !g.chip;
                        }
                    });
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(300.0);
                            widgets::section_header_row(ui, "World", "Pot 1 · indoor");
                            let f1 = |v: f64| format!("{v:.0} % VWC");
                            let f2 = |v: f64| format!("{v:.0} lx");
                            let f3 = |v: f64| format!("{v:.0} °C");
                            let s1 = [
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
                            let s2 = [
                                Stop {
                                    value: 0.0,
                                    label: "Night",
                                },
                                Stop {
                                    value: 300.0,
                                    label: "Living room",
                                },
                                Stop {
                                    value: 1000.0,
                                    label: "Bright office",
                                },
                            ];
                            StopSlider::new("Soil moisture", 0.0..=60.0, &f1)
                                .stops(&s1)
                                .show(ui, &mut g.moist);
                            StopSlider::new("Ambient light", 0.0..=1000.0, &f2)
                                .stops(&s2)
                                .show(ui, &mut g.lux);
                            StopSlider::new("Air temperature", -10.0..=45.0, &f3)
                                .show(ui, &mut g.temp);
                            widgets::checkbox(ui, &mut g.evap, "Evaporation · ≈ 1.4 %/h");
                            widgets::checkbox(ui, &mut g.noise, "Sensor noise & ADC quantization");
                            widgets::tab_strip(
                                ui,
                                &mut g.tab,
                                &["Serial monitor", "Probes & power"],
                            );
                            widgets::progress(ui, Some(0.62), "Installing arduino:avr");
                        });
                        ui.vertical(|ui| {
                            ui.set_width(300.0);
                            widgets::section_header(ui, "Parts");
                            for row in [
                                [
                                    (Icon::Uno, "UNO R3"),
                                    (Icon::Esp32, "ESP32"),
                                    (Icon::Pico, "Pico"),
                                ],
                                [
                                    (Icon::Breadboard, "Breadboard"),
                                    (Icon::Wire, "Jumper wire"),
                                    (Icon::Resistor, "Resistor"),
                                ],
                                [
                                    (Icon::Led, "LED"),
                                    (Icon::RgbLed, "RGB LED"),
                                    (Icon::Buzzer, "Buzzer"),
                                ],
                            ] {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    for (i, l) in row {
                                        widgets::part_tile(ui, i, l, egui::vec2(88.0, 76.0), false);
                                    }
                                });
                            }
                        });
                        ui.vertical(|ui| {
                            ui.set_width(60.0);
                            widgets::rail_button(ui, Icon::ExpandLeft, "Expand tools", false);
                            widgets::rail_button(ui, Icon::Multimeter, "Multimeter", true);
                            widgets::rail_button(ui, Icon::Oscilloscope, "Oscilloscope", true);
                            widgets::rail_button(ui, Icon::LogicAnalyzer, "Logic analyzer", false);
                            widgets::rail_button(ui, Icon::PowerSupply, "Power supply", false);
                            widgets::rail_button(
                                ui,
                                Icon::FunctionGenerator,
                                "Function generator",
                                false,
                            );
                            widgets::rail_button(ui, Icon::World, "World", false);
                            widgets::rail_button(ui, Icon::Inspector, "Inspector", false);
                        });
                        ui.vertical(|ui| {
                            ui.set_width(560.0);
                            widgets::card_frame(p).show(ui, |ui| {
                                ui.set_width(526.0);
                                ui.label(
                                    egui::RichText::new(".op — operating point")
                                        .font(font::heading(15.0))
                                        .color(p.text),
                                );
                                widgets::data_table(
                                    ui,
                                    "t",
                                    &[
                                        Col::Fr("quantity", 1.4),
                                        Col::Fr("value", 1.0),
                                        Col::Fr("bench probe", 1.0),
                                    ],
                                    &[
                                        Row::new(vec![
                                            Cell::new("V(VCC)"),
                                            Cell::new("4.952 V"),
                                            Cell::new("4.95 V").color(p.text_muted),
                                        ]),
                                        Row::new(vec![
                                            Cell::new("V(A0)"),
                                            Cell::new("1.882 V"),
                                            Cell::new("1.88 V").color(p.text_muted),
                                        ]),
                                    ],
                                    &TableStyle::default(),
                                );
                            });
                            widgets::section_header(ui, "Icons");
                            for chunk in Icon::ALL.chunks(16) {
                                ui.horizontal(|ui| {
                                    for i in chunk {
                                        i.show(ui, 22.0, p.text_dim);
                                    }
                                });
                            }
                        });
                    });
                });
            },
            G {
                moist: 42.0,
                lux: 180.0,
                temp: 21.0,
                evap: true,
                noise: true,
                ..Default::default()
            },
        );
    h.run();
    let img = h.render().expect("render");
    img.save(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots/widgets.png"),
    )
    .unwrap();
    h.snapshot("widget-gallery");
}
