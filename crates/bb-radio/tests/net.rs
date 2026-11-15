//! Network stack tests: DHCP lease flow, DNS, NTP, HTTP, ICMP, TCP with latency/jitter/loss and
//! retransmission, MQTT over TCP to the built-in broker, AP-drop fault, packet log + pcap/CSV.

use bb_core::SimTime;
use bb_radio::geo::{Pos, Wall};
use bb_radio::log::{Dir, Proto, read_pcapng};
use bb_radio::mqtt::{self, Message, Packet};
use bb_radio::net::{
    CloseReason, JoinError, NetConditions, NetEvent, Network, SockId, StationId, TcpState,
};
use bb_radio::wire;
use std::net::{Ipv4Addr, SocketAddrV4};

fn ms(x: u64) -> SimTime {
    SimTime::from_ms(x)
}

/// Advances in 1 ms steps, collecting the station's events.
fn run(net: &mut Network, sta: StationId, until: SimTime, out: &mut Vec<(SimTime, NetEvent)>) {
    let mut t = net.now();
    while t < until {
        t = (t + ms(1)).min(until);
        net.advance(t);
        for e in net.take_events(sta) {
            out.push((t, e));
        }
    }
}

fn run_until<F: Fn(&NetEvent) -> bool>(
    net: &mut Network,
    sta: StationId,
    limit: SimTime,
    f: F,
) -> (SimTime, NetEvent) {
    let mut t = net.now();
    while t < limit {
        t += ms(1);
        net.advance(t);
        for e in net.take_events(sta) {
            if f(&e) {
                return (t, e);
            }
        }
    }
    panic!("timed out at {t}");
}

fn joined(net: &mut Network, label: &str, pos: Pos) -> StationId {
    let s = net.add_station(label, pos);
    let now = net.now();
    net.join(s, "SIM-AP", Some("breadbox"), now).unwrap();
    run_until(net, s, now + SimTime::from_secs(10), |e| {
        matches!(e, NetEvent::GotIp { .. })
    });
    s
}

fn gw(port: u16) -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), port)
}

fn design_geometry(net: &mut Network) {
    net.geometry
        .walls
        .push(Wall::interior(Pos::new(1.5, -5.0), Pos::new(1.5, 5.0)));
}

#[test]
fn join_dhcp_lease_flow_and_rssi() {
    let mut net = Network::default();
    design_geometry(&mut net);
    let s = net.add_station("U2", Pos::new(3.2, 0.0));
    net.join(s, "SIM-AP", Some("breadbox"), SimTime::ZERO)
        .unwrap();
    let mut ev = vec![];
    run(&mut net, s, SimTime::from_secs(3), &mut ev);
    let kinds: Vec<&NetEvent> = ev.iter().map(|e| &e.1).collect();
    let NetEvent::WifiConnected {
        ssid,
        channel,
        rssi,
        ..
    } = kinds[0]
    else {
        panic!("{kinds:?}")
    };
    assert_eq!((ssid.as_str(), *channel, *rssi), ("SIM-AP", 6, -54));
    let NetEvent::GotIp {
        ip,
        gateway,
        netmask,
        dns,
    } = kinds[1]
    else {
        panic!("{kinds:?}")
    };
    assert_eq!(*ip, Ipv4Addr::new(10, 0, 0, 42));
    assert_eq!(
        (*gateway, *dns),
        (Ipv4Addr::new(10, 0, 0, 1), Ipv4Addr::new(10, 0, 0, 1))
    );
    assert_eq!(*netmask, Ipv4Addr::new(255, 255, 255, 0));
    // The packet log shows the full DORA exchange in order, tx from the station, rx back.
    let rows: Vec<(Dir, String)> = net
        .log()
        .visible()
        .filter(|e| e.proto == Proto::Dhcp)
        .map(|e| (e.dir, e.summary.clone()))
        .collect();
    assert_eq!(
        rows,
        vec![
            (Dir::Tx, "DHCP DISCOVER".to_string()),
            (Dir::Rx, "DHCP OFFER 10.0.0.42".to_string()),
            (Dir::Tx, "DHCP REQUEST 10.0.0.42".to_string()),
            (Dir::Rx, "DHCP ACK · lease 3600 s".to_string()),
        ]
    );
    let info = net.station_info(s).unwrap();
    assert_eq!(info.link.unwrap().walls, 1);
    // Second station gets .43; the first keeps .42 across a rejoin.
    let s2 = joined(&mut net, "U3", Pos::new(1.0, 1.0));
    assert_eq!(net.ip_of(s2), Some(Ipv4Addr::new(10, 0, 0, 43)));
    let now = net.now();
    net.leave(s, now).unwrap();
    assert_eq!(net.ip_of(s), None);
    net.join(s, "SIM-AP", Some("breadbox"), now).unwrap();
    let (_, e) = run_until(&mut net, s, now + SimTime::from_secs(5), |e| {
        matches!(e, NetEvent::GotIp { .. })
    });
    assert!(matches!(e, NetEvent::GotIp { ip, .. } if ip == Ipv4Addr::new(10, 0, 0, 42)));
}

#[test]
fn lease_renews_at_t1() {
    let mut net = Network::default();
    net.config.lease_secs = 60;
    let s = joined(&mut net, "U2", Pos::new(1.0, 0.0));
    let n0 = net
        .log()
        .visible()
        .filter(|e| e.summary.starts_with("DHCP REQUEST"))
        .count();
    let mut ev = vec![];
    run(&mut net, s, SimTime::from_secs(95), &mut ev);
    // Renewals at ~30 s, ~60 s, ~90 s after the ACK, no new GotIp (same address).
    let renewals = net
        .log()
        .visible()
        .filter(|e| e.summary.starts_with("DHCP REQUEST"))
        .count()
        - n0;
    assert_eq!(
        renewals,
        3,
        "{:?}",
        net.log()
            .visible()
            .map(|e| e.summary.clone())
            .collect::<Vec<_>>()
    );
    assert!(ev.iter().all(|e| !matches!(e.1, NetEvent::GotIp { .. })));
    assert_eq!(net.ip_of(s), Some(Ipv4Addr::new(10, 0, 0, 42)));
}

#[test]
fn join_failures_wrong_password_and_missing_ap() {
    let mut net = Network::default();
    let s = net.add_station("U2", Pos::new(2.0, 0.0));
    net.join(s, "SIM-AP", Some("nope"), SimTime::ZERO).unwrap();
    let (t, e) = run_until(&mut net, s, SimTime::from_secs(5), |_| true);
    assert_eq!(
        e,
        NetEvent::WifiJoinFailed {
            ssid: "SIM-AP".into(),
            reason: JoinError::WrongPassword
        }
    );
    assert!(t > SimTime::from_secs(1));
    let now = net.now();
    net.join(s, "OTHER", Some("x"), now).unwrap();
    let (_, e) = run_until(&mut net, s, now + SimTime::from_secs(5), |_| true);
    assert_eq!(
        e,
        NetEvent::WifiJoinFailed {
            ssid: "OTHER".into(),
            reason: JoinError::NoAp
        }
    );
    // Out of range: 2 km away behind walls.
    let far = net.add_station("U9", Pos::new(2000.0, 0.0));
    let now = net.now();
    assert!(net.scan(far).is_empty());
    net.join(far, "SIM-AP", Some("breadbox"), now).unwrap();
    let (_, e) = run_until(&mut net, far, now + SimTime::from_secs(5), |_| true);
    assert!(matches!(
        e,
        NetEvent::WifiJoinFailed {
            reason: JoinError::NoAp,
            ..
        }
    ));
    let scan = net.scan(s);
    assert_eq!(scan.len(), 1);
    assert!(scan[0].secure);
}

#[test]
fn dns_ntp_http_ping() {
    let mut net = Network::default();
    let s = joined(&mut net, "U2", Pos::new(1.0, 0.0));
    let now = net.now();
    let q = net.dns_resolve(s, "broker.sim", now).unwrap();
    let (_, e) = run_until(&mut net, s, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::DnsResolved { .. })
    });
    assert_eq!(
        e,
        NetEvent::DnsResolved {
            query: q,
            name: "broker.sim".into(),
            ip: Some(Ipv4Addr::new(10, 0, 0, 1))
        }
    );
    // Station names resolve to stations; unknown names fail.
    let now = net.now();
    net.dns_resolve(s, "esp-u2.sim", now).unwrap();
    let (_, e) = run_until(&mut net, s, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::DnsResolved { .. })
    });
    assert!(
        matches!(e, NetEvent::DnsResolved { ip: Some(ip), .. } if ip == Ipv4Addr::new(10, 0, 0, 42))
    );
    let now = net.now();
    net.dns_resolve(s, "nothing.example", now).unwrap();
    let (_, e) = run_until(&mut net, s, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::DnsResolved { .. })
    });
    assert!(matches!(e, NetEvent::DnsResolved { ip: None, .. }));
    assert!(
        net.log()
            .visible()
            .any(|e| e.summary == "DNS A broker.sim = 10.0.0.1")
    );

    // NTP: the reply carries the sim clock on the configured epoch.
    let u = net.udp_bind(s, 0).unwrap();
    let now = net.now();
    net.udp_send(u, gw(123), &wire::ntp_request(0.0), now)
        .unwrap();
    let (t, e) = run_until(&mut net, s, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::UdpData { .. })
    });
    let NetEvent::UdpData { data, from, .. } = e else {
        panic!()
    };
    assert_eq!(from, gw(123));
    let unix = wire::ntp_reply_time(&data).unwrap();
    let expect = net.config.epoch_unix as f64 + t.as_secs_f64();
    assert!((unix - expect).abs() < 0.01, "{unix} vs {expect}");

    // HTTP GET /get over TCP.
    let now = net.now();
    let sock = net.tcp_connect(s, gw(80), now).unwrap();
    run_until(&mut net, s, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::TcpConnected { .. })
    });
    let now = net.now();
    net.tcp_send(
        sock,
        b"GET /get?x=1&name=pot%201 HTTP/1.1\r\nHost: http.sim\r\nConnection: close\r\n\r\n",
        now,
    )
    .unwrap();
    let mut body = Vec::new();
    let mut closed = false;
    let mut t = now;
    while !closed && t < now + SimTime::from_secs(3) {
        t += ms(1);
        net.advance(t);
        for e in net.take_events(s) {
            match e {
                NetEvent::TcpData { data, .. } => body.extend(data),
                NetEvent::TcpClosed {
                    reason: CloseReason::PeerClosed,
                    ..
                } => closed = true,
                _ => {}
            }
        }
    }
    let text = String::from_utf8(body).unwrap();
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
    assert!(text.contains("\"name\": \"pot 1\""), "{text}");
    assert!(text.contains("\"origin\": \"10.0.0.42\""));
    assert!(closed);
    assert_eq!(net.tcp_state(sock), TcpState::Closed);
    assert!(
        net.log()
            .visible()
            .any(|e| e.summary.starts_with("HTTP GET /get"))
    );

    // Ping the gateway: RTT ≈ 2 × one-way latency (3 ms ± 1).
    let now = net.now();
    let p = net
        .ping(
            s,
            Ipv4Addr::new(10, 0, 0, 1),
            32,
            SimTime::from_secs(1),
            now,
        )
        .unwrap();
    let (_, e) = run_until(&mut net, s, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::PingReply { .. } | NetEvent::PingTimeout { .. })
    });
    let NetEvent::PingReply { ping, rtt } = e else {
        panic!("{e:?}")
    };
    assert_eq!(ping, p);
    assert!(rtt >= ms(4) && rtt <= ms(8), "{rtt:?}");
    let now = net.now();
    net.ping(
        s,
        Ipv4Addr::new(10, 0, 0, 99),
        32,
        SimTime::from_secs(1),
        now,
    )
    .unwrap();
    let (_, e) = run_until(&mut net, s, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::PingReply { .. } | NetEvent::PingTimeout { .. })
    });
    assert!(matches!(e, NetEvent::PingTimeout { .. }));
}

/// Connects a raw MQTT client over TCP and returns its socket.
fn mqtt_connect(net: &mut Network, s: StationId, client: &str, keep_alive: u16) -> SockId {
    let now = net.now();
    let sock = net.tcp_connect(s, gw(1883), now).unwrap();
    run_until(net, s, now + SimTime::from_secs(40), |e| {
        matches!(e, NetEvent::TcpConnected { .. })
    });
    let now = net.now();
    net.tcp_send(
        sock,
        &mqtt::encode(&Packet::Connect(mqtt::Connect::new(client, keep_alive))),
        now,
    )
    .unwrap();
    let (_, e) = run_until(net, s, now + SimTime::from_secs(40), |e| {
        matches!(e, NetEvent::TcpData { .. })
    });
    let NetEvent::TcpData { data, .. } = e else {
        panic!()
    };
    assert_eq!(
        mqtt::decode(&data).unwrap().unwrap().0,
        Packet::ConnAck {
            session_present: false,
            code: 0
        }
    );
    sock
}

#[test]
fn tcp_with_latency_jitter_and_loss_still_delivers_everything_in_order() {
    let mut net = Network::default();
    let s = joined(&mut net, "U2", Pos::new(1.0, 0.0));
    net.set_conditions(NetConditions {
        latency_ms: 40.0,
        jitter_ms: 10.0,
        loss_pct: 20.0,
        drop_ap_at_s: None,
        ap_down_s: 10.0,
    });
    let sock = mqtt_connect(&mut net, s, "lossy", 0);
    let watcher = net.broker_mut().virtual_connect("watch", SimTime::ZERO);
    let out = net
        .broker_mut()
        .virtual_subscribe(watcher, "loss/#", 0, SimTime::ZERO);
    net.apply_broker(out);
    // 60 publishes, one every 100 ms: every one arrives, in order, despite 20 % loss.
    let mut t = net.now();
    for i in 0..60 {
        let p = mqtt::encode(&Packet::Publish(mqtt::Publish {
            dup: false,
            qos: 0,
            retain: false,
            topic: format!("loss/{i}"),
            pid: None,
            payload: format!("{i}").into_bytes(),
        }));
        net.tcp_send(sock, &p, t).unwrap();
        t += ms(100);
        net.advance(t);
        net.take_events(s);
    }
    net.advance(t + SimTime::from_secs(60));
    let got: Vec<String> = net
        .broker_mut()
        .virtual_take(watcher)
        .iter()
        .map(|r| r.message.payload_str())
        .collect();
    let want: Vec<String> = (0..60).map(|i| i.to_string()).collect();
    assert_eq!(got, want);
    let drops = net.log().entries().filter(|e| e.dir == Dir::Drop).count();
    assert!(
        drops > 5,
        "expected the fault injector to drop frames ({drops})"
    );
    assert!(
        net.log()
            .entries()
            .any(|e| e.dir == Dir::Drop && e.text() == "dropped by fault injector (loss 20%)")
    );
    // Retransmitted segments appear twice in the log (same summary, later time).
    let pubs = net
        .log()
        .entries()
        .filter(|e| e.summary.starts_with("PUBLISH loss/"))
        .count();
    assert!(pubs > 60);
}

#[test]
fn tcp_latency_is_applied_per_direction() {
    let mut net = Network::default();
    let s = joined(&mut net, "U2", Pos::new(1.0, 0.0));
    net.set_conditions(NetConditions {
        latency_ms: 40.0,
        jitter_ms: 0.0,
        loss_pct: 0.0,
        drop_ap_at_s: None,
        ap_down_s: 10.0,
    });
    let now = net.now();
    let sock = net.tcp_connect(s, gw(7), now).unwrap();
    let (t, _) = run_until(&mut net, s, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::TcpConnected { .. })
    });
    // SYN → 40 ms → SYN-ACK → 40 ms.
    assert_eq!(t - now, ms(80));
    let now = net.now();
    net.tcp_send(sock, b"echo me", now).unwrap();
    let (t, e) = run_until(&mut net, s, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::TcpData { .. })
    });
    assert_eq!(
        e,
        NetEvent::TcpData {
            sock,
            data: b"echo me".to_vec()
        }
    );
    assert_eq!(t - now, ms(80));
    // Connection refused on a closed port.
    let now = net.now();
    let bad = net.tcp_connect(s, gw(4444), now).unwrap();
    let (_, e) = run_until(&mut net, s, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::TcpConnectFailed { .. })
    });
    assert_eq!(
        e,
        NetEvent::TcpConnectFailed {
            sock: bad,
            reason: CloseReason::Refused
        }
    );
    // Unreachable host: SYN retries exhaust.
    let now = net.now();
    let gone = net
        .tcp_connect(s, SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 200), 80), now)
        .unwrap();
    let (t, e) = run_until(&mut net, s, now + SimTime::from_secs(60), |e| {
        matches!(e, NetEvent::TcpConnectFailed { .. })
    });
    assert_eq!(
        e,
        NetEvent::TcpConnectFailed {
            sock: gone,
            reason: CloseReason::Timeout
        }
    );
    assert!(t - now > SimTime::from_secs(15));
}

#[test]
fn station_to_station_tcp_and_listen() {
    let mut net = Network::default();
    let a = joined(&mut net, "U2", Pos::new(1.0, 0.0));
    let b = joined(&mut net, "U3", Pos::new(0.0, 2.0));
    let l = net.tcp_listen(b, 8080).unwrap();
    let now = net.now();
    let ip_b = net.ip_of(b).unwrap();
    let c = net
        .tcp_connect(a, SocketAddrV4::new(ip_b, 8080), now)
        .unwrap();
    run_until(&mut net, a, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::TcpConnected { .. })
    });
    let (_, e) = run_until(&mut net, b, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::TcpAccepted { .. })
    });
    let NetEvent::TcpAccepted {
        listener,
        sock: sb,
        remote,
    } = e
    else {
        panic!()
    };
    assert_eq!(listener, l);
    assert_eq!(*remote.ip(), Ipv4Addr::new(10, 0, 0, 42));
    let now = net.now();
    net.tcp_send(c, b"hello b", now).unwrap();
    let (_, e) = run_until(&mut net, b, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::TcpData { .. })
    });
    assert_eq!(
        e,
        NetEvent::TcpData {
            sock: sb,
            data: b"hello b".to_vec()
        }
    );
    let now = net.now();
    net.close(sb, now);
    let (_, e) = run_until(&mut net, a, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::TcpClosed { .. })
    });
    assert_eq!(
        e,
        NetEvent::TcpClosed {
            sock: c,
            reason: CloseReason::PeerClosed
        }
    );
    net.advance(net.now() + SimTime::from_secs(2));
    assert_eq!(
        net.tcp_connection_count(),
        0,
        "graceful close forgets the connection"
    );
}

#[test]
fn ap_drop_disconnects_then_reconnects_and_broker_publishes_will() {
    let mut net = Network::default();
    let s = joined(&mut net, "U2", Pos::new(1.0, 0.0));
    let watcher = net.broker_mut().virtual_connect("watch", SimTime::ZERO);
    let out = net
        .broker_mut()
        .virtual_subscribe(watcher, "node/#", 0, SimTime::ZERO);
    net.apply_broker(out);
    let now = net.now();
    let sock = net.tcp_connect(s, gw(1883), now).unwrap();
    run_until(&mut net, s, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::TcpConnected { .. })
    });
    let mut c = mqtt::Connect::new("node", 10);
    c.will = Some(mqtt::Will {
        topic: "node/status".into(),
        payload: b"offline".to_vec(),
        qos: 0,
        retain: false,
    });
    let now = net.now();
    net.tcp_send(sock, &mqtt::encode(&Packet::Connect(c)), now)
        .unwrap();
    run_until(&mut net, s, now + SimTime::from_secs(2), |e| {
        matches!(e, NetEvent::TcpData { .. })
    });
    net.set_conditions(NetConditions {
        drop_ap_at_s: Some(10.0),
        ap_down_s: 5.0,
        ..NetConditions::default()
    });
    let mut ev = vec![];
    run(&mut net, s, SimTime::from_secs(10) + ms(5), &mut ev);
    assert!(net.ap_is_down());
    let kinds: Vec<_> = ev.iter().map(|e| e.1.clone()).collect();
    assert!(
        kinds.contains(&NetEvent::WifiDisconnected {
            reason: bb_radio::net::DisconnectReason::ApDown
        }),
        "{kinds:?}"
    );
    assert!(kinds.contains(&NetEvent::TcpClosed {
        sock,
        reason: CloseReason::LinkLost
    }));
    // The broker only notices through keep-alive (1.5 × 10 s after the last packet).
    assert!(net.broker_mut().virtual_take(watcher).is_empty());
    ev.clear();
    run(&mut net, s, SimTime::from_secs(25), &mut ev);
    assert!(!net.ap_is_down());
    // Auto-reconnect after the AP returns at t = 15 s.
    let t_ip = ev
        .iter()
        .find(|e| matches!(e.1, NetEvent::GotIp { .. }))
        .map(|e| e.0)
        .expect("reconnected");
    assert!(
        t_ip > SimTime::from_secs(15) && t_ip < SimTime::from_secs(19),
        "{t_ip:?}"
    );
    let will = net.broker_mut().virtual_take(watcher);
    assert_eq!(will.len(), 1);
    assert_eq!(
        will[0].message,
        Message::new("node/status", "offline", 0, false)
    );
    assert!(
        net.log()
            .entries()
            .any(|e| e.summary.contains("AP SIM-AP down"))
    );
}

#[test]
fn packet_log_exports_pcapng_and_csv() {
    let mut net = Network::default();
    let s = joined(&mut net, "U2", Pos::new(1.0, 0.0));
    let sock = mqtt_connect(&mut net, s, "garden-node", 60);
    let now = net.now();
    let payload = br#"{"m":42,"lux":180,"t":21}"#;
    let p = mqtt::encode(&Packet::Publish(mqtt::Publish {
        dup: false,
        qos: 0,
        retain: false,
        topic: "garden/pot1".into(),
        pid: None,
        payload: payload.to_vec(),
    }));
    net.tcp_send(sock, &p, now).unwrap();
    net.advance(now + SimTime::from_secs(1));
    let rows: Vec<String> = net
        .log()
        .visible()
        .map(|e| format!("{} {} {}", e.time_label(), e.dir.label(), e.text()))
        .collect();
    assert!(
        rows.iter()
            .any(|r| r.ends_with("tx MQTT CONNECT client=garden-node")),
        "{rows:#?}"
    );
    assert!(
        rows.iter()
            .any(|r| r.ends_with("tx PUBLISH garden/pot1 · 40 B · QoS 0")),
        "{rows:#?}"
    );
    assert!(rows.iter().any(|r| r.ends_with("rx MQTT CONNACK rc=0")));
    let pcap = net.log().to_pcapng();
    let frames = read_pcapng(&pcap).expect("valid pcapng");
    assert_eq!(
        frames.len(),
        net.log().entries().filter(|e| e.frame.is_some()).count()
    );
    // Every frame is Ethernet/IPv4 with a valid header checksum.
    for (_, f, _) in &frames {
        assert_eq!(&f[12..14], &[0x08, 0x00]);
        assert_eq!(wire::checksum(&f[14..34]), 0);
    }
    let pub_frame = frames
        .iter()
        .find(|(_, _, c)| c.as_deref() == Some("PUBLISH garden/pot1 · 40 B · QoS 0"))
        .unwrap();
    assert!(pub_frame.1.ends_with(payload));
    let csv = net.log().to_csv();
    assert!(csv.starts_with("time_s,dir,radio,proto,src,dst,bytes,frame,dropped\n"));
    assert!(csv.contains(",tx,U2,MQTT,10.0.0.42:"));
    assert!(csv.contains("PUBLISH garden/pot1 · 40 B · QoS 0"));
    // Broker annotations for the serial monitor.
    let notes = net.take_annotations();
    assert!(
        notes
            .iter()
            .any(|n| n.text == r#"PUB garden/pot1 {"m":42,"lux":180,"t":21} (40 B)"#),
        "{notes:?}"
    );
}

#[test]
fn radio_space_settings_explorer_and_merged_log() {
    use bb_radio::RadioSpace;
    let mut space = RadioSpace::default();
    let mut s = space.settings();
    assert_eq!(s.access_points[0].ssid, "SIM-AP");
    s.conditions.latency_ms = 40.0;
    s.conditions.loss_pct = 2.0;
    s.walls
        .push(Wall::interior(Pos::new(1.5, -5.0), Pos::new(1.5, 5.0)));
    let json = serde_json::to_string(&s).unwrap();
    let back: bb_radio::space::RadioSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s);
    space.apply_settings(&back);
    assert_eq!(space.settings(), s);
    // Explorer: subscribe + publish through the built-in broker.
    space.explorer_subscribe("garden/#");
    space.explorer_publish("garden/cmd", b"water", 1, false);
    space.advance(SimTime::from_ms(1));
    let got = space.explorer_take();
    assert_eq!(got[0].message.payload, b"water");
    let hm = space.add_hm10("U3", Pos::new(1.0, 0.0));
    space.power(hm, true, SimTime::from_ms(1));
    space.advance(SimTime::from_ms(400));
    let id = space.ble.scan()[0].id;
    space.ble.connect(id, None, SimTime::from_ms(400)).unwrap();
    space.advance(SimTime::from_ms(600));
    assert!(space.log_rows().iter().any(|r| r.proto == Proto::Ble));
    let cards = space.cards();
    assert_eq!(cards[0].title, "HM-10 BLE");
    assert_eq!(cards[0].status, "connected");
}
