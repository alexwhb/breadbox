use super::*;
use crate::builder::BenchBuilder;
use crate::project::Mode;

fn lib() -> Arc<Library> {
    Library::builtin()
}

#[test]
fn breadboard_strips_and_rails() {
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
    let bb = b.breadboard("breadboard_830", 0.0, 0.0);
    let r1 = b.two_lead("resistor", "R1", &bb, "a5", "a9", &[]);
    let r2 = b.two_lead("resistor", "R2", &bb, "e5", "f5", &[]);
    let r3 = b.two_lead("resistor", "R3", &bb, "t+3", "t+40", &[]);
    let r4 = b.two_lead("resistor", "R4", &bb, "j9", "b-2", &[]);
    let conn = Connectivity::compute(&b.project, &lib);
    // same column, same half
    assert!(conn.connected((&r1, "1"), (&r2, "1")));
    // across the channel: not connected
    assert!(!conn.connected((&r2, "1"), (&r2, "2")));
    // rail is continuous
    assert!(conn.connected((&r3, "1"), (&r3, "2")));
    // column 9 top vs bottom: separate
    assert!(!conn.connected((&r1, "2"), (&r4, "1")));
    assert_eq!(conn.insertions.len(), 8);
    // holes are reported per net
    let n = conn.net(conn.net_of(&r1, "1").unwrap());
    assert_eq!(n.holes.len(), 5);
    assert_eq!(n.pins.len(), 2);
}

#[test]
fn split_rails_property() {
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
    let bb = b.breadboard("breadboard_830", 0.0, 0.0);
    b.project
        .part_mut(&bb)
        .unwrap()
        .props
        .insert("split_rails".into(), PropValue::Bool(true));
    let r = b.two_lead("resistor", "R1", &bb, "t+3", "t+40", &[]);
    let conn = Connectivity::compute(&b.project, &lib);
    assert!(!conn.connected((&r, "1"), (&r, "2")));
}

#[test]
fn leads_plug_into_board_headers_and_wires_join() {
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
    let u1 = b.offboard("uno", "U1", 0.0, 0.0, &[]);
    // an LED pushed straight into D13 and the GND next to it (the classic trick)
    let uno = lib.get("uno").unwrap();
    let pos = |pin: &str| {
        let l = uno.bench.leads.iter().find(|l| l.pin == pin).unwrap();
        (l.x, l.y)
    };
    let (x13, y13) = pos("D13");
    let (xg, _) = pos("GND.3");
    let mut d = PartInstance::new("d1", "D1", "led").at(x13, y13);
    d.bench
        .as_mut()
        .unwrap()
        .leads
        .insert("k".into(), (xg - x13, 0.0));
    b.project.parts.push(d);
    let conn = Connectivity::compute(&b.project, &lib);
    assert!(conn.connected(("d1", "a"), (&u1, "D13")));
    assert!(conn.connected(("d1", "k"), (&u1, "GND")));
    // UNO GND pins are bonded; SDA is A4
    assert!(conn.connected((&u1, "GND.2"), (&u1, "GND.3")));
    assert!(conn.connected((&u1, "SDA"), (&u1, "A4")));
    assert_eq!(conn.net(conn.net_of(&u1, "GND").unwrap()).name, "0");
    assert_eq!(conn.net(conn.net_of(&u1, "D13").unwrap()).name, "D13");
    assert_eq!(conn.net(conn.net_of(&u1, "5V").unwrap()).name, "VCC");
}

use crate::part::PropValue;
use crate::project::PartInstance;

#[test]
fn garden_node_example_connectivity() {
    let lib = lib();
    let p = Project::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/garden-node"
    ))
    .unwrap();
    assert!(p.check(&lib).is_empty(), "{:?}", p.check(&lib));
    let c = Connectivity::compute(&p, &lib);
    assert!(c.connected(("u1", "D9"), ("r2", "1")));
    assert!(c.connected(("r2", "2"), ("d1", "a")));
    assert!(c.connected(("d1", "k"), ("u1", "GND")));
    assert!(c.connected(("ldr1", "2"), ("r1", "1")));
    assert!(c.connected(("ldr1", "2"), ("u1", "A1")));
    assert!(c.connected(("ldr1", "1"), ("u1", "5V")));
    assert!(c.connected(("j1", "AOUT"), ("u1", "A0")));
    assert!(c.connected(("u2", "RX"), ("u1", "D2")));
    assert!(c.connected(("u2", "VCC"), ("u1", "3V3")));
    assert!(c.connected(("u2", "EN"), ("u1", "3V3")));
    assert!(!c.connected(("u1", "D9"), ("u1", "D8")));
    // the bench is fully wired: no ratsnest
    assert!(c.ratsnest_bench.is_empty(), "{:?}", c.ratsnest_bench);
    // net names follow the MCU pins
    for (pin, name) in [
        ("A0", "A0"),
        ("A1", "A1"),
        ("D9", "D9"),
        ("D8", "D8"),
        ("5V", "VCC"),
    ] {
        assert_eq!(c.net(c.net_of("u1", pin).unwrap()).name, name);
    }
}

#[test]
fn schematic_wires_labels_and_ratsnest() {
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
    let bb = b.breadboard("breadboard_400", 0.0, 0.0);
    let r1 = b.two_lead("resistor", "R1", &bb, "a5", "a9", &[]);
    let r2 = b.two_lead("resistor", "R2", &bb, "a20", "a24", &[]);
    for (id, x, y) in [(&r1, 10.0, 10.0), (&r2, 30.0, 10.0)] {
        b.project.part_mut(id).unwrap().schematic = Some(crate::geometry::Placement::at(x, y));
    }
    // schematic says R1.2 — R2.1, but the bench does not
    let a = b.anchor(&r1, "2");
    let c = b.anchor(&r2, "1");
    b.sch_wire(vec![a, (a.0 + 5.0, a.1), c]);
    let conn = Connectivity::compute(&b.project, &lib);
    assert!(conn.connected((&r1, "2"), (&r2, "1")));
    assert!(!conn.connected_on_bench((&r1, "2"), (&r2, "1")));
    assert!(conn.connected_in_schematic((&r1, "2"), (&r2, "1")));
    assert_eq!(conn.ratsnest_bench.len(), 1);
    assert!(conn.ratsnest_schematic.is_empty());
    // now wire it on the bench: ratsnest disappears
    b.wire((&bb, "b9"), (&bb, "b20"), "orange");
    let conn = Connectivity::compute(&b.project, &lib);
    assert!(conn.ratsnest_bench.is_empty());
    // a bench-only connection shows up as schematic ratsnest
    b.wire((&bb, "c5"), (&bb, "c24"), "orange");
    let conn = Connectivity::compute(&b.project, &lib);
    assert_eq!(conn.ratsnest_schematic.len(), 1);
    // labels with the same name join nets
    let mut b2 = BenchBuilder::new(&lib, "t2", Mode::Pro);
    let ra = b2.schematic_only("resistor", "RA", 10.0, 10.0, 0.0, &[]);
    let rb = b2.schematic_only("resistor", "RB", 40.0, 40.0, 0.0, &[]);
    let (x1, y1) = b2.anchor(&ra, "2");
    let (x2, y2) = b2.anchor(&rb, "1");
    for (i, (x, y)) in [(x1, y1), (x2, y2)].iter().enumerate() {
        b2.project.schematic.labels.push(crate::project::NetLabel {
            id: format!("l{i}"),
            name: "SIG".into(),
            x: *x,
            y: *y,
            rot: 0.0,
        });
    }
    let g = b2.schematic_only("gnd", "#G1", 0.0, 0.0, 0.0, &[]);
    let ga = b2.anchor(&g, "0");
    let ra1 = b2.anchor(&ra, "1");
    b2.sch_wire(vec![ga, (ga.0, ra1.1), ra1]);
    let conn = Connectivity::compute(&b2.project, &lib);
    assert!(conn.connected((&ra, "2"), (&rb, "1")));
    assert_eq!(conn.net(conn.net_of(&ra, "2").unwrap()).name, "SIG");
    assert_eq!(conn.net(conn.net_of(&ra, "1").unwrap()).name, "0");
}

#[test]
fn t_junction_in_schematic() {
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "t", Mode::Pro);
    let r1 = b.schematic_only("resistor", "R1", 10.0, 10.0, 0.0, &[]);
    let r2 = b.schematic_only("resistor", "R2", 10.0, 30.0, 0.0, &[]);
    let a = b.anchor(&r1, "2");
    let c = b.anchor(&r2, "2");
    // a vertical bus from R1.2 down past R2.2's row, and a stub ending on its middle
    b.sch_wire(vec![a, (a.0 + 4.0, a.1), (a.0 + 4.0, c.1 + 10.0)]);
    b.sch_wire(vec![c, (a.0 + 4.0, c.1)]);
    let conn = Connectivity::compute(&b.project, &lib);
    assert!(conn.connected((&r1, "2"), (&r2, "2")));
}
