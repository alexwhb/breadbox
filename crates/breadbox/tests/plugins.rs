//! Plugins screen + plugin instruments on the bench (`TEST-5`, `PLUG-4`, `PLUG-6`): browse the
//! catalog, install the 4-channel scope through the permission prompt, open the Handheld DMM,
//! place it on garden-node (its probes take over A0 / GND), read ≈ 1.88 V after autorange,
//! clip the probes across a resistor by dragging the tips, turn the dial to Ω and read it.
//! Saves `docs/screenshots/plugins.png`.

mod common;

use breadbox::BreadboxApp;
use breadbox::canvas::pen::View;
use breadbox::router::Route;
use common::*;
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

type H = Harness<'static, BreadboxApp>;

fn open_example(h: &mut H, name: &str) {
    h.get_by_label(&format!("Open example {name}")).click();
    h.run_steps(4);
}

fn go(h: &mut H, r: Route) {
    h.state_mut().ctx.navigate(r);
    h.run_steps(3);
}

fn view(h: &H) -> View {
    h.state()
        .ctx
        .egui
        .data(|d| d.get_temp::<View>(egui::Id::unique("bench-view")))
        .expect("bench view")
}

fn pin_screen(h: &H, part: &str, pin: &str) -> egui::Pos2 {
    let doc = h.state().ctx.circuit.as_ref().expect("circuit");
    let p = doc.project().part(part).expect("part");
    let def = doc.lib.get(&p.part).expect("def");
    let leads = bb_circuit::Project::bench_leads(def, p).expect("leads");
    let (_, x, y) = leads.iter().find(|(n, _, _)| n == pin).expect("pin");
    view(h).to_screen(*x, *y)
}

fn label_text(h: &H, prefix: &str) -> Option<String> {
    h.query_all_by_label_contains(prefix).next().and_then(|n| {
        let a = n.accesskit_node();
        a.label().or_else(|| a.value())
    })
}

/// Drags with the primary button from `a` to `b` in a few steps.
fn drag(h: &mut H, a: egui::Pos2, b: egui::Pos2) {
    h.hover_at(a);
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: a,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(1);
    for i in 1..=8 {
        let t = i as f32 / 8.0;
        h.hover_at(a + (b - a) * t);
        h.run_steps(1);
    }
    h.event(egui::Event::PointerButton {
        pos: b,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(3);
}

#[test]
fn install_scope_place_dmm_read_a0_then_ohms() {
    let mut h = harness();
    open_example(&mut h, "garden-node");
    go(&mut h, Route::Plugins);
    // browse: categories + installed list + catalog cards
    assert!(has_label(&h, "Category Instruments"));
    assert!(has_label(&h, "Category Views & themes"));
    assert!(has_label(&h, "Plugin Handheld DMM"));
    assert!(has_label(&h, "Plugin Soil & weather pack"));
    assert!(has_label(&h, "plugin.json"));
    // install the 4-channel scope: permission prompt first
    h.get_by_label("Install 4-channel oscilloscope").click();
    h.run_steps(3);
    assert!(has_label_containing(&h, "Install 4-channel oscilloscope?"));
    assert!(has_label_containing(&h, "Read voltages and currents"));
    h.get_by_label("Allow and install").click();
    h.run_steps(4);
    assert!(has_label(&h, "Plugin 4-channel oscilloscope"));
    assert!(
        h.state()
            .ctx
            .plugins
            .registry
            .get("breadbox.scope-4ch")
            .is_some_and(|p| p.active())
    );
    // the Handheld DMM detail page
    h.get_by_label("Plugin Handheld DMM").click();
    h.run_steps(3);
    assert!(has_label_containing(&h, "INSTRUMENT · installed"));
    assert!(has_label_containing(&h, "6000 counts"));
    // Place on bench → the plugin DMM takes over the bench meter on A0
    h.get_by_label("Place on bench").click();
    h.run_steps(4);
    assert_eq!(h.state().ctx.route, Route::Workbench);
    {
        let doc = h.state().ctx.circuit.as_ref().unwrap();
        let dmm = doc.project().part("dmm1").expect("dmm1");
        assert_eq!(dmm.part, "dmm_handheld");
    }
    h.get_by_label("Run").click();
    let ok = wait_for(&mut h, 120.0, |h| {
        label_text(h, "DMM1 display:").is_some_and(|t| t.contains("1.8") && t.contains(" V"))
    });
    assert!(
        ok,
        "DMM never read A0: {:?}",
        label_text(&h, "DMM1 display:")
    );
    let t = label_text(&h, "DMM1 display:").unwrap();
    let v: f64 = t
        .trim_start_matches("DMM1 display: ")
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!((v - 1.88).abs() < 0.03, "A0 reads {v}");
    assert!(t.contains("AUTO"), "{t}");
    // the Plugins page shows the live callout + the live front panel
    go(&mut h, Route::Plugins);
    assert!(
        wait_for(&mut h, 30.0, |h| has_label_containing(
            h,
            "Probing A0 (J1 out): meter load shifts the node by −0.4 mV"
        )),
        "no live callout"
    );
    h.state_mut().ctx.toasts.clear();
    screenshot(&mut h, "plugins");
    // back on the bench: a 4k7 resistor on its own, probes dragged onto its leads, dial → Ω
    go(&mut h, Route::Workbench);
    {
        let app = h.state_mut();
        let doc = app.ctx.circuit.as_mut().unwrap();
        let lib = doc.lib.clone();
        let id = doc.editor.add_part(&lib, "resistor", 92.0, 34.0).unwrap();
        doc.editor
            .set_prop(&lib, &id, "resistance", bb_circuit::PropValue::Num(4700.0))
            .unwrap();
        doc.touched();
        assert_eq!(id, "r4");
    }
    h.run_steps(3);
    let tip = |h: &H, label: &str| h.get_by_label(label).rect().center();
    let vo = tip(&h, "DMM1 VΩ probe tip");
    let r1 = pin_screen(&h, "r4", "1");
    drag(&mut h, vo, r1);
    let com = tip(&h, "DMM1 COM probe tip");
    let r2 = pin_screen(&h, "r4", "2");
    drag(&mut h, com, r2);
    {
        let doc = h.state_mut().ctx.circuit.as_mut().unwrap();
        let conn = doc.connectivity();
        assert_eq!(
            conn.net_of("dmm1", "VO"),
            conn.net_of("r4", "1"),
            "VΩ clipped on R4"
        );
        assert_eq!(
            conn.net_of("dmm1", "COM"),
            conn.net_of("r4", "2"),
            "COM clipped on R4"
        );
    }
    h.get_by_label("DMM1 dial Ω").click();
    h.run_steps(2);
    assert!(
        wait_for(&mut h, 120.0, |h| {
            label_text(h, "DMM1 display:").is_some_and(|t| t.contains("kΩ") && t.contains("4.7"))
        }),
        "Ω never read: {:?}",
        label_text(&h, "DMM1 display:")
    );
    assert!(label_text(&h, "DMM1 dial:").unwrap().ends_with("Ω"));
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.path().is_dir() {
            if e.file_name() != "target" && e.file_name() != "src" {
                copy_dir(&e.path(), &dst);
            }
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

fn load_local(h: &mut H, dir: &std::path::Path, name: &str) {
    // SAFETY: the only test in this binary that answers file dialogs
    unsafe { std::env::set_var("BREADBOX_TEST_PICK", dir) };
    h.get_by_label("Load local plugin…").click();
    h.run_steps(2);
    h.get_by_label("Plugin folder…").click();
    h.run_steps(3);
    assert!(
        has_label_containing(h, &format!("Load {name}?")),
        "permission prompt for {name}"
    );
    h.get_by_label("Allow and load").click();
    h.run_steps(4);
    assert!(has_label(h, &format!("Plugin {name}")), "{name} installed");
}

#[test]
fn local_plugins_hot_reload_theme_sdk_docs_and_plugin_analysis() {
    let mut h = harness();
    let dir = repo().join("examples/rc-filter-bode");
    h.state_mut().open_path(&dir);
    h.run_steps(4);
    assert!(h.state().ctx.project.is_some());
    go(&mut h, Route::Plugins);
    // SDK docs
    h.get_by_label("Plugin SDK docs").click();
    h.run_steps(3);
    assert!(has_label(&h, "Breadbox Plugin SDK"));
    h.get_by_label("Back to plugins").click();
    h.run_steps(2);
    // a local analysis plugin (development folder) …
    let work = breadbox::plugins::temp_dir("local-plugins");
    let sens = work.join("sensitivity");
    copy_dir(&repo().join("plugins/examples/sensitivity"), &sens);
    load_local(&mut h, &sens, "DC sensitivity");
    assert!(has_label_containing(&h, "ANALYSIS · installed · local"));
    // … hot-reloads when its files change
    std::thread::sleep(std::time::Duration::from_millis(20));
    let manifest = sens.join("plugin.json");
    let text = std::fs::read_to_string(&manifest)
        .unwrap()
        .replace("per +1 %", "per +1 % (edited)");
    std::fs::write(&manifest, text).unwrap();
    assert!(
        wait_for(&mut h, 10.0, |h| h
            .state()
            .ctx
            .toasts
            .items()
            .iter()
            .any(|t| t.text.starts_with("Reloaded DC sensitivity"))),
        "no hot-reload notice"
    );
    // a theme plugin applies to the tokens
    let sol = work.join("solarized");
    copy_dir(&repo().join("plugins/examples/solarized"), &sol);
    load_local(&mut h, &sol, "Solarized light");
    h.get_by_label("Plugin Solarized light").click();
    h.run_steps(2);
    h.get_by_label("Apply theme").click();
    h.run_steps(3);
    assert!(breadbox::theme::plugin_palette_active());
    assert_eq!(
        breadbox::theme::pal_ctx(&h.state().ctx.egui).bg_app,
        egui::Color32::from_rgb(0xFD, 0xF6, 0xE3)
    );
    h.get_by_label("Stop using theme").click();
    h.run_steps(3);
    assert!(!breadbox::theme::plugin_palette_active());
    assert_eq!(
        h.state().ctx.egui.theme(),
        egui::Theme::Dark,
        "back to the user's theme"
    );
    // the analysis plugin shows up in Pro → Analysis and runs on the netlist
    go(&mut h, Route::Pro(breadbox::router::ProTab::Analysis));
    h.run_steps(4);
    h.get_by_label("Run DC sensitivity").click();
    h.run_steps(4);
    assert!(
        has_label_containing(&h, "DC sensitivity result:"),
        "no analysis output"
    );
}
