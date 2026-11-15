//! TEST-4 · every example project (PRJ-2) opens and runs 5 s on the threaded engine without
//! engine errors, damage, error logs, ERC errors or DRC problems, and does something visible;
//! the world (WORLD-2/5) drives the sensors: day/night with time-warp moves the LDR, a world
//! script dries the pot.

use bb_circuit::erc::ErcSeverity;
use bb_sim::{Command, LineKind};
use breadbox_e2e::*;

#[test]
fn every_example_runs_5_s_cleanly() {
    let names = example_names();
    assert!(names.len() >= 10, "{names:?}");
    for name in names {
        let Some(mut sim) = Sim::example(&name) else {
            eprintln!("{name}: needs a C++ compiler for its host board — skipped");
            continue;
        };
        sim.run_secs(5.0);
        let tel = sim.tel();
        assert!(sim.problems().is_empty(), "{name}: {:?}", sim.problems());
        let errors: Vec<_> = tel
            .erc
            .iter()
            .filter(|e| e.severity == ErcSeverity::Error)
            .collect();
        assert!(errors.is_empty(), "{name}: ERC errors {errors:#?}");
        assert!(
            tel.erc_summary.ends_with("DRC clean"),
            "{name}: {} {:#?}",
            tel.erc_summary,
            tel.drc
        );
        for m in &tel.mcus {
            assert!(!m.in_reset, "{name}: {} held in reset", m.reference);
            assert!(m.vcc > 3.0, "{name}: {} at {} V", m.reference, m.vcc);
            let data = sim.lines_of(&m.reference, LineKind::Data).len();
            let warn = sim.lines_of(&m.reference, LineKind::Warn);
            assert!(warn.is_empty(), "{name}: {warn:#?}");
            eprintln!("{name}: {} printed {data} lines", m.reference);
        }
        // nothing damaged or stressed, no smoke
        for p in &tel.parts {
            assert!(
                !p.visual.health.is_damaged(),
                "{name}: {} {:?}",
                p.reference,
                p.visual.health
            );
            assert_eq!(p.visual.smoke, 0.0, "{name}: {} smokes", p.reference);
        }
        assert!(tel.time >= secs(5.0));
        sim.shutdown();
    }
}

/// Day/night cycle at Ashland, OR (June 21, from 08:00) with the world running 3600× faster
/// than the circuit: one simulated second is an hour, the sun sets and rises, and the LDR
/// divider on A1 follows it (the sketch's light_raw and the night LED's duty).
#[test]
fn day_night_with_time_warp_drives_the_ldr() {
    let (mut p, dir) = example("garden-node");
    p.world.noise = false;
    let opts = bench_options(&p, &dir).unwrap();
    let mut sim = Sim::spawn(p, lib(), opts);
    sim.send(Command::SetWorld {
        key: "light.solar".into(),
        value: 1.0,
    });
    sim.send(Command::SetWorld {
        key: "world.time_warp".into(),
        value: 3600.0,
    });
    let mut samples = Vec::new(); // (sim s, world hour, sun elevation, LDR lux, light_raw, duty)
    for k in 1..=26 {
        sim.run_to(secs(k as f64));
        let tel = sim.tel();
        let hour = tel
            .world
            .local_time
            .as_ref()
            .map(|t| t.hour)
            .unwrap_or(f64::NAN);
        let raw = sim
            .data_lines("U1")
            .iter()
            .rev()
            .find_map(|l| field(&l.text, "light_raw"))
            .unwrap_or(f64::NAN);
        samples.push((
            k as f64,
            hour,
            tel.world.sun_elevation_deg,
            sim.reading("ldr1", "lux").unwrap(),
            raw,
            sim.probe("Night LED · D9").value,
        ));
    }
    let tel = sim.tel();
    assert!(
        (tel.world.world_secs - 26.0 * 3600.0).abs() < 60.0,
        "world time runs 3600×: {} s",
        tel.world.world_secs
    );
    let day = samples
        .iter()
        .filter(|s| s.2 > 30.0)
        .map(|s| s.4)
        .fold(f64::NAN, f64::max);
    let night = samples
        .iter()
        .filter(|s| s.2 < -10.0)
        .map(|s| s.4)
        .fold(f64::NAN, f64::min);
    assert!(
        day.is_finite() && night.is_finite(),
        "the run must see both day and night: {samples:#?}"
    );
    assert!(
        day > night + 150.0,
        "LDR follows the sun: day {day} vs night {night}\n{samples:#?}"
    );
    let duty_day = samples
        .iter()
        .filter(|s| s.2 > 30.0)
        .map(|s| s.5)
        .fold(f64::NAN, f64::min);
    let duty_night = samples
        .iter()
        .filter(|s| s.2 < -10.0)
        .map(|s| s.5)
        .fold(f64::NAN, f64::max);
    assert!(
        duty_night > duty_day + 20.0,
        "the night LED is brighter at night: {duty_night} vs {duty_day}"
    );
    sim.shutdown();
}

/// WORLD-5: a world script dries the pot (`moisture = 45 - 0.8*t_hours`); with time-warp the
/// sketch sees it within seconds of circuit time.
#[test]
fn world_script_dries_the_pot() {
    let (mut p, dir) = example("garden-node");
    p.world.noise = false;
    p.world.time_warp = 3600.0;
    p.world.pots[0].evaporation = false;
    p.world.scripts.push(bb_world::Script {
        source: "moisture = 45 - 0.8*t_hours".into(),
        enabled: true,
    });
    let opts = bench_options(&p, &dir).unwrap();
    let mut sim = Sim::spawn(p, lib(), opts);
    sim.run_secs(20.0);
    let tel = sim.tel();
    let vwc = tel.world.pots[0].vwc;
    let hours = tel.world.world_secs / 3600.0;
    assert!(
        (vwc - (45.0 - 0.8 * hours)).abs() < 0.5,
        "script: {vwc} % after {hours} h"
    );
    let m = sim
        .data_lines("U1")
        .iter()
        .rev()
        .find_map(|l| field(&l.text, "moisture"))
        .unwrap();
    assert!((m - vwc).abs() <= 3.0, "the sketch reads {m} % at {vwc} %");
    sim.shutdown();
}

/// 555-astable on the bench: the green LED blinks at the RC frequency of the classic
/// astable, f = 1.44 / ((R1 + 2·R2)·C) ≈ 1.46 Hz (R1 1 k, R2 10 k, C 47 µF).
#[test]
fn astable_555_blinks_at_its_rc_frequency() {
    let mut sim = Sim::example("555-astable").unwrap();
    sim.run_secs(2.0); // first cycle charges C from 0 V
    let mut lit = Vec::new();
    for _ in 0..600 {
        sim.run_for(ms(10));
        let on = matches!(sim.part("d1").visual.look, bb_core::Look::Led { current, .. } if current > 1e-3);
        lit.push((sim.time().as_secs_f64(), on));
    }
    let rises: Vec<f64> = lit
        .windows(2)
        .filter(|w| !w[0].1 && w[1].1)
        .map(|w| w[1].0)
        .collect();
    assert!(rises.len() >= 6, "LED blinks: {} rising edges", rises.len());
    let period = (rises[rises.len() - 1] - rises[0]) / (rises.len() - 1) as f64;
    let f = 1.0 / period;
    let expect = 1.44 / ((1e3 + 2.0 * 10e3) * 47e-6);
    assert!(
        (f - expect).abs() < expect * 0.1,
        "555 at {f:.3} Hz, the formula says {expect:.3} Hz"
    );
    // the LED is on while the output is high: R1 + R2 charging time (duty ≈ 52 %)
    let on = lit.iter().filter(|x| x.1).count() as f64 / lit.len() as f64;
    assert!((on - 0.524).abs() < 0.08, "duty {on}");
    sim.shutdown();
}

/// night-light: dark → the LED comes on (and brighter the darker it is), daylight → off.
#[test]
fn night_light_turns_on_in_the_dark() {
    let mut sim = Sim::example("night-light").unwrap();
    let led_level = |sim: &Sim| {
        sim.data_lines("U1")
            .iter()
            .rev()
            .find_map(|l| field(&l.text, "led"))
            .expect("light=… led=… lines")
    };
    let mut seen = Vec::new();
    for lux in [500.0, 20.0, 2.0] {
        sim.send(Command::SetWorld {
            key: "light.ambient_lux".into(),
            value: lux,
        });
        sim.run_secs(1.0);
        let current = match sim.part("d1").visual.look {
            bb_core::Look::Led { current, .. } => current,
            other => panic!("{other:?}"),
        };
        seen.push((lux, led_level(&sim), current));
    }
    assert_eq!(seen[0].1, 0.0, "daylight: off {seen:?}");
    assert!(seen[0].2 < 1e-4, "{seen:?}");
    assert!(
        seen[1].1 > 0.0 && seen[2].1 > seen[1].1,
        "darker → brighter {seen:?}"
    );
    assert!(seen[2].2 > 0.005, "LED current in the dark {seen:?}");
    sim.shutdown();
}
