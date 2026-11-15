//! API-level boards talking to bench devices through their buses (MCU-6 glue): ESP32 `Wire` →
//! BME280 part (transaction-level I²C into the device model), `SPI` → 74HC595, `Serial2` on
//! RX2/TX2 → HC-05 module → the virtual phone's Bluetooth terminal (real UART waveform on the
//! pins, both directions), and esp32-weather publishing through the bench's default radio
//! service built from the project settings.
//!
//! Skipped (with a note) when no C++ compiler is available.

mod common;
use bb_circuit::builder::BenchBuilder;
use bb_circuit::project::{FirmwareRef, Mode};
use bb_circuit::{Library, Project};
use bb_core::SimTime;
use bb_firmware::host::{HostKind, compile_host};
use bb_sim::radio::{PhoneCommand, RadioCommand};
use bb_sim::{Bench, BenchOptions};
use common::*;

fn sketch_dir(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/firmware/host")).join(name)
}

/// ESP32 (U1) on the bench with the sketch `name` compiled for it.
fn esp32_bench(
    lib: &Library,
    name: &str,
    extra: impl FnOnce(&mut BenchBuilder, &str),
) -> Option<Bench> {
    if bb_firmware::host::cxx().is_none() {
        eprintln!("no C++ compiler; skipping");
        return None;
    }
    let build = compile_host(&sketch_dir(name), HostKind::Esp32DevKit, None).unwrap();
    assert!(build.success, "{}", build.log);
    let mut b = BenchBuilder::new(lib, name, Mode::Playground);
    let u1 = b.offboard("esp32_devkit", "U1", 0.0, 0.0, &[]);
    extra(&mut b, &u1);
    let mut project: Project = b.finish();
    if let Some(p) = project.part_mut(&u1) {
        p.firmware = Some(FirmwareRef {
            sketch: format!("firmware/U1/{name}"),
            fqbn: Some("breadbox:host:esp32".into()),
            image: None,
        });
    }
    let mut opts = BenchOptions::default();
    opts.host_firmware.insert(u1.clone(), build.exe.unwrap());
    Some(Bench::new(project, std::sync::Arc::new(lib.clone()), opts).unwrap())
}

#[test]
fn esp32_wire_reads_a_bme280_part() {
    let lib = lib();
    let Some(mut b) = esp32_bench(&lib, "bme280_wire", |b, u1| {
        let s = b.offboard("bme280", "U2", 20.0, 0.0, &[]);
        b.wire((u1, "3V3"), (&s, "VIN"), "red");
        b.wire((u1, "GND"), (&s, "GND"), "black");
        b.wire((u1, "D21"), (&s, "SDA"), "blue");
        b.wire((u1, "D22"), (&s, "SCL"), "yellow");
    }) else {
        return;
    };
    b.set_world("air.temp_c", 24.5);
    b.run_until(SimTime::from_secs(3)).unwrap();
    let l = lines(&b, "u1");
    assert!(l.contains(&"chip_id=0x60".to_string()), "{l:#?}");
    assert!(l.contains(&"ctrl=0".to_string()), "{l:#?}");
    // nobody at 0x40: address NACK (Arduino's endTransmission → 2)
    assert!(l.contains(&"ghost=2".to_string()), "{l:#?}");
    let temps: Vec<f64> = l
        .iter()
        .filter_map(|x| x.strip_prefix("temp="))
        .filter_map(|t| t.parse().ok())
        .collect();
    assert!(temps.len() >= 3, "{l:#?}");
    let last = *temps.last().unwrap();
    assert!((last - 24.5).abs() < 0.3, "BME280 says {last} °C");
    let r = b.readings("u2");
    assert!(
        r.iter().any(|(k, v)| k == "transactions" && *v > 5.0),
        "{r:?}"
    );
}

#[test]
fn esp32_spi_shifts_into_a_74hc595() {
    let lib = lib();
    let Some(mut b) = esp32_bench(&lib, "spi_595", |b, u1| {
        let s = b.offboard("hc595", "U2", 20.0, 0.0, &[]);
        b.wire((u1, "3V3"), (&s, "VCC"), "red");
        b.wire((u1, "GND"), (&s, "GND"), "black");
        b.wire((u1, "3V3"), (&s, "MR"), "red");
        b.wire((u1, "GND"), (&s, "OE"), "black");
        b.wire((u1, "D23"), (&s, "DS"), "blue");
        b.wire((u1, "D18"), (&s, "SHCP"), "yellow");
        b.wire((u1, "D5"), (&s, "STCP"), "green");
    }) else {
        return;
    };
    b.run_until(SimTime::from_ms(1100)).unwrap();
    let sent: Vec<u8> = lines(&b, "u1")
        .iter()
        .filter_map(|x| x.strip_prefix("sent=0x"))
        .filter_map(|h| u8::from_str_radix(h, 16).ok())
        .collect();
    assert!(sent.len() >= 4, "{sent:?}");
    let latch = b
        .readings("u2")
        .iter()
        .find(|(k, _)| k == "latch")
        .map(|x| x.1)
        .unwrap();
    assert_eq!(latch as u8, *sent.last().unwrap());
}

#[test]
fn esp32_serial2_talks_to_the_phone_through_an_hc05() {
    let lib = lib();
    let Some(mut b) = esp32_bench(&lib, "serial2_spp", |b, u1| {
        let m = b.offboard("hc05", "U2", 20.0, 0.0, &[]);
        b.wire((u1, "3V3"), (&m, "VCC"), "red");
        b.wire((u1, "GND"), (&m, "GND"), "black");
        b.wire((u1, "TX2"), (&m, "RXD"), "blue");
        b.wire((u1, "RX2"), (&m, "TXD"), "yellow");
    }) else {
        return;
    };
    b.run_until(SimTime::from_secs(1)).unwrap();
    let tel = b.telemetry();
    let r = tel.radio.as_ref().expect("radio");
    let spp = r
        .phone
        .scan
        .iter()
        .find(|s| s.kind == "SPP")
        .expect("HC-05 in the scan");
    let id = spp.id;
    b.radio_command(RadioCommand::Phone(PhoneCommand::Connect {
        id,
        pin: Some("1234".into()),
    }))
    .unwrap();
    b.run_until(SimTime::from_ms(1500)).unwrap();
    b.radio_command(RadioCommand::Phone(PhoneCommand::TerminalOpen))
        .unwrap();
    b.run_until(SimTime::from_secs(3)).unwrap();
    b.radio_command(RadioCommand::Phone(PhoneCommand::TerminalSend(
        b"ping\n".to_vec(),
    )))
    .unwrap();
    b.run_until(SimTime::from_secs(4)).unwrap();
    let tel = b.telemetry();
    let r = tel.radio.as_ref().unwrap();
    assert!(
        r.phone.terminal.contains("hello\n"),
        "terminal: {:?} log {:#?}",
        r.phone.terminal,
        r.phone.log
    );
    let l = lines(&b, "u1");
    assert!(l.contains(&"got: ping".to_string()), "{l:#?}");
    // the HC-05 decoded the sketch's waveform at 9600 baud
    let sent = b.radio().unwrap().uart_sent_text("u2");
    assert!(sent.contains("hello\nhello\n"), "{sent:?}");
}

#[test]
fn esp32_weather_publishes_through_the_default_radio_service() {
    let dir = root().join("examples/esp32-weather");
    let project = Project::load(&dir).unwrap();
    if bb_firmware::host::cxx().is_none() {
        return;
    }
    let exes = bb_sim::firmware::compile_host_project(&project, &dir, &lib()).unwrap();
    let opts = BenchOptions {
        host_firmware: exes,
        ..Default::default()
    };
    let mut b = Bench::new(project, lib(), opts).unwrap();
    b.radio_command(RadioCommand::ExplorerSubscribe("weather/#".into()))
        .unwrap();
    b.run_until(SimTime::from_secs(8)).unwrap();
    let tel = b.telemetry();
    let r = tel.radio.as_ref().unwrap();
    let data: Vec<_> = r
        .mqtt
        .iter()
        .filter(|m| m.topic == "weather/esp32")
        .collect();
    assert!(data.len() >= 2, "{:#?}", r.mqtt);
    assert!(
        data[0].payload.starts_with("{\"t\":21.0,"),
        "{}",
        data[0].payload
    );
    let card = r
        .cards
        .iter()
        .find(|c| c.label == "U1")
        .expect("ESP32 card");
    assert_eq!(card.title, "ESP32 Wi-Fi + BLE");
    assert_eq!(card.status, "connected");
    // annotations go to U1's own monitor
    let mon = b.serial("u1").unwrap();
    assert!(
        mon.lines
            .iter()
            .any(|l| l.kind == bb_sim::LineKind::Net && l.text.starts_with("mqtt:")),
        "{:#?}",
        mon.lines
    );
}

/// A board with `sketch` compiled for `kind` and `wires` between two of its own pins.
fn looped_board(
    lib: &Library,
    kind: HostKind,
    part: &str,
    wires: &[(&str, &str)],
) -> Option<Bench> {
    if bb_firmware::host::cxx().is_none() {
        eprintln!("no C++ compiler; skipping");
        return None;
    }
    let build = compile_host(&sketch_dir("serial_loopback"), kind, None).unwrap();
    assert!(build.success, "{}", build.log);
    let mut b = BenchBuilder::new(lib, "loopback", Mode::Playground);
    let u1 = b.offboard(part, "U1", 0.0, 0.0, &[]);
    for (a, z) in wires {
        b.wire((&u1, a), (&u1, z), "yellow");
    }
    let mut project: Project = b.finish();
    if let Some(p) = project.part_mut(&u1) {
        p.firmware = Some(FirmwareRef {
            sketch: "firmware/U1/serial_loopback".into(),
            fqbn: None,
            image: None,
        });
    }
    let mut opts = BenchOptions::default();
    opts.host_firmware.insert(u1, build.exe.unwrap());
    Some(Bench::new(project, std::sync::Arc::new(lib.clone()), opts).unwrap())
}

/// Regression: a UART looped back to the *same* board (TX → RX) used to stay silent. The
/// board leads the bench, so its own TX edges reached its RX after it had run past them and
/// were stamped "now" — a whole frame collapsed into one instant; and the receiver decoded the
/// floating RX line before `begin()`, was busy when `begin()` came, and never switched to the
/// configured baud.
#[test]
fn esp32_serial1_and_serial2_loop_back_on_the_same_board() {
    let lib = lib();
    // Serial1 routed to D26 (RX) / D25 (TX) by begin(); Serial2 on its default RX2 / TX2
    let Some(mut b) = looped_board(
        &lib,
        HostKind::Esp32DevKit,
        "esp32_devkit",
        &[("D25", "D26"), ("TX2", "RX2")],
    ) else {
        return;
    };
    b.run_until(SimTime::from_secs(1)).unwrap();
    let l = lines(&b, "u1");
    for k in 1..=4 {
        assert!(l.contains(&format!("got1: one {k}")), "{l:#?}");
        assert!(l.contains(&format!("got2: two {k}")), "{l:#?}");
    }
    // nothing but the echoes (no break / garbage byte from the line before begin())
    assert!(
        l.iter()
            .all(|x| x.starts_with("got1: one ") || x.starts_with("got2: two ")),
        "{l:#?}"
    );
}

#[test]
fn pico_serial1_and_serial2_loop_back_on_the_same_board() {
    let lib = lib();
    // Serial1 = UART0 (TX GP0 → RX GP1), Serial2 = UART1 (TX GP8 → RX GP9)
    let Some(mut b) = looped_board(
        &lib,
        HostKind::Pico,
        "pico",
        &[("GP0", "GP1"), ("GP8", "GP9")],
    ) else {
        return;
    };
    b.run_until(SimTime::from_secs(1)).unwrap();
    let l = lines(&b, "u1");
    for k in 1..=4 {
        assert!(l.contains(&format!("got1: one {k}")), "{l:#?}");
        assert!(l.contains(&format!("got2: two {k}")), "{l:#?}");
    }
}

#[test]
fn an_unwired_uart_hears_nothing_and_a_reset_reopens_it() {
    let lib = lib();
    // only Serial2 looped: Serial1 (D25 → nothing, D26 floating) receives nothing
    let Some(mut b) = looped_board(
        &lib,
        HostKind::Esp32DevKit,
        "esp32_devkit",
        &[("TX2", "RX2")],
    ) else {
        return;
    };
    b.run_until(SimTime::from_ms(500)).unwrap();
    let l = lines(&b, "u1");
    assert!(l.iter().any(|x| x == "got2: two 2"), "{l:#?}");
    assert!(!l.iter().any(|x| x.starts_with("got1")), "{l:#?}");
    // EN reset: the sketch restarts, begin() reopens Serial2 and the echo comes back from 1
    b.mcu("u1").unwrap().reset();
    b.run_until(SimTime::from_ms(1000)).unwrap();
    let l = lines(&b, "u1");
    let after: Vec<&String> = l.iter().skip_while(|x| *x != "got2: two 2").collect();
    assert!(
        after.iter().filter(|x| **x == "got2: two 1").count() >= 1,
        "{l:#?}"
    );
}
