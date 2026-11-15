//! BLE: GATT model + virtual phone, HM-10 AT/transparent UART, HC-05 SPP, and the ble-button
//! firmware end to end (real AVR ↔ HM-10 ↔ phone).

mod common;

use bb_core::SimTime;
use bb_radio::ble::{
    BleError, BleSpace, BtKind, HM10_CHAR, HM10_SERVICE, PeripheralEvent, PhoneEvent, Uuid, props,
};
use bb_radio::geo::Pos;
use bb_radio::uart::UartFrame;
use bb_radio::{DeviceId, ModulePin, RadioSpace};
use common::*;

fn ms(x: u64) -> SimTime {
    SimTime::from_ms(x)
}

#[test]
fn gatt_and_phone_read_write_notify() {
    let mut ble = BleSpace::new(1);
    let p = ble.add_peripheral("U5", Pos::new(1.0, 0.0));
    let g = ble.gatt_mut(p).unwrap();
    let svc = g.add_service(Uuid::parse("181A").unwrap()); // Environmental Sensing
    let temp = g.add_characteristic(
        svc,
        Uuid::U16(0x2A6E),
        props::READ | props::NOTIFY,
        &[0x34, 0x08],
    );
    let ctl = g.add_characteristic(
        svc,
        Uuid::parse("6e400002-b5a3-f393-e0a9-e50e24dcca9e").unwrap(),
        props::WRITE,
        b"",
    );
    ble.set_name(p, "weather");
    ble.set_advertising(p, true);
    let far = ble.add_peripheral("U6", Pos::new(500.0, 0.0));
    ble.set_advertising(far, true);
    // Scan shows the near one with an RSSI, not the one 500 m away.
    let scan = ble.scan();
    assert_eq!(scan.len(), 1);
    assert_eq!(scan[0].name, "weather");
    assert_eq!(scan[0].kind, BtKind::Le);
    assert!((-60..-30).contains(&scan[0].rssi), "{}", scan[0].rssi);
    assert_eq!(scan[0].services, vec![Uuid::U16(0x181A)]);
    assert_eq!(ble.connect(far, None, ms(0)), Err(BleError::OutOfRange));
    ble.connect(p, None, ms(0)).unwrap();
    assert_eq!(ble.read(temp, ms(1)), Err(BleError::NotConnected));
    ble.advance(ms(100));
    let ev = ble.take_phone_events();
    assert!(matches!(ev[0].1, PhoneEvent::Connected { id } if id == p));
    assert!(ev[0].0 >= ms(40) && ev[0].0 <= ms(90));
    assert_eq!(
        ble.take_peripheral_events(p),
        vec![PeripheralEvent::Connected]
    );
    // Discovery.
    let services = ble.discover().unwrap();
    assert_eq!(services[0].characteristics.len(), 2);
    assert_eq!(
        services[0].characteristics[1].uuid.to_string(),
        "6e400002-b5a3-f393-e0a9-e50e24dcca9e"
    );
    // Read at the next connection event.
    ble.read(temp, ms(100)).unwrap();
    ble.advance(ms(140));
    let ev = ble.take_phone_events();
    assert_eq!(
        ev[0].1,
        PhoneEvent::ReadResult {
            id: p,
            handle: temp,
            data: vec![0x34, 0x08]
        }
    );
    assert!(ev[0].0 > ms(100) && ev[0].0 <= ms(131));
    // Write with response.
    ble.write(ctl, b"go", ms(140)).unwrap();
    assert_eq!(ble.write(temp, b"x", ms(140)), Err(BleError::NotPermitted));
    ble.advance(ms(200));
    assert!(
        ble.take_phone_events()
            .iter()
            .any(|e| e.1 == PhoneEvent::WriteDone { id: p, handle: ctl })
    );
    assert!(
        ble.take_peripheral_events(p)
            .contains(&PeripheralEvent::Write {
                handle: ctl,
                data: b"go".to_vec()
            })
    );
    // Notifications only after subscribing; long values are split into 20-byte packets.
    assert!(!ble.notify(p, temp, &[1, 2], ms(200)));
    ble.subscribe(temp, true, ms(200)).unwrap();
    ble.advance(ms(240));
    assert!(
        ble.take_peripheral_events(p)
            .contains(&PeripheralEvent::Subscribed {
                handle: temp,
                on: true
            })
    );
    assert!(ble.notify(p, temp, &[7u8; 30], ms(240)));
    ble.advance(ms(300));
    let notes: Vec<Vec<u8>> = ble
        .take_phone_events()
        .into_iter()
        .filter_map(|e| match e.1 {
            PhoneEvent::Notification { data, .. } => Some(data),
            _ => None,
        })
        .collect();
    assert_eq!(notes, vec![vec![7u8; 20], vec![7u8; 10]]);
    // Peripheral power-off drops the link.
    ble.set_powered(p, false, ms(300));
    assert!(
        ble.take_phone_events()
            .iter()
            .any(|e| e.1 == PhoneEvent::Disconnected { id: p })
    );
    assert!(ble.phone_connected().is_none());
}

/// Drives one UART module directly (no MCU).
struct Module {
    space: RadioSpace,
    dev: DeviceId,
    t: SimTime,
    out: Vec<UartFrame>,
}

impl Module {
    fn send(&mut self, s: &str) {
        let cfg = self.space.uart_config(self.dev).unwrap();
        for b in s.bytes() {
            self.t += cfg.frame_time();
            self.space.uart_rx(self.dev, b, self.t);
        }
    }
    fn run(&mut self, d: SimTime) -> String {
        let end = self.t + d;
        let from = self.out.len();
        while self.t < end {
            self.t = (self.t + ms(1)).min(end);
            self.space.advance(self.t);
            let f = self.space.take_uart_tx(self.dev);
            self.out.extend(f);
        }
        self.out[from..]
            .iter()
            .map(|f| f.data as u8 as char)
            .collect()
    }
}

#[test]
fn hm10_at_commands_and_transparent_uart() {
    let mut space = RadioSpace::default();
    let dev = space.add_hm10("U3", Pos::new(1.0, 0.0));
    space.power(dev, true, SimTime::ZERO);
    let mut m = Module {
        space,
        dev,
        t: SimTime::ZERO,
        out: vec![],
    };
    m.run(ms(400));
    // No line ending needed: the command ends after a pause.
    m.send("AT");
    assert_eq!(m.run(ms(60)), "OK");
    m.send("AT+NAMEgarden");
    assert_eq!(m.run(ms(60)), "OK+Set:garden");
    m.send("AT+NAME?");
    assert_eq!(m.run(ms(60)), "OK+NAME:garden");
    m.send("AT+ADDR?\r\n");
    assert_eq!(m.run(ms(60)), "OK+ADDR:A4C1381234A0");
    m.send("AT+NOTI1");
    assert_eq!(m.run(ms(60)), "OK+Set:1");
    m.send("AT+UUID?");
    assert_eq!(m.run(ms(60)), "OK+Get:0xFFE0");
    m.send("AT+BAUD?");
    assert_eq!(m.run(ms(60)), "OK+Get:0");
    // Phone connects → OK+CONN on the UART.
    let scan = m.space.ble.scan();
    assert_eq!(scan[0].name, "garden");
    let t = m.t;
    m.space.ble.connect(scan[0].id, None, t).unwrap();
    assert_eq!(m.run(ms(120)), "OK+CONN");
    let t = m.t;
    m.space.ble.terminal_open(t).unwrap();
    m.run(ms(50));
    // UART bytes → notifications on FFE1 (20-byte packets).
    m.send("hello from the bench, this is long\n");
    m.run(ms(100));
    let term: Vec<u8> = m
        .space
        .ble
        .take_phone_events()
        .into_iter()
        .filter_map(|e| match e.1 {
            PhoneEvent::Terminal { data, .. } => Some(data),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(
        String::from_utf8(term).unwrap(),
        "hello from the bench, this is long\n"
    );
    // Phone terminal → UART. AT text while connected is data, not a command.
    let t = m.t;
    m.space.ble.terminal_send(b"AT+NAME?", t).unwrap();
    assert_eq!(m.run(ms(60)), "AT+NAME?");
    let svc = m.space.ble.discover().unwrap();
    assert_eq!(svc[0].uuid, HM10_SERVICE);
    assert_eq!(svc[0].characteristics[0].uuid, HM10_CHAR);
    let t = m.t;
    m.space.ble.disconnect(t);
    // the hang-up is queued synchronously: the space must ask to be stepped now, or a bench
    // that only advances the radio at `next_wake()` never tells the MCU (OK+LOST)
    assert_eq!(m.space.next_wake(), Some(t));
    assert_eq!(m.run(ms(10)), "OK+LOST");
    // Baud change applies after AT+RESET.
    m.send("AT+BAUD4");
    assert_eq!(m.run(ms(60)), "OK+Set:4");
    assert_eq!(m.space.uart_config(dev).unwrap().baud, 9600);
    m.send("AT+RESET");
    assert_eq!(m.run(ms(400)), "OK+RESET");
    assert_eq!(m.space.uart_config(dev).unwrap().baud, 115_200);
}

#[test]
fn hc05_at_mode_and_spp_terminal() {
    let mut space = RadioSpace::default();
    let dev = space.add_hc05("U4", Pos::new(1.0, 1.0));
    space.set_pin(dev, ModulePin::Key, true, SimTime::ZERO);
    space.power(dev, true, SimTime::ZERO);
    let mut m = Module {
        space,
        dev,
        t: SimTime::ZERO,
        out: vec![],
    };
    m.run(ms(600));
    assert_eq!(m.space.uart_config(dev).unwrap().baud, 38_400);
    m.send("AT\r\n");
    assert_eq!(m.run(ms(20)), "OK\r\n");
    m.send("AT+NAME?\r\n");
    assert_eq!(m.run(ms(20)), "+NAME:HC-05\r\nOK\r\n");
    m.send("AT+NAME=robot\r\n");
    m.send("AT+PSWD=\"4321\"\r\n");
    m.send("AT+UART=57600,0,0\r\n");
    assert_eq!(m.run(ms(30)), "OK\r\nOK\r\nOK\r\n");
    m.send("AT+VERSION?\r\n");
    assert_eq!(m.run(ms(20)), "+VERSION:3.0-20170601\r\nOK\r\n");
    m.send("AT+FOO\r\n");
    assert_eq!(m.run(ms(20)), "ERROR:(0)\r\n");
    // Power-cycle with KEY low → data mode at the configured baud.
    let t = m.t;
    m.space.set_pin(dev, ModulePin::Key, false, t);
    m.space.power(dev, false, t);
    m.space.power(dev, true, t);
    m.run(ms(600));
    assert_eq!(m.space.uart_config(dev).unwrap().baud, 57_600);
    let scan = m.space.ble.scan();
    assert_eq!(scan[0].name, "robot");
    assert_eq!(scan[0].kind, BtKind::Classic);
    let t = m.t;
    assert_eq!(
        m.space.ble.connect(scan[0].id, Some("1234"), t),
        Err(BleError::WrongPin)
    );
    m.space.ble.connect(scan[0].id, Some("4321"), t).unwrap();
    m.run(ms(120));
    let t = m.t;
    m.space.ble.terminal_send(b"forward\n", t).unwrap();
    assert_eq!(m.run(ms(40)), "forward\n");
    m.send("ok, moving\n");
    m.run(ms(40));
    let term: Vec<u8> = m
        .space
        .ble
        .take_phone_events()
        .into_iter()
        .filter_map(|e| match e.1 {
            PhoneEvent::Terminal { data, .. } => Some(data),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(String::from_utf8(term).unwrap(), "ok, moving\n");
}

#[test]
fn ble_button_firmware_with_virtual_phone() {
    let mut space = RadioSpace::default();
    let hm = space.add_hm10("U2", Pos::new(0.5, 0.0));
    space.ble.phone_pos = Pos::new(2.0, 1.0);
    space.power(hm, true, SimTime::ZERO);
    let mut b = Bench::new(load_example("ble-button"), space, hm, uno(11), uno(10));
    let button = uno(4);
    b.avr.set_pin_input(button, Some(true));
    b.run_until(SimTime::from_secs(2));
    let lines: Vec<String> = b.lines.iter().map(|l| l.1.clone()).collect();
    assert!(lines.contains(&"AT -> OK".to_string()), "{lines:#?}");
    assert!(
        lines.contains(&"AT+NAMEbb-button -> OK+Set:bb-button".to_string()),
        "{lines:#?}"
    );
    assert!(lines.contains(&"AT+NOTI1 -> OK+Set:1".to_string()));
    assert!(lines.contains(&"ble-button ready".to_string()));

    // The phone finds it, connects and opens the terminal.
    let scan = b.space.ble.scan();
    assert_eq!(scan.len(), 1);
    assert_eq!(scan[0].name, "bb-button");
    let id = scan[0].id;
    let t = b.now();
    b.space.ble.connect(id, None, t).unwrap();
    b.run_until(t + ms(150));
    let t = b.now();
    b.space.ble.terminal_open(t).unwrap();
    b.run_until(t + ms(100));
    assert!(b.lines.iter().any(|l| l.1 == "phone connected"));

    // Press the button (active low) for 80 ms → notification on the phone.
    let t = b.now();
    let c = b.avr.cycle_at(t + ms(10));
    b.avr.schedule_pin_input(button, Some(false), c);
    let c = b.avr.cycle_at(t + ms(90));
    b.avr.schedule_pin_input(button, Some(true), c);
    b.run_until(t + ms(300));
    let mut term = String::new();
    let mut notified_at = None;
    for (at, e) in b.space.ble.take_phone_events() {
        if let PhoneEvent::Terminal { data, .. } = e {
            notified_at.get_or_insert(at);
            term.push_str(&String::from_utf8_lossy(&data));
        }
    }
    assert_eq!(term, "pressed #1\n");
    let lat = (notified_at.unwrap() - (t + ms(10))).as_secs_f64() * 1e3;
    assert!(lat < 80.0, "button → phone latency {lat} ms");

    // Phone types "on" → LED D13 lights; the sketch answers.
    let t = b.now();
    b.space.ble.terminal_send(b"on\n", t).unwrap();
    b.run_until(t + ms(200));
    let led_on = b
        .pins
        .iter()
        .any(|e| e.pin == uno(13) && e.state == bb_avr::PinState::Output { high: true });
    assert!(led_on, "LED did not turn on");
    let reply: String = b
        .space
        .ble
        .take_phone_events()
        .into_iter()
        .filter_map(|(_, e)| match e {
            PhoneEvent::Terminal { data, .. } => Some(String::from_utf8_lossy(&data).into_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(reply, "led on\n");
    // Read the characteristic directly (last value written by the module).
    let h = b.space.ble.discover().unwrap()[0].characteristics[0].handle;
    let t = b.now();
    b.space.ble.read(h, t).unwrap();
    b.run_until(t + ms(50));
    assert!(
        b.space
            .ble
            .take_phone_events()
            .iter()
            .any(|(_, e)| matches!(e, PhoneEvent::ReadResult { data, .. } if data == b"led on\n"))
    );
    let notes: Vec<String> = b.notes.iter().map(|n| n.line()).collect();
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("ble: phone connected to U2")),
        "{notes:#?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n == "ble: U2 → phone notify FFE1 \"pressed #1\" (11 B)"),
        "{notes:#?}"
    );
    // Disconnect → the sketch sees OK+LOST.
    let t = b.now();
    b.space.ble.disconnect(t);
    b.run_until(t + ms(100));
    assert!(b.lines.iter().any(|l| l.1 == "phone disconnected"));
}
