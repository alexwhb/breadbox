//! TEST-4 · projects and files (PRJ-1, PRJ-4, PRJ-5, PRO-2…6) through the app crate's own
//! code: Save As of an edited example → reopen → it runs; `.breadbox.zip` export → import →
//! it runs; every example's `.cir` runs in ngspice (when installed) and agrees with bb-spice;
//! an exported `.cir` imports back as a Pro project; Pro → Analysis runs on every example's
//! synced netlist.

use bb_circuit::Project;
use bb_circuit::project::Endpoint;
use bb_spice::{Netlist, PlotKind};
use breadbox::circuit::CircuitDoc;
use breadbox::screens::pro::{model, sync};
use breadbox_e2e::*;
use std::process::Command as Proc;
use std::sync::Arc;

fn light_raw(sim: &Sim) -> f64 {
    sim.data_lines("U1")
        .iter()
        .rev()
        .find_map(|l| field(&l.text, "light_raw"))
        .expect("telemetry lines")
}

/// garden-node with the divider resistor changed to 4.7 kΩ, an extra LED and wire, moister
/// soil — edits made through the shared `CircuitDoc` like the Workbench does.
fn edited_garden() -> CircuitDoc {
    let (_, dir) = example("garden-node");
    let mut doc = CircuitDoc::load(&dir, true).unwrap();
    let lib = doc.lib.clone();
    doc.editor
        .set_prop(&lib, "r1", "resistance", bb_circuit::PropValue::Num(4700.0))
        .unwrap();
    // an LED off the breadboard, its anode wired to 5 V (cathode free: no current)
    let led = doc.editor.add_part(&lib, "led", 140.0, 60.0).unwrap();
    doc.editor
        .add_wire(&lib, Endpoint::pin(&led, "a"), Endpoint::pin("u1", "5V"));
    let mut world = doc.project().world.clone();
    world.pots[0].vwc = 30.0;
    doc.editor.set_world(world);
    doc.touched();
    doc
}

/// Save As (examples are read-only): the folder is copied with its sketches, the in-memory
/// edits go into the copy's project.json, and the copy reopens identical and runs.
#[test]
fn save_as_then_reopen_round_trip_runs() {
    let scratch = TempDir::new("saveas");
    let doc = edited_garden();
    let app_project = breadbox::project::Project::load(&doc.dir).unwrap();
    let dest = scratch.path().join("garden-copy.breadbox");
    let np = app_project.save_as(&dest).unwrap();
    // what the app's Save As does with the in-memory edits
    let mut saved = doc.project().clone();
    breadbox::project::localize_firmware(&mut saved, &doc.dir, &np.dir).unwrap();
    saved.save(&np.dir).unwrap();
    // reopened: identical circuit (the prebuilt image now lives in the copy), sketches copied
    let reopened = CircuitDoc::load(&np.dir, false).unwrap();
    let mut expect = doc.project().clone();
    expect
        .part_mut("u1")
        .unwrap()
        .firmware
        .as_mut()
        .unwrap()
        .image = Some("firmware/prebuilt/garden-node.uno.hex".into());
    assert_eq!(
        reopened.project().to_json(),
        expect.to_json(),
        "project.json round trip"
    );
    assert!(
        np.dir
            .join("firmware/prebuilt/garden-node.uno.elf")
            .exists()
    );
    for f in ["garden-node.ino", "wifi_at.h", "mqtt_lite.h"] {
        let a = std::fs::read(doc.dir.join("firmware/U1/garden-node").join(f)).unwrap();
        let b = std::fs::read(np.dir.join("firmware/U1/garden-node").join(f))
            .unwrap_or_else(|e| panic!("{f}: {e}"));
        assert_eq!(a, b, "{f}");
    }
    let app2 = breadbox::project::Project::load(&np.dir).unwrap();
    assert_eq!(app2.name, "garden-node");
    assert_eq!(app2.bench_targets().len(), 1);
    // the copy runs on its own: its firmware image resolves from the copy's folder
    let p = Project::load(&np.dir).unwrap();
    let opts = bench_options(&p, &np.dir).unwrap();
    assert!(
        opts.firmware.contains_key("u1"),
        "the copy's firmware.image {:?} does not resolve from {}",
        p.part("u1").unwrap().firmware,
        np.dir.display()
    );
    let mut sim = Sim::spawn(p, lib(), opts);
    sim.run_secs(3.0);
    // 4.7 kΩ under the LDR at 180 lx: light_raw ≈ 1024 · 4.7k / (4.7k + 9.2k) ≈ 346
    let raw = light_raw(&sim);
    assert!(
        (raw - 346.0).abs() < 15.0,
        "edited divider: light_raw {raw}"
    );
    let m = sim
        .data_lines("U1")
        .iter()
        .rev()
        .find_map(|l| field(&l.text, "moisture"))
        .unwrap();
    // the sketch's fixed calibration reads ~1 % low on the real 4.95 V rail (41 % at 42 %)
    assert!((m - 30.0).abs() <= 3.0, "saved world: moisture {m}");
    sim.shutdown();
}

#[test]
fn bundle_export_import_round_trip_runs() {
    let scratch = TempDir::new("bundle");
    let doc = edited_garden();
    let zip = scratch.path().join("garden-node.breadbox.zip");
    breadbox::export::bundle_zip(&doc, &zip).unwrap();
    assert!(std::fs::metadata(&zip).unwrap().len() > 1000);
    let projects = scratch.path().join("projects");
    std::fs::create_dir_all(&projects).unwrap();
    let dest = breadbox::export::import_bundle(&zip, &projects).unwrap();
    assert_eq!(dest.file_name().unwrap(), "garden-node.breadbox");
    // importing again does not overwrite
    let dest2 = breadbox::export::import_bundle(&zip, &projects).unwrap();
    assert_eq!(dest2.file_name().unwrap(), "garden-node-2.breadbox");
    let p = Project::load(&dest).unwrap();
    let mut expect = doc.project().clone();
    // the prebuilt image is bundled and re-pointed into the project
    for part in expect.parts.iter_mut() {
        if let Some(fw) = part.firmware.as_mut()
            && let Some(img) = fw.image.as_mut()
        {
            *img = format!(
                "firmware/prebuilt/{}",
                std::path::Path::new(img)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
            );
        }
    }
    assert_eq!(p.to_json(), expect.to_json());
    assert!(
        dest.join("firmware/U1/garden-node/garden-node.ino")
            .exists()
    );
    let opts = bench_options(&p, &dest).unwrap();
    assert!(opts.firmware.contains_key("u1"), "bundled image loads");
    let mut sim = Sim::spawn(p, lib(), opts);
    sim.run_secs(3.0);
    assert!((light_raw(&sim) - 346.0).abs() < 15.0);
    sim.shutdown();
    // BOM + schematic SVG of the same project (Share menu)
    let bom = breadbox::export::bom_csv(doc.project(), &doc.lib);
    assert!(bom.starts_with("References,Qty,Part,Value"), "{bom}");
    assert!(bom.contains("LED"), "{bom}");
}

/// Every example exported as `.cir` (Share → .cir): bb-spice reads it back and runs it; with
/// ngspice installed, ngspice runs it too (`-b -r`, no errors) and its operating point agrees
/// with bb-spice's.
#[test]
fn cir_exports_run_in_ngspice_and_agree_with_bb_spice() {
    let scratch = TempDir::new("cir");
    let ng = ngspice();
    if ng.is_none() {
        eprintln!("ngspice not installed: checking the bb-spice side only");
    }
    for name in example_names() {
        let (p, _) = example(&name);
        let text = breadbox::export::cir(&p, &lib()).unwrap();
        let nl = Netlist::parse(&text).unwrap_or_else(|e| panic!("{name}: {e}\n{text}"));
        let ours = nl
            .run()
            .unwrap_or_else(|e| panic!("{name}: bb-spice: {e}\n{text}"));
        let Some(ng) = &ng else { continue };
        let cir = scratch.path().join(format!("{name}.cir"));
        let raw = scratch.path().join(format!("{name}.raw"));
        std::fs::write(&cir, &text).unwrap();
        let out = Proc::new(ng)
            .arg("-b")
            .arg("-r")
            .arg(&raw)
            .arg(&cir)
            .env("SPICE_ASCIIRAWFILE", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.status.success(),
            "{name}: ngspice failed:\n{log}\n{text}"
        );
        let bad: Vec<&str> = log
            .lines()
            .filter(|l| {
                let l = l.to_ascii_lowercase();
                l.contains("error") || l.contains("unknown parameter") || l.contains("fatal")
            })
            .collect();
        assert!(bad.is_empty(), "{name}: {bad:#?}\n{text}");
        let plots = bb_spice::export::read_raw(&std::fs::read_to_string(&raw).unwrap())
            .unwrap_or_else(|e| panic!("{name}: rawfile: {e:?}"));
        let (Some(theirs), Some(mine)) = (
            plots.iter().find(|p| p.kind == PlotKind::Op),
            ours.plot(PlotKind::Op),
        ) else {
            continue;
        };
        // nodes whose voltage is set by gmin (an off transistor's collector held by picoamps)
        // are ill-conditioned: ngspice and bb-spice place junction gmin slightly differently
        // (pA apart, far inside SPEC-6's 1 nA). Find them by moving gmin in bb-spice itself.
        let low_gmin = text.replacen("\n.end", "\n.options gmin=1e-13\n.end", 1);
        let alt = Netlist::parse(&low_gmin).unwrap().run().unwrap();
        let alt = alt.plot(PlotKind::Op).unwrap();
        let mut compared = 0;
        for v in &theirs.vectors {
            let n = v.name.to_ascii_lowercase();
            let Some(w) = mine.vector(&n) else { continue };
            let (a, b) = (v.reals()[0], w.reals()[0]);
            if n.starts_with("i(") {
                // SPEC-6: 0.1 % or 1 nA
                assert!(
                    (a - b).abs() <= 1e-9 + 1e-3 * a.abs(),
                    "{name}: {n} ngspice {a} vs bb-spice {b}"
                );
                compared += 1;
                continue;
            }
            if !n.starts_with("v(") || n.contains('.') {
                continue;
            }
            let moved = alt
                .vector(&n)
                .map(|x| (x.reals()[0] - b).abs())
                .unwrap_or(0.0);
            if moved > 1e-4 {
                eprintln!("{name}: {n} is gmin-dominated ({b} V, moves {moved} V) — skipped");
                continue;
            }
            // SPEC-6: 0.1 % (or 1 µV)
            assert!(
                (a - b).abs() <= 1e-6 + 1e-3 * a.abs(),
                "{name}: {n} ngspice {a} vs bb-spice {b}"
            );
            compared += 1;
        }
        assert!(compared > 0, "{name}: nothing to compare");
        eprintln!("{name}: {compared} node voltages / source currents agree with ngspice");
    }
}

/// PRJ-4: a `.cir` (here the RC filter's export) imports as a Pro project with an
/// auto-placed schematic, and runs the same.
#[test]
fn cir_import_builds_a_pro_project() {
    let (p, _) = example("rc-filter-bode");
    let text = breadbox::export::cir(&p, &lib()).unwrap();
    let imported = bb_circuit::spice_io::import_cir(&text, "imported", &lib()).unwrap();
    assert_eq!(imported.mode, bb_circuit::project::Mode::Pro);
    assert!(imported.parts.len() >= 3, "{:#?}", imported.parts);
    assert!(
        imported.parts.iter().all(|x| x.schematic.is_some()),
        "auto-placed schematic"
    );
    let again = breadbox::export::cir(&imported, &lib()).unwrap();
    let a = Netlist::parse(&text).unwrap().run().unwrap();
    let b = Netlist::parse(&again).unwrap().run().unwrap();
    let (pa, pb) = (a.plot(PlotKind::Ac).unwrap(), b.plot(PlotKind::Ac).unwrap());
    let va = pa.vector("v(out)").or_else(|| pa.vectors.get(1)).unwrap();
    let vb = pb.vector(&va.name).unwrap();
    for i in 0..va.len() {
        assert!((va.complex_at(i) - vb.complex_at(i)).abs() < 1e-9);
    }
}

/// Pro → Analysis on the netlist synced from each example's circuit (with the live bench's
/// MCU outputs replayed where it has firmware): every one runs without errors or warnings and
/// shows something.
#[test]
fn pro_analyses_run_on_every_synced_example() {
    for name in example_names() {
        let (p, dir) = example(&name);
        let doc = CircuitDoc::load(&dir, true).unwrap();
        // the live bench feeds the MCU replay + bench-probe column (AVR examples are quick)
        let tel = bench_options(&p, &dir)
            .filter(|o| o.host_firmware.is_empty())
            .map(|opts| {
                let mut sim = Sim::spawn(p.clone(), lib(), opts);
                sim.run_secs(2.0);
                let t = sim.tel();
                sim.shutdown();
                t
            });
        let text = sync::synced_netlist(&doc, tel.as_deref()).unwrap();
        let profile = sync::profile_for(&doc);
        let view = model::run(
            &text,
            &format!("{name}.cir"),
            None,
            &profile,
            bb_spice::CancelToken::new(),
            Arc::new(|_| {}),
        )
        .unwrap_or_else(|e| panic!("{name}: {e:?}\n{text}"));
        assert!(view.warnings.is_empty(), "{name}: {:?}", view.warnings);
        assert!(
            view.summary.ends_with("0 convergence warnings"),
            "{name}: {}",
            view.summary
        );
        let shows = view.tran.is_some()
            || view.dc.is_some()
            || view.ac.is_some()
            || view.mc.is_some()
            || !view.op.is_empty();
        assert!(shows, "{name}: the Analysis tab would be empty\n{text}");
        // the netlist is ngspice-runnable through Export .cir as well
        let exported = breadbox::screens::pro::export_text(&text);
        Netlist::parse(&exported).unwrap_or_else(|e| panic!("{name}: export: {e}"));
    }
}

/// garden-node in Pro: the synced `.op` sees the soil sensor and the LDR divider the bench
/// sees, the light sweep crosses the bench's operating point, and editing R1 in the netlist
/// edits the circuit (PRO-2 round trip).
#[test]
fn garden_node_pro_matches_the_bench() {
    let (p, dir) = example("garden-node");
    let mut sim = Sim::spawn(p.clone(), lib(), bench_options(&p, &dir).unwrap());
    sim.run_secs(3.0);
    let tel = sim.tel();
    let bench_a1 = sim.probe("Light sensor → A1").value;
    let bench_a0 = sim.probe("Soil sensor → A0").avg;
    sim.shutdown();
    let mut doc = CircuitDoc::load(&dir, true).unwrap();
    let text = sync::synced_netlist(&doc, Some(&tel)).unwrap();
    let view = model::run(
        &text,
        "garden-node.cir",
        None,
        &sync::profile_for(&doc),
        bb_spice::CancelToken::new(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let op = |q: &str| {
        view.op
            .iter()
            .find(|r| r.quantity.eq_ignore_ascii_case(q))
            .unwrap_or_else(|| panic!("{q} in {:#?}", view.op))
            .raw
    };
    assert!(
        (op("V(A1)") - bench_a1).abs() < 0.03,
        "A1 {} vs bench {bench_a1}",
        op("V(A1)")
    );
    assert!(
        (op("V(A0)") - bench_a0).abs() < 0.03,
        "A0 {} vs bench {bench_a0}",
        op("V(A0)")
    );
    assert!(view.tran.is_some() && view.dc.is_some() && view.mc.is_some());
    let edited = text.replace("r1 a1 0 10000", "r1 a1 0 22k");
    assert_ne!(edited, text);
    match sync::map_edit(doc.project(), &text, &edited) {
        sync::EditMapping::Props(props) => sync::apply(&mut doc, &props).unwrap(),
        other => panic!("{other:?}"),
    }
    let r1 = doc.project().part("r1").unwrap().props["resistance"].as_f64();
    assert_eq!(r1, Some(22_000.0));
}
