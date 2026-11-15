//! Behavioural devices driven by real Arduino libraries.

mod common;
use bb_core::{DeviceEvent, Look, Pos, Rgb, SimTime};
use common::*;

fn ms(v: u64) -> SimTime {
    SimTime::from_ms(v)
}

fn reading(b: &bb_sim::Bench, part: &str, key: &str) -> f64 {
    b.readings(part)
        .iter()
        .find(|r| r.0 == key)
        .map(|r| r.1)
        .unwrap_or_else(|| panic!("{part} has no {key}: {:?}", b.readings(part)))
}

#[test]
fn ds18b20_dallas_temperature_search_and_convert() {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, "ds18b20");
    b.project.world.noise = false;
    b.project.world.air.temp_c = 19.25;
    let s = b.rigid("ds18b20", "U2", &bb, "GND", "e10", 0.0, &[]);
    b.wire((&bb, "a10"), (&bb, "t-10"), "black");
    b.wire((&bb, "a12"), (&bb, "t+12"), "red");
    b.two_lead("resistor", "R1", &bb, "c11", "c16", &[("resistance", n(4700.0))]);
    b.wire((&bb, "a16"), (&bb, "t+15"), "red");
    b.wire((&bb, "b11"), (&u1, "D4"), "yellow");
    let mut bench = start(b.finish(), &[(&u1, "ds18b20")]);
    bench.run_until(ms(2600)).unwrap();
    let l = lines(&bench, &u1);
    assert_eq!(l.first().map(String::as_str), Some("devices=1"), "{l:?}");
    let temps: Vec<f64> = l.iter().filter_map(|x| field(x, "t")).collect();
    assert!(!temps.is_empty(), "{l:?}");
    assert!(temps.iter().all(|t| (*t - 19.25).abs() < 1e-9), "{temps:?}");
    assert!(reading(&bench, &s, "conversions") >= 1.0);
}

fn i2c_bench(name: &str, def: &str) -> (bb_circuit::Project, String, String) {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, name);
    b.project.world.noise = false;
    let d = b.offboard(def, "U2", 40.0, 24.0, &[]);
    let (vcc, gnd) = match def {
        "bme280" => ("VIN", "GND"),
        _ => ("VCC", "GND"),
    };
    b.wire((&d, vcc), (&bb, "t+10"), "red");
    b.wire((&d, gnd), (&bb, "t-10"), "black");
    b.wire((&d, "SDA"), (&u1, "A4"), "blue");
    b.wire((&d, "SCL"), (&u1, "A5"), "yellow");
    (b.finish(), u1, d)
}

#[test]
fn bme280_adafruit_library_reads_world_air() {
    let (mut p, u1, _d) = i2c_bench("bme", "bme280");
    p.world.air.temp_c = 22.5;
    p.world.air.rh = 48.0;
    p.world.air.pressure_pa = 98_765.0;
    let mut bench = start(p, &[(&u1, "bme280")]);
    bench.run_until(ms(1500)).unwrap();
    let l = lines(&bench, &u1);
    assert_eq!(l.first().map(String::as_str), Some("bme: ok"), "{l:?}");
    let last = l.iter().rev().find(|x| x.starts_with("t=")).unwrap_or_else(|| panic!("{l:?}"));
    let (t, pr, h) = (field(last, "t").unwrap(), field(last, "p").unwrap(), field(last, "h").unwrap());
    assert!((t - 22.5).abs() < 0.02, "{last}");
    assert!((pr - 987.65).abs() < 0.05, "{last}");
    assert!((h - 48.0).abs() < 0.05, "{last}");
}

#[test]
fn ssd1306_adafruit_library_draws_pixels() {
    let (p, u1, d) = i2c_bench("oled", "ssd1306");
    let mut bench = start(p, &[(&u1, "oled")]);
    bench.run_until(ms(1500)).unwrap();
    assert!(lines(&bench, &u1).contains(&"oled: done".to_string()), "{:?}", lines(&bench, &u1));
    match bench.visual(&d).unwrap().look {
        Look::Pixels { width, height, data, on, .. } => {
            assert_eq!((width, height), (128, 64));
            assert!(on);
            let px = |x: usize, y: usize| data[y * 128 + x];
            // the filled 20×20 rectangle at (100, 40)
            for y in 40..60 {
                for x in 100..120 {
                    assert_eq!(px(x, y), 1, "({x},{y})");
                }
            }
            assert_eq!(px(99, 50), 0);
            assert_eq!(px(110, 39), 0);
            // "Hi!" at size 2 lights pixels in the top-left
            let text: usize = (0..16).flat_map(|y| (0..36).map(move |x| (x, y))).map(|(x, y)| px(x, y) as usize).sum();
            assert!(text > 40, "{text}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn lcd_i2c_backpack_shows_text() {
    let (p, u1, d) = i2c_bench("lcd-i2c", "lcd_1602_i2c");
    let mut bench = start(p, &[(&u1, "lcd_i2c")]);
    bench.run_until(ms(1500)).unwrap();
    match bench.visual(&d).unwrap().look {
        Look::TextLcd { text, backlight, display_on, .. } => {
            assert_eq!(text[0].trim_end(), "I2C backpack");
            assert_eq!(text[1].trim_end(), "works!");
            assert!(backlight && display_on);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn neopixel_strip_decodes_the_800khz_protocol() {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, "neopixel");
    let s = b.offboard("ws2812_strip", "LED1", 40.0, 24.0, &[("count", n(8.0))]);
    b.wire((&s, "VDD"), (&bb, "t+10"), "red");
    b.wire((&s, "GND"), (&bb, "t-10"), "black");
    b.wire((&s, "DIN"), (&u1, "D6"), "green");
    let mut bench = start(b.finish(), &[(&u1, "neopixel")]);
    bench.run_until(ms(500)).unwrap();
    match bench.visual(&s).unwrap().look {
        Look::LedArray { leds } => {
            assert_eq!(leds.len(), 8);
            assert_eq!(leds[0], Rgb::new(255, 0, 0));
            assert_eq!(leds[1], Rgb::new(0, 255, 0));
            assert_eq!(leds[2], Rgb::new(0, 0, 0));
            assert_eq!(leds[7], Rgb::new(0, 0, 64));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(reading(&bench, &s, "frames"), 1.0);
}

#[test]
fn hc_sr04_measures_the_distance_to_a_hand() {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, "sonar");
    b.project.world.noise = false;
    let s = b.offboard("hc_sr04", "U2", 40.0, 24.0, &[]);
    b.wire((&s, "VCC"), (&bb, "t+10"), "red");
    b.wire((&s, "GND"), (&bb, "t-10"), "black");
    b.wire((&s, "TRIG"), (&u1, "D7"), "green");
    b.wire((&s, "ECHO"), (&u1, "D8"), "yellow");
    // a hand 30 cm in front (the sensor faces −y on the bench)
    let at = Pos::new(40.0 * 0.00254, 24.0 * 0.00254);
    b.project.world.objects = vec![bb_world::ObjectConfig {
        id: "hand".into(),
        pos: Pos::new(at.x, at.y - 0.32),
        radius: 0.02,
        ..Default::default()
    }];
    let mut bench = start(b.finish(), &[(&u1, "sonar")]);
    bench.run_until(ms(500)).unwrap();
    let cms: Vec<f64> = lines(&bench, &u1).iter().filter_map(|l| field(l, "cm")).collect();
    assert!(cms.len() >= 3, "{cms:?}");
    assert!((cms.last().unwrap() - 30.0).abs() < 1.0, "{cms:?}");
    // move the hand away: 1 m
    bench.set_world("hand.y", at.y - 1.02);
    bench.run_until(ms(900)).unwrap();
    let cms: Vec<f64> = lines(&bench, &u1).iter().filter_map(|l| field(l, "cm")).collect();
    assert!((cms.last().unwrap() - 100.0).abs() < 2.5, "{cms:?}");
}

#[test]
fn hc595_shift_register_latches_shifted_bytes() {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, "595");
    // 74HC595 straddling the channel: pin 1 (Q1) at f10 … pin 16 (VCC) at e10
    let u = b.rigid("hc595", "U2", &bb, "Q1", "f10", 0.0, &[]);
    b.wire((&bb, "a10"), (&bb, "t+10"), "red"); // VCC
    b.wire((&bb, "j17"), (&bb, "t-12"), "black"); // GND (pin 8)
    b.wire((&bb, "a11"), (&u1, "D11"), "blue"); // DS → wait: pin 14 is DS
    let mut project = b.finish();
    // fix up the data/clock/latch wiring by pin name instead of holes
    project.wires.pop();
    let mut b2 = bb_circuit::builder::BenchBuilder::new(&lib, "595", bb_circuit::project::Mode::Playground);
    b2.project = project;
    b2.wire((&u, "DS"), (&u1, "D11"), "blue");
    b2.wire((&u, "SHCP"), (&u1, "D12"), "yellow");
    b2.wire((&u, "STCP"), (&u1, "D8"), "green");
    b2.wire((&u, "MR"), (&bb, "t+20"), "red");
    b2.wire((&u, "OE"), (&bb, "t-20"), "black");
    let mut bench = start(b2.finish(), &[(&u1, "shift595")]);
    bench.run_until(ms(150)).unwrap();
    assert_eq!(reading(&bench, &u, "latch"), 0xA5 as f64);
    bench.run_until(ms(350)).unwrap();
    assert_eq!(reading(&bench, &u, "latch"), 0x3C as f64);
    // Q2 (bit 2 of 0x3C) drives high, Q0 low
    let q2 = bench.pin_voltage(&u, "Q2").unwrap();
    let q0 = bench.pin_voltage(&u, "Q0").unwrap();
    assert!(q2 > 4.5 && q0 < 0.5, "{q2} {q0}");
}

#[test]
fn tone_on_a_passive_buzzer_sounds_at_the_right_pitch() {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, "tone");
    let bz = b.two_lead("buzzer_passive", "BZ1", &bb, "a10", "a13", &[]);
    b.wire((&u1, "D8"), (&bb, "b10"), "orange");
    b.wire((&bb, "b13"), (&bb, "t-12"), "black");
    let mut bench = start(b.finish(), &[(&u1, "tone")]);
    bench.run_until(ms(250)).unwrap();
    let f1 = reading(&bench, &bz, "freq_hz");
    bench.run_until(ms(550)).unwrap();
    let f2 = reading(&bench, &bz, "freq_hz");
    assert!((f1 - 440.0).abs() < 5.0, "{f1}");
    assert!((f2 - 880.0).abs() < 10.0, "{f2}");
    let audio: Vec<_> = bench
        .take_events()
        .into_iter()
        .filter_map(|e| match e.event {
            DeviceEvent::Audio(a) => Some(a),
            _ => None,
        })
        .collect();
    assert!(audio.len() >= 10);
    let a = &audio[3];
    assert_eq!(a.sample_rate, 22_050);
    // a square wave: about f/22050·2 sign changes per sample
    let flips = a.samples.windows(2).filter(|w| (w[0] > 0.0) != (w[1] > 0.0)).count();
    let expect = 2.0 * 440.0 * a.samples.len() as f64 / 22_050.0;
    assert!((flips as f64 - expect).abs() < 3.0, "{flips} vs {expect}");
}

#[test]
fn uln2003_drives_the_28byj48_stepper() {
    let lib = lib();
    let (mut b, u1, bb) = uno_bench(&lib, "steps");
    let uln = b.rigid("uln2003", "U2", &bb, "1B", "f10", 0.0, &[]);
    let m = b.offboard("stepper_28byj48", "M1", 40.0, 24.0, &[]);
    for (k, d) in [(1, "D8"), (2, "D9"), (3, "D10"), (4, "D11")] {
        b.wire((&uln, &format!("{k}B")), (&u1, d), "blue");
    }
    for (k, c) in [(1, "A"), (2, "B"), (3, "C"), (4, "D")] {
        b.wire((&uln, &format!("{k}C")), (&m, c), "orange");
    }
    b.wire((&uln, "E"), (&bb, "t-20"), "black");
    b.wire((&uln, "COM"), (&bb, "t+20"), "red");
    b.wire((&m, "COM"), (&bb, "t+21"), "red");
    let mut bench = start(b.finish(), &[(&u1, "steps")]);
    bench.run_until(ms(600)).unwrap();
    assert!(lines(&bench, &u1).contains(&"steps: forward done".to_string()));
    // 64 coil states = 63 full steps of 2 half-steps (the first energizing only aligns the rotor)
    assert_eq!(reading(&bench, &m, "half_steps"), 126.0);
    match bench.visual(&m).unwrap().look {
        Look::Stepper { angle_deg, .. } => assert!((angle_deg - 126.0 * 360.0 / 4096.0).abs() < 1e-9, "{angle_deg}"),
        other => panic!("{other:?}"),
    }
}


