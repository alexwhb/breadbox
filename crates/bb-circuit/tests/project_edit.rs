//! Project round trip + migration, undo/redo, copy/paste.

use bb_circuit::edit::Editor;
use bb_circuit::project::{Endpoint, Mode, Project, ProjectError, Quantity};
use bb_circuit::{Connectivity, Library, PropValue};

fn examples() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples"))
}

#[test]
fn every_example_round_trips_byte_for_byte() {
    for e in std::fs::read_dir(examples()).unwrap() {
        let dir = e.unwrap().path();
        let file = dir.join("project.json");
        if !file.exists() {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        let p = Project::from_json(&text).unwrap();
        assert_eq!(p.format, bb_circuit::FORMAT);
        assert_eq!(p.to_json(), text, "{}", dir.display());
    }
}

#[test]
fn save_is_atomic_and_load_reads_it_back() {
    let dir = std::env::temp_dir().join(format!("bb-proj-{}", std::process::id()));
    let mut p = Project::new("scratch", Mode::Playground);
    p.extra
        .insert("future_key".into(), serde_json::json!({"kept": true}));
    p.save(&dir).unwrap();
    assert!(!dir.join(".project.json.tmp").exists());
    let q = Project::load(&dir).unwrap();
    assert_eq!(p, q);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn migrates_breadbox_0_and_rejects_newer() {
    let v0 = r#"{
      "name": "old",
      "circuit": {
        "parts": [{"id": "r1", "ref": "R1", "part": "resistor"},
                  {"id": "d1", "ref": "D1", "part": "led"}],
        "wires": [{"id": "w1", "a": "r1.2", "b": "d1.a", "color": "red"}]
      },
      "environment": {"noise": false}
    }"#;
    let p = Project::from_json(v0).unwrap();
    assert_eq!(p.format, "breadbox/1");
    assert_eq!(p.parts.len(), 2);
    assert_eq!(p.wires[0].a, Endpoint::pin("r1", "2"));
    assert!(!p.world.noise);
    let lib = Library::builtin();
    let c = Connectivity::compute(&p, &lib);
    assert!(c.connected(("r1", "2"), ("d1", "a")));
    let newer = r#"{"format": "breadbox/7", "name": "x"}"#;
    assert!(matches!(
        Project::from_json(newer),
        Err(ProjectError::TooNew(_))
    ));
    assert!(Project::from_json("{nope").is_err());
}

#[test]
fn undo_redo_add_move_prop_wire_delete() {
    let lib = Library::builtin();
    let mut ed = Editor::new(Project::new("t", Mode::Playground));
    let bb = ed.add_part(&lib, "breadboard_400", 10.0, 0.0).unwrap();
    let r = ed.add_part(&lib, "resistor", 14.0, 6.0).unwrap();
    assert_eq!(ed.project.part(&r).unwrap().reference, "R1");
    let e0 = ed.electrical_generation;
    ed.set_prop(&lib, &r, "resistance", PropValue::Text("4k7".into()))
        .unwrap();
    assert_eq!(
        ed.project.part(&r).unwrap().props["resistance"],
        PropValue::Num(4700.0)
    );
    assert!(ed.electrical_generation > e0);
    assert!(
        ed.set_prop(&lib, &r, "resistance", PropValue::Text("abc".into()))
            .is_err()
    );
    assert!(
        ed.set_prop(&lib, &r, "colour", PropValue::Num(1.0))
            .is_err()
    );
    ed.move_parts(std::slice::from_ref(&r), 1.2, 0.9, true);
    let at = ed.project.part(&r).unwrap().bench.as_ref().unwrap().at;
    assert_eq!((at.x, at.y), (15.0, 7.0));
    ed.rotate(&r).unwrap();
    let w = ed.add_wire(&lib, Endpoint::pin(&bb, "t+1"), Endpoint::pin(&r, "1"));
    assert_eq!(ed.project.wire(&w).unwrap().color, "red");
    let wg = ed.add_wire(&lib, Endpoint::pin(&bb, "b-1"), Endpoint::pin(&r, "2"));
    assert_eq!(ed.project.wire(&wg).unwrap().color, "black");
    ed.add_probe("R1 top", Endpoint::pin(&r, "1"), Quantity::Voltage);
    let snapshot = ed.project.clone();
    // deleting the resistor removes its wires and probe
    ed.delete(std::slice::from_ref(&r), &[]);
    assert!(ed.project.part(&r).is_none());
    assert!(ed.project.wires.is_empty());
    assert!(ed.project.probes.is_empty());
    assert!(ed.undo());
    assert_eq!(ed.project, snapshot);
    // unlimited undo back to empty, then redo everything
    while ed.undo() {}
    assert!(ed.project.parts.is_empty());
    while ed.redo() {}
    assert!(ed.project.part(&r).is_none(), "redo replays the delete too");
    assert!(ed.undo());
    assert_eq!(ed.project, snapshot);
    assert_eq!(ed.undo_label(), Some("Add probe"));
}

#[test]
fn copy_paste_and_duplicate_remap_ids_and_refs() {
    let lib = Library::builtin();
    let mut ed = Editor::new(Project::new("t", Mode::Playground));
    let r1 = ed.add_part(&lib, "resistor", 0.0, 0.0).unwrap();
    let d1 = ed.add_part(&lib, "led", 6.0, 0.0).unwrap();
    ed.add_wire(&lib, Endpoint::pin(&r1, "2"), Endpoint::pin(&d1, "a"));
    let clip = ed.copy(&[r1.clone(), d1.clone()]);
    assert_eq!(clip.wires.len(), 1);
    let new = ed.paste(&lib, &clip, 0.0, 10.0);
    assert_eq!(new.len(), 2);
    let refs: Vec<String> = new
        .iter()
        .map(|id| ed.project.part(id).unwrap().reference.clone())
        .collect();
    assert_eq!(refs, vec!["R2", "D2"]);
    let c = Connectivity::compute(&ed.project, &lib);
    assert!(c.connected((&new[0], "2"), (&new[1], "a")));
    assert!(!c.connected((&new[0], "2"), (&d1, "a")));
    let dup = ed.duplicate(&lib, std::slice::from_ref(&r1));
    assert_eq!(ed.project.part(&dup[0]).unwrap().reference, "R3");
    assert!(ed.undo());
    assert!(ed.project.part(&dup[0]).is_none());
    ed.set_reference(&r1, "R10").unwrap();
    assert!(ed.set_reference(&d1, "R10").is_err());
}

#[test]
fn radio_settings_are_one_undo_step_and_survive_save_load() {
    let lib = Library::builtin();
    let mut ed = Editor::new(Project::new("radios", Mode::Playground));
    ed.add_part(&lib, "uno", 0.0, 0.0).unwrap();
    let electrical = ed.electrical_generation;
    let a = serde_json::json!({ "bench_pos": { "x": 3.2, "y": 0.0 }, "conditions": { "latency_ms": 3.0 } });
    let b = serde_json::json!({ "bench_pos": { "x": 7.5, "y": -1.0 }, "conditions": { "latency_ms": 120.0 } });
    ed.set_radios("Move the bench", a.clone());
    ed.set_radios("Network conditions", b.clone());
    // the same value again is not a new step
    ed.set_radios("Network conditions", b.clone());
    assert_eq!(ed.project.radios, b);
    assert_eq!(ed.undo_label(), Some("Network conditions"));
    // not electrical: no bench rebuild
    assert_eq!(ed.electrical_generation, electrical);
    assert!(ed.undo());
    assert_eq!(ed.project.radios, a);
    assert!(ed.undo());
    assert!(ed.project.radios.is_null());
    assert!(ed.redo());
    assert!(ed.redo());
    assert_eq!(ed.project.radios, b);
    assert_eq!(ed.redo_label(), None);
    // saved with the project
    let dir = std::env::temp_dir().join(format!("bb-radios-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    ed.project.save(&dir).unwrap();
    let back = Project::load(&dir).unwrap();
    assert_eq!(back.radios, b);
    let _ = std::fs::remove_dir_all(&dir);
}
