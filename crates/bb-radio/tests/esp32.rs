//! `MCU-6` × `RF-*`: ESP32 sketches compiled for bb-firmware's host core run in lock-step with
//! simulated time, their Wi-Fi / sockets / BLE served by `RadioSpace::host_service` — the glue
//! bb-sim will use (a `HostIo` whose `service` forwards to the radio space).

use bb_core::SimTime;
use bb_firmware::host::{HostBoard, HostConfig, HostEvent, HostIo, HostKind, HostPinState, HostStop, compile_host, cxx};
use bb_radio::ble::PhoneEvent;
use bb_radio::geo::Pos;
use bb_radio::mqtt::Message;
use bb_radio::{DeviceId, RadioSpace};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The co-sim glue: radio service requests go to the radio space.
struct Glue<'a> {
    space: &'a mut RadioSpace,
    dev: DeviceId,
}

impl HostIo for Glue<'_> {
    fn service(&mut self, channel: u8, request: &[u8], at: SimTime) -> Vec<u8> {
        self.space.host_service(self.dev, channel, request, at)
    }
}

/// DHT22 on a pin: answers the host's start signal with the real 40-bit waveform.
struct Dht22 {
    pin: u8,
    low_since: Option<SimTime>,
    temp_c: f64,
    humidity: f64,
    reads: u32,
}

impl Dht22 {
    fn frame(&self) -> [u8; 5] {
        let h = (self.humidity * 10.0).round() as u16;
        let t = (self.temp_c.abs() * 10.0).round() as u16 | if self.temp_c < 0.0 { 0x8000 } else { 0 };
        let b = [(h >> 8) as u8, h as u8, (t >> 8) as u8, t as u8];
        [b[0], b[1], b[2], b[3], b.iter().fold(0u8, |a, x| a.wrapping_add(*x))]
    }

    fn on_event(&mut self, e: &HostEvent, board: &mut HostBoard) {
        let HostEvent::Pin { time, pin, state } = e else { return };
        if *pin != self.pin {
            return;
        }
        match state {
            HostPinState::Output { high: false } => self.low_since = Some(*time),
            HostPinState::Input { .. } => {
                let Some(t0) = self.low_since.take() else { return };
                if *time - t0 < SimTime::from_us(800) {
                    return;
                }
                self.reads += 1;
                let us = |x: u64| SimTime::from_us(x);
                let mut t = *time + us(30);
                let mut put = |at: SimTime, level: Option<bool>| board.schedule_pin_input(self.pin, level, at);
                put(t, Some(false));
                t += us(80);
                put(t, Some(true));
                t += us(80);
                for byte in self.frame() {
                    for bit in (0..8).rev() {
                        put(t, Some(false));
                        t += us(50);
                        put(t, Some(true));
                        t += us(if byte >> bit & 1 == 1 { 70 } else { 26 });
                    }
                }
                put(t, Some(false));
                put(t + us(50), None);
            }
            _ => {}
        }
    }
}

fn build(dir: &Path) -> Option<PathBuf> {
    if cxx().is_none() {
        eprintln!("no C++ compiler; skipping");
        return None;
    }
    let tmp = std::env::temp_dir().join(format!("bb-radio-host-{}-{}", dir.file_name().unwrap().to_string_lossy(), std::process::id()));
    let b = compile_host(dir, HostKind::Esp32DevKit, Some(&tmp)).unwrap();
    assert!(b.success, "{}", b.log);
    b.exe
}

struct Rig {
    space: RadioSpace,
    dev: DeviceId,
    board: HostBoard,
    dht: Option<Dht22>,
    lines: Vec<(SimTime, String)>,
    partial: String,
    events: Vec<HostEvent>,
}

impl Rig {
    fn run_until(&mut self, end: SimTime) {
        let mut t = self.board.time();
        while t < end {
            t = (t + SimTime::from_ms(1)).min(end);
            loop {
                let stop = self.board.run(t, &mut Glue { space: &mut self.space, dev: self.dev });
                for e in self.board.take_events() {
                    if let Some(d) = self.dht.as_mut() {
                        d.on_event(&e, &mut self.board);
                    }
                    self.events.push(e);
                }
                match stop {
                    HostStop::ReachedTime => break,
                    HostStop::Output => continue,
                    other => panic!("{other:?}\n{}", self.board.stderr()),
                }
            }
            self.space.advance(self.board.time());
            for (at, c) in self.board.take_serial(0) {
                if c == b'\n' {
                    self.lines.push((at, std::mem::take(&mut self.partial).trim_end_matches('\r').to_string()));
                } else {
                    self.partial.push(c as char);
                }
            }
        }
    }

    fn has_line(&self, s: &str) -> bool {
        self.lines.iter().any(|l| l.1 == s)
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn esp32_weather_joins_wifi_and_publishes_mqtt_in_sim_time() {
    let Some(exe) = build(&root().join("examples/esp32-weather/firmware/U1/esp32-weather")) else { return };
    let mut space = RadioSpace::default();
    let dev = space.add_host_radio("U1", Pos::new(2.0, 1.0));
    let sub = space.net.broker_mut().virtual_connect("dashboard", SimTime::ZERO);
    let out = space.net.broker_mut().virtual_subscribe(sub, "weather/#", 0, SimTime::ZERO);
    space.net.apply_broker(out);
    let mut cfg = HostConfig::new(HostKind::Esp32DevKit);
    cfg.stop_on_output = true;
    let board = HostBoard::spawn(&exe, cfg).unwrap();
    let dht = Dht22 { pin: 4, low_since: None, temp_c: 21.7, humidity: 48.3, reads: 0 };
    let mut rig = Rig { space, dev, board, dht: Some(dht), lines: vec![], partial: String::new(), events: vec![] };
    let wall = Instant::now();
    rig.run_until(SimTime::from_secs(12));
    let wall = wall.elapsed().as_secs_f64();

    assert!(rig.has_line("Wi-Fi: joining SIM-AP"), "{:#?}", rig.lines);
    let joined = rig.lines.iter().find(|l| l.1.starts_with("Wi-Fi: connected, IP 10.0.0.42, RSSI -")).expect("joined");
    assert!(joined.0 > SimTime::from_ms(600) && joined.0 < SimTime::from_secs(2), "{:?}", joined.0);
    assert!(rig.has_line("MQTT: connected"));
    assert!(rig.has_line("t=21.7 C h=48.3 % -> weather/esp32"), "{:#?}", rig.lines);
    assert!(rig.dht.as_ref().unwrap().reads >= 4);

    // The dashboard got the retained status + a publish every 2 s of *simulated* time.
    let msgs = rig.space.net.broker_mut().virtual_take(sub);
    assert_eq!(msgs[0].message, Message::new("weather/status", "online", 0, false));
    let data: Vec<_> = msgs.iter().filter(|m| m.message.topic == "weather/esp32").collect();
    assert!(data.len() >= 5, "{msgs:#?}");
    assert!(data.iter().all(|m| m.message.payload_str() == r#"{"t":21.7,"h":48.3}"#));
    for w in data.windows(2) {
        let dt = (w[1].time - w[0].time).as_secs_f64();
        assert!((dt - 2.0).abs() < 0.06, "publish interval {dt}");
    }
    eprintln!("12 s of ESP32 sim time took {wall:.2} s of wall time");

    // Command from the dashboard → LED on GPIO2.
    let t = rig.board.time();
    let out = rig.space.net.broker_mut().virtual_publish(sub, Message::new("weather/cmd", "led on", 0, false), t);
    rig.space.net.apply_broker(out);
    rig.run_until(t + SimTime::from_secs(1));
    assert!(rig.has_line("MQTT: weather/cmd = led on"), "{:#?}", rig.lines);
    assert!(rig.events.iter().any(|e| matches!(e, HostEvent::Pin { pin: 2, state: HostPinState::Output { high: true }, .. })));

    // Serial-monitor annotations from the radio side.
    let notes: Vec<String> = rig.space.take_annotations().iter().map(|n| n.line()).collect();
    assert!(notes.iter().any(|n| n.starts_with("wifi: WiFi.begin(\"SIM-AP\") → connected rssi=") && n.ends_with("ip=10.0.0.42")), "{notes:#?}");
    assert!(notes.iter().any(|n| n == "mqtt: CONNECT client=esp32-weather → CONNACK 0"));
    assert!(notes.iter().any(|n| n == r#"mqtt: PUB weather/esp32 {"t":21.7,"h":48.3} (36 B)"#), "{notes:#?}");

    // Power loss: the broker publishes the will after 1.5 × keep-alive (15 s).
    drop(rig.board);
    let t0 = rig.space.now();
    rig.space.advance(t0 + SimTime::from_secs(25));
    let will: Vec<_> = rig.space.net.broker_mut().virtual_take(sub);
    assert!(will.iter().any(|m| m.message.topic == "weather/status" && m.message.payload == b"offline"), "{will:#?}");
}

#[test]
fn esp32_ble_server_reaches_the_virtual_phone_plus_http_udp_dns() {
    let Some(exe) = build(&root().join("crates/bb-radio/tests/sketches/esp32_ble_http")) else { return };
    let mut space = RadioSpace::default();
    let dev = space.add_host_radio("U1", Pos::new(1.0, 0.0));
    space.ble.phone_pos = Pos::new(2.0, 0.5);
    let board = HostBoard::spawn(&exe, HostConfig::new(HostKind::Esp32DevKit)).unwrap();
    let mut rig = Rig { space, dev, board, dht: None, lines: vec![], partial: String::new(), events: vec![] };
    rig.run_until(SimTime::from_secs(3));
    assert!(rig.has_line("ble: advertising"));
    assert!(rig.has_line("wifi: 10.0.0.42"), "{:#?}", rig.lines);
    assert!(rig.has_line("http: 200 args ok"), "{:#?}", rig.lines);
    assert!(rig.has_line("udp: udp-ping from 10.0.0.1"), "{:#?}", rig.lines);
    assert!(rig.has_line("dns: 1 10.0.0.1"));

    // The phone sees the ESP32 next to HM-10s and connects.
    let scan = rig.space.ble.scan();
    let e = scan.iter().find(|s| s.name == "esp32-uart").expect("advertising");
    assert!(e.rssi > -60);
    let id = e.id;
    let t = rig.board.time();
    rig.space.ble.connect(id, None, t).unwrap();
    rig.run_until(t + SimTime::from_ms(200));
    assert!(rig.has_line("ble: connected"), "{:#?}", rig.lines);
    let svc = rig.space.ble.discover().unwrap();
    assert_eq!(svc[0].uuid.to_string(), "6e400001-b5a3-f393-e0a9-e50e24dcca9e");
    let t = rig.board.time();
    rig.space.ble.terminal_open(t).unwrap();
    rig.run_until(t + SimTime::from_secs(2));
    let notes: Vec<String> = rig
        .space
        .ble
        .take_phone_events()
        .into_iter()
        .filter_map(|(_, e)| match e {
            PhoneEvent::Terminal { data, .. } => Some(String::from_utf8_lossy(&data).into_owned()),
            _ => None,
        })
        .collect();
    assert!(notes.len() >= 3, "{notes:?}");
    assert!(notes.iter().all(|n| n.starts_with("count=")));
    // Phone → ESP32 write callback.
    let t = rig.board.time();
    rig.space.ble.terminal_send(b"reset", t).unwrap();
    rig.run_until(t + SimTime::from_ms(300));
    assert!(rig.has_line("ble rx: reset"), "{:#?}", rig.lines);
    let t = rig.board.time();
    rig.space.ble.disconnect(t);
    rig.run_until(t + SimTime::from_ms(100));
    assert!(rig.has_line("ble: disconnected"));
    // It advertises again.
    assert!(rig.space.ble.scan().iter().any(|s| s.name == "esp32-uart"));
}
