//! TEST-4 · damage + supply (WB-13, SIM-6) on the threaded engine: an LED wired without its
//! resistor burns out (magic smoke, stays damaged after the circuit is fixed) until Repair all;
//! a short across the UNO's 5 V trips the USB polyfuse, browns the MCU out, and everything
//! comes back once the short is removed.

use bb_circuit::Project;
use bb_circuit::project::{Endpoint, Wire};
use bb_core::{DeviceEvent, Health, Look};
use bb_sim::Command;
use breadbox_e2e::*;

fn led_current(sim: &Sim, id: &str) -> f64 {
    match sim.part(id).visual.look {
        Look::Led { current, .. } => current,
        other => panic!("{other:?}"),
    }
}

/// The blink example with R1 replaced by a jumper between the breadboard strips its leads sat
/// in (what a user does on the bench by pulling the resistor and bridging the gap).
fn without_resistor(p: &Project) -> Project {
    let lib = lib();
    let conn = bb_circuit::connectivity::Connectivity::compute(p, &lib);
    let hole = |pin: &str| -> Endpoint {
        let mates = conn.net_mates("r1", pin);
        let mate = mates
            .iter()
            .find(|t| t.part == "bb1")
            .or_else(|| mates.iter().find(|t| t.part != "r1"))
            .cloned()
            .unwrap_or_else(|| panic!("R1.{pin} connects to nothing: {mates:?}"));
        Endpoint::pin(&mate.part, &mate.pin)
    };
    let (a, b) = (hole("1"), hole("2"));
    let mut q = p.clone();
    q.parts.retain(|x| x.id != "r1");
    q.wires.push(Wire {
        id: "w-bridge".into(),
        a,
        b,
        color: "yellow".into(),
        bends: vec![],
    });
    q
}

#[test]
fn led_burnout_then_repair_all() {
    let (p, dir) = example("blink");
    let opts = bench_options(&p, &dir).unwrap();
    let mut sim = Sim::spawn(p.clone(), lib(), opts);
    sim.run_secs(0.5);
    let healthy = led_current(&sim, "d1");
    assert!(
        (0.008..0.016).contains(&healthy),
        "D1 through 220 Ω: {healthy} A"
    );
    // pull R1 and bridge the gap
    let bad = without_resistor(&p);
    sim.send(Command::UpdateProject(Box::new(bad.clone())));
    sim.run_secs(0.1);
    let tel = sim.tel_now();
    assert!(
        tel.erc.iter().any(|e| e.code == "led_no_resistor"),
        "ERC warns: {:?}",
        tel.erc
    );
    sim.run_secs(2.5);
    let dmg = sim.damage();
    assert!(
        dmg.iter()
            .any(|(src, why)| src == "D1" && why.contains("LED burnt out")),
        "{dmg:?}"
    );
    let d1 = sim.part("d1");
    assert!(matches!(d1.visual.health, Health::Damaged { .. }), "{d1:?}");
    assert!(d1.visual.smoke > 0.0, "magic smoke");
    assert!(led_current(&sim, "d1") < 1e-6, "a burnt LED is open");
    // the MCU pin survived (the LED went first)
    assert_eq!(sim.part("u1").visual.health, Health::Ok);
    // fix the circuit: the LED stays damaged until it is repaired
    sim.send(Command::UpdateProject(Box::new(p.clone())));
    sim.run_secs(2.5);
    assert!(matches!(
        sim.part("d1").visual.health,
        Health::Damaged { .. }
    ));
    assert!(led_current(&sim, "d1") < 1e-6);
    sim.send(Command::RepairAll);
    sim.run_to(secs(8.5)); // blink: D13 is HIGH from 8 s to 9 s
    assert_eq!(sim.probe("LED · D13").text, "HIGH");
    assert_eq!(sim.part("d1").visual.health, Health::Ok);
    let after = led_current(&sim, "d1");
    assert!(
        (after - healthy).abs() < healthy * 0.1,
        "repaired LED {after} A vs {healthy} A before"
    );
    // smoke clears
    assert!(sim.part("d1").visual.smoke < 0.05);
    sim.shutdown();
}

#[test]
fn shorted_uno_5v_trips_the_usb_polyfuse_and_recovers() {
    let (p, dir) = example("blink");
    let opts = bench_options(&p, &dir).unwrap();
    let mut sim = Sim::spawn(p.clone(), lib(), opts);
    sim.run_secs(1.0);
    let boots = sim.tel().mcus[0].reset_count;
    let mut shorted = p.clone();
    shorted.wires.push(Wire {
        id: "w-short".into(),
        a: Endpoint::pin("u1", "5V"),
        b: Endpoint::pin("u1", "GND"),
        color: "yellow".into(),
        bends: vec![],
    });
    sim.send(Command::UpdateProject(Box::new(shorted)));
    sim.run_secs(0.5);
    let stress: Vec<String> = sim
        .events
        .iter()
        .filter_map(|e| match &e.event {
            DeviceEvent::Stress { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert!(
        stress.iter().any(|r| r.contains("USB polyfuse tripped")),
        "{stress:?}"
    );
    assert_eq!(sim.reading("u1", "fuse_tripped"), Some(1.0));
    let i = sim.reading("u1", "usb_current_a").unwrap();
    assert!(i < 0.15, "the tripped fuse limits the short to {i} A");
    let m = sim.tel().mcus[0].clone();
    assert!(m.vcc < 1.0, "rail collapsed to {}", m.vcc);
    assert!(m.in_reset, "browned out");
    match sim.part("u1").visual.look {
        Look::Board { power, .. } => assert!(!power, "power LED off"),
        other => panic!("{other:?}"),
    }
    // pull the short: the PTC cools for a second, resets, the sketch boots again
    sim.send(Command::UpdateProject(Box::new(p)));
    sim.run_secs(3.0);
    assert_eq!(sim.reading("u1", "fuse_tripped"), Some(0.0));
    let m = sim.tel().mcus[0].clone();
    assert!((m.vcc - 5.0).abs() < 0.1, "5 V back: {}", m.vcc);
    assert!(!m.in_reset);
    assert!(m.reset_count > boots, "rebooted after the brown-out");
    // and it blinks again
    let mut levels = Vec::new();
    for _ in 0..4 {
        sim.run_secs(0.5);
        levels.push(sim.probe("LED · D13").text.clone());
    }
    assert!(
        levels.iter().any(|l| l == "HIGH") && levels.iter().any(|l| l == "LOW"),
        "{levels:?}"
    );
    // nothing was destroyed by the short
    assert!(sim.damage().is_empty(), "{:?}", sim.damage());
    sim.shutdown();
}

/// The stand-alone USB power block (PS1) feeding a breadboard: a short trips its polyfuse.
#[test]
fn usb_power_block_polyfuse() {
    let lib = lib();
    let mut b = bb_circuit::builder::BenchBuilder::new(
        &lib,
        "short",
        bb_circuit::project::Mode::Playground,
    );
    let ps = b.offboard("usb_power", "PS1", 0.0, 0.0, &[]);
    let bb = b.breadboard("breadboard_400", 10.0, 0.0);
    b.wire((&ps, "VBUS"), (&bb, "t+1"), "red");
    b.wire((&ps, "GND"), (&bb, "t-1"), "black");
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "a5",
        "a9",
        &[("resistance", bb_circuit::PropValue::Num(1e3))],
    );
    b.wire((&bb, "b5"), (&bb, "t+4"), "red");
    b.wire((&bb, "b9"), (&bb, "t-8"), "black");
    let healthy = b.finish();
    let mut shorted = healthy.clone();
    shorted.wires.push(Wire {
        id: "w-short".into(),
        a: Endpoint::pin(&bb, "t+20"),
        b: Endpoint::pin(&bb, "t-20"),
        color: "yellow".into(),
        bends: vec![],
    });
    let mut sim = Sim::spawn(shorted, lib.clone(), Default::default());
    sim.run_secs(0.3);
    assert_eq!(sim.reading(&ps, "fuse_tripped"), Some(1.0));
    sim.send(Command::UpdateProject(Box::new(healthy)));
    sim.run_secs(2.0);
    assert_eq!(sim.reading(&ps, "fuse_tripped"), Some(0.0));
    let v = sim.with(move |b| b.pin_voltage(&ps, "VBUS").unwrap());
    assert!((v - 4.98).abs() < 0.03, "{v}");
    sim.shutdown();
}
