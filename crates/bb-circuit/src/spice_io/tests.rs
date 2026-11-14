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
