use super::*;
use crate::connectivity::Connectivity;
use bb_spice::Session;

#[test]
fn import_cir_builds_a_pro_project_that_simulates_the_same() {
    let lib = Library::builtin();
    let text = "rc filter + buffer
V1 in 0 DC 5 AC 1
R1 in out 1k
C1 out 0 100n
E1 buf 0 out 0 2
RL buf 0 10k
D1 buf clamp D1N4148
VCL clamp 0 DC 3
.model D1N4148 D(IS=2.52n RS=0.568 N=1.752)
.subckt unused a b
R1 a b 1
.ends unused
.op
.ac dec 10 10 100k
.end
";
    let p = import_cir(text, "imported", &lib).unwrap();
    assert_eq!(p.mode, Mode::Pro);
    assert!(p.check(&lib).is_empty(), "{:?}", p.check(&lib));
    let r1 = p.part_by_ref("R1").unwrap();
    assert_eq!(r1.part, "resistor");
    assert_eq!(r1.props["resistance"], PropValue::Num(1000.0));
    assert_eq!(p.part_by_ref("C1").unwrap().part, "capacitor");
    assert_eq!(p.part_by_ref("E1").unwrap().part, "spice_card");
    assert!(p.spice.directives.contains(".model D1N4148"));
    assert!(p.spice.directives.contains(".subckt unused"));
    // every part was placed in the schematic and connected by labels / GND symbols
    let conn = Connectivity::compute(&p, &lib);
    let r1id = r1.id.clone();
    let c1id = p.part_by_ref("C1").unwrap().id.clone();
    assert!(conn.connected((&r1id, "2"), (&c1id, "1")));
    assert_eq!(conn.net(conn.net_of(&c1id, "2").unwrap()).name, "0");
    assert!(conn.ratsnest_schematic.is_empty());
    // the imported circuit solves to the same operating point
    let nl = pro_netlist(&p, &lib).unwrap();
    let mut s = Session::new(&nl).unwrap();
    s.op().unwrap();
    let orig = bb_spice::Netlist::parse(text).unwrap();
    let mut o = Session::new(&orig).unwrap();
    o.op().unwrap();
    for node in ["out", "buf"] {
        let a = s.voltage(node).unwrap();
        let b = o.voltage(node).unwrap();
        assert!((a - b).abs() < 1e-6, "{node}: {a} vs {b}");
    }
    // analyses survive
    assert!(nl.analyses.len() >= 2);
    // export → import round trip keeps the element count
    let exported = export_cir(&p, &lib).unwrap();
    let again = import_cir(&exported, "again", &lib).unwrap();
    let count = |p: &Project| p.parts.iter().filter(|x| x.part != "gnd").count();
    assert_eq!(count(&again), count(&p));
}

#[test]
fn rc_filter_example_runs_ac_and_has_the_right_corner() {
    let lib = Library::builtin();
    let p = Project::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/rc-filter-bode"
    ))
    .unwrap();
    let nl = pro_netlist(&p, &lib).unwrap();
    let res = nl.run().unwrap();
    let ac = res.plot(bb_spice::PlotKind::Ac).unwrap();
    let f = ac.vector("frequency").unwrap();
    let out = ac.vector("v(out)").unwrap();
    // |H| at fc = 1/(2π·1k·100n) ≈ 1.59 kHz is −3 dB
    let fc = 1.0 / (2.0 * std::f64::consts::PI * 1e3 * 100e-9);
    let n = f.len();
    let mut best = (f64::MAX, 0.0);
    for i in 0..n {
        let fi = f.real_at(i);
        let d = (fi.ln() - fc.ln()).abs();
        if d < best.0 {
            best = (d, out.complex_at(i).abs());
        }
    }
    assert!(best.0 < 0.08, "closest point {}", best.0);
    assert!(
        (best.1 - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.04,
        "{}",
        best.1
    );
}

const RC: &str = "* RC low-pass
V1 in 0 DC 0 AC 1
R1 in out 1k
C1 out 0 159n
.ac dec 20 10 100k
.end
";

#[test]
fn a_typed_netlist_builds_the_circuit_and_keeps_placements_on_later_edits() {
    let lib = Library::builtin();
    let empty = Project::new("typed", Mode::Pro);
    assert!(is_netlist_circuit(&empty));
    let p = netlist_to_circuit(RC, &empty, &lib).unwrap();
    assert_eq!(p.name, "typed");
    let refs = |p: &Project| {
        let mut v: Vec<String> = p
            .parts
            .iter()
            .filter(|x| x.part != "gnd")
            .map(|x| format!("{}:{}", x.reference, x.part))
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        refs(&p),
        ["C1:capacitor", "R1:resistor", "V1:vsource"].map(String::from)
    );
    assert!(p.spice.directives.contains(".ac dec 20 10 100k"));
    assert!(is_netlist_circuit(&p));
    let conn = Connectivity::compute(&p, &lib);
    let r1 = p.part_by_ref("R1").unwrap().id.clone();
    let c1 = p.part_by_ref("C1").unwrap().id.clone();
    assert!(conn.connected((&r1, "2"), (&c1, "1")));
    assert!(conn.ratsnest_schematic.is_empty());
    // the user moves R1 in the schematic, then edits its value and adds R2 in the netlist
    let mut moved = p.clone();
    let at = Placement::at(60.0, 40.0);
    moved
        .parts
        .iter_mut()
        .find(|x| x.id == r1)
        .unwrap()
        .schematic = Some(at);
    let edited = RC
        .replace("R1 in out 1k", "R1 in out 2k")
        .replace(".ac", "R2 out 0 100k\n.ac");
    let q = netlist_to_circuit(&edited, &moved, &lib).unwrap();
    let r1q = q.part_by_ref("R1").unwrap();
    assert_eq!(r1q.id, r1, "same part");
    assert_eq!(r1q.schematic, Some(at), "kept where the user put it");
    assert_eq!(r1q.props["resistance"], PropValue::Num(2000.0));
    assert!(q.part_by_ref("R2").is_some());
    let conn = Connectivity::compute(&q, &lib);
    // R1's labels moved with it: still wired to C1
    assert!(conn.connected((&r1, "2"), (&c1, "1")));
    assert!(conn.ratsnest_schematic.is_empty());
    // the circuit solves like the text
    let nl = pro_netlist(&q, &lib).unwrap();
    let res = nl.run().unwrap();
    assert!(res.plot(bb_spice::PlotKind::Ac).is_some());
}

#[test]
fn what_the_circuit_cannot_hold_is_reported_not_lost() {
    let lib = Library::builtin();
    let empty = Project::new("typed", Mode::Pro);
    // half typed: a parse error, not a detach
    assert!(matches!(
        netlist_to_circuit("* t\nR1 in\n.end\n", &empty, &lib),
        Err(SpiceIoError::Parse(_))
    ));
    // an include the netlister cannot resolve
    let inc = RC.replace(".ac", ".include models/missing.lib\n.ac");
    assert!(matches!(
        netlist_to_circuit(&inc, &empty, &lib),
        Err(SpiceIoError::Unrepresentable(_)) | Err(SpiceIoError::Parse(_))
    ));
    // a subcircuit instance with more nodes than a generic SPICE part has pins (16)
    let big = RC.replace(
        ".ac",
        "X9 n1 n2 n3 n4 n5 n6 n7 n8 n9 n10 n11 n12 n13 n14 n15 n16 n17 big\n\
         .subckt big a1 a2 a3 a4 a5 a6 a7 a8 a9 a10 a11 a12 a13 a14 a15 a16 a17\n\
         R1 a1 a17 1k\n.ends big\n.ac",
    );
    assert!(matches!(
        netlist_to_circuit(&big, &empty, &lib),
        Err(SpiceIoError::Unrepresentable(m)) if m.contains("X9")
    ));
    // extra element parameters stay on a SPICE card instead of being dropped
    let tc = RC.replace("R1 in out 1k", "R1 in out 1k tc1=0.004");
    let p = netlist_to_circuit(&tc, &empty, &lib).unwrap();
    let r1 = p.part_by_ref("R1").unwrap();
    assert_eq!(r1.part, "spice_card");
    assert!(
        matches!(&r1.props["card"], PropValue::Text(t) if t.contains("tc1=0.004")),
        "{:?}",
        r1.props
    );
    // boards and breadboard wiring are not a netlist circuit
    let garden = Project::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/garden-node"
    ))
    .unwrap();
    assert!(!is_netlist_circuit(&garden));
}

#[test]
fn replacing_the_circuit_is_one_undo_step() {
    let lib = Library::builtin();
    let mut ed = crate::edit::Editor::new(Project::new("typed", Mode::Pro));
    let a = netlist_to_circuit(RC, &ed.project, &lib).unwrap();
    assert!(ed.replace_circuit("Netlist", &a));
    assert!(!ed.replace_circuit("Netlist", &a), "no change, no step");
    let b = netlist_to_circuit(&RC.replace("1k", "2k"), &ed.project, &lib).unwrap();
    assert!(ed.replace_circuit("Netlist", &b));
    let r1 = |ed: &crate::edit::Editor| {
        ed.project.part_by_ref("R1").unwrap().props["resistance"].clone()
    };
    assert_eq!(r1(&ed), PropValue::Num(2000.0));
    assert!(ed.undo());
    assert_eq!(r1(&ed), PropValue::Num(1000.0));
    assert_eq!(ed.project.parts, a.parts);
    assert_eq!(ed.project.schematic.labels, a.schematic.labels);
    assert!(ed.undo());
    assert!(ed.project.parts.is_empty() && ed.project.schematic.labels.is_empty());
    assert!(ed.project.spice.directives.is_empty());
    assert!(ed.redo() && ed.redo());
    assert_eq!(r1(&ed), PropValue::Num(2000.0));
}
