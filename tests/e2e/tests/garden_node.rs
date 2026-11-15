//! TEST-4 · garden-node (PRJ-3) end to end on the threaded engine: the real compiled sketch on
//! the UNO, the ESP-01 joining `SIM-AP` and publishing to the built-in broker, the MQTT
//! explorer as a virtual subscriber, the packet log, World sliders, probes, the bench meter and
//! scope, the Handheld DMM plugin loading A0, and the fault injector.

use bb_circuit::project::{Endpoint, Wire};
use bb_core::Look;
use bb_radio::net::NetConditions;
use bb_sim::radio::{MqttRow, RadioCommand};
use bb_sim::{Command, LineKind};
use breadbox_e2e::*;

/// (time s, soil_raw, moisture, light_raw, led_pwm) of every telemetry line seen so far.
fn readings(sim: &Sim) -> Vec<(f64, f64, f64, f64, f64)> {
    sim.data_lines("U1")
        .into_iter()
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

fn garden() -> Sim {
    let sim = Sim::example("garden-node").expect("garden-node has only AVR firmware");
    // the MQTT explorer is the virtual subscriber (RF-7)
    sim.send(Command::Radio(RadioCommand::ExplorerSubscribe(
        "garden/#".into(),
    )));
    sim
}

fn publishes(sim: &Sim) -> Vec<MqttRow> {
    sim.tel()
        .radio
        .as_ref()
        .expect("radio telemetry")
        .mqtt
        .iter()
        .filter(|r| r.topic == "garden/pot1")
        .cloned()
        .collect()
}

/// ADC reading of the LDR divider (LDR on top, 10 kΩ to GND) at `lux`, 5 V reference.
fn light_raw_at(lux: f64, vcc: f64) -> f64 {
    let r_ldr = 350e3 * lux.powf(-0.7);
    let v = vcc * 10e3 / (10e3 + r_ldr);
    v / vcc * 1024.0
}

#[test]
fn garden_node_full_run() {
    let mut sim = garden();
    sim.run_secs(12.0);

    // ---- serial: boot line, telemetry lines tracking the World, radio annotations
    let all: Vec<&str> = sim
        .serial
        .iter()
        .filter(|l| l.mcu == "U1")
        .map(|l| l.text.as_str())
        .collect();
    assert!(
        all[0].starts_with("Breadbox sim · ATmega328P @ 16 MHz · reset cause: power-on"),
        "{:?}",
        &all[..3.min(all.len())]
    );
    let r = readings(&sim);
    assert!(r.len() >= 15, "≥ 15 telemetry lines in 12 s: {all:#?}");
    for &(_, soil, moist, light, pwm) in &r {
        // design: soil_raw=382…388 moisture=41…43% light_raw=528…536 led_pwm=122
        assert!((soil - 385.0).abs() <= 8.0, "soil_raw {soil}");
        assert!((40.0..=43.0).contains(&moist), "moisture {moist}");
        assert!((light - 532.0).abs() <= 10.0, "light_raw {light}");
        assert!(
            (pwm - (255.0 - (light / 4.0).floor())).abs() <= 1.0,
            "led_pwm {pwm} vs light {light}"
        );
    }
    // lines arrive about every 0.5 s (delay(500) + loop work + the 1 s publish)
    let gaps: Vec<f64> = r.windows(2).map(|w| w[1].0 - w[0].0).collect();
    assert!(
        gaps.iter().all(|g| (0.45..2.5).contains(g)),
        "telemetry line gaps {gaps:?}"
    );
    let net: Vec<&str> = sim
        .lines_of("U1", LineKind::Net)
        .into_iter()
        .map(|l| l.text.as_str())
        .collect();
    assert!(
        net.iter()
            .any(|l| l.starts_with("wifi: AT+CWJAP=\"SIM-AP\" → OK")
                && l.contains("rssi=-54 dBm")
                && l.contains("ip=10.0.0.42")),
        "{net:#?}"
    );
    assert!(
        net.iter()
            .any(|l| l.starts_with("mqtt: PUB garden/pot1 {\"m\":")),
        "{net:#?}"
    );

    // ---- MQTT: the virtual subscriber receives garden/pot1 at ~1 Hz
    let pubs = publishes(&sim);
    assert!(pubs.len() >= 8, "{} publishes: {pubs:#?}", pubs.len());
    let t: Vec<f64> = pubs.iter().map(|p| p.time.as_secs_f64()).collect();
    let rate = (t.len() - 1) as f64 / (t[t.len() - 1] - t[0]);
    assert!(
        (0.85..=1.15).contains(&rate),
        "publish rate {rate} Hz: {t:?}"
    );
    let max_gap = t.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max);
    assert!(max_gap < 1.8, "longest gap between publishes {max_gap} s");
    for p in &pubs {
        let v: serde_json::Value = serde_json::from_str(&p.payload)
            .unwrap_or_else(|e| panic!("payload {:?}: {e}", p.payload));
        let m = v["m"].as_f64().unwrap();
        let lux = v["lux"].as_f64().unwrap();
        let temp = v["t"].as_f64().unwrap();
        assert!((40.0..=43.0).contains(&m), "{}", p.payload);
        assert!((lux - 180.0).abs() < 25.0, "{}", p.payload);
        assert!((temp - 21.0).abs() <= 4.0, "chip temp {}", p.payload);
        assert_eq!(p.qos, 0);
    }

    // ---- radio card + packet log (RF-4, RF-7)
    let tel = sim.tel();
    let radio = tel.radio.as_ref().unwrap();
    let card = radio
        .cards
        .iter()
        .find(|c| c.label == "U2")
        .unwrap_or_else(|| panic!("{:#?}", radio.cards));
    assert_eq!(card.status, "connected", "{card:#?}");
    let row = |k: &str| {
        card.rows
            .iter()
            .find(|r| r.0 == k)
            .map(|r| r.1.clone())
            .unwrap_or_default()
    };
    assert_eq!(row("RSSI"), "-54 dBm (3.2 m, 1 wall)", "{card:#?}");
    assert!(row("IP").starts_with("10.0.0.42"), "{card:#?}");
    let log: Vec<String> = radio
        .log
        .iter()
        .map(|r| format!("{} {} {}", r.time_label, r.dir, r.text))
        .collect();
    assert!(
        radio
            .log
            .iter()
            .any(|r| r.dir == "tx" && r.text.starts_with("PUBLISH garden/pot1")),
        "{log:#?}"
    );
    let (tx, rx) = crossbeam_channel::bounded(1);
    sim.send(Command::ExportPacketLog {
        format: "csv".into(),
        reply: tx,
    });
    let csv = String::from_utf8(rx.recv().unwrap()).unwrap();
    for needle in [
        "DHCP DISCOVER",
        "DHCP REQUEST 10.0.0.42",
        "DHCP ACK",
        "MQTT CONNECT client=garden-node",
        "CONNACK",
        "PUBLISH garden/pot1",
    ] {
        assert!(csv.contains(needle), "packet log lacks {needle:?}:\n{csv}");
    }
    let (tx, rx) = crossbeam_channel::bounded(1);
    sim.send(Command::ExportPacketLog {
        format: "pcapng".into(),
        reply: tx,
    });
    let pcap = rx.recv().unwrap();
    assert_eq!(
        &pcap[..4],
        &[0x0a, 0x0d, 0x0d, 0x0a],
        "pcap-ng section header"
    );

    // ---- probes, bench meter (A0 ≈ sensor model), scope on D9, LEDs, rails
    let model = 3.03 - 0.02737 * 42.0;
    let a0 = sim.probe("Soil sensor → A0");
    assert!((a0.avg - model).abs() < 0.01, "A0 {} vs {model}", a0.avg);
    let meter = tel.meters.first().expect("bench meter");
    assert!((meter.value - model).abs() < 0.03, "meter {}", meter.value);
    let scope = tel.scopes.first().expect("mini scope");
    assert!((scope.frequency - 490.2).abs() < 3.0, "{}", scope.frequency);
    assert!((scope.duty - 0.478).abs() < 0.02, "{}", scope.duty);
    let led = |id: &str| match sim.part(id).visual.look {
        Look::Led { current, .. } => current,
        other => panic!("{other:?}"),
    };
    assert!(led("d1") > 0.004 && led("d1") < 0.009, "D1 {}", led("d1"));
    assert!(led("d2") < 1e-4, "D2 {}", led("d2"));
    assert_eq!(tel.erc_summary, "ERC 1 warning · DRC clean");
    assert!(sim.problems().is_empty(), "{:?}", sim.problems());
    sim.shutdown();
}

#[test]
fn garden_node_d9_duty_tracks_ambient_light() {
    let mut sim = garden();
    sim.run_secs(3.0);
    let mut seen = Vec::new();
    for lux in [5.0, 50.0, 180.0, 600.0, 2000.0] {
        sim.send(Command::SetWorld {
            key: "light.ambient_lux".into(),
            value: lux,
        });
        sim.run_secs(1.5);
        let duty = sim.probe("Night LED · D9").value;
        let last = *readings(&sim).last().unwrap();
        let vcc = sim.tel().mcus.iter().find(|m| m.part == "u1").unwrap().vcc;
        // the LDR sees the ambient light plus the night LED next to it (WORLD-2)
        let at_ldr = sim.reading("ldr1", "lux").unwrap();
        assert!(at_ldr >= lux * 0.999, "{lux} lx ambient, LDR sees {at_ldr}");
        if lux <= 5.0 {
            assert!(
                at_ldr > lux + 1.0,
                "the bright night LED adds light: {at_ldr} lx"
            );
        }
        let raw = light_raw_at(at_ldr, vcc);
        assert!(
            (last.3 - raw).abs() <= raw * 0.03 + 4.0,
            "{lux} lx: light_raw {} vs LDR model {raw:.0} ({at_ldr:.1} lx at the LDR)",
            last.3
        );
        let pwm = (255.0 - (last.3 / 4.0).floor()).clamp(0.0, 255.0);
        assert!(
            (duty - pwm / 255.0 * 100.0).abs() < 1.5,
            "{lux} lx: D9 duty {duty} % vs led_pwm {pwm}"
        );
        seen.push((lux, duty));
    }
    assert!(
        seen.windows(2).all(|w| w[1].1 < w[0].1),
        "brighter → dimmer night LED: {seen:?}"
    );
    sim.shutdown();
}

#[test]
fn garden_node_soil_slider_moves_moisture_within_two_seconds() {
    let mut sim = garden();
    sim.run_secs(3.0);
    let before = readings(&sim).last().unwrap().2;
    let t0 = sim.time().as_secs_f64();
    sim.send(Command::SetWorld {
        key: "pot1.vwc".into(),
        value: 20.0,
    });
    sim.run_secs(2.0);
    let r = readings(&sim);
    let last = r.iter().rfind(|x| x.0 <= t0 + 2.0).unwrap();
    // capacitive probe τ ≈ 1.8 s: within 2 s the reading covers most of the step
    let moved = (before - last.2) / (before - 20.0);
    assert!(
        moved > 0.5,
        "moisture {before} → {} after {:.2} s ({:.0} % of the step)",
        last.2,
        last.0 - t0,
        moved * 100.0
    );
    sim.run_secs(5.0);
    let end = readings(&sim).last().unwrap().2;
    assert!((end - 20.0).abs() <= 1.5, "settles at {end}");
    // below 30 % the sketch lights the water LED on D8 and publishes the new value
    assert_eq!(sim.probe("Water LED · D8").text, "HIGH");
    let p = publishes(&sim);
    let v: serde_json::Value = serde_json::from_str(&p.last().unwrap().payload).unwrap();
    assert!((v["m"].as_f64().unwrap() - 20.0).abs() <= 1.5, "{v}");
    // "Water the pot" brings it back up
    sim.send(Command::Water {
        pot: "pot1".into(),
        ml: 250.0,
    });
    sim.run_secs(6.0);
    let watered = readings(&sim).last().unwrap().2;
    assert!(watered > 35.0, "after watering: {watered}");
    sim.shutdown();
}

fn wire(p: &mut bb_circuit::Project, id: &str, a: (&str, &str), b: (&str, &str)) {
    p.wires.retain(|w| w.id != id);
    p.wires.push(Wire {
        id: id.into(),
        a: Endpoint::pin(a.0, a.1),
        b: Endpoint::pin(b.0, b.1),
        color: "red".into(),
        bends: vec![],
    });
}

/// The Handheld DMM plugin (installed through the registry like the app does) clipped to A0:
/// it reads the sensor model, and its 10 MΩ input really shifts the node by the design's
/// −0.4 mV compared with the same bench without the meter.
#[test]
fn dmm_plugin_on_a0_reads_the_sensor_model_and_loads_the_node() {
    let scratch = TempDir::new("plugins");
    let mut reg = plugin_registry(scratch.path());
    let card = reg
        .catalog()
        .into_iter()
        .find(|c| c.id == "community.dmm-handheld")
        .expect("DMM in the catalog");
    let host = bb_plugin::PluginHost::new(&mut reg, None);
    assert!(host.errors.is_empty(), "{:?}", host.errors);
    let info = host
        .instruments()
        .into_iter()
        .find(|i| i.plugin == card.id)
        .expect("the DMM is an instrument");
    let (base, dir) = example("garden-node");
    let mk = |with_meter: bool| {
        let mut p = base.clone();
        p.world.noise = false;
        p.instruments.retain(|i| i.kind != "dmm");
        if with_meter {
            p.part_mut("dmm1").unwrap().part = info.part.clone();
            wire(&mut p, "wdmm1", ("dmm1", "VO"), ("j1", "AOUT"));
            wire(&mut p, "wdmm2", ("dmm1", "COM"), ("bb1", "t-1"));
        } else {
            p.parts.retain(|x| x.id != "dmm1");
        }
        let mut opts = host.bench_options();
        opts.firmware = bb_sim::firmware::project_images(&p, &dir);
        Sim::spawn(p, host.library(), opts)
    };
    let mut bare = mk(false);
    let mut sim = mk(true);
    bare.run_secs(3.0);
    sim.run_secs(3.0);
    let a0_bare = bare.with(|b| b.pin_voltage("j1", "AOUT").unwrap());
    let (a0, callout) = sim.with(|b| {
        (
            b.pin_voltage("j1", "AOUT").unwrap(),
            bb_plugin::host::loading_callout(b, "dmm1"),
        )
    });
    let c = callout.expect("loading callout");
    assert_eq!(
        c.text,
        "Probing A0 (J1 out): meter load shifts the node by −0.4 mV — shown as it would be on a real bench."
    );
    assert!(
        ((a0 - a0_bare) - c.shift).abs() < 2e-5,
        "node moved {} mV, callout says {} mV",
        (a0 - a0_bare) * 1e3,
        c.shift * 1e3
    );
    let v = sim.reading("dmm1", "value").unwrap();
    let model = 3.03 - 0.02737 * 42.0;
    assert!(
        (v - a0).abs() <= a0 * 0.0005 + 0.002,
        "DMM {v} vs node {a0}"
    );
    assert!((v - model).abs() < 0.01, "DMM {v} vs sensor model {model}");
    let panel = sim.part("dmm1").panel.expect("live panel");
    assert_eq!(panel["displays"]["lcd"]["unit"], "V");
    assert!(
        panel["displays"]["lcd"]["text"]
            .as_str()
            .unwrap()
            .starts_with("1.8")
    );
    // the meter tracks the soil slider like the sensor does
    sim.send(Command::SetWorld {
        key: "pot1.vwc".into(),
        value: 25.0,
    });
    sim.run_secs(8.0);
    let v = sim.reading("dmm1", "value").unwrap();
    let model = 3.03 - 0.02737 * 25.0;
    assert!(
        (v - model).abs() < 0.015,
        "DMM {v} vs model {model} at 25 %"
    );
    bare.shutdown();
    sim.shutdown();
}

/// RF-3: packet loss shows amber `!` rows while TCP keeps the publishes flowing; dropping the
/// AP disconnects the ESP-01, and the sketch reconnects and publishes again once it is back.
#[test]
fn garden_node_fault_injector_loss_and_ap_drop_reconnect() {
    let mut sim = garden();
    sim.run_secs(5.0);
    let base = publishes(&sim).len();
    assert!(base >= 2, "publishing before the faults");
    sim.send(Command::Radio(RadioCommand::SetConditions(NetConditions {
        loss_pct: 25.0,
        ..Default::default()
    })));
    sim.run_secs(8.0);
    let tel = sim.tel();
    let radio = tel.radio.as_ref().unwrap();
    let dropped: Vec<_> = radio.log.iter().filter(|r| r.dropped).collect();
    assert!(!dropped.is_empty(), "no `!` rows at 25 % loss");
    assert!(dropped.iter().all(|r| r.dir == "!"), "{dropped:#?}");
    assert!(
        dropped
            .iter()
            .any(|r| r.text.contains("dropped by fault injector")),
        "{dropped:#?}"
    );
    let during_loss = publishes(&sim).len() - base;
    assert!(
        during_loss >= 3,
        "TCP retransmits through 25 % loss: {during_loss} publishes in 8 s"
    );

    // drop the AP 1 s from now for 4 s
    let now = sim.time().as_secs_f64();
    sim.send(Command::Radio(RadioCommand::SetConditions(NetConditions {
        drop_ap_at_s: Some(now + 1.0),
        ap_down_s: 4.0,
        ..Default::default()
    })));
    sim.run_secs(2.5);
    let card_status = |sim: &Sim| {
        sim.tel()
            .radio
            .as_ref()
            .unwrap()
            .cards
            .iter()
            .find(|c| c.label == "U2")
            .unwrap()
            .status
            .clone()
    };
    assert_ne!(card_status(&sim), "connected", "AP is down");
    let n_down = publishes(&sim).len();
    sim.run_secs(2.0);
    assert_eq!(
        publishes(&sim).len(),
        n_down,
        "nothing published while the AP is down"
    );
    // AP back at now+5; the module rejoins, the sketch re-opens the broker link (≤ 5 s retry)
    sim.run_secs(15.0);
    assert_eq!(card_status(&sim), "connected");
    let after: Vec<f64> = publishes(&sim)
        .iter()
        .map(|p| p.time.as_secs_f64())
        .filter(|t| *t > now + 5.0)
        .collect();
    assert!(
        after.len() >= 4,
        "publishes resumed after the AP came back: {after:?}"
    );
    let net: Vec<String> = sim
        .lines_of("U1", LineKind::Net)
        .into_iter()
        .map(|l| l.text.clone())
        .collect();
    assert!(
        net.iter().filter(|l| l.contains("mqtt: CONNECT")).count() >= 2
            || net
                .iter()
                .filter(|l| l.starts_with("wifi:") && l.contains("ip="))
                .count()
                >= 2,
        "reconnect annotated in the serial monitor: {net:#?}"
    );
    sim.shutdown();
}
