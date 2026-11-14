use super::*;
use crate::builder::BenchBuilder;
use crate::project::Mode;
use bb_spice::{Session, SessionOptions};

fn lib() -> std::sync::Arc<Library> {
    Library::builtin()
}

fn op(s: &mut Session) {
    s.op().unwrap();
}

fn example(name: &str) -> Project {
    Project::load(format!(
        "{}/../../examples/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn usb_resistor_led_matches_hand_written_spice() {
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "led", Mode::Playground);
    let ps = b.offboard("usb_power", "PS1", 0.0, 0.0, &[]);
    let bb = b.breadboard("breadboard_400", 10.0, 0.0);
    b.wire((&ps, "VBUS"), (&bb, "t+1"), "red");
    b.wire((&ps, "GND"), (&bb, "t-1"), "black");
    let r1 = b.two_lead(
        "resistor",
        "R1",
        &bb,
        "a3",
        "a7",
        &[("resistance", PropValue::Num(220.0))],
    );
    let d1 = b.two_lead(
        "led",
        "D1",
        &bb,
        "b7",
        "b8",
        &[("color", PropValue::Text("red".into()))],
    );
    b.wire((&bb, "c3"), (&bb, "t+3"), "red");
    b.wire((&bb, "c8"), (&bb, "t-5"), "black");
    let built = build(&b.project, &lib, &NetlistOptions::bench()).unwrap();
    assert!(built.warnings.is_empty());
    let mut s = built.session(SessionOptions::default()).unwrap();
    op(&mut s);
    let node_a = built.node_of(&d1, "a").unwrap().to_string();
    let v_ours = s.voltage(&node_a).unwrap();
    let i_ours = s.current(built.element(&d1, "D").unwrap()).unwrap();

    let hand = "led by hand
V1 src 0 5
Rcable src f 0.3
Rfuse f vbus 0.1
R1 vbus a 220
D1 a 0 LED_RED
.model LED_RED D(Is=1e-18 N=1.9 Rs=5)
.end
";
    let nl = bb_spice::Netlist::parse(hand).unwrap();
    let mut h = Session::new(&nl).unwrap();
    h.op().unwrap();
    let v_hand = h.voltage("a").unwrap();
    let i_hand = h.current("d1").unwrap();
    assert!((v_ours - v_hand).abs() < 1e-9, "{v_ours} vs {v_hand}");
    assert!((i_ours - i_hand).abs() < 1e-9);
    assert!((i_hand - 0.0137).abs() < 0.001, "{i_hand}");
    assert_eq!(built.element(&r1, "R"), Some("r1"));
    // ratings resolved: resistor power from its property, LED current
    let r = built.ratings.iter().find(|r| r.part == r1).unwrap();
    assert_eq!(r.kind, RatingKind::Power);
    assert_eq!(r.limit, 0.25);
    assert!(
        built
            .ratings
            .iter()
            .any(|r| r.part == d1 && r.element == "d1")
    );
}

#[test]
fn garden_node_bench_netlist() {
    let lib = lib();
    let p = example("garden-node");
    let built = build(&p, &lib, &NetlistOptions::bench()).unwrap();
    let c = &built.connectivity;
    let kind = |part: &str, pin: &str| built.net_kind[c.net_of(part, pin).unwrap().0].clone();
    assert_eq!(kind("u1", "A1"), NetKind::Analog("A1".into()));
    assert_eq!(kind("u1", "D9"), NetKind::Analog("D9".into()));
    // sensor output straight into A0, ESP-01 UART lines: closed-form digital nets
    assert_eq!(kind("u1", "A0"), NetKind::Digital);
    assert_eq!(kind("u1", "D2"), NetKind::Digital);
    assert_eq!(kind("u1", "D3"), NetKind::Digital);
    assert_eq!(kind("u1", "D13"), NetKind::Isolated);
    // ports: D9/D8/A1 have high+low ports; capsoil VCC is a device port on the 5 V net
    let mcu = &built.mcus[0];
    assert_eq!(mcu.board, "uno");
    assert_eq!(mcu.vcc_node.as_deref(), Some("VCC"));
    let d9 = mcu.pins.iter().find(|x| x.pin == "D9").unwrap();
    assert!(d9.hi_port.is_some() && d9.lo_port.is_some());
    assert!(
        built
            .ports
            .iter()
            .any(|x| x.part == "j1" && x.pin == "VCC" && x.role == PortRole::Device)
    );
    assert!(built.mcus[0].firmware.is_some());
    // devices: capsoil (pot1), LEDs, LDR, resistors (thermal), board power, ESP-01
    let behaviours: Vec<&str> = built.devices.iter().map(|d| d.behaviour.as_str()).collect();
    for b in ["capsoil", "led", "ldr", "thermal", "board_power", "esp01"] {
        assert!(behaviours.contains(&b), "{b} in {behaviours:?}");
    }
    let j1 = built.devices.iter().find(|d| d.part == "j1").unwrap();
    assert_eq!(j1.attach.get("pot").map(String::as_str), Some("pot1"));
    assert!(j1.pos.is_some());
    // DC operating point with the MCU pins high-Z: the LDR divider
    let mut s = built.session(SessionOptions::default()).unwrap();
    s.op().unwrap();
    let vcc = s.voltage("vcc").unwrap();
    let a1 = s.voltage("a1").unwrap();
    let r_ldr = 350e3 * 180f64.powf(-0.7);
    assert!((vcc - 4.98).abs() < 0.01, "{vcc}");
    assert!((a1 - vcc * 10e3 / (10e3 + r_ldr)).abs() < 1e-4, "{a1}");
    assert!(
        (s.voltage("3v3").unwrap() - 3.3).abs() < 0.02,
        "{:?}",
        s.voltage("3v3")
    );
    // driving D9 high lights D1 through R2
    let port = s.port(d9.hi_port.as_ref().unwrap()).unwrap();
    s.set_port(port, bb_core::PinDrive::Thevenin { v: 0.0, r: 25.0 });
    s.op().unwrap();
    let i = s.current(built.element("d1", "D").unwrap()).unwrap();
    assert!(i > 0.010 && i < 0.016, "{i}");
}

#[test]
fn garden_node_pro_netlist_matches_the_design() {
    let lib = lib();
    let p = example("garden-node");
    let shown = crate::spice_io::pro_display_text(&p, &lib)
        .unwrap()
        .to_ascii_lowercase();
    assert!(
        shown.contains("xu1 vcc 0 a0 a1 d3 d2 d9 d8 atmega328p"),
        "{shown}"
    );
    assert!(shown.contains("fw=garden-node.hex"), "{shown}");
    let text = crate::spice_io::export_cir(&p, &lib).unwrap();
    let lower = text.to_ascii_lowercase();
    assert!(
        lower.contains("* breadbox mcu xu1: fw=garden-node.hex"),
        "{text}"
    );
    assert!(
        lower.contains("xj1 vcc 0 a0 capsoil params: vwc=42"),
        "{text}"
    );
    assert!(
        lower.contains("r2 d9 n1 220") && lower.contains("d1 n1 0 led_amber"),
        "{text}"
    );
    assert!(lower.contains("rldr1 vcc a1"), "{text}");
    assert!(lower.contains("led_amber"), "{text}");
    assert!(lower.contains(".subckt capsoil"), "{text}");
    // it runs: .op gives the design's operating point
    let nl = crate::spice_io::pro_netlist(&p, &lib).unwrap();
    let mut s = Session::new(&nl).unwrap();
    s.op().unwrap();
    let a0 = s.voltage("a0").unwrap();
    assert!((a0 - 1.88).abs() < 0.01, "{a0}");
    let a1 = s.voltage("a1").unwrap();
    assert!((a1 - 2.59).abs() < 0.02, "{a1}");
    // the export parses back
    bb_spice::Netlist::parse(&text).unwrap();
    // directives became analyses
    assert!(!nl.analyses.is_empty());
}

#[test]
fn floating_nodes_get_shunts_and_damage_changes_the_netlist() {
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
    let bb = b.breadboard("breadboard_400", 0.0, 0.0);
    // a capacitor alone on two strips: both nodes float
    b.two_lead("capacitor", "C1", &bb, "a3", "a6", &[]);
    let d1 = b.two_lead("led", "D1", &bb, "a10", "a12", &[]);
    let built = build(&b.project, &lib, &NetlistOptions::bench()).unwrap();
    let text = built.netlist.to_spice_text().to_ascii_lowercase();
    assert!(text.contains("rshunt_"), "{text}");
    let mut s = built.session(SessionOptions::default()).unwrap();
    s.op().unwrap();
    // damaged LED: open → no diode; short → 50 mΩ
    let mut opts = NetlistOptions::bench();
    opts.damaged.insert(d1.clone(), DamageEffect::Open);
    let built = build(&b.project, &lib, &opts).unwrap();
    assert!(built.element(&d1, "D").is_none());
    assert!(!built.devices.iter().any(|d| d.part == d1 && !d.damaged));
    let mut opts = NetlistOptions::bench();
    opts.damaged.insert(d1.clone(), DamageEffect::Short);
    let built = build(&b.project, &lib, &opts).unwrap();
    assert_eq!(built.element(&d1, "R:dmg"), Some("rd1_dmg"));
}

#[test]
fn every_example_netlists_and_solves() {
    let lib = lib();
    for name in [
        "garden-node",
        "blink",
        "night-light",
        "thermometer",
        "ble-button",
        "servo-sweep",
        "esp32-weather",
        "555-astable",
        "rc-filter-bode",
        "op-amp-amplifier",
    ] {
        let p = example(name);
        assert!(p.check(&lib).is_empty(), "{name}: {:?}", p.check(&lib));
        let built =
            build(&p, &lib, &NetlistOptions::bench()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut s = built
            .session(SessionOptions::default())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        s.op().unwrap_or_else(|e| panic!("{name}: {e}"));
        let pro = crate::spice_io::pro_netlist(&p, &lib).unwrap_or_else(|e| panic!("{name}: {e}"));
        let text = pro.to_spice_text();
        bb_spice::Netlist::parse(&text).unwrap_or_else(|e| panic!("{name}: {e}\n{text}"));
    }
}

#[test]
fn every_library_part_netlists() {
    // drop each part on a breadboard alone, with USB power on the rails, and solve
    let lib = lib();
    for def in lib.parts() {
        if def.pins.is_empty() || def.is_breadboard() || def.bench.hidden {
            continue;
        }
        let mut b = BenchBuilder::new(&lib, "t", Mode::Playground);
        b.offboard(&def.id, "X1", 0.0, 60.0, &[]);
        b.offboard("usb_power", "PS1", 100.0, 100.0, &[]);
        let built = build(&b.project, &lib, &NetlistOptions::bench())
            .unwrap_or_else(|e| panic!("{}: {e}", def.id));
        let mut s = built
            .session(SessionOptions::default())
            .unwrap_or_else(|e| panic!("{}: {e}", def.id));
        s.op().unwrap_or_else(|e| panic!("{}: {e}", def.id));
        let pro = build(&b.project, &lib, &NetlistOptions::pro(None))
            .unwrap_or_else(|e| panic!("{} (pro): {e}", def.id));
        bb_spice::Netlist::parse(&pro.netlist.to_spice_text())
            .unwrap_or_else(|e| panic!("{} (pro export): {e}", def.id));
    }
}
