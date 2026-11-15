//! The garden-node example (PRJ-3) on the real compiled sketch: UNO + ESP-01 (AT responder
//! until the radio glue lands) + capacitive soil sensor in pot 1 + LDR divider + night/water
//! LEDs, mini scope on D9, meter on A0.

mod common;
use bb_circuit::Project;
use bb_core::SimTime;
use bb_sim::{Bench, BenchOptions, LineKind};
use common::*;

fn garden(seed: Option<u64>) -> Bench {
    let dir = root().join("examples/garden-node");
    let project = Project::load(&dir).unwrap();
    let opts = BenchOptions {
        seed,
        firmware: bb_sim::firmware::project_images(&project, &dir),
        ..Default::default()
    };
    assert!(opts.firmware.contains_key("u1"), "garden-node image");
    Bench::new(project, lib(), opts).unwrap()
}

/// (time s, soil_raw, moisture, light_raw, led_pwm) of every telemetry line.
fn readings(b: &Bench) -> Vec<(f64, f64, f64, f64, f64)> {
    b.serial("u1")
        .unwrap()
        .data_lines()
        .filter(|l| l.text.starts_with("soil_raw="))
        .map(|l| {
            (
                l.time.as_secs_f64(),
                field(&l.text, "soil_raw").unwrap(),
                field(&l.text, "moisture").unwrap(),
                field(&l.text, "light_raw").unwrap(),
                field(&l.text, "led_pwm").unwrap(),
            )
        })
        .collect()
}

#[test]
fn prints_telemetry_publishes_and_probes_match_the_design() {
    let mut b = garden(None);
    b.run_until(SimTime::from_secs(4)).unwrap();
    let r = readings(&b);
    assert!(r.len() >= 4, "{:?}", lines(&b, "u1"));
    for &(_, soil, moist, light, pwm) in &r {
        // design: soil_raw=382…388 moisture=41…43% light_raw=528…536 led_pwm=122
        assert!((soil - 385.0).abs() <= 8.0, "soil_raw {soil}");
        assert!((40.0..=43.0).contains(&moist), "moisture {moist}");
        assert!((light - 532.0).abs() <= 10.0, "light_raw {light}");
        assert!((pwm - 122.0).abs() <= 3.0, "led_pwm {pwm}");
    }
    let mon = b.serial("u1").unwrap();
    let net: Vec<&str> = mon
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Net)
        .map(|l| l.text.as_str())
        .collect();
    assert!(net.iter().any(|l| l.starts_with("wifi: AT+CWJAP=\"SIM-AP\" → OK")), "{net:?}");
    assert!(net.iter().any(|l| l.starts_with("mqtt: PUB garden/pot1 {\"m\":")), "{net:?}");
    assert!(mon.lines[0].text.starts_with("Breadbox sim · ATmega328P @ 16 MHz · reset cause: power-on"));
    let tel = b.telemetry();
    let probe = |label: &str| tel.probes.iter().find(|p| p.label == label).unwrap().clone();
    let a0 = probe("Soil sensor → A0");
    let model = 3.03 - 0.02737 * 42.0;
    assert!((a0.avg - model).abs() < 0.01, "A0 avg {} vs model {model}", a0.avg);
    let a1 = probe("Light sensor → A1");
    assert!((a1.value - 2.60).abs() < 0.03, "A1 {}", a1.value);
    let d9 = probe("Night LED · D9");
    assert!((d9.value - 47.8).abs() < 1.5, "D9 duty {}", d9.value);
    assert_eq!(probe("Water LED · D8").text, "LOW");
    // the bench meter on A0 and the scope on D9 (490 Hz PWM)
    assert!((tel.meters[0].value - model).abs() < 0.03, "meter {}", tel.meters[0].value);
    let s = &tel.scopes[0];
    assert!((s.frequency - 490.2).abs() < 3.0, "scope f {}", s.frequency);
    assert!((s.duty - 0.478).abs() < 0.02, "scope duty {}", s.duty);
    assert_eq!(tel.erc_summary, "ERC 1 warning · DRC clean");
    // LED D1 glows at about half brightness, D2 is dark
    let led = |id: &str| match &tel.parts.iter().find(|p| p.part == id).unwrap().visual.look {
        bb_core::Look::Led { current, .. } => *current,
        other => panic!("{other:?}"),
    };
    assert!(led("d1") > 0.004 && led("d1") < 0.009, "D1 {}", led("d1"));
    assert!(led("d2") < 1e-4, "D2 {}", led("d2"));
    // supply rails / USB budget
    let rails = &tel.power.rails;
    assert!(rails.iter().any(|r| r.name == "VCC" && (r.voltage - 4.97).abs() < 0.05), "{rails:?}");
    assert!(tel.power.usb_current > 0.04 && tel.power.usb_current < 0.2, "{}", tel.power.usb_current);
}

#[test]
fn soil_slider_moves_moisture_with_the_sensor_time_constant() {
    let mut b = garden(None);
    b.run_until(SimTime::from_secs(3)).unwrap();
    let before = readings(&b).last().unwrap().2;
    b.set_world("pot1.vwc", 20.0);
    b.run_until(SimTime::from_secs(9)).unwrap();
    let r = readings(&b);
    let at = |t: f64| r.iter().filter(|x| x.0 <= t).next_back().unwrap().2;
    // τ = 1.8 s: after ~2 s the reading has covered ~2/3 of the step, after 6 s all of it
    let two_s = at(5.1);
    let expected = before - (before - 20.0) * (1.0 - (-2.0f64 / 1.8).exp());
    assert!((two_s - expected).abs() < 3.0, "{two_s} vs {expected} (before {before})");
    let end = r.last().unwrap().2;
    assert!((end - 20.0).abs() <= 1.5, "{end}");
    // below 30 % the sketch lights the water LED on D8
    let tel = b.telemetry();
    let d8 = tel.probes.iter().find(|p| p.label == "Water LED · D8").unwrap();
    assert_eq!(d8.text, "HIGH");
}

#[test]
fn night_led_duty_tracks_ambient_light() {
    let mut b = garden(None);
    let duty = |b: &mut Bench| {
        let tel = b.telemetry();
        tel.probes.iter().find(|p| p.label == "Night LED · D9").unwrap().value
    };
    b.run_until(SimTime::from_secs(3)).unwrap();
    let living = duty(&mut b);
    b.set_world("light.ambient_lux", 5.0);
    b.run_until(SimTime::from_secs(4)).unwrap();
    let night = duty(&mut b);
    b.set_world("light.ambient_lux", 1000.0);
    b.run_until(SimTime::from_secs(5)).unwrap();
    let office = duty(&mut b);
    assert!(night > living + 20.0 && living > office + 15.0, "{night} {living} {office}");
    let pwm_last = readings(&b).last().unwrap().4;
    // 1000 lx: R_LDR = 350k·1000^-0.7 ≈ 2.78 kΩ → light_raw ≈ 800 → pwm ≈ 55
    assert!((pwm_last - 55.0).abs() < 4.0, "{pwm_last}");
}

#[test]
fn replay_is_deterministic_and_seeds_matter() {
    let run = |seed| {
        let mut b = garden(Some(seed));
        b.run_until(SimTime::from_secs(3)).unwrap();
        b.set_world("light.ambient_lux", 600.0);
        b.run_until(SimTime::from_secs(5)).unwrap();
        let lines: Vec<(u64, String)> = b
            .serial("u1")
            .unwrap()
            .lines
            .iter()
            .map(|l| (l.time.as_ps(), l.text.clone()))
            .collect();
        let tel = b.telemetry();
        let probes: Vec<(String, u64, u64, u64)> = tel
            .probes
            .iter()
            .map(|p| (p.label.clone(), p.value.to_bits(), p.avg.to_bits(), p.max.to_bits()))
            .collect();
        let scope: Vec<(u64, u64)> = tel.scopes[0].traces[0]
            .iter()
            .map(|(t, v)| (t.to_bits(), v.to_bits()))
            .collect();
        (lines, probes, scope)
    };
    let a = run(7);
    let b = run(7);
    assert_eq!(a, b, "same seed → identical serial output and probe traces");
    let c = run(8);
    assert_ne!(a.0, c.0, "a different seed changes the sensor noise");
}
