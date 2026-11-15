//! Radio glue on the full bench (RF-*, PRJ-3, TEST-4): the ESP-01 / HM-10 parts run the real
//! `bb-radio` modems through their UART pins, the radio space comes from the project's
//! settings, annotations land in the right serial monitor, and the Radios panel + virtual
//! phone are driven through `Command::Radio` / telemetry.

mod common;
use bb_circuit::Project;
use bb_core::SimTime;
use bb_radio::net::NetConditions;
use bb_sim::radio::{PhoneCommand, ProjectRadios, RadioCommand};
use bb_sim::{Bench, BenchOptions, LineKind};
use common::*;

fn example(name: &str) -> (Project, BenchOptions) {
    let dir = root().join("examples").join(name);
    let project = Project::load(&dir).unwrap();
    let opts = BenchOptions {
        firmware: bb_sim::firmware::project_images(&project, &dir),
        ..Default::default()
    };
    (project, opts)
}

fn garden() -> Bench {
    let (p, o) = example("garden-node");
    Bench::new(p, lib(), o).unwrap()
}

fn subscribe(b: &mut Bench, filter: &str) {
    b.radio_command(RadioCommand::ExplorerSubscribe(filter.into()))
        .unwrap();
}

#[test]
fn garden_node_publishes_to_the_builtin_broker_through_the_real_modem() {
    let mut b = garden();
    subscribe(&mut b, "garden/#");
    b.run_until(SimTime::from_secs(14)).unwrap();
    let tel = b.telemetry();
    let radio = tel.radio.as_ref().expect("radio telemetry");
    // MQTT explorer: JSON once per second tracking the sliders' values
    let msgs: Vec<_> = radio
        .mqtt
        .iter()
        .filter(|m| m.topic == "garden/pot1")
        .collect();
    assert!(
        msgs.len() >= 9,
        "{} publishes: {:?}",
        msgs.len(),
        radio.mqtt
    );
    assert!(
        msgs[0].time < SimTime::from_secs(5),
        "first at {}",
        msgs[0].time
    );
    let span = (msgs.last().unwrap().time - msgs[0].time).as_secs_f64();
    let rate = (msgs.len() - 1) as f64 / span;
    assert!((rate - 1.0).abs() < 0.12, "rate {rate}");
    for m in &msgs {
        let v: serde_json::Value = serde_json::from_str(&m.payload).unwrap();
        assert!(
            (40..=43).contains(&v["m"].as_i64().unwrap()),
            "{}",
            m.payload
        );
        assert!(
            (150..=210).contains(&v["lux"].as_i64().unwrap()),
            "{}",
            m.payload
        );
    }
    // the card the design shows
    let card = radio.cards.iter().find(|c| c.label == "U2").unwrap();
    assert_eq!(card.title, "ESP8266 Wi-Fi");
    assert_eq!(card.status, "connected");
    let row = |k: &str| {
        card.rows
            .iter()
            .find(|r| r.0 == k)
            .map(|r| r.1.clone())
            .unwrap_or_default()
    };
    assert_eq!(row("Access point"), "SIM-AP · ch 6");
    assert_eq!(row("RSSI"), "-54 dBm (3.2 m, 1 wall)");
    assert_eq!(row("IP"), "10.0.0.42 / DHCP");
    assert_eq!(row("Broker"), "mqtt://10.0.0.1 (built-in)");
    // packet log rows of the design
    let rows: Vec<(String, String)> = radio
        .log
        .iter()
        .map(|r| (r.dir.clone(), r.text.clone()))
        .collect();
    for want in [
        ("tx", "DHCP REQUEST 10.0.0.42"),
        ("rx", "DHCP ACK · lease 3600 s"),
        ("tx", "MQTT CONNECT client=garden-node"),
    ] {
        assert!(
            rows.contains(&(want.0.into(), want.1.into())),
            "{want:?} not in {rows:#?}"
        );
    }
    assert!(
        rows.iter().any(|r| r.0 == "tx"
            && r.1.starts_with("PUBLISH garden/pot1 · ")
            && r.1.ends_with(" B · QoS 0")),
        "{rows:#?}"
    );
    // annotations reach U1's serial monitor (the MCU wired to U2) as cyan lines
    let mon = b.serial("u1").unwrap();
    let net: Vec<&str> = mon
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Net)
        .map(|l| l.text.as_str())
        .collect();
    assert!(
        net.contains(&"wifi: AT+CWJAP=\"SIM-AP\" → OK rssi=-54 dBm ip=10.0.0.42"),
        "{net:#?}"
    );
    assert!(
        net.iter()
            .any(|l| l.starts_with("mqtt: PUB garden/pot1 {\"m\":")),
        "{net:#?}"
    );
    // the firmware still prints its telemetry
    assert!(lines(&b, "u1").iter().any(|l| l.starts_with("soil_raw=")));
}

#[test]
fn soil_slider_reaches_the_broker() {
    let mut b = garden();
    subscribe(&mut b, "garden/pot1");
    b.run_until(SimTime::from_secs(6)).unwrap();
    b.set_world("pot1.vwc", 20.0);
    b.run_until(SimTime::from_secs(16)).unwrap();
    let tel = b.telemetry();
    let msgs = &tel.radio.as_ref().unwrap().mqtt;
    let m = |s: &str| -> i64 {
        serde_json::from_str::<serde_json::Value>(s).unwrap()["m"]
            .as_i64()
            .unwrap()
    };
    let early: Vec<i64> = msgs
        .iter()
        .filter(|x| x.time < SimTime::from_secs(6))
        .map(|x| m(&x.payload))
        .collect();
    let late: Vec<i64> = msgs
        .iter()
        .filter(|x| x.time > SimTime::from_secs(13))
        .map(|x| m(&x.payload))
        .collect();
    assert!(early.iter().all(|v| (40..=43).contains(v)), "{early:?}");
    assert!(!late.is_empty() && late.iter().all(|v| *v < 25), "{late:?}");
}

#[test]
fn fault_injector_drops_and_ap_outage_reconnects() {
    let (mut p, o) = example("garden-node");
    let mut radios = ProjectRadios::default();
    radios.settings.conditions = NetConditions {
        latency_ms: 40.0,
        jitter_ms: 10.0,
        loss_pct: 2.0,
        drop_ap_at_s: Some(12.0),
        ap_down_s: 4.0,
    };
    radios.store(&mut p);
    let mut b = Bench::new(p, lib(), o).unwrap();
    subscribe(&mut b, "garden/#");
    b.run_until(SimTime::from_secs(40)).unwrap();
    let tel = b.telemetry();
    let r = tel.radio.as_ref().unwrap();
    assert_eq!(r.conditions.loss_pct, 2.0);
    let before = r
        .mqtt
        .iter()
        .filter(|m| m.time < SimTime::from_secs(12))
        .count();
    let after = r
        .mqtt
        .iter()
        .filter(|m| m.time > SimTime::from_secs(24))
        .count();
    assert!(before >= 7, "{before} before the drop");
    if after < 12 {
        let svc = b.radio().unwrap();
        panic!(
            "{after} publishes after reconnecting; module said: {}",
            svc.uart_received_text("u2")
        );
    }
    let svc = b.radio().unwrap();
    let all: Vec<String> = svc.space.net.log().entries().map(|e| e.text()).collect();
    assert!(
        all.iter()
            .any(|t| t == "dropped by fault injector (loss 2%)"),
        "no drop rows"
    );
    let drop_rows = r.log.iter().filter(|x| x.dropped && x.dir == "!").count()
        + svc
            .space
            .net
            .log()
            .entries()
            .filter(|e| e.dropped.is_some())
            .count();
    assert!(drop_rows > 0);
    let connects = all
        .iter()
        .filter(|t| *t == "MQTT CONNECT client=garden-node")
        .count();
    assert!(connects >= 2, "reconnected {connects}");
    let mon = b.serial("u1").unwrap();
    assert!(
        mon.lines
            .iter()
            .any(|l| l.kind == LineKind::Net && l.text.contains("WIFI DISCONNECT (AP down)")),
        "no disconnect annotation"
    );
}

#[test]
fn conditions_and_explorer_publish_via_commands_and_pcap_export() {
    let mut b = garden();
    b.run_until(SimTime::from_secs(4)).unwrap();
    b.radio_command(RadioCommand::SetConditions(NetConditions {
        latency_ms: 40.0,
        jitter_ms: 10.0,
        loss_pct: 0.0,
        drop_ap_at_s: None,
        ap_down_s: 10.0,
    }))
    .unwrap();
    subscribe(&mut b, "cmd/#");
    b.radio_command(RadioCommand::ExplorerPublish {
        topic: "cmd/led".into(),
        payload: b"on".to_vec(),
        qos: 1,
        retain: false,
    })
    .unwrap();
    b.run_until(SimTime::from_secs(6)).unwrap();
    let tel = b.telemetry();
    let r = tel.radio.unwrap();
    assert_eq!(r.conditions.latency_ms, 40.0);
    assert!(
        r.mqtt
            .iter()
            .any(|m| m.topic == "cmd/led" && m.payload == "on")
    );
    let pcap = b.radio().unwrap().export_pcapng();
    let pk = bb_radio::log::read_pcapng(&pcap).unwrap();
    assert!(pk.len() > 10, "{} packets", pk.len());
    assert!(b.radio().unwrap().export_csv().lines().count() > 10);
}

#[test]
fn ble_button_notifies_the_virtual_phone() {
    let (p, o) = example("ble-button");
    let mut b = Bench::new(p, lib(), o).unwrap();
    b.run_until(SimTime::from_secs(2)).unwrap();
    let tel = b.telemetry();
    let r = tel.radio.as_ref().unwrap();
    assert!(r.has_ble);
    let dev = r.phone.scan.iter().find(|s| s.kind == "BLE").expect("scan");
    assert!(dev.rssi < -30 && dev.rssi > -90, "rssi {}", dev.rssi);
    let id = dev.id;
    b.radio_command(RadioCommand::Phone(PhoneCommand::Connect { id, pin: None }))
        .unwrap();
    b.run_until(SimTime::from_ms(2500)).unwrap();
    let tel = b.telemetry();
    let r = tel.radio.as_ref().unwrap();
    assert_eq!(r.phone.connected, Some(id));
    let ch = r
        .phone
        .services
        .iter()
        .flat_map(|s| s.characteristics.iter())
        .find(|c| c.props.iter().any(|p| p == "notify"))
        .expect("notify characteristic")
        .handle;
    b.radio_command(RadioCommand::Phone(PhoneCommand::Subscribe {
        handle: ch,
        on: true,
    }))
    .unwrap();
    b.run_until(SimTime::from_secs(3)).unwrap();
    // press the button: the sketch sends over the HM-10, the phone gets a notification
    let button = b
        .project()
        .parts
        .iter()
        .find(|p| p.part == "push_button")
        .map(|p| p.id.clone())
        .expect("button part");
    b.input(&button, &bb_core::UserInput::Press(true)).unwrap();
    b.run_until(SimTime::from_ms(3300)).unwrap();
    b.input(&button, &bb_core::UserInput::Press(false)).unwrap();
    b.run_until(SimTime::from_secs(5)).unwrap();
    let tel = b.telemetry();
    let r = tel.radio.as_ref().unwrap();
    let notes: Vec<&str> = r
        .phone
        .log
        .iter()
        .filter(|l| l.kind == "notify")
        .map(|l| l.text.as_str())
        .collect();
    assert!(!notes.is_empty(), "phone log: {:#?}", r.phone.log);
    // the HM-10 STATE pin is high while connected
    let readings = b.readings(
        &b.project()
            .parts
            .iter()
            .find(|p| p.part == "hm10")
            .unwrap()
            .id
            .clone(),
    );
    assert!(
        readings.iter().any(|(k, v)| k == "link" && *v == 1.0),
        "{readings:?}"
    );
}
