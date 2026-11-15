//! Engine behaviour: SIM-7 (re-flash / MCU reset keep the bench), live edits, instrument
//! loads, the threaded SimHandle (SIM-3, SIM-5) and the debugger under the scheduler.

mod common;
use bb_circuit::netlist::Load;
use bb_circuit::project::{Endpoint, Project};
use bb_core::SimTime;
use bb_sim::{Bench, BenchOptions, Command, DebugCommand, RunStatus, SimHandle, Speed};
use common::*;
use std::time::{Duration, Instant};

fn garden() -> (Project, BenchOptions) {
    let dir = root().join("examples/garden-node");
    let project = Project::load(&dir).unwrap();
    let opts = BenchOptions {
        firmware: bb_sim::firmware::project_images(&project, &dir),
        ..Default::default()
    };
    (project, opts)
}

fn moisture_lines(b: &Bench) -> usize {
    lines(b, "u1")
        .iter()
        .filter(|l| l.starts_with("soil_raw="))
        .count()
}

#[test]
fn reflash_and_mcu_reset_keep_the_bench() {
    let (p, opts) = garden();
    let mut b = Bench::new(p, lib(), opts).unwrap();
    b.run_until(SimTime::from_secs(3)).unwrap();
    b.set_world("pot1.vwc", 25.0);
    let world_t = bb_core::WorldView::world_secs(b.world());
    // reset only the MCU: the sketch restarts, the world and the soil sensor keep going
    b.mcu("u1").unwrap().reset();
    // the sketch restarts (waits up to 2 s for the ESP-01's "ready", re-joins, re-connects)
    b.run_until(SimTime::from_secs(9)).unwrap();
    let mon = b.serial("u1").unwrap();
    assert!(
        mon.lines
            .iter()
            .any(|l| l.text.contains("reset cause: external"))
    );
    assert!(bb_core::WorldView::world_secs(b.world()) > world_t + 5.9);
    // the pot keeps evaporating (≈1.4 %/h) — the world was not reset
    let vwc = bb_core::WorldView::soil_vwc(b.world(), "pot1").unwrap();
    assert!(vwc < 25.0 && vwc > 24.99, "{vwc}");
    let last = lines(&b, "u1")
        .into_iter()
        .rfind(|l| l.starts_with("soil_raw="))
        .unwrap();
    assert!(field(&last, "moisture").unwrap() < 30.0, "{last}");
    // flash a different sketch (blink on D13): devices/world untouched, the new code runs
    let before = moisture_lines(&b);
    b.mcu("u1").unwrap().flash(&fixture("blink")).unwrap();
    b.run_until(SimTime::from_secs(11)).unwrap();
    assert_eq!(moisture_lines(&b), before, "old firmware gone");
    assert!(lines(&b, "u1").contains(&"blink".to_string()));
    assert!((bb_core::WorldView::soil_vwc(b.world(), "pot1").unwrap() - 25.0).abs() < 0.01);
    let tel = b.telemetry();
    let a0 = tel
        .probes
        .iter()
        .find(|p| p.label == "Soil sensor → A0")
        .unwrap();
    assert!(
        (a0.value - (3.03 - 0.02737 * 25.0)).abs() < 0.02,
        "{}",
        a0.value
    );
}

#[test]
fn circuit_edits_rebuild_without_losing_mcu_or_world_state() {
    let (p, opts) = garden();
    let mut b = Bench::new(p.clone(), lib(), opts).unwrap();
    b.run_until(SimTime::from_secs(3)).unwrap();
    let n0 = moisture_lines(&b);
    let resets0 = b.mcu("u1").unwrap().reset_count();
    // change R2 (the night LED's resistor) to 1 kΩ: the LED current drops, the sketch keeps going
    let i_before = b.element_current("d1", "D").unwrap().abs().max(
        b.telemetry()
            .parts
            .iter()
            .find(|x| x.part == "d1")
            .map(|x| x.readings[0].1)
            .unwrap_or(0.0),
    );
    b.set_part_param("r2", "resistance", 1000.0).unwrap();
    b.run_until(SimTime::from_secs(4)).unwrap();
    let tel = b.telemetry();
    let i_after = tel.parts.iter().find(|x| x.part == "d1").unwrap().readings[0].1;
    assert!(i_after < 0.45 * i_before, "{i_before} → {i_after}");
    // structural edit: add an LED + resistor on D13
    let mut p2 = p.clone();
    let lib = lib();
    let mut bb =
        bb_circuit::builder::BenchBuilder::new(&lib, "x", bb_circuit::project::Mode::Playground);
    bb.project = p2.clone();
    bb.two_lead(
        "resistor",
        "R9",
        "bb1",
        "c50",
        "c54",
        &[("resistance", n(220.0))],
    );
    bb.two_lead("led", "D9", "bb1", "d54", "d55", &[]);
    bb.wire(("u1", "D13"), ("bb1", "a50"), "orange");
    bb.wire(("bb1", "a55"), ("bb1", "t-49"), "black");
    p2 = bb.project;
    b.update_project(p2).unwrap();
    b.run_until(SimTime::from_secs(6)).unwrap();
    assert_eq!(b.mcu("u1").unwrap().reset_count(), resets0, "no MCU reset");
    assert!(moisture_lines(&b) > n0 + 4, "the sketch kept printing");
    assert!(b.stats.rebuilds >= 1);
    assert!(
        b.element_current("d9", "D").is_some(),
        "the new LED is simulated"
    );
}

#[test]
fn instrument_load_shifts_the_node_like_a_real_meter() {
    let (p, opts) = garden();
    let mut b = Bench::new(p, lib(), opts).unwrap();
    b.run_until(SimTime::from_millis_compat(500)).unwrap();
    let v0 = b.pin_voltage("j1", "AOUT").unwrap();
    // a 10 kΩ meter input on the sensor output (2.2 kΩ source): −18 % of the reading
    b.add_load(Load {
        id: "dmm".into(),
        a: Endpoint::pin("j1", "AOUT"),
        b: Endpoint::pin("u1", "GND"),
        r: 10e3,
        c: 100e-12,
    })
    .unwrap();
    b.run_until(SimTime::from_millis_compat(600)).unwrap();
    let v1 = b.pin_voltage("j1", "AOUT").unwrap();
    let expect = v0 * 10e3 / 12.2e3;
    assert!((v1 - expect).abs() < 0.02, "{v0} → {v1} (expect {expect})");
    b.remove_load("dmm").unwrap();
    b.run_until(SimTime::from_millis_compat(700)).unwrap();
    assert!((b.pin_voltage("j1", "AOUT").unwrap() - v0).abs() < 0.03);
}

trait Ms {
    fn from_millis_compat(ms: u64) -> SimTime;
}
impl Ms for SimTime {
    fn from_millis_compat(ms: u64) -> SimTime {
        SimTime::from_ms(ms)
    }
}

fn wait_for(
    h: &SimHandle,
    what: &str,
    f: impl Fn(&bb_sim::Telemetry) -> bool,
) -> std::sync::Arc<bb_sim::Telemetry> {
    let t0 = Instant::now();
    loop {
        let t = h.telemetry();
        if f(&t) {
            return t;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(60),
            "timed out waiting for {what}: {:?} {:?}",
            t.status,
            t.time
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn sim_handle_runs_paces_steps_and_streams() {
    let (p, opts) = garden();
    let h = SimHandle::spawn(p, lib(), opts).unwrap();
    h.send(Command::SetSpeed(Speed::Max));
    h.send(Command::Play);
    let t = wait_for(&h, "2 s simulated", |t| t.time >= SimTime::from_secs(2));
    assert_eq!(t.status, RunStatus::Running);
    assert!(!t.parts.is_empty() && !t.probes.is_empty());
    h.send(Command::Pause);
    let t = wait_for(&h, "paused", |t| t.status == RunStatus::Paused);
    std::thread::sleep(Duration::from_millis(50));
    let t2 = h.telemetry();
    assert_eq!(t.time, t2.time, "paused time stands still");
    // single step of 10 ms
    h.send(Command::Step(SimTime::from_ms(10)));
    let t3 = wait_for(&h, "step", |x| {
        x.time >= t2.time + SimTime::from_ms(10) && x.status == RunStatus::Paused
    });
    assert!(
        t3.time - t2.time < SimTime::from_ms(11),
        "{:?}",
        t3.time - t2.time
    );
    // serial lines streamed
    let lines = h.serial_lines();
    assert!(
        lines.iter().any(|l| l.text.starts_with("soil_raw=")),
        "{lines:?}"
    );
    // world slider while running at 1× real time: pacing within ±25 %
    h.send(Command::SetWorld {
        key: "light.ambient_lux".into(),
        value: 900.0,
    });
    h.send(Command::SetSpeed(Speed::Factor(1.0)));
    h.send(Command::Play);
    let start = h.telemetry().time;
    let wall = Instant::now();
    std::thread::sleep(Duration::from_millis(1200));
    let t4 = h.telemetry();
    let sim = (t4.time - start).as_secs_f64();
    let real = wall.elapsed().as_secs_f64();
    assert!((sim / real - 1.0).abs() < 0.25, "sim {sim} s in {real} s");
    assert!((t4.world.ambient_lux - 900.0).abs() < 1e-9);
    // re-flash keeps the bench running; MCU-only reset
    h.send(Command::ResetMcu("u1".into()));
    h.send(Command::Flash {
        part: "u1".into(),
        image: Box::new(fixture("blink")),
    });
    std::thread::sleep(Duration::from_millis(300));
    let lines = h.serial_lines();
    assert!(
        lines.iter().any(|l| l.text.contains("Flashed virtual U1")),
        "{lines:?}"
    );
    h.shutdown();
}

#[test]
fn debugger_breakpoint_pauses_the_bench() {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, "blink");
    b.wire((&u1, "D13"), (&bb, "a5"), "orange");
    b.two_lead(
        "resistor",
        "R1",
        &bb,
        "c5",
        "c9",
        &[("resistance", n(220.0))],
    );
    b.two_lead("led", "D1", &bb, "d9", "d10", &[]);
    b.wire((&bb, "a10"), (&bb, "t-8"), "black");
    let mut opts = BenchOptions::default();
    opts.firmware.insert(u1.clone(), fixture("blink"));
    let h = SimHandle::spawn(b.finish(), lib.clone(), opts).unwrap();
    let elf = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/blink.uno.elf"
    ))
    .unwrap();
    h.send(Command::Debug(DebugCommand::Attach {
        part: u1.clone(),
        elf,
    }));
    h.send(Command::Debug(DebugCommand::AddBreakpoint {
        part: u1.clone(),
        file: "blink.ino".into(),
        line: 11,
        hits: None,
    }));
    h.send(Command::SetSpeed(Speed::Max));
    h.send(Command::Play);
    let t = wait_for(&h, "breakpoint", |t| {
        t.mcus.first().is_some_and(|m| m.paused.is_some())
    });
    let paused = t.mcus[0].paused.clone().unwrap();
    assert!(paused.contains("Breakpoint"), "{paused}");
    // first hit: after the first 500 ms delay (digitalWrite LOW on line 11)
    assert!((t.time.as_secs_f64() - 0.5).abs() < 0.01, "{:?}", t.time);
    h.send(Command::Debug(DebugCommand::Continue));
    let t2 = wait_for(&h, "second hit", |x| {
        x.time > t.time + SimTime::from_ms(100) && x.mcus[0].paused.is_some()
    });
    assert!((t2.time.as_secs_f64() - 1.5).abs() < 0.01, "{:?}", t2.time);
    h.shutdown();
}
