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

/// MCU-6 host-core features the examples don't use, on an ESP32 on the bench: LEDC PWM
/// (`ledcAttach` / `ledcWrite`, 25 %) and `analogWrite` (75 %) measured by duty probes on the
/// pins, and `micros()` across `delayMicroseconds(1500)`. (Serial2 on its pins is covered by
/// `host_bridges`, ESP32 ↔ HC-05.)
#[test]
fn esp32_ledc_analog_write_and_micros() {
    use bb_circuit::builder::BenchBuilder;
    use bb_circuit::project::{Endpoint, Mode, ProbeDef, Quantity};
    if bb_firmware::host::cxx().is_none() {
        eprintln!("no C++ compiler; skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("bb-host-features-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let sketch = dir.join("firmware/U1/features");
    std::fs::create_dir_all(&sketch).unwrap();
    std::fs::write(
        sketch.join("features.ino"),
        r#"
const int LED = 2;
void setup() {
  Serial.begin(115200);
  ledcAttach(LED, 1000, 8);
  ledcWrite(LED, 64);
  analogWrite(4, 191);
  unsigned long t0 = micros();
  delayMicroseconds(1500);
  unsigned long dt = micros() - t0;
  Serial.print("dt_us=");
  Serial.println(dt);
}
void loop() {
  delay(10);
}
"#,
    )
    .unwrap();
    let lib = lib();
    let mut b = BenchBuilder::new(&lib, "features", Mode::Playground);
    let u1 = b.offboard("esp32_devkit", "U1", 0.0, 0.0, &[]);
    // an LED + 220 Ω on each PWM pin
    for (k, pin) in ["D2", "D4"].iter().enumerate() {
        let x = 40.0 + 12.0 * k as f64;
        let r = b.offboard(
            "resistor",
            &format!("R{}", k + 1),
            x,
            0.0,
            &[("resistance", bb_circuit::PropValue::Num(220.0))],
        );
        let d = b.offboard("led", &format!("D{}", k + 1), x, 8.0, &[]);
        b.wire((&u1, pin), (&r, "1"), "orange");
        b.wire((&r, "2"), (&d, "a"), "orange");
        b.wire((&d, "k"), (&u1, "GND"), "black");
    }
    let mut project = b.finish();
    project.part_mut(&u1).unwrap().firmware = Some(bb_circuit::project::FirmwareRef {
        sketch: "firmware/U1/features".into(),
        fqbn: None,
        image: None,
    });
    for (id, pin) in [("d2", "D2"), ("d4", "D4")] {
        project.probes.push(ProbeDef {
            id: id.into(),
            label: id.into(),
            target: Endpoint::pin(&u1, pin),
            quantity: Quantity::Duty,
            pinned: None,
        });
    }
    project.save(&dir).unwrap();
    let exes = bb_sim::firmware::compile_host_project(&project, &dir, &lib).unwrap();
    let mut bench = Bench::new(
        project,
        lib.clone(),
        BenchOptions {
            host_firmware: exes,
            ..Default::default()
        },
    )
    .unwrap();
    bench.run_until(SimTime::from_ms(300)).unwrap();
    let out = lines(&bench, &u1);
    let dt: f64 = out
        .iter()
        .find_map(|l| l.strip_prefix("dt_us="))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("{out:?}"));
    assert!((1500.0..1520.0).contains(&dt), "micros across 1500 µs: {dt}");
    let tel = bench.telemetry();
    let duty = |id: &str| tel.probes.iter().find(|p| p.id == id).map(|p| p.value);
    let (d2, d4) = (duty("d2").unwrap(), duty("d4").unwrap());
    assert!((d2 - 25.0).abs() < 1.0, "ledcWrite 64/255 → {d2} %");
    assert!((d4 - 74.9).abs() < 1.0, "analogWrite 191/255 → {d4} %");
    let _ = std::fs::remove_dir_all(&dir);
}
