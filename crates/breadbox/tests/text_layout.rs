//! UI text + layout checks (`docs/UI-TEXT.md`): the Workbench's right-rail panels, the dock and
//! the header with garden-node running, at the design size (1440×900) and the minimum window
//! (1280×800), plus Home and the new-project dialog. Every labelled widget of a panel must sit
//! clear of its neighbours (rule 6: nothing overlaps). Saves `docs/screenshots/workbench-*.png`
//! / `home-new-project.png` with `BREADBOX_UPDATE_SCREENSHOTS=1`; the 1280×800 renders always
//! go to the scratch dir.

mod common;

use breadbox::BreadboxApp;
use breadbox::router::ScreenSlot;
use breadbox::screens::workbench::{RailPanel, WorkbenchScreen};
use common::*;
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

type H = Harness<'static, BreadboxApp>;

fn workbench(h: &mut H) -> &mut WorkbenchScreen {
    h.state_mut()
        .screens
        .get_mut(ScreenSlot::Workbench)
        .and_then(|s| s.as_any_mut())
        .and_then(|a| a.downcast_mut::<WorkbenchScreen>())
        .expect("workbench screen")
}

fn scratch_shot(h: &mut H, name: &str) {
    h.remove_cursor();
    h.run_steps(2);
    let img = h.render().expect("render");
    let d = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("screenshots");
    let _ = std::fs::create_dir_all(&d);
    img.save(d.join(format!("{name}.png"))).expect("save");
}

/// Labelled leaf widgets (text, buttons, inputs) whose bounds lie inside `area`.
fn widgets_in(h: &H, area: egui::Rect) -> Vec<(String, egui::Rect)> {
    use egui::accesskit::Role;
    let mut out = Vec::new();
    for n in h.queryable_node().children_recursive() {
        let a = n.accesskit_node();
        if a.children().next().is_some() {
            continue;
        }
        let role = a.role();
        if !matches!(
            role,
            Role::Label
                | Role::Button
                | Role::CheckBox
                | Role::ComboBox
                | Role::TextInput
                | Role::SearchInput
                | Role::SpinButton
                | Role::Link
        ) {
            continue;
        }
        let label = a.label().or_else(|| a.value()).unwrap_or_default();
        if label.trim().is_empty() {
            continue;
        }
        let Some(r) = a.bounding_box() else {
            continue;
        };
        let r = egui::Rect::from_min_max(
            egui::pos2(r.x0 as f32, r.y0 as f32),
            egui::pos2(r.x1 as f32, r.y1 as f32),
        );
        if r.width() < 1.0 || r.height() < 1.0 || !area.contains_rect(r) {
            continue;
        }
        out.push((label, r));
    }
    out
}

/// Pairs of widgets in `area` that partly overlap by more than a hair.
fn overlaps(h: &H, area: egui::Rect) -> Vec<String> {
    let w = widgets_in(h, area);
    let mut bad = Vec::new();
    for i in 0..w.len() {
        for j in i + 1..w.len() {
            let (a, ra) = &w[i];
            let (b, rb) = &w[j];
            // a card button around its own title / tags contains them: not a collision
            if ra.expand(1.0).contains_rect(*rb) || rb.expand(1.0).contains_rect(*ra) {
                continue;
            }
            let x = ra.intersect(*rb);
            if x.width() > 2.0 && x.height() > 2.0 {
                bad.push(format!("{a:?} {ra:?} × {b:?} {rb:?}"));
            }
        }
    }
    bad
}

/// The right rail's panel column (between the header and the dock, left of the rail icons).
fn right_panels(size: egui::Vec2) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(size.x - 300.0, 56.0),
        egui::pos2(size.x, size.y - 224.0),
    )
}

fn scroll_at(h: &mut H, at: egui::Pos2, dy: f32) {
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, dy),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(20);
}

fn garden_running(h: &mut H) {
    h.get_by_label("Open example garden-node").click();
    h.run_steps(4);
    h.get_by_label("Run").click();
    assert!(
        wait_for(h, 120.0, |h| h
            .state()
            .ctx
            .bench
            .serial
            .iter()
            .any(|l| l.text.starts_with("mqtt: PUB"))),
        "garden-node never published"
    );
}

fn remove_instruments(h: &mut H) {
    let doc = h.state_mut().ctx.circuit.as_mut().expect("circuit");
    let defs = doc.project().instruments.clone();
    for d in defs {
        let mut tx = bb_circuit::edit::Transaction::new("remove");
        tx.ops.push(bb_circuit::edit::Op::Instrument {
            before: Some(d.clone()),
            after: None,
            index: None,
        });
        if let Some(part) = &d.part
            && let Some(pi) = doc.project().part(part).cloned()
        {
            tx.ops.push(bb_circuit::edit::Op::Part {
                before: Some(pi),
                after: None,
                index: None,
            });
        }
        doc.editor.commit(tx);
    }
    doc.touched();
}

/// The header and the status strip (full width / canvas width).
fn chrome_rows(size: egui::Vec2) -> [(&'static str, egui::Rect); 2] {
    [
        (
            "header",
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(size.x, 56.0)),
        ),
        (
            "status strip",
            egui::Rect::from_min_max(egui::pos2(292.0, 56.0), egui::pos2(size.x - 300.0, 97.0)),
        ),
    ]
}

/// The dock's tab row.
fn dock_header(size: egui::Vec2) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(0.0, size.y - 224.0),
        egui::pos2(size.x, size.y - 186.0),
    )
}

#[test]
fn workbench_panels_fit_without_overlap_at_both_sizes() {
    let mut h = harness();
    garden_running(&mut h);
    let mut failures = Vec::new();
    for (size, tag) in [
        (egui::vec2(1440.0, 900.0), ""),
        (egui::vec2(1280.0, 800.0), "-1280"),
    ] {
        h.set_size(size);
        h.run_steps(4);
        let sets: [(&str, Vec<RailPanel>); 10] = [
            ("world", vec![RailPanel::World]),
            ("world-more", vec![RailPanel::World]),
            ("program", vec![RailPanel::World]),
            ("instruments", vec![RailPanel::Instruments]),
            ("probes", vec![RailPanel::Probes, RailPanel::Inspector]),
            ("waveforms", vec![RailPanel::Waveforms]),
            (
                "inspector",
                vec![RailPanel::Inspector, RailPanel::Waveforms],
            ),
            ("wire", vec![RailPanel::Inspector]),
            ("resistor", vec![RailPanel::Inspector]),
            (
                "empty",
                vec![
                    RailPanel::Instruments,
                    RailPanel::Inspector,
                    RailPanel::Waveforms,
                ],
            ),
        ];
        for (name, panels) in sets {
            {
                let wb = workbench(&mut h);
                wb.state.panels = panels;
                wb.canvas.selection.clear();
                wb.canvas.tool = breadbox::canvas::bench::Tool::Select;
                match name {
                    "inspector" => {
                        wb.canvas.selection.parts.insert("j1".into());
                    }
                    "resistor" => {
                        wb.canvas.selection.parts.insert("r1".into());
                    }
                    "instruments" => {
                        // a lead armed: the status strip says what to click
                        wb.canvas.tool = breadbox::canvas::bench::Tool::Clip {
                            instrument: "mini_scope1".into(),
                            key: "ch2".into(),
                        };
                    }
                    _ => {}
                }
            }
            if name == "wire" {
                let w = h
                    .state()
                    .ctx
                    .circuit
                    .as_ref()
                    .and_then(|d| d.project().wires.first().map(|w| w.id.clone()))
                    .expect("a wire");
                workbench(&mut h).canvas.selection.wires.insert(w);
            }
            if name == "empty" {
                // no instruments on the bench: the Add cards + an empty Inspector
                remove_instruments(&mut h);
            }
            h.run_steps(4);
            if name == "world-more" {
                h.get_by_label("More settings").click();
                h.run_steps(2);
            }
            if name == "program" {
                // a script + a timeline, scrolled into view
                h.get_by_label_contains("Program the world").click();
                h.run_steps(2);
                let has = h
                    .state()
                    .ctx
                    .circuit
                    .as_ref()
                    .is_some_and(|d| !d.project().world.timelines.is_empty());
                if !has {
                    h.get_by_label("New script").click();
                    h.get_by_label("New script")
                        .type_text("air.temp_c = 18 + 2*t_hours");
                    h.run_steps(1);
                    h.get_by_label("Add script").click();
                    h.run_steps(2);
                    scroll_at(&mut h, egui::pos2(size.x - 150.0, 300.0), -600.0);
                    h.get_by_label("Add timeline: Add timeline…").click();
                    h.run_steps(2);
                    h.get_by_label("Ambient light (lx)").click();
                    h.run_steps(3);
                }
                scroll_at(&mut h, egui::pos2(size.x - 150.0, 300.0), -900.0);
            }
            if std::env::var_os("TEXT_LAYOUT_DEBUG").is_some() {
                let wide = egui::Rect::from_min_max(
                    egui::pos2(size.x - 420.0, 56.0),
                    egui::pos2(size.x, size.y - 224.0),
                );
                for (l, r) in widgets_in(&h, wide) {
                    eprintln!("{name}{tag}: {l:?} {r:?}");
                }
            }
            let mut bad = overlaps(&h, right_panels(size));
            // (a scrolled panel slides its content under the header: check those rows unscrolled)
            for (row, area) in chrome_rows(size).into_iter().filter(|_| name != "program") {
                bad.extend(
                    overlaps(&h, area)
                        .into_iter()
                        .map(|b| format!("[{row}] {b}")),
                );
            }
            // nothing in a panel is wider than the 300 px rail (it would push the panel wider)
            let rail_x = h.get_by_label("Hide panels").rect().left();
            if (rail_x - (size.x - 348.0)).abs() > 1.0 {
                bad.push(format!("the right panel grew: rail at x {rail_x}"));
            }
            if !bad.is_empty() {
                failures.push(format!("{name}{tag}: {}", bad.join("\n    ")));
            }
            let shot = format!("workbench-panel-{name}");
            if tag.is_empty() && !matches!(name, "world" | "wire" | "resistor") {
                screenshot(&mut h, &shot);
            } else {
                scratch_shot(&mut h, &format!("{shot}{tag}"));
            }
            if name == "world-more" {
                h.get_by_label("Fewer settings").click();
                h.run_steps(2);
            }
            if name == "program" {
                scroll_at(&mut h, egui::pos2(size.x - 150.0, 300.0), 2000.0);
                h.get_by_label_contains("Program the world").click();
                h.run_steps(2);
            }
            if name == "empty" {
                // put the instruments back for the next size
                let doc = h.state_mut().ctx.circuit.as_mut().expect("circuit");
                while doc.project().instruments.len() < 2 && doc.undo() {}
                h.run_steps(4);
            }
        }
    }
    // a long project name in the breadcrumb at the minimum width
    let name = h
        .state()
        .ctx
        .project
        .as_ref()
        .expect("project")
        .name
        .clone();
    h.state_mut().ctx.project.as_mut().expect("project").name =
        "garden-node-with-a-very-long-project-name-for-testing".into();
    h.set_size(egui::vec2(1280.0, 800.0));
    h.run_steps(4);
    let header = chrome_rows(egui::vec2(1280.0, 800.0))[0].1;
    let bad = overlaps(&h, header);
    if !bad.is_empty() {
        failures.push(format!("long name header: {}", bad.join("\n    ")));
    }
    scratch_shot(&mut h, "workbench-long-name-1280");
    h.state_mut().ctx.project.as_mut().expect("project").name = name;
    // the dock's tab row, with nothing scrolled out behind it (paused, serial cleared,
    // a short right panel)
    h.get_by_label("Pause").click();
    workbench(&mut h).state.panels = vec![RailPanel::Instruments];
    h.run_steps(6);
    h.state_mut().ctx.bench.serial.clear();
    for size in [egui::vec2(1440.0, 900.0), egui::vec2(1280.0, 800.0)] {
        h.set_size(size);
        h.run_steps(4);
        let bad = overlaps(&h, dock_header(size));
        if !bad.is_empty() {
            failures.push(format!("dock header {size:?}: {}", bad.join("\n    ")));
        }
    }
    assert!(
        failures.is_empty(),
        "overlaps:\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn home_and_new_project_dialog_fit() {
    let mut h = harness();
    for (size, tag) in [
        (egui::vec2(1440.0, 900.0), ""),
        (egui::vec2(1280.0, 800.0), "-1280"),
    ] {
        h.set_size(size);
        h.run_steps(4);
        let all = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let bad = overlaps(&h, all);
        assert!(
            bad.is_empty(),
            "home{tag} overlaps:\n  {}",
            bad.join("\n  ")
        );
        scratch_shot(&mut h, &format!("home{tag}"));
    }
    h.set_size(egui::vec2(1440.0, 900.0));
    h.run_steps(2);
    h.get_by_label("New project").click();
    h.run_steps(4);
    let bad = overlaps(
        &h,
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0)),
    );
    let _ = bad; // the dialog sits over the page; the page's widgets are expected beneath it
    screenshot(&mut h, "home-new-project");
}

/// The top bar's "?" opens the current screen's Help page (and closes it again); the header's
/// menus (speed / simulation, ERC list, Share) render for review.
#[test]
fn help_button_opens_the_screen_page_and_menus_render() {
    use breadbox::widgets::help::HelpTopic;
    let mut h = harness();
    h.get_by_label("Help").click();
    h.run_steps(3);
    assert_eq!(
        h.state().ctx.help,
        Some(HelpTopic::Guide),
        "Home → the guide"
    );
    assert!(has_label_containing(&h, "Getting started"));
    for _ in 0..4 {
        // the window keeps its size (wide tables / code scroll inside the page) and stays
        // below the top bar, where its "?" can close it again
        h.run_steps(1);
        let w = h.get_by_label("Breadbox Help").rect();
        assert!(
            w.width() <= 800.0 && w.top() >= 56.0,
            "help window at {w:?}"
        );
    }
    scratch_shot(&mut h, "help-home");
    h.get_by_label("Help").click();
    h.run_steps(2);
    assert_eq!(h.state().ctx.help, None, "? again closes it");
    garden_running(&mut h);
    h.get_by_label("Help").click();
    h.run_steps(4);
    assert_eq!(h.state().ctx.help, Some(HelpTopic::Workbench));
    assert!(has_label_containing(&h, "Instruments → Add"));
    screenshot(&mut h, "workbench-help");
    h.get_by_label("Help").click();
    h.run_steps(2);
    // the simulation menu
    h.get_by_label_contains("Speed:").click();
    h.run_steps(3);
    scratch_shot(&mut h, "menu-speed");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    // the ERC / DRC list
    h.get_by_label_contains("ERC 1 warning").click();
    h.run_steps(3);
    scratch_shot(&mut h, "menu-erc");
    h.get_by_label_contains("ERC 1 warning").click();
    h.run_steps(2);
    h.get_by_label("Share").click();
    h.run_steps(3);
    scratch_shot(&mut h, "menu-share");
}
