use super::*;
use crate::builder::BenchBuilder;
use crate::part::PropValue;
use crate::project::Mode;

fn lib() -> Arc<Library> {
    Library::builtin()
}

fn codes(items: &[ErcItem]) -> Vec<&str> {
    items.iter().map(|i| i.code.as_str()).collect()
}

fn run(p: &Project) -> (Vec<ErcItem>, Vec<ErcItem>) {
    let lib = lib();
    let c = Connectivity::compute(p, &lib);
    (erc(p, &lib, &c), drc(p, &lib, &c))
}

#[test]
fn garden_node_has_the_designs_one_warning() {
    let p = Project::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/garden-node"
    ))
    .unwrap();
    let (e, d) = run(&p);
    assert_eq!(codes(&e), vec!["overvoltage"], "{e:#?}");
    assert_eq!(
        e[0].message,
        "U2.RX is 3.3 V tolerant, driven by 5 V D2 — add a divider"
    );
    assert_eq!(summary("ERC", &e), "ERC 1 warning");
    assert!(d.is_empty(), "{d:#?}");
    assert_eq!(summary("DRC", &d), "DRC clean");
}

#[test]
fn led_without_resistor_and_with_one() {
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
    let ps = b.offboard("usb_power", "PS1", 0.0, 0.0, &[]);
    let bb = b.breadboard("breadboard_400", 10.0, 0.0);
    b.wire((&ps, "VBUS"), (&bb, "t+1"), "red");
    b.wire((&ps, "GND"), (&bb, "t-1"), "black");
    b.two_lead("led", "D1", &bb, "a5", "a6", &[]);
    b.wire((&bb, "b5"), (&bb, "t+4"), "red");
    b.wire((&bb, "b6"), (&bb, "t-5"), "black");
    let (e, _) = run(&b.project);
    assert!(codes(&e).contains(&"led_no_resistor"), "{e:#?}");
    // with a resistor in series it is fine
    let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
    let ps = b.offboard("usb_power", "PS1", 0.0, 0.0, &[]);
    let bb = b.breadboard("breadboard_400", 10.0, 0.0);
    b.wire((&ps, "VBUS"), (&bb, "t+1"), "red");
    b.wire((&ps, "GND"), (&bb, "t-1"), "black");
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "a3",
        "a5",
        &[("resistance", PropValue::Num(220.0))],
    );
    b.two_lead("led", "D1", &bb, "b5", "b6", &[]);
    b.wire((&bb, "c3"), (&bb, "t+4"), "red");
    b.wire((&bb, "c6"), (&bb, "t-5"), "black");
    let (e, d) = run(&b.project);
    assert!(e.is_empty(), "{e:#?}");
    assert!(d.is_empty(), "{d:#?}");
    // an LED straight on an MCU pin to GND is flagged too
    let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
    let u1 = b.offboard("uno", "U1", 0.0, 0.0, &[]);
    let bb = b.breadboard("breadboard_400", 30.0, 0.0);
    b.two_lead("led", "D1", &bb, "a5", "a6", &[]);
    b.wire((&u1, "D13"), (&bb, "b5"), "orange");
    b.wire((&u1, "GND"), (&bb, "b6"), "black");
    let (e, _) = run(&b.project);
    assert!(codes(&e).contains(&"led_no_resistor"), "{e:#?}");
}

#[test]
fn supply_shorts_and_conflicts() {
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
    let u1 = b.offboard("uno", "U1", 0.0, 0.0, &[]);
    b.wire((&u1, "5V"), (&u1, "GND"), "red");
    let (e, _) = run(&b.project);
    assert!(codes(&e).contains(&"supply_short"), "{e:#?}");
    let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
    let u1 = b.offboard("uno", "U1", 0.0, 0.0, &[]);
    b.wire((&u1, "5V"), (&u1, "3V3"), "red");
    let (e, _) = run(&b.project);
    let s = e
        .iter()
        .find(|i| i.code == "supply_short")
        .expect("conflict");
    assert!(
        s.message.contains("5 V") && s.message.contains("3.3 V"),
        "{}",
        s.message
    );
    assert_eq!(s.severity, ErcSeverity::Error);
}

#[test]
fn a_negative_rail_is_not_a_short() {
    // op-amp-amplifier: VN's + on ground, − on VEE makes −12 V — not a supply short
    let p = Project::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/op-amp-amplifier"
    ))
    .unwrap();
    let (e, _) = run(&p);
    assert!(e.iter().all(|i| i.severity != ErcSeverity::Error), "{e:#?}");
    // a battery with both terminals on the ground rail is one
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
    let bt = b.offboard("battery_pack", "BT1", 0.0, 0.0, &[]);
    let bb = b.breadboard("breadboard_400", 10.0, 0.0);
    b.wire((&bt, "minus"), (&bb, "t-1"), "black");
    b.wire((&bt, "plus"), (&bb, "t-5"), "red");
    let (e, _) = run(&b.project);
    assert!(codes(&e).contains(&"supply_short"), "{e:#?}");
}

#[test]
fn missing_ground_floating_input_and_output_conflict() {
    let lib = lib();
    // battery + resistor, battery − not connected anywhere: still has a ground (its own −).
    // A lone 555 with nothing on TRIG/THR inputs but a wire between them: floating input.
    let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
    let bt = b.offboard("battery_pack", "BT1", 0.0, 0.0, &[]);
    let bb = b.breadboard("breadboard_400", 10.0, 0.0);
    b.rigid("ne555", "U1", &bb, "GND", "f10", 0.0, &[]);
    b.wire((&bt, "plus"), (&bb, "a10"), "red");
    b.wire((&bt, "minus"), (&bb, "j10"), "black");
    b.wire((&bb, "a12"), (&bb, "j11"), "green"); // THR ↔ TRIG, nothing else
    let (e, _) = run(&b.project);
    assert!(codes(&e).contains(&"floating_input"), "{e:#?}");
    // two sensor outputs tied together
    let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
    let a = b.offboard("pir", "U1", 0.0, 0.0, &[]);
    let c = b.offboard("pir", "U2", 10.0, 0.0, &[]);
    b.wire((&a, "OUT"), (&c, "OUT"), "yellow");
    let (e, _) = run(&b.project);
    assert!(codes(&e).contains(&"output_conflict"), "{e:#?}");
    assert!(codes(&e).contains(&"unconnected_pin"), "{e:#?}");
    // nothing references ground although a supply exists
    let mut b = BenchBuilder::new(&lib, "t", Mode::Pro);
    b.schematic_only("vsource", "V1", 0.0, 0.0, 0.0, &[]);
    let (e, _) = run(&b.project);
    assert!(codes(&e).contains(&"missing_ground"), "{e:#?}");
}

#[test]
fn drc_unplugged_and_shorted_leads() {
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
    let bb = b.breadboard("breadboard_400", 0.0, 0.0);
    // vertical resistor inside one strip: both leads in column 5, rows a and d
    b.two_lead("resistor", "R1", &bb, "a5", "d5", &[]);
    // a resistor lying on the bench, not plugged in
    b.offboard("resistor", "R2", 0.5, 40.3, &[]);
    // two leads in one hole
    b.two_lead("resistor", "R3", &bb, "a10", "a14", &[]);
    b.two_lead("resistor", "R4", &bb, "a14", "a18", &[]);
    let (_, d) = run(&b.project);
    let c = codes(&d);
    assert!(c.contains(&"shorted_leads"), "{d:#?}");
    assert!(c.contains(&"not_plugged"), "{d:#?}");
    assert!(c.contains(&"hole_conflict"), "{d:#?}");
    assert!(
        summary("DRC", &d).starts_with("DRC 1 error"),
        "{}",
        summary("DRC", &d)
    );
}
