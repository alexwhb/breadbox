//! LAN bridge against a real TCP MQTT broker running in the test (bb-radio's broker behind a
//! threaded TCP server on 127.0.0.1), with a "real dashboard" client on the LAN side.

use bb_core::SimTime;
use bb_radio::bridge::{BridgeConfig, BridgeStatus, LanBridge};
use bb_radio::mqtt::{self, Broker, BrokerOut, Connect, Message, Packet, Publish, StreamDecoder};
use bb_radio::net::Network;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Shared = Arc<Mutex<(Broker, HashMap<u64, TcpStream>)>>;

/// A real MQTT broker on a loopback port. Returns the port and a handle to drop clients.
fn lan_broker() -> (u16, Shared) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let shared: Shared = Arc::new(Mutex::new((Broker::default(), HashMap::new())));
    let s2 = shared.clone();
    std::thread::spawn(move || {
        let mut next = 1u64;
        for conn in l.incoming() {
            let Ok(mut conn) = conn else { break };
            let key = next;
            next += 1;
            {
                let mut g = s2.lock().unwrap();
                g.1.insert(key, conn.try_clone().unwrap());
                g.0.open(key, SimTime::ZERO);
            }
            let s3 = s2.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    let n = match conn.read(&mut buf) {
                        Ok(0) | Err(_) => {
                            let mut g = s3.lock().unwrap();
                            let out = g.0.closed(key, SimTime::ZERO);
                            deliver(&mut g, out);
                            g.1.remove(&key);
                            return;
                        }
                        Ok(n) => n,
                    };
                    let mut g = s3.lock().unwrap();
                    let out = g.0.input(key, &buf[..n], SimTime::ZERO);
                    deliver(&mut g, out);
                }
            });
        }
    });
    (port, shared)
}

fn deliver(g: &mut (Broker, HashMap<u64, TcpStream>), out: Vec<BrokerOut>) {
    for o in out {
        match o {
            BrokerOut::Send(k, b) => {
                if let Some(s) = g.1.get_mut(&k) {
                    let _ = s.write_all(&b);
                }
            }
            BrokerOut::Close(k) => {
                if let Some(s) = g.1.remove(&k) {
                    let _ = s.shutdown(Shutdown::Both);
                }
            }
        }
    }
}

/// A blocking MQTT client for the LAN side.
struct Dashboard {
    s: TcpStream,
    dec: StreamDecoder,
}

impl Dashboard {
    fn connect(port: u16, id: &str, filter: &str) -> Dashboard {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        s.write_all(&mqtt::encode(&Packet::Connect(Connect::new(id, 0)))).unwrap();
        s.write_all(&mqtt::encode(&Packet::Subscribe { pid: 1, topics: vec![(filter.into(), 0)] })).unwrap();
        let mut d = Dashboard { s, dec: StreamDecoder::default() };
        d.wait(|p| matches!(p, Packet::SubAck { .. }));
        d
    }
    fn publish(&mut self, topic: &str, payload: &str) {
        let p = Publish { dup: false, qos: 0, retain: false, topic: topic.into(), pid: None, payload: payload.as_bytes().to_vec() };
        self.s.write_all(&mqtt::encode(&Packet::Publish(p))).unwrap();
    }
    fn poll(&mut self) -> Vec<Packet> {
        let mut buf = [0u8; 4096];
        if let Ok(n) = self.s.read(&mut buf) {
            self.dec.push(&buf[..n]);
        }
        let mut v = vec![];
        while let Ok(Some(p)) = self.dec.next_packet() {
            v.push(p);
        }
        v
    }
    fn wait(&mut self, f: impl Fn(&Packet) -> bool) -> Packet {
        let t0 = Instant::now();
        loop {
            for p in self.poll() {
                if f(&p) {
                    return p;
                }
            }
            assert!(t0.elapsed() < Duration::from_secs(5), "timeout");
        }
    }
}

/// Steps simulated time while real time passes (the sim must never wait for the bridge).
fn pump_until(net: &mut Network, br: &mut LanBridge, t: &mut SimTime, mut done: impl FnMut(&mut Network, &mut LanBridge) -> bool) {
    let t0 = Instant::now();
    while !done(net, br) {
        let s = Instant::now();
        br.pump(net, *t);
        assert!(s.elapsed() < Duration::from_millis(50), "pump blocked for {:?}", s.elapsed());
        *t = *t + SimTime::from_ms(1);
        net.advance(*t);
        std::thread::sleep(Duration::from_millis(1));
        assert!(t0.elapsed() < Duration::from_secs(10), "timed out; bridge {:?}", br.status());
    }
}

#[test]
fn bridge_mirrors_both_ways_without_loops() {
    let (port, _shared) = lan_broker();
    let mut dash = Dashboard::connect(port, "dashboard", "garden/#");
    let mut net = Network::default();
    let mut t = SimTime::ZERO;
    let mut br = LanBridge::start(BridgeConfig { topics: vec!["garden/#".into()], ..BridgeConfig::new("127.0.0.1", port) });
    pump_until(&mut net, &mut br, &mut t, |_, b| *b.status() == BridgeStatus::Connected);
    // A simulated device (virtual client stands in for the firmware) publishes in the sim…
    let dev = net.broker_mut().virtual_connect("garden-node", t);
    let out = net.broker_mut().virtual_subscribe(dev, "garden/cmd/#", 0, t);
    net.apply_broker(out);
    let out = net.broker_mut().virtual_publish(dev, Message::new("garden/pot1", r#"{"m":42}"#, 0, false), t);
    net.apply_broker(out);
    pump_until(&mut net, &mut br, &mut t, |_, b| b.stats.to_lan >= 1);
    // …and the real dashboard sees it.
    let p = dash.wait(|p| matches!(p, Packet::Publish(_)));
    let Packet::Publish(p) = p else { unreachable!() };
    assert_eq!((p.topic.as_str(), p.payload.as_slice()), ("garden/pot1", &br#"{"m":42}"#[..]));
    // The dashboard commands the device.
    dash.publish("garden/cmd/water", "on");
    let mut got = vec![];
    pump_until(&mut net, &mut br, &mut t, |n, _| {
        got.extend(n.broker_mut().virtual_take(dev));
        !got.is_empty()
    });
    assert_eq!(got[0].message, Message::new("garden/cmd/water", "on", 0, false));
    // No echo loops: let things settle and check counts stayed at one each way.
    for _ in 0..200 {
        br.pump(&mut net, t);
        t = t + SimTime::from_ms(1);
        net.advance(t);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(br.stats.to_lan, 1, "{:?}", br.stats);
    assert_eq!(br.stats.from_lan, 1, "{:?}", br.stats);
    // The dashboard hears its own command once (it subscribed to garden/#; MQTT 3.1.1 has no
    // no-local) — but nothing comes back through the bridge.
    let extra: Vec<String> = dash.poll().into_iter().filter_map(|p| match p {
        Packet::Publish(p) => Some(p.topic),
        _ => None,
    }).collect();
    assert!(extra.len() <= 1 && extra.iter().all(|t| t == "garden/cmd/water"), "loop: {extra:?}");
    assert!(net.broker_mut().virtual_take(dev).is_empty());
    br.stop(&mut net, t);
}

#[test]
fn bridge_reconnects_and_never_blocks_the_sim() {
    // Nothing listening yet: the bridge keeps retrying in the background.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let mut net = Network::default();
    let mut t = SimTime::ZERO;
    let mut br = LanBridge::start(BridgeConfig::new("127.0.0.1", port));
    let start = Instant::now();
    for _ in 0..100 {
        let s = Instant::now();
        br.pump(&mut net, t);
        assert!(s.elapsed() < Duration::from_millis(20));
        t = t + SimTime::from_ms(1);
    }
    assert!(start.elapsed() < Duration::from_secs(2));
    std::thread::sleep(Duration::from_millis(300));
    br.pump(&mut net, t);
    assert!(matches!(br.status(), BridgeStatus::Disconnected(_) | BridgeStatus::Connecting), "{:?}", br.status());
    // Now start a broker on that port: the bridge connects on its own.
    let listener = TcpListener::bind(("127.0.0.1", port));
    let Ok(listener) = listener else { return }; // port reused by someone else: nothing to test
    drop(listener);
    let (port2, _s) = lan_broker();
    let mut br2 = LanBridge::start(BridgeConfig::new("127.0.0.1", port2));
    pump_until(&mut net, &mut br2, &mut t, |_, b| *b.status() == BridgeStatus::Connected);
    br2.stop(&mut net, t);
    br.stop(&mut net, t);
}
