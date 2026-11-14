//! [`RadioSpace`]: the one object a co-simulator holds for everything radio on a bench — the
//! Wi-Fi network (with its services, packet log and fault injector), the BLE air with the
//! virtual phone, the LoRa medium with its gateway, and every UART radio module (ESP-01, HM-10,
//! HC-05, RYLR896) plus host-board (ESP32) radios, addressed by [`DeviceId`].
//!
//! Driving it (see `docs/ARCHITECTURE.md` §6b):
//!
//! 1. `advance(now)` whenever simulated time moves (at least at every [`RadioSpace::next_wake`]).
//! 2. Wire each UART module to MCU pins with a [`UartWire`]: feed the MCU's TX edges with
//!    [`UartWire::mcu_edge`], stop the MCU at [`UartWire::next_decision`], call
//!    [`UartWire::pump`], and schedule [`UartWire::module_edges`] on the MCU's RX pin.
//! 3. Show [`RadioSpace::take_annotations`] in the serial monitor of the MCU wired to the module.

use crate::ble::{BleSpace, Hc05, Hm10};
use crate::espat::{EspAt, EspAtConfig};
use crate::geo::Pos;
use crate::lora::{LoraSpace, Rylr896};
use crate::net::{AnnotationKind, NetConfig, Network, StationId};
use crate::uart::{UartConfig, UartDecoder, UartFrame};
use bb_core::SimTime;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeviceId(pub u32);

/// Serial-monitor annotation (`RF-8`), e.g. `wifi: AT+CWJAP="SIM-AP" → OK rssi=-54 dBm ip=10.0.0.42`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Annotation {
    pub time: SimTime,
    /// Radio the event belongs to (route it to the serial monitor of the MCU wired to it).
    pub device: Option<DeviceId>,
    /// Part reference of that radio ("U2").
    pub label: String,
    pub kind: AnnotationKind,
    pub text: String,
}

impl Annotation {
    /// The line as the serial monitor prints it.
    pub fn line(&self) -> String {
        format!("{}: {}", self.kind.prefix(), self.text)
    }
}

/// Module pins other than the UART.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModulePin {
    /// ESP-01 RST (active low), HM-10 RESETB, RYLR896 RST.
    Reset,
    /// ESP-01 CH_PD / EN (high = enabled).
    Enable,
    /// HC-05 KEY/EN: high at power-up = AT mode (38400 baud).
    Key,
}

#[derive(Debug, Clone)]
pub enum Module {
    Esp01(EspAt),
    Hm10(Hm10),
    Hc05(Hc05),
    Rylr896(Rylr896),
    /// A host-emulated board's radio (ESP32 Wi-Fi + BLE), served through [`crate::host`].
    Host(crate::host::HostRadio),
}

#[derive(Debug, Clone)]
struct Slot {
    label: String,
    module: Module,
}

/// Summary for the radio panel cards (`RF-7`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RadioCard {
    pub device: DeviceId,
    pub label: String,
    /// "ESP8266 Wi-Fi", "HM-10 BLE", …
    pub title: String,
    /// "connected", "joining", "idle", "off", "advertising", …
    pub status: String,
    /// Label/value rows, e.g. ("Access point", "SIM-AP · ch 6"), ("RSSI", "-54 dBm (3.2 m, 1 wall)").
    pub rows: Vec<(String, String)>,
}

/// Everything radio on one bench.
#[derive(Debug)]
pub struct RadioSpace {
    pub net: Network,
    pub ble: BleSpace,
    pub lora: LoraSpace,
    slots: Vec<Slot>,
    notes: Vec<Annotation>,
    now: SimTime,
}

impl Default for RadioSpace {
    fn default() -> Self {
        RadioSpace::new(NetConfig::default())
    }
}

impl RadioSpace {
    pub fn new(config: NetConfig) -> Self {
        let seed = config.seed;
        RadioSpace { net: Network::new(config), ble: BleSpace::new(seed ^ 0xb1e), lora: LoraSpace::new(seed ^ 0x10ba), slots: vec![], notes: vec![], now: SimTime::ZERO }
    }

    pub fn now(&self) -> SimTime {
        self.now
    }

    // ================================================================ devices

    fn push(&mut self, label: &str, module: Module) -> DeviceId {
        self.slots.push(Slot { label: label.into(), module });
        DeviceId(self.slots.len() as u32 - 1)
    }

    /// Adds an ESP-01 (ESP8266, ESP-AT firmware) at `pos`. Not powered until [`RadioSpace::power`].
    pub fn add_esp01(&mut self, label: &str, pos: Pos, config: EspAtConfig) -> DeviceId {
        let sta = self.net.add_station(label, pos);
        self.push(label, Module::Esp01(EspAt::new(label, sta, config)))
    }

    /// Adds an HM-10 BLE module (peripheral with the FFE0/FFE1 transparent-UART service).
    pub fn add_hm10(&mut self, label: &str, pos: Pos) -> DeviceId {
        let p = self.ble.add_peripheral(label, pos);
        self.push(label, Module::Hm10(Hm10::new(label, p)))
    }

    /// Adds an HC-05 Bluetooth SPP module.
    pub fn add_hc05(&mut self, label: &str, pos: Pos) -> DeviceId {
        let p = self.ble.add_classic(label, pos);
        self.push(label, Module::Hc05(Hc05::new(label, p)))
    }

    /// Adds a REYAX RYLR896 LoRa module.
    pub fn add_rylr896(&mut self, label: &str, pos: Pos) -> DeviceId {
        let n = self.lora.add_node(label, pos);
        self.push(label, Module::Rylr896(Rylr896::new(label, n)))
    }

    /// Registers the radio of a host-emulated board (ESP32: Wi-Fi station + BLE peripheral).
    pub fn add_host_radio(&mut self, label: &str, pos: Pos) -> DeviceId {
        let sta = self.net.add_station(label, pos);
        self.net.set_hostname(sta, &format!("esp32-{}", label.to_ascii_lowercase()));
        let p = self.ble.add_peripheral(label, pos);
        self.ble.set_advertising(p, false);
        self.push(label, Module::Host(crate::host::HostRadio::new(label, sta, p)))
    }

    pub fn devices(&self) -> Vec<(DeviceId, &str)> {
        self.slots.iter().enumerate().map(|(i, s)| (DeviceId(i as u32), s.label.as_str())).collect()
    }

    pub fn module(&self, id: DeviceId) -> Option<&Module> {
        self.slots.get(id.0 as usize).map(|s| &s.module)
    }

    pub fn module_mut(&mut self, id: DeviceId) -> Option<&mut Module> {
        self.slots.get_mut(id.0 as usize).map(|s| &mut s.module)
    }

    pub fn label(&self, id: DeviceId) -> Option<&str> {
        self.slots.get(id.0 as usize).map(|s| s.label.as_str())
    }

    pub fn esp01(&self, id: DeviceId) -> Option<&EspAt> {
        match self.module(id)? {
            Module::Esp01(m) => Some(m),
            _ => None,
        }
    }

    pub fn hm10(&self, id: DeviceId) -> Option<&Hm10> {
        match self.module(id)? {
            Module::Hm10(m) => Some(m),
            _ => None,
        }
    }

    /// Wi-Fi station of a device (ESP-01 or host board).
    pub fn station(&self, id: DeviceId) -> Option<StationId> {
        match self.module(id)? {
            Module::Esp01(m) => Some(m.station),
            Module::Host(h) => Some(h.station),
            _ => None,
        }
    }

    /// Moves a radio on the floor plan.
    pub fn set_position(&mut self, id: DeviceId, pos: Pos) {
        match self.module(id) {
            Some(Module::Esp01(m)) => {
                let s = m.station;
                self.net.set_station_pos(s, pos);
            }
            Some(Module::Host(h)) => {
                let (s, p) = (h.station, h.peripheral);
                self.net.set_station_pos(s, pos);
                self.ble.set_position(p, pos);
            }
            Some(Module::Hm10(m)) => {
                let p = m.peripheral;
                self.ble.set_position(p, pos);
            }
            Some(Module::Hc05(m)) => {
                let p = m.device;
                self.ble.set_position(p, pos);
            }
            Some(Module::Rylr896(m)) => {
                let n = m.node;
                self.lora.set_position(n, pos);
            }
            None => {}
        }
    }

    /// Supply on/off.
    pub fn power(&mut self, id: DeviceId, on: bool, now: SimTime) {
        self.advance(now);
        let RadioSpace { net, ble, lora, slots, .. } = self;
        let Some(slot) = slots.get_mut(id.0 as usize) else { return };
        match &mut slot.module {
            Module::Esp01(m) => {
                if on {
                    m.power_on(net, now)
                } else {
                    m.power_off(net, now)
                }
            }
            Module::Hm10(m) => m.power(ble, on, now),
            Module::Hc05(m) => m.power(ble, on, now),
            Module::Rylr896(m) => m.power(lora, on, now),
            Module::Host(_) => {}
        }
        self.collect_notes();
    }

    /// Drives a control pin (reset, enable, HC-05 KEY).
    pub fn set_pin(&mut self, id: DeviceId, pin: ModulePin, high: bool, now: SimTime) {
        self.advance(now);
        let RadioSpace { net, ble, lora, slots, .. } = self;
        let Some(slot) = slots.get_mut(id.0 as usize) else { return };
        match (&mut slot.module, pin) {
            (Module::Esp01(m), ModulePin::Reset) if !high => m.power_off(net, now),
            (Module::Esp01(m), ModulePin::Reset) => m.power_on(net, now),
            (Module::Esp01(m), ModulePin::Enable) => {
                if high {
                    m.power_on(net, now)
                } else {
                    m.power_off(net, now)
                }
            }
            (Module::Hc05(m), ModulePin::Key) => m.set_key(high),
            (Module::Hm10(m), ModulePin::Reset) if !high => m.power(ble, false, now),
            (Module::Hm10(m), ModulePin::Reset) => m.power(ble, true, now),
            (Module::Rylr896(m), ModulePin::Reset) => m.power(lora, high, now),
            _ => {}
        }
    }

    /// UART format a module listens/speaks at (for its [`UartDecoder`]).
    pub fn uart_config(&self, id: DeviceId) -> Option<UartConfig> {
        Some(match self.module(id)? {
            Module::Esp01(m) => m.uart_config(),
            Module::Hm10(m) => m.uart_config(),
            Module::Hc05(m) => m.uart_config(),
            Module::Rylr896(m) => m.uart_config(),
            Module::Host(_) => return None,
        })
    }

    /// A byte arrived on a module's RX pin at `at`.
    pub fn uart_rx(&mut self, id: DeviceId, byte: u8, at: SimTime) {
        self.advance(at);
        let RadioSpace { net, ble, lora, slots, .. } = self;
        let Some(slot) = slots.get_mut(id.0 as usize) else { return };
        match &mut slot.module {
            Module::Esp01(m) => m.rx_byte(net, byte, at),
            Module::Hm10(m) => m.rx_byte(ble, byte, at),
            Module::Hc05(m) => m.rx_byte(ble, byte, at),
            Module::Rylr896(m) => m.rx_byte(lora, byte, at),
            Module::Host(_) => {}
        }
        self.collect_notes();
    }

    /// Frames the module wants on its TX pin (start times are ≥ the time they were produced).
    pub fn take_uart_tx(&mut self, id: DeviceId) -> Vec<UartFrame> {
        match self.module_mut(id) {
            Some(Module::Esp01(m)) => m.take_tx(),
            Some(Module::Hm10(m)) => m.take_tx(),
            Some(Module::Hc05(m)) => m.take_tx(),
            Some(Module::Rylr896(m)) => m.take_tx(),
            _ => vec![],
        }
    }

    // ================================================================ time

    /// Earliest time anything radio needs attention.
    pub fn next_wake(&self) -> Option<SimTime> {
        let mut t = [self.net.next_event_time(), self.ble.next_event_time(), self.lora.next_event_time()].into_iter().flatten().min();
        for s in &self.slots {
            let w = match &s.module {
                Module::Esp01(m) => m.next_wake(),
                Module::Hm10(m) => m.next_wake(),
                Module::Hc05(m) => m.next_wake(),
                Module::Rylr896(m) => m.next_wake(),
                Module::Host(h) => h.next_wake(),
            };
            if let Some(w) = w {
                t = Some(t.map_or(w, |x| x.min(w)));
            }
        }
        t
    }

    /// Runs every radio subsystem up to `now`, interleaving them in time order.
    pub fn advance(&mut self, now: SimTime) {
        let mut guard = 0u32;
        while let Some(t) = self.next_wake() {
            if t > now {
                break;
            }
            let t = t.max(self.now);
            self.step_to(t);
            guard += 1;
            if guard > 1_000_000 {
                break; // a module that keeps asking for the same instant would hang the sim
            }
        }
        if now > self.now {
            self.step_to(now);
        }
    }

    fn step_to(&mut self, t: SimTime) {
        self.now = self.now.max(t);
        let t = self.now;
        self.net.advance(t);
        self.ble.advance(t);
        self.lora.advance(t);
        let RadioSpace { net, ble, lora, slots, .. } = self;
        for s in slots.iter_mut() {
            match &mut s.module {
                Module::Esp01(m) => m.poll(net, t),
                Module::Hm10(m) => m.poll(ble, t),
                Module::Hc05(m) => m.poll(ble, t),
                Module::Rylr896(m) => m.poll(lora, t),
                Module::Host(h) => h.poll(net, ble, t),
            }
        }
        // The LoRa gateway forwards uplinks to the broker.
        let ups = self.lora.take_gateway_uplinks();
        if !ups.is_empty() {
            crate::lora::forward_uplinks(&mut self.net, &mut self.lora, ups, t);
        }
        crate::lora::forward_downlinks(&mut self.net, &mut self.lora, t);
        self.collect_notes();
    }

    fn collect_notes(&mut self) {
        let device_of_station = |slots: &[Slot], s: StationId| {
            slots.iter().position(|x| match &x.module {
                Module::Esp01(m) => m.station == s,
                Module::Host(h) => h.station == s,
                _ => false,
            })
        };
        for n in self.net.take_annotations() {
            let d = device_of_station(&self.slots, n.station);
            let label = self.net.station_label(n.station).unwrap_or("").to_string();
            self.notes.push(Annotation { time: n.time, device: d.map(|i| DeviceId(i as u32)), label, kind: n.kind, text: n.text });
        }
        for (i, s) in self.slots.iter_mut().enumerate() {
            let notes = match &mut s.module {
                Module::Esp01(m) => m.take_notes(),
                Module::Hm10(m) => m.take_notes(),
                Module::Hc05(m) => m.take_notes(),
                Module::Rylr896(m) => m.take_notes(),
                Module::Host(h) => h.take_notes(),
            };
            for (time, kind, text) in notes {
                self.notes.push(Annotation { time, device: Some(DeviceId(i as u32)), label: s.label.clone(), kind, text });
            }
        }
        self.notes.sort_by_key(|n| n.time);
    }

    /// Serial-monitor annotations since the last call, in time order.
    pub fn take_annotations(&mut self) -> Vec<Annotation> {
        std::mem::take(&mut self.notes)
    }

    // ================================================================ UI data

    /// Radio cards for the panel (`RF-7`).
    pub fn cards(&self) -> Vec<RadioCard> {
        let mut out = vec![];
        for (i, s) in self.slots.iter().enumerate() {
            let device = DeviceId(i as u32);
            let card = match &s.module {
                Module::Esp01(m) => wifi_card(&self.net, device, &s.label, "ESP8266 Wi-Fi", m.station, m.status(&self.net).powered),
                Module::Host(h) => wifi_card(&self.net, device, &s.label, "ESP32 Wi-Fi + BLE", h.station, true),
                Module::Hm10(m) => m.card(device, &self.ble),
                Module::Hc05(m) => m.card(device, &self.ble),
                Module::Rylr896(m) => m.card(device, &self.lora),
            };
            out.push(card);
        }
        out
    }
}

fn wifi_card(net: &Network, device: DeviceId, label: &str, title: &str, sta: StationId, powered: bool) -> RadioCard {
    let info = net.station_info(sta);
    let mut rows = vec![];
    let status = match &info {
        _ if !powered => "off".to_string(),
        Some(i) => match i.state {
            crate::net::WifiStateKind::Connected => "connected".into(),
            crate::net::WifiStateKind::Joining => "joining".into(),
            crate::net::WifiStateKind::Idle => "idle".into(),
        },
        None => "idle".into(),
    };
    if let Some(i) = info {
        if let (Some(ssid), Some(ch)) = (&i.ssid, i.channel) {
            rows.push(("Access point".into(), format!("{ssid} · ch {ch}")));
        }
        if let Some(l) = i.link {
            let walls = match l.walls {
                0 => String::new(),
                1 => ", 1 wall".into(),
                n => format!(", {n} walls"),
            };
            rows.push(("RSSI".into(), format!("{} dBm ({:.1} m{walls})", l.rssi_dbm.round() as i32, l.distance_m)));
        }
        if let Some(ip) = i.ip {
            rows.push(("IP".into(), format!("{ip} / {}", if i.dhcp { "DHCP" } else { "static" })));
        }
        rows.push(("Broker".into(), format!("mqtt://{} (built-in)", net.gateway())));
    }
    RadioCard { device, label: label.into(), title: title.into(), status, rows }
}

/// Wires one UART module to an MCU: decodes the MCU's TX line into module bytes and turns the
/// module's frames into level changes for the MCU's RX pin.
#[derive(Debug, Clone)]
pub struct UartWire {
    pub device: DeviceId,
    rx: UartDecoder,
    /// Every byte the MCU sent (with framing info) — a logic-analyser view for tests/UI.
    pub sent: Vec<crate::uart::RxByte>,
    /// Every frame the module sent.
    pub received: Vec<UartFrame>,
}

impl UartWire {
    pub fn new(space: &RadioSpace, device: DeviceId) -> Self {
        let cfg = space.uart_config(device).unwrap_or_default();
        UartWire { device, rx: UartDecoder::new(cfg), sent: vec![], received: vec![] }
    }

    /// The MCU's TX line changed level at `t` (idle = high).
    pub fn mcu_edge(&mut self, t: SimTime, high: bool) {
        self.rx.edge(t, high);
    }

    /// When a byte currently on the wire will complete: stop the MCU there, then [`UartWire::pump`].
    pub fn next_decision(&self) -> Option<SimTime> {
        self.rx.next_decision()
    }

    /// Delivers every byte completed by `now` to the module (at its exact time).
    pub fn pump(&mut self, space: &mut RadioSpace, now: SimTime) {
        if let Some(cfg) = space.uart_config(self.device)
            && cfg != self.rx.config()
            && !self.rx.busy()
        {
            self.rx.set_config(cfg);
        }
        for b in self.rx.poll(now) {
            self.sent.push(b);
            space.uart_rx(self.device, b.data as u8, b.time);
        }
    }

    /// Level changes for the MCU's RX pin produced since the last call (time ordered).
    pub fn module_edges(&mut self, space: &mut RadioSpace) -> Vec<(SimTime, bool)> {
        let frames = space.take_uart_tx(self.device);
        let edges = frames.iter().flat_map(|f| f.transitions()).collect();
        self.received.extend(frames);
        edges
    }

    /// Text the module sent at `baud` (frames at other speeds — the ESP boot log — skipped).
    pub fn received_text(&self, baud: u32) -> String {
        let b: Vec<u8> = self.received.iter().filter(|f| f.config.baud == baud).map(|f| f.data as u8).collect();
        String::from_utf8_lossy(&b).into_owned()
    }

    /// Text the MCU sent.
    pub fn sent_text(&self) -> String {
        let b: Vec<u8> = self.sent.iter().map(|f| f.data as u8).collect();
        String::from_utf8_lossy(&b).into_owned()
    }
}

impl RadioSpace {
    /// Serves one host-board service request (`HostIo::service` in `bb-firmware`): Wi-Fi,
    /// sockets and BLE for an ESP32 registered with [`RadioSpace::add_host_radio`]. See
    /// [`crate::host`] for the byte format.
    pub fn host_service(&mut self, id: DeviceId, channel: u8, request: &[u8], now: SimTime) -> Vec<u8> {
        self.advance(now);
        let RadioSpace { net, ble, slots, .. } = self;
        let reply = match slots.get_mut(id.0 as usize).map(|s| &mut s.module) {
            Some(Module::Host(h)) => h.serve(net, ble, channel, request, now),
            _ => {
                let mut w = crate::host::Writer::default();
                w.i32(-1);
                w.done()
            }
        };
        self.collect_notes();
        reply
    }
}
