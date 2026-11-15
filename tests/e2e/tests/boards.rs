//! TEST-4 · boards on the threaded engine: blink timing on the AVR core, ESP32 Wi-Fi + MQTT
//! (API-level host board), the HM-10 + virtual phone (read, notify, terminal), the Pico's knob,
//! and the thermometer's LCD following the world's air temperature.

use bb_circuit::project::{Endpoint, ProbeDef, Quantity};
use bb_core::{Look, UserInput};
use bb_sim::radio::{PhoneCommand, RadioCommand};
use bb_sim::{Command, LineKind};
use breadbox_e2e::*;

fn board_l(sim: &Sim, part: &str) -> f32 {
    match sim.part(part).visual.look {
        Look::Board { l, .. } => l,
        other => panic!("{other:?}"),
    }
}

/// blink: D13 high 1 s / low 1 s, measured by the probe tool's frequency + duty probes and by
/// sampling the pin mid-phase. Firmware compiled by the toolchain when installed.
#[test]
fn blink_timing() {
    let (p, dir) = example("blink");
    let (img, _elf, src) = avr_firmware(&p, &dir, "u1", false);
    eprintln!("blink firmware: {src:?}");
    let mut opts = bench_options(&p, &dir).unwrap();
    opts.firmware.insert("u1".into(), img);
    let mut sim = Sim::spawn(p, lib(), opts);
    for (id, q) in [("f13", Quantity::Frequency), ("d13", Quantity::Duty)] {
        sim.send(Command::AddProbe(ProbeDef {
            id: id.into(),
            label: id.into(),
            target: Endpoint::pin("u1", "D13"),
            quantity: q,
            pinned: None,
        }));
    }
    // sample the logic probe in the middle of each half period
    let mut levels = Vec::new();
    let mut t = 0.5;
    while t < 8.0 {
        sim.run_to(secs(t));
        levels.push(sim.probe("LED · D13").text.clone());
        t += 1.0;
    }
    assert_eq!(
        levels,
        ["HIGH", "LOW", "HIGH", "LOW", "HIGH", "LOW", "HIGH", "LOW"],
        "D13 mid-phase levels"
    );
    let f = sim.probe("f13").value;
    let duty = sim.probe("d13").value;
    assert!(
        (f - 0.5).abs() < 0.5 * 0.001,
        "blink frequency {f} Hz (±0.1 %)"
    );
    assert!((duty - 50.0).abs() < 0.1, "duty {duty} %");
    // the bench LED on D13 follows
    let lit = matches!(sim.part("d1").visual.look, Look::Led { current, .. } if current > 1e-3);
    assert!(!lit, "LOW phase at t = 7.5 s");
    sim.run_to(secs(8.5));
    let lit = matches!(sim.part("d1").visual.look, Look::Led { current, .. } if current > 1e-3);
    assert!(lit, "HIGH phase at t = 8.5 s");
    assert!(sim.problems().is_empty(), "{:?}", sim.problems());
    sim.shutdown();
}

/// esp32-weather: the ESP32 joins SIM-AP, reads the DHT22 (bit protocol on GPIO4) every 2 s and
/// publishes JSON to `weather/esp32`; the retained LWT topic says `online`; a command from the
/// MQTT explorer switches the on-board LED; the reading follows the world's air.
#[test]
fn esp32_weather_wifi_mqtt_and_commands() {
    let Some(mut sim) = Sim::example("esp32-weather") else {
        eprintln!("no C++ compiler: skipping the ESP32 host board");
        return;
    };
    sim.send(Command::Radio(RadioCommand::ExplorerSubscribe(
        "weather/#".into(),
    )));
    sim.run_secs(9.0);
    let data: Vec<String> = sim
        .data_lines("U1")
        .iter()
        .map(|l| l.text.clone())
        .collect();
    let joined = data
        .iter()
        .find(|l| l.starts_with("Wi-Fi: connected, IP "))
        .unwrap_or_else(|| panic!("{data:#?}"));
    assert!(
        joined.contains("10.0.0.") && joined.contains("RSSI -"),
        "{joined}"
    );
    assert!(data.iter().any(|l| l == "MQTT: connected"), "{data:#?}");
    let readings: Vec<(f64, f64)> = data
        .iter()
        .filter(|l| l.ends_with("-> weather/esp32"))
        .map(|l| (field(l, "t").unwrap(), field(l, "h").unwrap()))
        .collect();
    assert!(readings.len() >= 3, "{data:#?}");
    for (t, h) in &readings {
        assert!((t - 21.0).abs() < 0.6, "DHT22 t={t}");
        assert!((h - 45.0).abs() < 2.5, "DHT22 h={h}");
    }
    let tel = sim.tel();
    let radio = tel.radio.as_ref().unwrap();
    let pubs: Vec<_> = radio
        .mqtt
        .iter()
        .filter(|r| r.topic == "weather/esp32")
        .collect();
    assert!(pubs.len() >= 3, "{:#?}", radio.mqtt);
    let dt: Vec<f64> = pubs
        .windows(2)
        .map(|w| (w[1].time - w[0].time).as_secs_f64())
        .collect();
    assert!(
        dt.iter().all(|d| (d - 2.0).abs() < 0.1),
        "every 2 s: {dt:?}"
    );
    let v: serde_json::Value = serde_json::from_str(&pubs[0].payload).unwrap();
    assert!((v["t"].as_f64().unwrap() - 21.0).abs() < 0.6, "{v}");
    let status = radio
        .mqtt
        .iter()
        .find(|r| r.topic == "weather/status")
        .expect("retained status");
    assert_eq!(status.payload, "online");
    let mcu = tel.mcus.iter().find(|m| m.part == "u1").unwrap();
    assert_eq!(mcu.engine, "API-level");

    // a dashboard command switches GPIO2 (LED_BUILTIN)
    sim.send(Command::Radio(RadioCommand::ExplorerPublish {
        topic: "weather/cmd".into(),
        payload: b"led on".to_vec(),
        qos: 0,
        retain: false,
    }));
    sim.run_secs(0.5);
    sim.run_secs(0.2);
    assert!(
        sim.data_lines("U1")
            .iter()
            .any(|l| l.text == "MQTT: weather/cmd = led on"),
        "command reached the sketch"
    );
    assert!(board_l(&sim, "u1") > 0.9, "LED on: {}", board_l(&sim, "u1"));
    sim.send(Command::Radio(RadioCommand::ExplorerPublish {
        topic: "weather/cmd".into(),
        payload: b"led off".to_vec(),
        qos: 0,
        retain: false,
    }));
    sim.run_secs(0.5);
    sim.run_secs(0.2);
    assert!(
        board_l(&sim, "u1") < 0.1,
        "LED off: {}",
        board_l(&sim, "u1")
    );

    // warm the room: the DHT22 reports it (thermal lag of the sensor allowed)
    sim.send(Command::SetWorld {
        key: "air.temp_c".into(),
        value: 30.0,
    });
    sim.send(Command::SetWorld {
        key: "air.rh".into(),
        value: 60.0,
    });
    sim.run_secs(30.0);
    let last = sim
        .data_lines("U1")
        .iter()
        .rev()
        .find(|l| l.text.ends_with("-> weather/esp32"))
        .map(|l| (field(&l.text, "t").unwrap(), field(&l.text, "h").unwrap()))
        .unwrap();
    assert!((last.0 - 30.0).abs() < 1.0, "t follows the room: {last:?}");
    assert!((last.1 - 60.0).abs() < 3.0, "h follows the room: {last:?}");
    assert!(sim.problems().is_empty(), "{:?}", sim.problems());
    sim.shutdown();
}

/// ble-button: the virtual phone scans the HM-10, connects (the sketch sees `OK+CONN`),
/// subscribes to FFE1 and gets `pressed #n` notifications when the button is pressed; it reads
/// the characteristic, and the Bluetooth terminal switches the LED and answers `?`.
#[test]
fn ble_button_virtual_phone_read_notify_and_terminal() {
    let mut sim = Sim::example("ble-button").unwrap();
    sim.run_secs(2.0);
    let data = |sim: &Sim| -> Vec<String> {
        sim.data_lines("U1")
            .iter()
            .map(|l| l.text.clone())
            .collect()
    };
    assert!(
        data(&sim).iter().any(|l| l == "ble-button ready"),
        "{:#?}",
        data(&sim)
    );
    assert!(
        data(&sim)
            .iter()
            .any(|l| l.starts_with("AT+NAMEbb-button -> OK+Set")),
        "{:#?}",
        data(&sim)
    );
    let tel = sim.tel();
    let radio = tel.radio.as_ref().unwrap();
    assert!(radio.has_ble);
    let dev = radio
        .phone
        .scan
        .iter()
        .find(|s| s.name == "bb-button")
        .unwrap_or_else(|| panic!("scan: {:#?}", radio.phone.scan));
    assert_eq!(dev.kind, "BLE");
    assert!((-90..-20).contains(&dev.rssi), "rssi {}", dev.rssi);
    let id = dev.id;
    sim.send(Command::Radio(RadioCommand::Phone(PhoneCommand::Connect {
        id,
        pin: None,
    })));
    sim.run_secs(1.0);
    assert!(data(&sim).iter().any(|l| l == "phone connected"));
    let tel = sim.tel();
    let phone = &tel.radio.as_ref().unwrap().phone;
    assert_eq!(phone.connected, Some(id));
    let ch = phone
        .services
        .iter()
        .flat_map(|s| s.characteristics.iter())
        .find(|c| c.uuid.to_ascii_uppercase().contains("FFE1"))
        .unwrap_or_else(|| panic!("FFE1 in {:#?}", phone.services))
        .clone();
    assert!(ch.props.iter().any(|p| p == "notify"), "{ch:#?}");
    sim.send(Command::Radio(RadioCommand::Phone(
        PhoneCommand::Subscribe {
            handle: ch.handle,
            on: true,
        },
    )));
    sim.run_secs(0.3);
    // press the button twice
    for _ in 0..2 {
        sim.send(Command::Input {
            part: "sw1".into(),
            input: UserInput::Press(true),
        });
        sim.run_secs(0.15);
        sim.send(Command::Input {
            part: "sw1".into(),
            input: UserInput::Press(false),
        });
        sim.run_secs(0.35);
    }
    assert!(
        data(&sim).iter().any(|l| l == "pressed #2"),
        "{:#?}",
        data(&sim)
    );
    let tel = sim.tel();
    let phone = &tel.radio.as_ref().unwrap().phone;
    let notes: Vec<&str> = phone
        .log
        .iter()
        .filter(|r| r.kind == "notify")
        .map(|r| r.text.as_str())
        .collect();
    assert!(
        notes.iter().any(|t| t.contains("pressed #1"))
            && notes.iter().any(|t| t.contains("pressed #2")),
        "notifications: {:#?}",
        phone.log
    );
    // read the characteristic: the phone logs a read with the module's last value
    if ch.props.iter().any(|p| p == "read") {
        sim.send(Command::Radio(RadioCommand::Phone(PhoneCommand::Read {
            handle: ch.handle,
        })));
        sim.run_secs(0.3);
        let tel = sim.tel();
        let phone = &tel.radio.as_ref().unwrap().phone;
        assert!(
            phone.log.iter().any(|r| r.kind == "read"),
            "read row: {:#?}",
            phone.log
        );
    }
    // the Bluetooth terminal: "on" lights D13, "?" answers with the press count
    sim.send(Command::Radio(RadioCommand::Phone(
        PhoneCommand::TerminalOpen,
    )));
    sim.send(Command::Radio(RadioCommand::Phone(
        PhoneCommand::TerminalSend(b"on\n".to_vec()),
    )));
    sim.run_secs(0.5);
    sim.run_secs(0.1);
    assert!(board_l(&sim, "u1") > 0.9, "LED on: {}", board_l(&sim, "u1"));
    sim.send(Command::Radio(RadioCommand::Phone(
        PhoneCommand::TerminalSend(b"?\n".to_vec()),
    )));
    sim.run_secs(0.5);
    let tel = sim.tel();
    let term = &tel.radio.as_ref().unwrap().phone.terminal;
    assert!(term.contains("led on"), "terminal: {term:?}");
    assert!(term.contains("presses=2"), "terminal: {term:?}");
    // disconnect → the sketch sees OK+LOST
    sim.send(Command::Radio(RadioCommand::Phone(
        PhoneCommand::Disconnect,
    )));
    sim.run_secs(0.5);
    let uart = sim.with(|b| {
        b.radio()
            .map(|r| format!("{:?}", r.uart_transcript("u2")))
            .unwrap_or_default()
    });
    assert!(
        data(&sim).iter().any(|l| l == "phone disconnected"),
        "{:#?}\nphone {:#?}\nuart {uart}",
        data(&sim),
        sim.tel().radio.as_ref().unwrap().phone.log
    );
    assert!(sim.problems().is_empty(), "{:?}", sim.problems());
    sim.shutdown();
}

/// ble-button: the phone taps "Notify" and the button is pressed at the same simulated instant
/// (a paused bench, or a starved engine thread at Max speed) or a few ms later. The CCCD write
/// goes out in the phone's half of the next connection event, the HM-10's notification in its
/// half of that event or a later one, so `pressed #1` always reaches the phone (this used to be
/// dropped when the press came < 10 ms after the subscribe — the flaky GUI test).
#[test]
fn ble_button_press_right_after_subscribe_notifies() {
    for gap_ms in [0u64, 3] {
        let mut sim = Sim::example("ble-button").unwrap();
        sim.run_secs(2.0);
        let tel = sim.tel();
        let id = tel.radio.as_ref().unwrap().phone.scan[0].id;
        sim.send(Command::Radio(RadioCommand::Phone(PhoneCommand::Connect {
            id,
            pin: None,
        })));
        sim.run_secs(1.0);
        let tel = sim.tel();
        let ch = tel.radio.as_ref().unwrap().phone.services[0].characteristics[0].clone();
        sim.send(Command::Radio(RadioCommand::Phone(
            PhoneCommand::Subscribe {
                handle: ch.handle,
                on: true,
            },
        )));
        if gap_ms > 0 {
            sim.run_for(ms(gap_ms));
        }
        sim.send(Command::Input {
            part: "sw1".into(),
            input: UserInput::Press(true),
        });
        sim.run_secs(0.15);
        sim.send(Command::Input {
            part: "sw1".into(),
            input: UserInput::Press(false),
        });
        sim.run_secs(0.35);
        let tel = sim.tel();
        let log: Vec<(String, String)> = tel
            .radio
            .as_ref()
            .unwrap()
            .phone
            .log
            .iter()
            .map(|r| (r.kind.clone(), r.text.clone()))
            .collect();
        let sub = log
            .iter()
            .position(|(k, t)| k == "subscribed" && t == "notifications on · FFE1");
        let note = log
            .iter()
            .position(|(k, t)| k == "notify" && t == "pressed #1\n");
        assert!(
            sub.is_some() && note.is_some() && sub < note,
            "gap {gap_ms} ms: {log:#?}"
        );
        sim.shutdown();
    }
}

/// pico-blink: the API-level Pico blinks GP25 at 2 Hz and reads the knob on GP26 (12-bit).
#[test]
fn pico_blink_reads_the_knob() {
    let Some(mut sim) = Sim::example("pico-blink") else {
        eprintln!("no C++ compiler: skipping the Pico host board");
        return;
    };
    let reads = |sim: &Sim| -> Vec<f64> {
        sim.data_lines("U1")
            .iter()
            .filter_map(|l| l.text.strip_prefix("A0 raw="))
            .filter_map(|r| r.split_whitespace().next()?.parse().ok())
            .collect()
    };
    sim.run_secs(2.0);
    let last = *reads(&sim).last().expect("A0 readings");
    assert!((last - 2047.0).abs() < 60.0, "knob at 50 %: {last}");
    sim.send(Command::Input {
        part: "rv1".into(),
        input: UserInput::Position(0.25),
    });
    sim.run_secs(1.5);
    let last = *reads(&sim).last().unwrap();
    assert!((last - 1024.0).abs() < 60.0, "knob at 25 %: {last}");
    let mcu = sim.tel().mcus[0].clone();
    assert_eq!(mcu.engine, "API-level");
    sim.shutdown();
}

/// thermometer: TMP36 → A0 → LiquidCrystal 16×2; the LCD and the serial line follow the room.
#[test]
fn thermometer_lcd_follows_the_room_temperature() {
    let mut sim = Sim::example("thermometer").unwrap();
    sim.run_secs(3.0);
    let lcd = |sim: &Sim| match sim.part("lcd1").visual.look {
        Look::TextLcd { text, .. } => text,
        other => panic!("{other:?}"),
    };
    let rows = lcd(&sim);
    assert!(rows[0].starts_with("Temp: 2"), "{rows:?}");
    let c: f64 = rows[0][6..10].trim().parse().unwrap();
    assert!((c - 21.0).abs() < 1.0, "{rows:?}");
    assert!(rows[1].starts_with("lo "), "{rows:?}");
    sim.send(Command::SetWorld {
        key: "air.temp_c".into(),
        value: 35.0,
    });
    sim.run_secs(60.0);
    let rows = lcd(&sim);
    let c: f64 = rows[0][6..10].trim().parse().unwrap();
    assert!((c - 35.0).abs() < 1.5, "{rows:?}");
    let serial: Vec<f64> = sim
        .data_lines("U1")
        .iter()
        .filter_map(|l| l.text.strip_suffix(" C")?.parse().ok())
        .collect();
    assert!((serial.last().unwrap() - c).abs() < 0.2, "{serial:?}");
    assert!(
        sim.lines_of("U1", LineKind::Warn).is_empty(),
        "{:?}",
        sim.lines_of("U1", LineKind::Warn)
    );
    sim.shutdown();
}
