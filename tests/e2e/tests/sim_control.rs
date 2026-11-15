//! TEST-4 · engine control through `SimHandle` (SIM-3, SIM-7): deterministic replay of a
//! scripted session, re-flashing keeps the bench (analog state, world, radio module), MCU-only
//! reset, full reset, single steps and pause/play.

use bb_sim::{Command, LineKind, RunStatus, Speed};
use breadbox_e2e::*;
use std::time::{Duration, Instant};

/// Everything observable from one scripted garden-node session.
#[derive(Debug, PartialEq)]
struct Trace {
    serial: Vec<(u64, String)>,
    probes: Vec<(String, u64, u64)>,
    mqtt: Vec<(u64, String)>,
    parts: Vec<(String, String)>,
}

fn scripted(seed: u64) -> Trace {
    let (p, dir) = example("garden-node");
    let mut opts = bench_options(&p, &dir).unwrap();
    opts.seed = Some(seed);
    let mut sim = Sim::spawn(p, lib(), opts);
    sim.send(Command::Radio(
        bb_sim::radio::RadioCommand::ExplorerSubscribe("#".into()),
    ));
    sim.run_secs(2.5);
    sim.send(Command::SetWorld {
        key: "light.ambient_lux".into(),
        value: 600.0,
    });
    sim.run_secs(1.25);
    sim.send(Command::SetWorld {
        key: "pot1.vwc".into(),
        value: 30.0,
    });
    sim.run_secs(1.0);
    sim.send(Command::Water {
        pot: "pot1".into(),
        ml: 100.0,
    });
    sim.run_secs(2.0);
    let tel = sim.tel_now();
    let trace = Trace {
        serial: sim
            .serial
            .iter()
            .map(|l| (l.time.as_ps(), l.text.clone()))
            .collect(),
        probes: tel
            .probes
            .iter()
            .map(|p| (p.label.clone(), p.value.to_bits(), p.avg.to_bits()))
            .collect(),
        mqtt: tel
            .radio
            .as_ref()
            .unwrap()
            .mqtt
            .iter()
            .map(|r| (r.time.as_ps(), r.payload.clone()))
            .collect(),
        parts: tel
            .parts
            .iter()
            .map(|p| (p.part.clone(), format!("{:?}", p.visual)))
            .collect(),
    };
    sim.shutdown();
    trace
}

#[test]
fn deterministic_replay_through_the_handle() {
    let a = scripted(7);
    let b = scripted(7);
    assert!(a.serial.len() > 10 && a.mqtt.len() > 3, "{a:#?}");
    assert_eq!(
        a, b,
        "same seed + same commands at the same times → identical runs"
    );
    let c = scripted(8);
    assert_ne!(
        a.serial, c.serial,
        "another seed changes the sensor noise / jitter"
    );
}

/// SIM-7: flashing new firmware resets only the MCU — time keeps going, the soil sensor's
/// output (an analog state with τ ≈ 1.8 s) carries on from where it was, the world keeps the
/// slider, and the ESP-01 stays joined.
#[test]
fn reflash_keeps_the_bench_state() {
    let (p, dir) = example("garden-node");
    let (img, _elf, src) = avr_firmware(&p, &dir, "u1", false);
    eprintln!("garden-node firmware for the re-flash: {src:?}");
    let opts = bench_options(&p, &dir).unwrap();
    let mut sim = Sim::spawn(p, lib(), opts);
    sim.run_secs(4.0);
    sim.send(Command::SetWorld {
        key: "pot1.vwc".into(),
        value: 15.0,
    });
    sim.run_secs(1.0);
    let a0_before = sim.probe("Soil sensor → A0").value;
    let joined_before = sim
        .lines_of("U1", LineKind::Net)
        .iter()
        .filter(|l| l.text.starts_with("wifi: AT+CWJAP"))
        .count();
    let boots = sim.tel().mcus[0].reset_count;
    sim.send(Command::Flash {
        part: "u1".into(),
        image: Box::new(img),
    });
    sim.run_for(ms(2));
    let t = sim.time();
    assert!(t > secs(5.0), "time keeps running across the flash: {t:?}");
    let sys: Vec<String> = sim
        .lines_of("U1", LineKind::System)
        .iter()
        .map(|l| l.text.clone())
        .collect();
    assert!(
        sys.iter()
            .any(|l| l == "Flashed virtual U1 · core reset · bench state kept"),
        "{sys:#?}"
    );
    let a0_after = sim.probe("Soil sensor → A0").value;
    assert!(
        (a0_after - a0_before).abs() < 0.02,
        "analog state continuous: {a0_before} → {a0_after}"
    );
    let m = sim.tel().mcus[0].clone();
    assert!(m.reset_count > boots, "the core was reset");
    // the sketch starts over; the sensor keeps converging towards 15 % instead of starting at 42
    sim.run_secs(8.0);
    let tel = sim.tel_now();
    assert!(
        (tel.world.pots[0].vwc - 15.0).abs() < 0.5,
        "world kept the slider: {:?}",
        tel.world.pots[0]
    );
    let after: Vec<f64> = sim
        .data_lines("U1")
        .iter()
        .filter(|l| l.time > t)
        .filter_map(|l| field(&l.text, "moisture"))
        .collect();
    assert!(!after.is_empty(), "the re-flashed sketch prints");
    assert!(
        after.iter().all(|m| *m < 30.0),
        "moisture continues from the pre-flash sensor state: {after:?}"
    );
    // ±2.5 %: the sensor is still converging, and the realistic ADC (WORLD-6: offset / gain
    // error up to ±2 LSB, INL, ±1 LSB noise) moves a reading by up to ~5 codes ≈ 1 %
    assert!((after.last().unwrap() - 15.0).abs() <= 2.5, "{after:?}");
    // the ESP-01 was not reset: the sketch's join succeeds again and publishes
    let joins = sim
        .lines_of("U1", LineKind::Net)
        .iter()
        .filter(|l| l.text.starts_with("wifi: AT+CWJAP"))
        .count();
    assert!(joins > joined_before, "rejoined after the flash");
    assert_eq!(
        sim.tel()
            .radio
            .as_ref()
            .unwrap()
            .cards
            .iter()
            .find(|c| c.label == "U2")
            .unwrap()
            .status,
        "connected"
    );
    sim.shutdown();
}

#[test]
fn reset_mcu_only_then_full_reset() {
    let mut sim = Sim::example("garden-node").unwrap();
    sim.run_secs(3.0);
    sim.send(Command::SetWorld {
        key: "light.ambient_lux".into(),
        value: 20.0,
    });
    sim.send(Command::ResetMcu("u1".into()));
    sim.run_secs(3.0);
    let sys: Vec<String> = sim
        .lines_of("U1", LineKind::System)
        .iter()
        .map(|l| l.text.clone())
        .collect();
    assert!(
        sys.iter().any(|l| l == "MCU reset (bench state kept)"),
        "{sys:#?}"
    );
    assert!(sim.time() >= secs(6.0));
    assert!(
        (sim.tel().world.ambient_lux - 20.0).abs() < 1e-9,
        "world kept"
    );
    // full reset: back to t = 0, the project's world, a fresh power-on
    sim.send(Command::Reset);
    let t = sim.time();
    assert!(t < secs(0.1), "reset rewinds the clock: {t:?}");
    let n = sim.serial.len();
    sim.run_secs(1.0);
    let fresh: Vec<&str> = sim.serial[n..].iter().map(|l| l.text.as_str()).collect();
    assert!(
        fresh
            .iter()
            .any(|l| l.starts_with("Breadbox sim · ATmega328P @ 16 MHz · reset cause: power-on")),
        "{fresh:#?}"
    );
    sim.shutdown();
}

#[test]
fn single_steps_land_on_exact_times() {
    let mut sim = Sim::example("blink").unwrap();
    for _ in 0..5 {
        sim.run_for(ms(1)); // "single ms"
    }
    assert_eq!(sim.time(), ms(5));
    sim.run_for(bb_core::SimTime::from_us(1)); // "single quantum"
    assert_eq!(sim.time(), ms(5) + bb_core::SimTime::from_us(1));
    assert_eq!(sim.tel().status, RunStatus::Paused);
    sim.shutdown();
}

/// Play after a Step that was paused half-way must run freely, not stop at the old step's end.
#[test]
fn play_after_an_interrupted_step_runs_on() {
    let sim = Sim::example("blink").unwrap();
    sim.send(Command::SetSpeed(Speed::Max));
    // the Step and the Pause land in the same engine batch: the step never completes
    sim.send(Command::Step(ms(200)));
    sim.send(Command::Pause);
    let t0 = sim.time();
    assert!(t0 < ms(200));
    sim.send(Command::Play);
    let wall = Instant::now();
    loop {
        let tel = sim.handle.telemetry();
        if tel.time > ms(400) {
            break;
        }
        assert!(
            wall.elapsed() < Duration::from_secs(60),
            "Play stopped at {:?} (status {:?})",
            tel.time,
            tel.status
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    sim.send(Command::Pause);
    sim.shutdown();
}
