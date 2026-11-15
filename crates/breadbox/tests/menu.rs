//! The native menu bar does what it says (`APP-4`): every item of `platform::MENUS` is sent
//! through its command (exactly what a click on the native item does) and its effect is
//! checked on the real app — dialogs are answered through `BREADBOX_TEST_PICK`.

mod common;

use breadbox::BreadboxApp;
use breadbox::platform::{MENUS, command_for};
use breadbox::project::ProjectMode;
use breadbox::router::{ProTab, Route, ScreenSlot};
use breadbox::screens::workbench::WorkbenchScreen;
use breadbox::theme::ThemeChoice;
use breadbox::widgets::help::HelpTopic;
use common::*;
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::collections::BTreeSet;
use std::path::Path;

type H = Harness<'static, BreadboxApp>;

/// Clicks the menu item `id` (its command) and lets the app handle it.
fn menu(h: &mut H, done: &mut BTreeSet<&'static str>, id: &'static str) {
    let c = command_for(id).unwrap_or_else(|| panic!("menu id {id}"));
    h.state_mut().ctx.command(c);
    h.run_steps(4);
    done.insert(id);
}

/// Like [`menu`] but stops after the frame that handled it (its output holds the viewport
/// commands the item sent).
fn menu_frame(h: &mut H, done: &mut BTreeSet<&'static str>, id: &'static str) {
    let c = command_for(id).unwrap_or_else(|| panic!("menu id {id}"));
    h.state_mut().ctx.command(c);
    h.step();
    done.insert(id);
}

fn pick(p: &Path) {
    // SAFETY: the only test in this binary; nothing else reads the environment concurrently
    unsafe { std::env::set_var("BREADBOX_TEST_PICK", p) };
}

fn route(h: &H) -> Route {
    h.state().ctx.route
}

fn toasted(h: &H, needle: &str) -> bool {
    h.state()
        .ctx
        .toasts
        .items()
        .iter()
        .any(|t| t.text.contains(needle))
}

fn parts(h: &H) -> Vec<String> {
    h.state()
        .ctx
        .circuit
        .as_ref()
        .map(|c| c.project().parts.iter().map(|p| p.id.clone()).collect())
        .unwrap_or_default()
}

fn workbench(h: &mut H) -> &mut WorkbenchScreen {
    h.state_mut()
        .screens
        .get_mut(ScreenSlot::Workbench)
        .and_then(|s| s.as_any_mut())
        .and_then(|a| a.downcast_mut::<WorkbenchScreen>())
        .expect("workbench screen")
}

fn sim_time(h: &mut H) -> f64 {
    h.state_mut()
        .ctx
        .sim()
        .and_then(|s| s.telemetry())
        .map(|t| t.time.as_secs_f64())
        .unwrap_or(0.0)
}

#[test]
fn every_menu_item_does_what_it_says() {
    let scratch = breadbox::plugins::temp_dir("menu");
    let projects = scratch.join("projects");
    std::fs::create_dir_all(&projects).unwrap();
    // SAFETY: as above
    unsafe { std::env::set_var("BREADBOX_PROJECTS_DIR", &projects) };
    let mut done = BTreeSet::new();
    let mut h = harness();

    // Breadbox → About (macOS shows the standard panel; the command names the version)
    menu(&mut h, &mut done, "about");
    assert!(toasted(&h, env!("CARGO_PKG_VERSION")));

    // Help → the bundled docs in the Help window
    menu(&mut h, &mut done, "docs");
    assert_eq!(h.state().ctx.help, Some(HelpTopic::Guide));
    assert!(has_label(&h, "Breadbox Help"));
    assert!(has_label_containing(&h, "circuit playground"));
    menu(&mut h, &mut done, "help-sdk");
    assert_eq!(h.state().ctx.help, Some(HelpTopic::PluginSdk));
    assert!(has_label_containing(&h, "Breadbox Plugin SDK"));
    menu(&mut h, &mut done, "help-lessons");
    assert!(has_label_containing(&h, "Lesson format"));
    h.state_mut().ctx.help = None;
    h.run_steps(2);

    // View → Theme
    menu(&mut h, &mut done, "theme-light");
    assert_eq!(h.state().ctx.theme, ThemeChoice::Light);
    menu(&mut h, &mut done, "theme-dark");
    assert_eq!(h.state().ctx.theme, ThemeChoice::Dark);

    // View → screens that need no project; Workbench without one says so
    menu(&mut h, &mut done, "plugins");
    assert_eq!(route(&h), Route::Plugins);
    menu(&mut h, &mut done, "lessons");
    assert_eq!(route(&h), Route::Lessons);
    menu(&mut h, &mut done, "home");
    assert_eq!(route(&h), Route::Home);

    // File → New Playground / Pro Project: Home with the new-project dialog in that mode
    menu(&mut h, &mut done, "new-pro");
    assert_eq!(route(&h), Route::Home);
    assert!(has_label(&h, "New project"));
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    menu(&mut h, &mut done, "new-playground");
    assert!(has_label(&h, "New project"));
    h.key_press(egui::Key::Escape);
    h.run_steps(3);

    // File → Open Project… (the folder dialog answers examples/blink)
    let blink = h
        .state()
        .ctx
        .resources
        .examples()
        .expect("examples")
        .join("blink");
    pick(&blink);
    menu(&mut h, &mut done, "open");
    assert_eq!(
        h.state().ctx.project.as_ref().map(|p| p.name.clone()),
        Some("blink".into())
    );
    assert_eq!(route(&h), Route::Workbench);
    assert!(h.state().ctx.project.as_ref().unwrap().read_only);

    // View → the project screens
    menu(&mut h, &mut done, "firmware");
    assert_eq!(route(&h), Route::Firmware);
    menu(&mut h, &mut done, "pro");
    assert_eq!(route(&h), Route::Pro(ProTab::Analysis));
    menu(&mut h, &mut done, "workbench");
    assert_eq!(route(&h), Route::Workbench);

    // Simulate → Run / Pause, Reset Bench
    menu(&mut h, &mut done, "run");
    assert!(h.state_mut().ctx.bench_link().is_running());
    assert!(
        wait_for(&mut h, 30.0, |h| sim_time(h) > 0.5),
        "the bench never ran"
    );
    menu(&mut h, &mut done, "run");
    assert!(!h.state_mut().ctx.bench_link().is_running());
    h.run_steps(4);
    let before = sim_time(&mut h);
    menu(&mut h, &mut done, "reset");
    assert!(
        wait_for(&mut h, 10.0, |h| sim_time(h) < before),
        "reset kept the clock at {before}"
    );

    // Simulate → Step One Quantum / Step 1 ms advance the paused bench; Reset MCU Only
    // restarts the firmware without resetting the bench clock
    let t0 = sim_time(&mut h);
    menu(&mut h, &mut done, "step-ms");
    assert!(
        wait_for(&mut h, 10.0, |h| sim_time(h) >= t0 + 0.000_999),
        "Step 1 ms left the clock at {t0}"
    );
    assert!(!h.state_mut().ctx.bench_link().is_running());
    let t1 = sim_time(&mut h);
    menu(&mut h, &mut done, "step-quantum");
    assert!(
        wait_for(&mut h, 10.0, |h| sim_time(h) > t1),
        "Step One Quantum left the clock at {t1}"
    );
    assert!(
        sim_time(&mut h) - t1 < 0.001,
        "a quantum is shorter than 1 ms"
    );
    let t2 = sim_time(&mut h);
    menu(&mut h, &mut done, "reset-mcu");
    menu(&mut h, &mut done, "step-ms");
    assert!(
        wait_for(&mut h, 10.0, |h| h
            .state()
            .ctx
            .bench
            .serial
            .iter()
            .any(|l| l.text.contains("MCU reset"))),
        "Reset MCU Only logs the reset"
    );
    assert!(sim_time(&mut h) >= t2, "the bench clock keeps running");

    // Edit → Cut / Copy / Paste on the bench's parts; Undo / Redo
    let n0 = parts(&h).len();
    let r1 = parts(&h)
        .into_iter()
        .find(|p| p.starts_with('r'))
        .expect("a resistor");
    workbench(&mut h).canvas.selection.parts.insert(r1.clone());
    menu_frame(&mut h, &mut done, "copy");
    // the item asks the window for the clipboard; egui-winit answers with the event
    let asked = |h: &H, c: egui::ViewportCommand| {
        h.output()
            .viewport_output
            .values()
            .any(|v| v.commands.contains(&c))
    };
    assert!(asked(&h, egui::ViewportCommand::RequestCopy));
    h.event(egui::Event::Copy);
    h.run_steps(2);
    menu_frame(&mut h, &mut done, "paste");
    assert!(asked(&h, egui::ViewportCommand::RequestPaste));
    h.event(egui::Event::Paste(String::new()));
    h.run_steps(3);
    assert_eq!(parts(&h).len(), n0 + 1, "paste adds a copy of {r1}");
    menu(&mut h, &mut done, "undo");
    assert_eq!(parts(&h).len(), n0, "undo removes the pasted part");
    menu(&mut h, &mut done, "redo");
    assert_eq!(parts(&h).len(), n0 + 1, "redo pastes it again");
    let pasted: Vec<String> = parts(&h).into_iter().filter(|p| *p != r1).collect();
    let copy_id = pasted.last().unwrap().clone();
    {
        let w = workbench(&mut h);
        w.canvas.selection.clear();
        w.canvas.selection.parts.insert(copy_id.clone());
    }
    menu_frame(&mut h, &mut done, "cut");
    assert!(asked(&h, egui::ViewportCommand::RequestCut));
    h.event(egui::Event::Cut);
    h.run_steps(3);
    assert!(!parts(&h).contains(&copy_id), "cut removes {copy_id}");
    assert_eq!(parts(&h).len(), n0);

    // Edit → Find… focuses the parts search; Select All selects the text it holds
    menu(&mut h, &mut done, "find");
    h.run_steps(2);
    assert!(
        h.state().ctx.text_has_focus(),
        "Find focuses the search field"
    );
    h.get_by_label("Search parts").type_text("led");
    h.run_steps(2);
    menu(&mut h, &mut done, "select-all");
    h.get_by_label("Search parts").type_text("button");
    h.run_steps(3);
    assert!(
        h.query_all_by_value("button").next().is_some(),
        "Select All + typing replaces the search text"
    );
    h.key_press(egui::Key::Escape);
    h.run_steps(2);

    // Simulate → Verify / Upload open the Firmware screen and start there
    menu(&mut h, &mut done, "verify");
    assert_eq!(route(&h), Route::Firmware);
    menu(&mut h, &mut done, "upload");
    assert_eq!(route(&h), Route::Firmware);
    // Simulate → Run Analyses; File → Export .cir…
    menu(&mut h, &mut done, "analyses");
    assert_eq!(route(&h), Route::Pro(ProTab::Analysis));
    let cir = scratch.join("blink.cir");
    pick(&cir);
    menu(&mut h, &mut done, "export-cir");
    let text = std::fs::read_to_string(&cir).expect("exported .cir");
    assert!(text.lines().any(|l| l.trim_start().starts_with(".end")));

    // File → Save on a bundled example saves a copy (examples are read-only); Save As…
    let copy1 = projects.join("blink-copy.breadbox");
    pick(&copy1);
    menu(&mut h, &mut done, "save");
    assert_eq!(h.state().ctx.project.as_ref().unwrap().dir, copy1);
    assert!(!h.state().ctx.project.as_ref().unwrap().read_only);
    assert!(copy1.join("project.json").is_file());
    menu(&mut h, &mut done, "save");
    assert!(toasted(&h, "Saved"));
    let copy2 = projects.join("blink-again.breadbox");
    pick(&copy2);
    menu(&mut h, &mut done, "save-as");
    assert_eq!(h.state().ctx.project.as_ref().unwrap().dir, copy2);

    // File → Import Project Bundle… (a shared .breadbox.zip unpacks and opens)
    let zip = scratch.join("shared.breadbox.zip");
    breadbox::export::bundle_zip(h.state().ctx.circuit.as_ref().unwrap(), &zip).unwrap();
    menu(&mut h, &mut done, "close");
    assert!(h.state().ctx.project.is_none());
    assert_eq!(route(&h), Route::Home);
    pick(&zip);
    menu(&mut h, &mut done, "import-bundle");
    let p = h.state().ctx.project.clone().expect("the bundle opened");
    assert!(p.dir.starts_with(&projects), "{}", p.dir.display());
    assert_eq!(p.mode, ProjectMode::Playground);
    assert_eq!(route(&h), Route::Workbench);

    // File → Import .cir Netlist… opens a SPICE netlist as a new Pro project
    let cir_in = scratch.join("lowpass.cir");
    std::fs::write(
        &cir_in,
        "* RC low-pass\nV1 in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 159n\n.ac dec 20 10 100k\n.end\n",
    )
    .unwrap();
    pick(&cir_in);
    menu(&mut h, &mut done, "import-cir");
    assert_eq!(route(&h), Route::Pro(ProTab::Analysis));
    let p = h.state().ctx.project.clone().expect("the netlist opened");
    assert_eq!(p.mode, ProjectMode::Pro);

    // every Breadbox menu item was exercised
    let all: BTreeSet<&str> = MENUS
        .iter()
        .flat_map(|(_, items)| items.iter().map(|s| s.id))
        .collect();
    let missing: Vec<&&str> = all.difference(&done).collect();
    assert!(missing.is_empty(), "menu items not exercised: {missing:?}");
}
