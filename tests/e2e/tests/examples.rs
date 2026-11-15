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
