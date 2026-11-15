//! Every UNO example project (PRJ-3) runs the committed fixture image of its sketch on the
//! bench and does what its description promises.

mod common;
use bb_circuit::Project;
use bb_core::{Look, SimTime, UserInput};
use bb_sim::{Bench, BenchOptions};
use common::*;

fn open(name: &str) -> Bench {
    open_with(name, |_| {})
}

fn open_with(name: &str, edit: impl FnOnce(&mut Project)) -> Bench {
    let dir = root().join("examples").join(name);
    let mut project = Project::load(&dir).unwrap();
    edit(&mut project);
    let opts = BenchOptions {
        firmware: bb_sim::firmware::project_images(&project, &dir),
        ..Default::default()
    };
    assert!(opts.firmware.contains_key("u1"), "{name}: fixture image");
    let mut b = Bench::new(project, lib(), opts).unwrap();
    let tel = b.telemetry();
    assert!(
        tel.erc
            .iter()
            .all(|i| i.severity != bb_circuit::erc::ErcSeverity::Error)
            && tel.drc.is_empty(),
        "{name}: {}",
        tel.erc_summary
    );
    b
}

fn reading(b: &Bench, part: &str, key: &str) -> f64 {
    b.readings(part)
        .into_iter()
        .find(|r| r.0 == key)
        .map(|r| r.1)
        .unwrap_or_else(|| panic!("{part}.{key}: {:?}", b.readings(part)))
}

fn led_on(b: &Bench, part: &str) -> bool {
    match b.visual(part).unwrap().look {
        Look::Led { brightness, .. } => brightness > 0.3,
        other => panic!("{other:?}"),
    }
}

#[test]
fn blink_example_blinks_d1_at_1hz() {
    let mut b = open("blink");
    let mut states = Vec::new();
    for k in 0..8 {
        b.run_until(SimTime::from_ms(250 + 500 * k)).unwrap();
        states.push(led_on(&b, "d1"));
    }
    assert_eq!(
        states,
        [true, true, false, false, true, true, false, false],
        "{states:?}"
    );
}

#[test]
fn night_light_example_turns_the_led_on_after_dark() {
    let mut b = open("night-light");
    b.run_until(SimTime::from_secs(1)).unwrap();
    let last = |b: &Bench| {
        lines(b, "u1")
            .into_iter()
            .rfind(|l| l.starts_with("light="))
            .unwrap()
    };
    let day = last(&b);
    assert_eq!(field(&day, "led"), Some(0.0), "{day}");
    assert!(field(&day, "light").unwrap() > 300.0, "{day}");
    assert!(!led_on(&b, "d1"));
    // dusk: the LDR's resistance climbs, A0 drops below DARK, the LED dims up
    b.set_world("lux", 30.0);
    b.run_until(SimTime::from_secs(2)).unwrap();
    let dusk = last(&b);
    let (light, led) = (field(&dusk, "light").unwrap(), field(&dusk, "led").unwrap());
    assert!(
        (100.0..300.0).contains(&light) && (40.0..200.0).contains(&led),
        "{dusk}"
    );
    // night: full brightness
    b.set_world("lux", 0.1);
    b.run_until(SimTime::from_secs(3)).unwrap();
    let night = last(&b);
    let (light, led) = (
        field(&night, "light").unwrap(),
        field(&night, "led").unwrap(),
    );
    assert!(light < 100.0 && led == 255.0, "{night}");
    // the transistor switches the LED: real collector current through D1
    b.run_until(SimTime::from_secs_f64(3.3)).unwrap();
    assert!(led_on(&b, "d1"));
    let i = reading(&b, "d1", "current_a");
    // (5 V − 3.0 V white LED − 0.1 V Vce(sat)) / 220 Ω ≈ 8.6 mA
    assert!((0.007..0.011).contains(&i), "LED current {i}");
}

#[test]
fn thermometer_example_shows_the_air_temperature_on_the_lcd() {
    let mut b = open_with("thermometer", |p| p.world.air.temp_c = 24.0);
    b.run_until(SimTime::from_secs(3)).unwrap();
    let text = match b.visual("lcd1").unwrap().look {
        Look::TextLcd { text, .. } => text.concat(),
        other => panic!("{other:?}"),
    };
    let t: f64 = text
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter_map(|w| w.parse().ok())
        .find(|v: &f64| *v > 10.0 && *v < 40.0)
        .unwrap_or_else(|| panic!("no temperature on the LCD: {text:?}"));
    assert!((t - 24.0).abs() < 1.0, "{text:?}");
}

#[test]
fn ble_button_example_configures_the_hm10_and_counts_presses() {
    let mut b = open("ble-button");
    b.run_until(SimTime::from_ms(1500)).unwrap();
    let l = lines(&b, "u1");
    assert!(l.iter().any(|x| x == "AT -> OK"), "{l:?}");
    assert!(
        l.iter()
            .any(|x| x == "AT+NAMEbb-button -> OK+Set:bb-button"),
        "{l:?}"
    );
    assert!(l.iter().any(|x| x == "ble-button ready"), "{l:?}");
    for k in 0..2u64 {
        b.input("sw1", &UserInput::Press(true)).unwrap();
        b.run_until(SimTime::from_ms(1600 + 400 * k)).unwrap();
        b.input("sw1", &UserInput::Press(false)).unwrap();
        b.run_until(SimTime::from_ms(1800 + 400 * k)).unwrap();
    }
    let l = lines(&b, "u1");
    assert!(l.iter().any(|x| x == "pressed #2"), "{l:?}");
    // the HM-10's RXD sits behind the 1k/2k divider: ≈ 3.3 V idle level
    let v = b.pin_voltage("u2", "RXD").unwrap();
    assert!((3.0..3.6).contains(&v), "RXD {v}");
}

#[test]
fn servo_sweep_example_sweeps_the_horn() {
    let mut b = open("servo-sweep");
    let mut angles = Vec::new();
    for k in 1..=40 {
        b.run_until(SimTime::from_ms(100 * k)).unwrap();
        angles.push(reading(&b, "m1", "angle_deg"));
    }
    let lo = angles.iter().cloned().fold(f64::MAX, f64::min);
    let hi = angles.iter().cloned().fold(f64::MIN, f64::max);
    assert!(lo < 20.0 && hi > 160.0, "{angles:?}");
}
