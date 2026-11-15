//! LoRa medium, RYLR896 AT modem, virtual gateway → MQTT.

use bb_core::SimTime;
use bb_radio::geo::Pos;
use bb_radio::log::{Dir, DropReason};
use bb_radio::lora::{LoraParams, LoraSpace, Region, Terrain};
use bb_radio::mqtt::Message;
use bb_radio::{DeviceId, RadioSpace};

fn ms(x: u64) -> SimTime {
    SimTime::from_ms(x)
}

#[test]
fn path_loss_by_terrain_and_range() {
    let l = LoraSpace::new(1);
    let a = Pos::new(0.0, 0.0);
    // 915 MHz free-space reference at 1 m ≈ 31.7 dB; 14 dBm at 1 km rural (n = 2.5) ≈ -92.7 dBm.
    let mut l = l;
    l.terrain = Terrain::Rural;
    let r = l.rssi_between(a, Pos::new(1000.0, 0.0), 14.0, 915_000_000);
    assert!((r + 92.7).abs() < 0.2, "{r}");
    l.terrain = Terrain::Urban;
    let u = l.rssi_between(a, Pos::new(1000.0, 0.0), 14.0, 915_000_000);
    assert!(u < r - 14.0);
    // SF12 reaches further than SF7.
    let sf7 = LoraParams {
        sf: 7,
        ..LoraParams::default()
    };
    let sf12 = LoraParams::default();
    l.terrain = Terrain::Suburban;
    let at = |d: f64| l.rssi_between(a, Pos::new(d, 0.0), 14.0, 915_000_000);
    let reach = |p: LoraParams| {
        (1..2000)
            .map(|k| k as f64 * 10.0)
            .take_while(|d| at(*d) >= p.sensitivity())
            .last()
            .unwrap_or(0.0)
    };
    assert!(
        reach(sf12) > 1.8 * reach(sf7),
        "{} vs {}",
        reach(sf12),
        reach(sf7)
    );
}

fn two_nodes(sf: u8) -> (LoraSpace, bb_radio::lora::NodeId, bb_radio::lora::NodeId) {
    let mut l = LoraSpace::new(7);
    let a = l.add_node("A", Pos::new(0.0, 0.0));
    let b = l.add_node("B", Pos::new(300.0, 0.0));
    let p = LoraParams {
        sf,
        ..LoraParams::default()
    };
    for (n, addr) in [(a, 1u16), (b, 2u16)] {
        l.set_powered(n, true);
        l.configure(n, 915_000_000, p, 14.0, 6, addr);
    }
    (l, a, b)
}

#[test]
fn airtime_delivery_duty_cycle_and_collisions() {
    let (mut l, a, b) = two_nodes(9);
    let end = l.transmit(a, 2, b"hello", ms(0)).unwrap();
    let air = LoraParams {
        sf: 9,
        ..LoraParams::default()
    }
    .airtime(5);
    assert_eq!(end, SimTime::from_secs_f64(air));
    // Busy while transmitting; delivered at the end of the frame.
    assert!(l.transmit(a, 2, b"x", ms(10)).is_err());
    l.advance(end - SimTime::from_us(1));
    assert!(l.take_rx(b).is_empty());
    l.advance(end);
    let rx = l.take_rx(b);
    assert_eq!(rx.len(), 1);
    assert_eq!((rx[0].from, rx[0].data.as_slice()), (1, &b"hello"[..]));
    assert!(rx[0].rssi < -60.0 && rx[0].snr > 0.0, "{rx:?}");
    // Wrong network id / address: not received.
    l.configure(
        b,
        915_000_000,
        LoraParams {
            sf: 9,
            ..LoraParams::default()
        },
        14.0,
        7,
        2,
    );
    let e = l.transmit(a, 2, b"x", ms(1000)).unwrap();
    l.advance(e);
    assert!(l.take_rx(b).is_empty());
    // EU868 1 %: after a frame of T s, silent for 99·T.
    let (mut l, a, _) = two_nodes(12);
    l.region = Region::Eu868 { duty_pct: 1.0 };
    let e = l.transmit(a, 2, b"0123456789", ms(0)).unwrap();
    let t = e.as_secs_f64();
    // SF12/125 kHz, preamble 4 (RYLR896 default), 10 bytes: 860.16 ms.
    assert!((t - 0.860_16).abs() < 1e-6, "{t}");
    let err = l.transmit(a, 2, b"again", e + ms(1000)).unwrap_err();
    assert!(
        matches!(err, bb_radio::lora::LoraError::DutyCycle(w) if (w - (100.0 * t - t - 1.0)).abs() < 0.5),
        "{err:?}"
    );
    assert!(
        l.transmit(a, 2, b"again", SimTime::from_secs_f64(100.0 * t + 0.01))
            .is_ok()
    );

    // Collision: two equally strong senders overlapping at the receiver → both lost.
    let mut l = LoraSpace::new(9);
    let p = LoraParams {
        sf: 7,
        ..LoraParams::default()
    };
    let r = l.add_node("R", Pos::new(0.0, 0.0));
    let s1 = l.add_node("S1", Pos::new(200.0, 0.0));
    let s2 = l.add_node("S2", Pos::new(-200.0, 0.0));
    let s3 = l.add_node("S3", Pos::new(0.0, 30.0));
    for (n, addr) in [(r, 1), (s1, 2), (s2, 3), (s3, 4)] {
        l.set_powered(n, true);
        l.configure(n, 915_000_000, p, 14.0, 0, addr);
    }
    l.gateway.enabled = false;
    l.transmit(s1, 1, b"one", ms(0)).unwrap();
    l.transmit(s2, 1, b"two", ms(5)).unwrap();
    l.advance(ms(200));
    assert!(l.take_rx(r).is_empty());
    let col = l
        .log
        .entries()
        .filter(|e| e.dir == Dir::Drop && e.dropped == Some(DropReason::Collision))
        .count();
    assert_eq!(col, 2);
    // Capture effect: a much stronger (closer) sender survives the overlap.
    l.transmit(s1, 1, b"weak", ms(1000)).unwrap();
    l.transmit(s3, 1, b"strong", ms(1005)).unwrap();
    l.advance(ms(1300));
    let rx = l.take_rx(r);
    assert_eq!(rx.len(), 1);
    assert_eq!(rx[0].data, b"strong");
    // Different SF: quasi-orthogonal, both survive.
    l.configure(s2, 915_000_000, LoraParams { sf: 9, ..p }, 14.0, 0, 3);
    l.configure(r, 915_000_000, p, 14.0, 0, 1);
    l.transmit(s1, 1, b"sf7", ms(2000)).unwrap();
    l.transmit(s2, 1, b"sf9", ms(2002)).unwrap();
    l.advance(ms(2500));
    assert_eq!(l.take_rx(r).len(), 1); // R listens on SF7 only, no collision from SF9
}

struct Rig {
    space: RadioSpace,
    t: SimTime,
}

impl Rig {
    fn send(&mut self, dev: DeviceId, s: &str) {
        let cfg = self.space.uart_config(dev).unwrap();
        for b in format!("{s}\r\n").bytes() {
            self.t += cfg.frame_time();
            self.space.uart_rx(dev, b, self.t);
        }
    }
    fn run(&mut self, d: SimTime) {
        let end = self.t + d;
        while self.t < end {
            self.t = (self.t + ms(2)).min(end);
            self.space.advance(self.t);
        }
    }
    fn text(&mut self, dev: DeviceId) -> String {
        self.space
            .take_uart_tx(dev)
            .iter()
            .map(|f| f.data as u8 as char)
            .collect()
    }
}

#[test]
fn rylr896_at_commands_peer_to_peer_and_gateway_mqtt() {
    let mut space = RadioSpace::default();
    let a = space.add_rylr896("U7", Pos::new(0.0, 0.0));
    let b = space.add_rylr896("U8", Pos::new(800.0, 0.0));
    space.lora.gateway.pos = Pos::new(400.0, 100.0);
    space.power(a, true, SimTime::ZERO);
    space.power(b, true, SimTime::ZERO);
    let mut r = Rig {
        space,
        t: SimTime::ZERO,
    };
    r.run(ms(200));
    assert_eq!(r.text(a), "+READY\r\n");
    r.text(b);
    for (dev, addr) in [(a, 101), (b, 102)] {
        r.send(dev, &format!("AT+ADDRESS={addr}"));
        r.send(dev, "AT+NETWORKID=6");
        r.send(dev, "AT+PARAMETER=9,7,1,8");
        r.run(ms(20));
        assert_eq!(r.text(dev), "+OK\r\n+OK\r\n+OK\r\n");
    }
    r.send(a, "AT+PARAMETER?");
    r.send(a, "AT+BAND?");
    r.send(a, "AT+VER?");
    r.send(a, "AT+FOO");
    r.send(a, "AT+SEND=102,4,hello");
    r.run(ms(20));
    assert_eq!(
        r.text(a),
        "+PARAMETER=9,7,1,8\r\n+BAND=915000000\r\n+VER=RYLR89C_V1.2.7\r\n+ERR=4\r\n+ERR=5\r\n"
    );
    // Subscribe to the gateway's uplinks.
    let sub = r.space.net.broker_mut().virtual_connect("lns", r.t);
    let out = r
        .space
        .net
        .broker_mut()
        .virtual_subscribe(sub, "lora/#", 0, r.t);
    r.space.net.apply_broker(out);
    r.send(a, "AT+SEND=102,10,temp=21,5C");
    let sent = r.t;
    r.run(ms(400));
    let air = LoraParams {
        sf: 9,
        preamble: 8,
        ..LoraParams::default()
    }
    .airtime(10);
    assert_eq!(r.text(a), "+OK\r\n");
    let rcv = r.text(b);
    assert!(rcv.starts_with("+RCV=101,10,temp=21,5C,-"), "{rcv}");
    // The gateway forwarded it to MQTT.
    let up = r.space.net.broker_mut().virtual_take(sub);
    assert_eq!(up.len(), 1);
    assert_eq!(up[0].message.topic, "lora/6/101/up");
    let j: serde_json::Value = serde_json::from_str(&up[0].message.payload_str()).unwrap();
    assert_eq!(j["data"], "temp=21,5C");
    assert_eq!(j["sf"], 9);
    assert!(j["rssi"].as_f64().unwrap() < -50.0);
    assert!((up[0].time - sent).as_secs_f64() >= air - 1e-6);
    // Downlink: publish to lora/6/102/down → node 102 receives it from the gateway (address 0).
    let t = r.t;
    let out = r.space.net.broker_mut().virtual_publish(
        sub,
        Message::new("lora/6/102/down", "water=on", 0, false),
        t,
    );
    r.space.net.apply_broker(out);
    r.run(ms(400));
    let rcv = r.text(b);
    assert!(rcv.starts_with("+RCV=0,8,water=on,"), "{rcv}");
    let notes: Vec<String> = r
        .space
        .take_annotations()
        .iter()
        .map(|n| n.line())
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("lora: TX to 102 \"temp=21,5C\" (SF9,")),
        "{notes:#?}"
    );
    // Card for the radio panel.
    let card = r
        .space
        .cards()
        .into_iter()
        .find(|c| c.title == "RYLR896 LoRa")
        .unwrap();
    assert!(
        card.rows
            .iter()
            .any(|(k, v)| k == "Modulation" && v == "SF9 · 125 kHz · 4/5"),
        "{card:?}"
    );
}
