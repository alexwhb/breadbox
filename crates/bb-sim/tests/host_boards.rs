//! API-level boards on the bench (MCU-6 glue): the pico-blink and esp32-weather example sketches
//! are compiled natively against bb-firmware's host core and run behind `McuCore` — pins,
//! PWM, `analogRead` from the analog solution, the DHT22 device answering the bit protocol,
//! and Wi-Fi/MQTT served by the radio space through `RadioService`.
//!
//! Skipped (with a note) when no C++ compiler is available.

mod common;
use bb_circuit::Project;
use bb_core::{Look, SimTime, UserInput};
use bb_radio::RadioSpace;
use bb_radio::geo::Pos;
use bb_radio::mqtt::Message;
use bb_sim::{Bench, BenchOptions, RadioService};
use common::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The board's L LED over the last 60 ms (telemetry averages it over frames ≥ 50 ms).
fn board_led(b: &mut Bench, at: SimTime) -> bool {
    b.run_until(at - SimTime::from_ms(60)).unwrap();
    let _ = b.telemetry();
    b.run_until(at).unwrap();
    let tel = b.telemetry();
    let board = tel.parts.iter().find(|p| p.part == "u1").unwrap();
    match &board.visual.look {
        Look::Board { l, .. } => *l > 0.5,
        other => panic!("{other:?}"),
    }
}

fn build(project: &Project, dir: &std::path::Path) -> Option<BenchOptions> {
    if bb_firmware::host::cxx().is_none() {
        eprintln!("no C++ compiler; skipping");
        return None;
    }
    let exes = bb_sim::firmware::compile_host_project(project, dir, &lib()).unwrap();
    assert!(exes.contains_key("u1"), "{exes:?}");
    Some(BenchOptions {
        host_firmware: exes,
        ..Default::default()
    })
}

#[test]
fn pico_blink_runs_on_the_bench_and_reads_the_knob() {
    let dir = root().join("examples/pico-blink");
    let project = Project::load(&dir).unwrap();
    let Some(opts) = build(&project, &dir) else {
        return;
    };
    let mut b = Bench::new(project, lib(), opts).unwrap();
    // the on-board LED (GP25) blinks at 2 Hz: sample the board look mid-phase
    let mut on = Vec::new();
    for k in 0..6u64 {
        on.push(board_led(&mut b, SimTime::from_ms(125 + 250 * k)));
    }
    assert_eq!(on, [true, false, true, false, true, false], "{on:?}");
    // A0 (GP26) reads the knob: 50 % of 3.3 V, 12-bit
    let reads = |b: &Bench| -> Vec<f64> {
        lines(b, "u1")
            .iter()
            .filter_map(|l| l.strip_prefix("A0 raw="))
            .filter_map(|r| r.split_whitespace().next()?.parse().ok())
            .collect()
    };
    let r = reads(&b);
    assert!(r.len() >= 2, "{:?}", lines(&b, "u1"));
    let last = *r.last().unwrap();
    assert!((last - 2047.0).abs() < 60.0, "{r:?}");
    // turn the knob to 25 %
    b.input("rv1", &UserInput::Position(0.25)).unwrap();
    b.run_until(SimTime::from_ms(2600)).unwrap();
    let last = *reads(&b).last().unwrap();
    assert!((last - 1024.0).abs() < 60.0, "{last}");
    let tel = b.telemetry();
    let m = tel.mcus.iter().find(|m| m.part == "u1").unwrap();
    assert_eq!(m.engine, "API-level");
}

#[test]
fn esp32_weather_reads_the_dht22_device_and_publishes_over_mqtt() {
    let dir = root().join("examples/esp32-weather");
    let mut project = Project::load(&dir).unwrap();
    project.world.air.temp_c = 23.4;
    project.world.air.rh = 51.0;
    let Some(mut opts) = build(&project, &dir) else {
        return;
    };
    // a custom radio space (replacing the default one from the project settings), re-created
    // with every bench
    let dash = Arc::new(AtomicU64::new(0));
    let dash2 = dash.clone();
    opts.services.clear();
    opts.services.push(Arc::new(move |_p: &Project| {
        let mut svc = RadioService::new(RadioSpace::default());
        svc.add_host("u1", "U1", Pos::new(2.0, 1.0));
        let sub = svc
            .space
            .net
            .broker_mut()
            .virtual_connect("dashboard", SimTime::ZERO);
        let out = svc
            .space
            .net
            .broker_mut()
            .virtual_subscribe(sub, "weather/#", 0, SimTime::ZERO);
        svc.space.net.apply_broker(out);
        dash2.store(sub, Ordering::SeqCst);
        Box::new(svc)
    }));
    let mut b = Bench::new(project, lib(), opts).unwrap();
    let dash = dash.load(Ordering::SeqCst);
    b.run_until(SimTime::from_secs(8)).unwrap();
    let l = lines(&b, "u1");
    assert!(l.iter().any(|x| x == "MQTT: connected"), "{l:#?}");
    assert!(
        l.iter().any(|x| x == "t=23.4 C h=51.0 % -> weather/esp32"),
        "{l:#?}"
    );
    // the dashboard got the publishes, every 2 s of simulated time
    let svc = b
        .service_mut("radio")
        .and_then(|s| s.downcast_mut::<RadioService>())
        .unwrap();
    let msgs = svc.space.net.broker_mut().virtual_take(dash);
    let data: Vec<_> = msgs
        .iter()
        .filter(|m| m.message.topic == "weather/esp32")
        .collect();
    assert!(data.len() >= 2, "{msgs:#?}");
    assert_eq!(data[0].message.payload_str(), r#"{"t":23.4,"h":51.0}"#);
    // a command from the dashboard switches the on-board LED (GPIO2)
    let t = b.time();
    let svc = b
        .service_mut("radio")
        .and_then(|s| s.downcast_mut::<RadioService>())
        .unwrap();
    let out = svc.space.net.broker_mut().virtual_publish(
        dash,
        Message::new("weather/cmd", "led on", 0, false),
        t,
    );
    svc.space.net.apply_broker(out);
    assert!(board_led(&mut b, t + SimTime::from_secs(1)));
    let l = lines(&b, "u1");
    assert!(
        l.iter().any(|x| x == "MQTT: weather/cmd = led on"),
        "{l:#?}"
    );
}
