//! `PRJ-3` / `RF-5` end to end without bb-sim: the real garden-node firmware (compiled by
//! arduino-cli with avr-gcc) runs on `bb_avr::Avr`; its SoftwareSerial pins D2 (TX) / D3 (RX)
//! are wired bit-for-bit to the ESP-01 modem at 9600 baud; A0/A1 get voltages; a virtual MQTT
//! client subscribes to `garden/#` on the built-in broker.

mod common;

use bb_core::SimTime;
use bb_radio::espat::EspAtConfig;
use bb_radio::geo::{Pos, Wall};
use bb_radio::log::Dir;
use bb_radio::mqtt::{BrokerEvent, Received};
use bb_radio::net::NetConditions;
use bb_radio::{ModulePin, RadioSpace};
use common::*;

/// The design's default world: 42 % moisture → soil_raw ≈ 385, 180 lux → light_raw ≈ 532.
fn soil_volts(raw: f64) -> f64 {
    raw * 5.0 / 1024.0 + 0.001
}

fn map(x: i32, in_min: i32, in_max: i32, out_min: i32, out_max: i32) -> i32 {
    (x - in_min) * (out_max - out_min) / (in_max - in_min) + out_min
}

/// Same arithmetic as `luxFromRaw` in wifi_at.h (AVR `float` = f32).
fn lux_from_raw(raw: i32) -> i32 {
    if raw <= 0 {
        return 0;
    }
    let raw = raw.min(1022) as f32;
    let r = 10000.0f32 * (1023.0f32 / raw - 1.0);
    (350000.0f32 / r).powf(1.0 / 0.7).round() as i32
}

fn garden_bench() -> Bench {
    let mut space = RadioSpace::default();
    space.net.geometry.walls.push(Wall::interior(Pos::new(1.5, -5.0), Pos::new(1.5, 5.0)));
    let esp = space.add_esp01("U2", Pos::new(3.2, 0.0), EspAtConfig::with_baud(9600));
    space.power(esp, true, SimTime::ZERO);
    let mut avr = load_example("garden-node");
    avr.set_temperature(21.0);
    let mut b = Bench::new(avr, space, esp, uno(2), uno(3));
    // Moisture ramps from 42 % worth of sensor voltage to wet after 8 s; light steps to dusk.
    b.volts = Box::new(|pin, t| {
        let s = t.as_secs_f64();
        if pin == uno(14) {
            if s < 8.0 { soil_volts(385.0) } else { soil_volts(300.0) }
        } else if pin == uno(15) {
            if s < 8.0 { soil_volts(532.0) } else { soil_volts(180.0) }
        } else {
            0.0
        }
    });
    b
}

#[test]
fn garden_node_joins_publishes_and_prints() {
    let mut b = garden_bench();
    let sub = b.space.net.broker_mut().virtual_connect("dashboard", SimTime::ZERO);
    let out = b.space.net.broker_mut().virtual_subscribe(sub, "garden/#", 0, SimTime::ZERO);
    b.space.net.apply_broker(out);

    b.run_until(SimTime::from_secs(16));

    // --- Wi-Fi join + DHCP: the modem joined SIM-AP and got 10.0.0.42.
    let wifi: Vec<String> = b.notes.iter().map(|n| n.line()).collect();
    assert!(
        wifi.iter().any(|l| l == "wifi: AT+CWJAP=\"SIM-AP\" → OK rssi=-54 dBm ip=10.0.0.42"),
        "annotations: {wifi:#?}\nsent: {:?}\nreceived: {:?}",
        b.wire.sent_text(),
        b.wire.received_text(9600)
    );
    let log: Vec<(String, String)> = b.space.net.log().visible().map(|e| (e.dir.label().to_string(), e.text())).collect();
    assert!(log.contains(&("tx".into(), "DHCP REQUEST 10.0.0.42".into())), "{log:#?}");
    assert!(log.contains(&("rx".into(), "DHCP ACK · lease 3600 s".into())));
    assert!(log.contains(&("tx".into(), "MQTT CONNECT client=garden-node".into())));
    let card = &b.space.cards()[0];
    assert_eq!(card.status, "connected");
    assert!(card.rows.contains(&("RSSI".into(), "-54 dBm (3.2 m, 1 wall)".into())), "{card:?}");
    assert!(card.rows.contains(&("IP".into(), "10.0.0.42 / DHCP".into())));

    // --- The broker saw the CONNECT from the firmware.
    let evs = b.space.net.take_broker_events();
    assert!(evs.iter().any(|(_, e)| matches!(e, BrokerEvent::Connected { client, .. } if client == "garden-node")));

    // --- Publishes: JSON once per second, tracking the ADC voltages.
    let msgs: Vec<Received> = b.space.net.broker_mut().virtual_take(sub);
    assert!(msgs.len() >= 12, "only {} publishes: {msgs:?}", msgs.len());
    let first = msgs[0].time;
    assert!(first < SimTime::from_secs(4), "first publish at {first}");
    // Long-run rate: one per second.
    let span = (msgs.last().unwrap().time - first).as_secs_f64();
    let rate = (msgs.len() - 1) as f64 / span;
    assert!((rate - 1.0).abs() < 0.08, "rate {rate}/s over {span} s");
    for w in msgs.windows(2) {
        let dt = (w[1].time - w[0].time).as_secs_f64();
        assert!((0.4..1.7).contains(&dt), "gap {dt}");
    }
    let expect = |soil: i32, light: i32| format!("{{\"m\":{},\"lux\":{},\"t\":21}}", map(soil, 620, 284, 0, 60), lux_from_raw(light));
    for m in &msgs {
        assert_eq!(m.message.topic, "garden/pot1");
        let s = m.time.as_secs_f64();
        let payload = m.message.payload_str();
        if s < 7.9 {
            assert_eq!(payload, expect(385, 532), "at {s}");
        } else if s > 8.7 {
            assert_eq!(payload, expect(300, 180), "at {s}");
        }
    }
    assert_eq!(expect(385, 532), r#"{"m":41,"lux":180,"t":21}"#);

    // --- Serial monitor (USART0 at 115200).
    let lines: Vec<&str> = b.lines.iter().map(|l| l.1.as_str()).collect();
    let data: Vec<&str> = lines.iter().copied().filter(|l| l.starts_with("soil_raw=")).collect();
    assert!(data.len() >= 20, "{lines:#?}");
    assert!(data.contains(&"soil_raw=385 moisture=41% light_raw=532 led_pwm=122"), "{data:#?}");
    assert!(data.contains(&"soil_raw=300 moisture=57% light_raw=180 led_pwm=210"), "{data:#?}");
    // Lines every ~0.5 s.
    let times: Vec<f64> = b.lines.iter().filter(|l| l.1.starts_with("soil_raw=")).map(|l| l.0.as_secs_f64()).collect();
    for w in times.windows(2) {
        assert!((0.49..0.75).contains(&(w[1] - w[0])), "line gap {}", w[1] - w[0]);
    }
    // Annotations for the serial monitor: the publish line.
    assert!(wifi.iter().any(|l| l == r#"mqtt: PUB garden/pot1 {"m":41,"lux":180,"t":21} (40 B)"#), "{wifi:#?}");

    // --- LEDs: the night LED PWM duty follows light; the water LED is off (41 % ≥ 30 %).
    let d8: Vec<_> = b.pins.iter().filter(|e| e.pin == uno(8)).collect();
    assert!(d8.iter().all(|e| e.state != bb_avr::PinState::Output { high: true }));
}

#[test]
fn garden_node_survives_loss_latency_and_ap_drop() {
    let mut b = garden_bench();
    b.space.net.set_conditions(NetConditions { latency_ms: 40.0, jitter_ms: 10.0, loss_pct: 2.0, drop_ap_at_s: Some(12.0), ap_down_s: 4.0 });
    let sub = b.space.net.broker_mut().virtual_connect("dashboard", SimTime::ZERO);
    let out = b.space.net.broker_mut().virtual_subscribe(sub, "garden/#", 0, SimTime::ZERO);
    b.space.net.apply_broker(out);
    b.run_until(SimTime::from_secs(40));
    let msgs = b.space.net.broker_mut().virtual_take(sub);
    let before = msgs.iter().filter(|m| m.time < SimTime::from_secs(12)).count();
    let after = msgs.iter().filter(|m| m.time > SimTime::from_secs(24)).count();
    assert!(before >= 7, "{before} before the drop");
    assert!(after >= 12, "{after} after reconnecting: {:#?}", b.notes.iter().map(|n| n.line()).collect::<Vec<_>>());
    let lines: Vec<String> = b.notes.iter().map(|n| n.line()).collect();
    assert!(lines.iter().any(|l| l.contains("WIFI DISCONNECT (AP down)")), "{lines:#?}");
    // The modem told the firmware, which reconnected the MQTT link itself.
    let rx = b.wire.received_text(9600);
    assert!(rx.contains("WIFI DISCONNECT"));
    assert!(rx.matches("WIFI GOT IP").count() >= 2);
    let connects = b.space.net.log().visible().filter(|e| e.text() == "MQTT CONNECT client=garden-node" && e.dir == Dir::Tx).count();
    assert!(connects >= 2, "reconnected {connects}");
    // Fault-injector rows exist.
    assert!(b.space.net.log().entries().any(|e| e.text() == "dropped by fault injector (loss 2%)"));
}

#[test]
fn modem_reset_pin_reboots_with_boot_garbage() {
    let mut b = garden_bench();
    b.run_until(SimTime::from_secs(3));
    let n0 = b.wire.received.iter().filter(|f| f.config.baud == 74_880).count();
    assert!(n0 > 300, "boot log at 74880 baud ({n0} bytes)");
    let t = b.now();
    b.space.set_pin(b.module, ModulePin::Reset, false, t);
    b.space.set_pin(b.module, ModulePin::Reset, true, t + SimTime::from_ms(1));
    b.run_until(SimTime::from_secs(4));
    let n1 = b.wire.received.iter().filter(|f| f.config.baud == 74_880).count();
    assert_eq!(n1, 2 * n0);
    assert_eq!(b.wire.received_text(9600).matches("ready").count(), 2);
}
