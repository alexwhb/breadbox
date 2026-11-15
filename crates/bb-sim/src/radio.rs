//! The bench's virtual RF space (`RF-*` × `bb-sim`): [`RadioService`] wraps one
//! `bb_radio::RadioSpace` per bench and is created by [`service_factory`] from the project's
//! `radios` settings (access points, network conditions, floor plan, LAN bridge, LoRa).
//!
//! * UART radio modules (ESP-01, HM-10, HC-05, RYLR896) are bench devices
//!   (`devices::radio::RadioModule`) that decode their RX pin with exact bit timing, hand the
//!   bytes to their modem here and drive their TX pin with the modem's frames. Frames produced
//!   while the service steps wake the device through [`crate::Service::take_device_wakes`].
//! * API-level boards (ESP32) get a host radio on their first Wi-Fi / BLE request
//!   ([`crate::Service::host_service`]).
//! * Radio annotations (`wifi: …`, `mqtt: PUB …`) become [`crate::SimEvent`]s from the module's
//!   designator; the bench routes them to the serial monitor of the MCU wired to it (RF-8).
//! * The Radios panel (RF-7) and the virtual phone read [`RadioTelemetry`] from the telemetry
//!   snapshot and act through [`RadioCommand`] (`Command::Radio`).

use crate::bench::{Service, ServiceFactory, ServiceNote};
use bb_circuit::Project;
use bb_core::SimTime;
use bb_radio::ble::{BtKind, PeripheralId, PhoneEvent};
use bb_radio::bridge::{BridgeConfig, BridgeStatus};
use bb_radio::espat::EspAtConfig;
use bb_radio::geo::{Pos, Wall};
use bb_radio::mqtt::Received;
use bb_radio::net::{AccessPoint, NetConditions, NetConfig};
use bb_radio::space::{Module, RadioSettings};
use bb_radio::uart::{UartConfig, UartFrame};
use bb_radio::{DeviceId, ModulePin, RadioSpace};
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

/// Device wake token a [`RadioService`] uses to tell a module device that frames are waiting.
pub const TOKEN_RADIO_TX: u64 = 0x5241_4449_4f00;

/// Where the bench sits on the floor plan by default: 3.2 m from the access point, behind one
/// interior wall — the design's `-54 dBm (3.2 m, 1 wall)`.
pub const DEFAULT_BENCH_POS: Pos = Pos { x: 3.2, y: 0.0 };

/// Which modem a module device runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleKind {
    /// ESP-01 (ESP8266, ESP-AT) at its configured default baud.
    Esp01 {
        baud: u32,
    },
    Hm10,
    Hc05,
    Rylr896,
}

/// `project.json` → `radios`: the radio space settings plus where the bench stands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectRadios {
    #[serde(flatten)]
    pub settings: RadioSettings,
    /// Position of the bench (and every radio on it) on the floor plan.
    #[serde(default = "default_bench_pos")]
    pub bench_pos: Pos,
}

fn default_bench_pos() -> Pos {
    DEFAULT_BENCH_POS
}

impl Default for ProjectRadios {
    fn default() -> Self {
        let mut settings = RadioSpace::default().settings();
        // one interior wall between the AP (origin) and the bench
        settings
            .walls
            .push(Wall::interior(Pos::new(1.5, -5.0), Pos::new(1.5, 5.0)));
        ProjectRadios {
            settings,
            bench_pos: DEFAULT_BENCH_POS,
        }
    }
}

impl ProjectRadios {
    /// The project's settings (defaults when absent or unreadable).
    pub fn from_project(p: &Project) -> ProjectRadios {
        if p.radios.is_null() {
            return ProjectRadios::default();
        }
        serde_json::from_value(p.radios.clone()).unwrap_or_default()
    }
    /// Writes the settings back into a project (`radios`).
    pub fn store(&self, p: &mut Project) {
        p.radios = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
    }
}

/// A service factory building the radio space from a project's settings (part of
/// `BenchOptions::default()`).
pub fn service_factory() -> ServiceFactory {
    Arc::new(|p: &Project| Box::new(RadioService::for_project(p)) as Box<dyn Service>)
}

/// One MQTT message seen by the explorer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MqttRow {
    pub time: SimTime,
    pub topic: String,
    pub payload: String,
    pub qos: u8,
    pub retain: bool,
}

/// One packet-log row as the panel shows it (`01.870 tx DHCP REQUEST 10.0.0.42`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PacketRow {
    pub seq: u64,
    pub time: SimTime,
    /// `01.870`.
    pub time_label: String,
    /// `tx` / `rx` / `!`.
    pub dir: String,
    pub text: String,
    /// Amber fault-injector rows.
    pub dropped: bool,
    pub radio: Option<String>,
    /// `Wi-Fi`, `MQTT`, `BLE`, `LoRa`, …
    pub proto: String,
}

/// Radio card (RF-7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardView {
    pub device: u32,
    /// Designator (`U2`).
    pub label: String,
    /// `ESP8266 Wi-Fi`.
    pub title: String,
    /// `connected`, `joining`, `idle`, `off`, `advertising`, …
    pub status: String,
    pub rows: Vec<(String, String)>,
}

/// A device in the virtual phone's scan list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanRow {
    pub id: u32,
    pub name: String,
    pub addr: String,
    pub rssi: i32,
    /// `BLE` or `SPP`.
    pub kind: String,
    pub services: Vec<String>,
}

/// A GATT characteristic as the phone shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharView {
    pub handle: u16,
    pub uuid: String,
    /// `read`, `write`, `write-no-response`, `notify`, `indicate`.
    pub props: Vec<String>,
    pub value: Vec<u8>,
    pub value_text: String,
    pub notifying: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceView {
    pub uuid: String,
    pub characteristics: Vec<CharView>,
}

/// One line of the phone's activity log / Bluetooth terminal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhoneRow {
    pub time: SimTime,
    /// `connected`, `disconnected`, `notify`, `read`, `write`, `terminal`, `error`.
    pub kind: String,
    pub device: Option<u32>,
    pub handle: Option<u16>,
    pub data: Vec<u8>,
    pub text: String,
}

/// The virtual phone (RF-6) as the UI renders it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PhoneView {
    pub scan: Vec<ScanRow>,
    pub connected: Option<u32>,
    pub connected_name: Option<String>,
    pub services: Vec<ServiceView>,
    pub log: Vec<PhoneRow>,
    /// Bluetooth serial terminal text received so far.
    pub terminal: String,
}

/// Everything the Radios panel (RF-7) and the virtual phone need for one frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RadioTelemetry {
    pub cards: Vec<CardView>,
    /// The last rows of the packet log (TCP pure ACKs hidden).
    pub log: Vec<PacketRow>,
    pub conditions: NetConditions,
    pub access_points: Vec<AccessPoint>,
    /// A BLE / SPP radio exists on the bench (otherwise the panel shows the
    /// "No BLE radio on this bench…" empty state).
    pub has_ble: bool,
    pub has_lora: bool,
    pub phone: PhoneView,
    pub mqtt: Vec<MqttRow>,
    pub explorer_subscriptions: Vec<String>,
    pub lan_bridge: Option<BridgeConfig>,
    /// `connecting`, `connected`, `disconnected: <why>`, `stopped`.
    pub lan_bridge_status: Option<String>,
    pub bench_pos: Pos,
}

/// Commands from the Radios panel / phone (`Command::Radio`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RadioCommand {
    SetConditions(NetConditions),
    SetAccessPoints(Vec<AccessPoint>),
    SetLanBridge(Option<BridgeConfig>),
    SetBenchPos(Pos),
    ApplySettings(RadioSettings),
    ExplorerSubscribe(String),
    ExplorerUnsubscribe(String),
    ExplorerPublish {
        topic: String,
        payload: Vec<u8>,
        qos: u8,
        retain: bool,
    },
    ClearLog,
    Phone(PhoneCommand),
}

/// Virtual-phone actions (RF-6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PhoneCommand {
    Connect { id: u32, pin: Option<String> },
    Disconnect,
    Read { handle: u16 },
    Write { handle: u16, data: Vec<u8> },
    Subscribe { handle: u16, on: bool },
    TerminalOpen,
    TerminalSend(Vec<u8>),
}

const LOG_ROWS: usize = 200;
const PHONE_ROWS: usize = 200;
const MQTT_ROWS: usize = 200;

/// The radio space as a bench service.
pub struct RadioService {
    pub space: RadioSpace,
    /// API-level board part id → its radio.
    hosts: HashMap<String, DeviceId>,
    /// Module part id → radio.
    modules: BTreeMap<String, DeviceId>,
    /// Part id → designator.
    refs: HashMap<String, String>,
    bench_pos: Pos,
    tx: HashMap<DeviceId, Vec<UartFrame>>,
    wakes: Vec<(String, SimTime, u64)>,
    notes: Vec<ServiceNote>,
    phone_log: VecDeque<PhoneRow>,
    terminal: Vec<u8>,
    mqtt_log: VecDeque<MqttRow>,
    subscriptions: Vec<String>,
    uart_log: HashMap<DeviceId, VecDeque<UartByte>>,
    active: bool,
}

/// One byte on a module's UART (logic-analyser / "UART spy" view).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UartByte {
    /// Stop-bit sample time (MCU → module) or start-bit edge (module → MCU).
    pub time: SimTime,
    /// `true` = MCU → module (the module's RX pin).
    pub to_module: bool,
    pub byte: u8,
}

const UART_LOG: usize = 16_384;

impl RadioService {
    /// A bare service around an existing space (no project settings).
    pub fn new(space: RadioSpace) -> RadioService {
        RadioService {
            space,
            hosts: HashMap::new(),
            modules: BTreeMap::new(),
            refs: HashMap::new(),
            bench_pos: DEFAULT_BENCH_POS,
            tx: HashMap::new(),
            wakes: Vec::new(),
            notes: Vec::new(),
            phone_log: VecDeque::new(),
            terminal: Vec::new(),
            mqtt_log: VecDeque::new(),
            subscriptions: Vec::new(),
            uart_log: HashMap::new(),
            active: false,
        }
    }

    fn log_uart(&mut self, id: DeviceId, b: UartByte) {
        let l = self.uart_log.entry(id).or_default();
        l.push_back(b);
        while l.len() > UART_LOG {
            l.pop_front();
        }
    }

    /// Bytes seen on a module part's UART pins (both directions, time ordered per direction).
    pub fn uart_transcript(&self, part: &str) -> Vec<UartByte> {
        self.modules
            .get(part)
            .and_then(|id| self.uart_log.get(id))
            .map(|l| l.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Text the MCU sent to a module part.
    pub fn uart_sent_text(&self, part: &str) -> String {
        let b: Vec<u8> = self
            .uart_transcript(part)
            .iter()
            .filter(|x| x.to_module)
            .map(|x| x.byte)
            .collect();
        String::from_utf8_lossy(&b).into_owned()
    }

    /// Text a module part sent to the MCU at `baud` (`None` = every speed).
    pub fn uart_received_text(&self, part: &str) -> String {
        let b: Vec<u8> = self
            .uart_transcript(part)
            .iter()
            .filter(|x| !x.to_module)
            .map(|x| x.byte)
            .collect();
        String::from_utf8_lossy(&b).into_owned()
    }

    /// The radio space for a project: its `radios` settings (or the default bench: `SIM-AP`,
    /// one wall, bench 3.2 m away), seeded from the project seed.
    pub fn for_project(p: &Project) -> RadioService {
        let cfg = NetConfig {
            seed: NetConfig::default().seed ^ p.settings.seed,
            ..NetConfig::default()
        };
        let mut space = RadioSpace::new(cfg);
        let r = ProjectRadios::from_project(p);
        space.apply_settings(&r.settings);
        let mut s = RadioService::new(space);
        s.bench_pos = r.bench_pos;
        s.refs = p
            .parts
            .iter()
            .map(|x| (x.id.clone(), x.reference.clone()))
            .collect();
        s
    }

    fn reference(&self, part: &str) -> String {
        self.refs
            .get(part)
            .cloned()
            .unwrap_or_else(|| part.to_ascii_uppercase())
    }

    /// Gives the board part `part` (id, e.g. `"u1"`) a Wi-Fi + BLE radio at `pos`.
    pub fn add_host(&mut self, part: &str, label: &str, pos: Pos) -> DeviceId {
        self.active = true;
        let id = self.space.add_host_radio(label, pos);
        self.hosts.insert(part.to_string(), id);
        id
    }
    pub fn host(&self, part: &str) -> Option<DeviceId> {
        self.hosts.get(part).copied()
    }

    /// Whether anything radio exists on this bench (modules, host radios, explorer, bridge).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Radio of a module part (`"u2"`).
    pub fn module(&self, part: &str) -> Option<DeviceId> {
        self.modules.get(part).copied()
    }

    /// Registers (or finds, after a rebuild) the modem behind a module part.
    pub fn attach(&mut self, part: &str, kind: ModuleKind) -> DeviceId {
        if let Some(id) = self.modules.get(part) {
            return *id;
        }
        self.active = true;
        let label = self.reference(part);
        let pos = self.bench_pos;
        let id = match kind {
            ModuleKind::Esp01 { baud } => {
                self.space
                    .add_esp01(&label, pos, EspAtConfig::with_baud(baud.max(300)))
            }
            ModuleKind::Hm10 => self.space.add_hm10(&label, pos),
            ModuleKind::Hc05 => self.space.add_hc05(&label, pos),
            ModuleKind::Rylr896 => self.space.add_rylr896(&label, pos),
        };
        self.modules.insert(part.to_string(), id);
        id
    }

    /// Module supply on/off.
    pub fn power(&mut self, id: DeviceId, on: bool, at: SimTime) {
        self.space.power(id, on, at);
        self.collect(at);
    }

    pub fn set_pin(&mut self, id: DeviceId, pin: ModulePin, high: bool, at: SimTime) {
        self.space.set_pin(id, pin, high, at);
        self.collect(at);
    }

    /// UART format the module listens at now.
    pub fn uart_config(&self, id: DeviceId) -> Option<UartConfig> {
        self.space.uart_config(id)
    }

    /// A byte from the MCU arrived at the module at `at`.
    pub fn uart_rx(&mut self, id: DeviceId, byte: u8, at: SimTime) {
        self.log_uart(
            id,
            UartByte {
                time: at,
                to_module: true,
                byte,
            },
        );
        self.space.uart_rx(id, byte, at);
        self.collect(at);
    }

    /// Frames the module wants on its TX pin (buffered + new).
    pub fn take_frames(&mut self, id: DeviceId) -> Vec<UartFrame> {
        let mut v = self.tx.remove(&id).unwrap_or_default();
        v.extend(self.space.take_uart_tx(id));
        v.sort_by_key(|f| f.start);
        for f in &v {
            self.log_uart(
                id,
                UartByte {
                    time: f.start,
                    to_module: false,
                    byte: f.data as u8,
                },
            );
        }
        v
    }

    /// Whether the phone is connected to this module's peripheral (HM-10 / HC-05 STATE pin).
    pub fn link_up(&self, id: DeviceId) -> bool {
        let p = match self.space.module(id) {
            Some(Module::Hm10(m)) => m.peripheral,
            Some(Module::Hc05(m)) => m.device,
            Some(Module::Esp01(m)) => {
                return self.space.esp01(id).is_some_and(|_| {
                    self.space
                        .net
                        .station_info(m.station)
                        .is_some_and(|i| i.ip.is_some())
                });
            }
            _ => return false,
        };
        self.space.ble.is_connected(p)
    }

    /// Card status text of a radio (`connected`, …).
    pub fn status(&self, id: DeviceId) -> String {
        self.space
            .cards()
            .into_iter()
            .find(|c| c.device == id)
            .map(|c| c.status)
            .unwrap_or_default()
    }

    /// Drains modem output, annotations, phone events and explorer messages.
    fn collect(&mut self, now: SimTime) {
        let parts: Vec<(String, DeviceId)> = self
            .modules
            .iter()
            .map(|(p, id)| (p.clone(), *id))
            .collect();
        for (part, id) in parts {
            let frames = self.space.take_uart_tx(id);
            if !frames.is_empty() {
                let at = frames.iter().map(|f| f.start).min().unwrap_or(now).min(now);
                self.tx.entry(id).or_default().extend(frames);
                self.wakes.push((part, at, TOKEN_RADIO_TX));
            }
        }
        for a in self.space.take_annotations() {
            let part = a.device.and_then(|d| {
                self.modules
                    .iter()
                    .chain(self.hosts.iter())
                    .find(|(_, id)| **id == d)
                    .map(|(p, _)| p.clone())
            });
            self.notes.push(ServiceNote {
                time: a.time,
                part,
                source: a.label.clone(),
                channel: a.kind.prefix().to_string(),
                text: a.text,
            });
        }
        for (t, e) in self.space.ble.take_phone_events() {
            self.phone_event(t, e);
        }
        if !self.subscriptions.is_empty() {
            for r in self.space.explorer_take() {
                self.push_mqtt(r);
            }
        }
    }

    fn push_mqtt(&mut self, r: Received) {
        self.mqtt_log.push_back(MqttRow {
            time: r.time,
            topic: r.message.topic.clone(),
            payload: String::from_utf8_lossy(&r.message.payload).into_owned(),
            qos: r.message.qos,
            retain: r.message.retain,
        });
        while self.mqtt_log.len() > MQTT_ROWS {
            self.mqtt_log.pop_front();
        }
    }

    fn phone_row(&mut self, row: PhoneRow) {
        self.phone_log.push_back(row);
        while self.phone_log.len() > PHONE_ROWS {
            self.phone_log.pop_front();
        }
    }

    fn phone_event(&mut self, time: SimTime, e: PhoneEvent) {
        let text = |d: &[u8]| String::from_utf8_lossy(d).into_owned();
        let row = match e {
            PhoneEvent::Connected { id } => PhoneRow {
                time,
                kind: "connected".into(),
                device: Some(id.0),
                handle: None,
                data: vec![],
                text: format!("connected to {}", self.space.ble.name(id).unwrap_or("?")),
            },
            PhoneEvent::ConnectFailed { id, reason } => PhoneRow {
                time,
                kind: "error".into(),
                device: Some(id.0),
                handle: None,
                data: vec![],
                text: format!("connect failed: {reason}"),
            },
            PhoneEvent::Disconnected { id } => PhoneRow {
                time,
                kind: "disconnected".into(),
                device: Some(id.0),
                handle: None,
                data: vec![],
                text: "disconnected".into(),
            },
            PhoneEvent::Notification { id, handle, data } => PhoneRow {
                time,
                kind: "notify".into(),
                device: Some(id.0),
                handle: Some(handle),
                text: text(&data),
                data,
            },
            PhoneEvent::ReadResult { id, handle, data } => PhoneRow {
                time,
                kind: "read".into(),
                device: Some(id.0),
                handle: Some(handle),
                text: text(&data),
                data,
            },
            PhoneEvent::WriteDone { id, handle } => PhoneRow {
                time,
                kind: "write".into(),
                device: Some(id.0),
                handle: Some(handle),
                data: vec![],
                text: "write ok".into(),
            },
            PhoneEvent::Terminal { id, data } => {
                self.terminal.extend_from_slice(&data);
                if self.terminal.len() > 16_384 {
                    let cut = self.terminal.len() - 16_384;
                    self.terminal.drain(..cut);
                }
                PhoneRow {
                    time,
                    kind: "terminal".into(),
                    device: Some(id.0),
                    handle: None,
                    text: text(&data),
                    data,
                }
            }
        };
        self.phone_row(row);
    }

    /// Applies a panel / phone command at bench time `now`.
    pub fn command(&mut self, cmd: RadioCommand, now: SimTime) -> Result<(), String> {
        self.space.advance(now);
        let r = match cmd {
            RadioCommand::SetConditions(c) => {
                self.space.net.set_conditions(c);
                Ok(())
            }
            RadioCommand::SetAccessPoints(aps) => {
                *self.space.net.access_points_mut() = aps;
                Ok(())
            }
            RadioCommand::SetLanBridge(b) => {
                self.space.set_lan_bridge(b);
                self.active = true;
                Ok(())
            }
            RadioCommand::SetBenchPos(pos) => {
                self.bench_pos = pos;
                let ids: Vec<DeviceId> = self
                    .modules
                    .values()
                    .chain(self.hosts.values())
                    .copied()
                    .collect();
                for id in ids {
                    self.space.set_position(id, pos);
                }
                Ok(())
            }
            RadioCommand::ApplySettings(s) => {
                self.space.apply_settings(&s);
                Ok(())
            }
            RadioCommand::ExplorerSubscribe(f) => {
                self.active = true;
                if !self.subscriptions.contains(&f) {
                    self.subscriptions.push(f.clone());
                }
                self.space.explorer_subscribe(&f);
                Ok(())
            }
            RadioCommand::ExplorerUnsubscribe(f) => {
                self.subscriptions.retain(|x| *x != f);
                self.space.explorer_unsubscribe(&f);
                Ok(())
            }
            RadioCommand::ExplorerPublish {
                topic,
                payload,
                qos,
                retain,
            } => {
                self.active = true;
                self.space.explorer_publish(&topic, &payload, qos, retain);
                Ok(())
            }
            RadioCommand::ClearLog => {
                self.space.net.log_mut().clear();
                self.space.ble.log.clear();
                self.space.lora.log.clear();
                self.phone_log.clear();
                self.mqtt_log.clear();
                Ok(())
            }
            RadioCommand::Phone(p) => self.phone(p, now),
        };
        self.collect(now);
        r
    }

    fn phone(&mut self, p: PhoneCommand, now: SimTime) -> Result<(), String> {
        let ble = &mut self.space.ble;
        let r = match p {
            PhoneCommand::Connect { id, pin } => ble.connect(PeripheralId(id), pin.as_deref(), now),
            PhoneCommand::Disconnect => {
                ble.disconnect(now);
                Ok(())
            }
            PhoneCommand::Read { handle } => ble.read(handle, now),
            PhoneCommand::Write { handle, data } => ble.write(handle, &data, now),
            PhoneCommand::Subscribe { handle, on } => ble.subscribe(handle, on, now),
            PhoneCommand::TerminalOpen => ble.terminal_open(now),
            PhoneCommand::TerminalSend(d) => ble.terminal_send(&d, now),
        };
        if let Err(e) = &r {
            self.phone_row(PhoneRow {
                time: now,
                kind: "error".into(),
                device: None,
                handle: None,
                data: vec![],
                text: e.to_string(),
            });
        }
        r.map_err(|e| e.to_string())
    }

    /// Current settings for the project file.
    pub fn project_settings(&self) -> ProjectRadios {
        ProjectRadios {
            settings: self.space.settings(),
            bench_pos: self.bench_pos,
        }
    }

    /// Packet log export (`RF-4`): pcap-ng bytes.
    pub fn export_pcapng(&self) -> Vec<u8> {
        self.space.net.log().to_pcapng()
    }
    /// Packet log export as CSV.
    pub fn export_csv(&self) -> String {
        self.space.net.log().to_csv()
    }

    /// Snapshot for the panel.
    pub fn telemetry(&self) -> RadioTelemetry {
        let cards = self
            .space
            .cards()
            .into_iter()
            .map(|c| CardView {
                device: c.device.0,
                label: c.label,
                title: c.title,
                status: c.status,
                rows: c.rows,
            })
            .collect();
        let rows = self.space.log_rows();
        let visible: Vec<_> = rows.into_iter().filter(|e| !e.ack_only).collect();
        let start = visible.len().saturating_sub(LOG_ROWS);
        let log = visible[start..]
            .iter()
            .map(|e| PacketRow {
                seq: e.seq,
                time: e.time,
                time_label: e.time_label(),
                dir: e.dir.label().to_string(),
                text: e.text(),
                dropped: e.dropped.is_some(),
                radio: e.radio.clone(),
                proto: e.proto.label().to_string(),
            })
            .collect();
        let mut has_ble = false;
        let mut has_lora = false;
        for (id, _) in self.space.devices() {
            match self.space.module(id) {
                Some(Module::Hm10(_)) | Some(Module::Hc05(_)) | Some(Module::Host(_)) => {
                    has_ble = true
                }
                Some(Module::Rylr896(_)) => has_lora = true,
                _ => {}
            }
        }
        let ble = &self.space.ble;
        let scan = ble
            .scan()
            .into_iter()
            .map(|e| ScanRow {
                id: e.id.0,
                name: e.name,
                addr: e.addr,
                rssi: e.rssi,
                kind: match e.kind {
                    BtKind::Le => "BLE".into(),
                    BtKind::Classic => "SPP".into(),
                },
                services: e.services.iter().map(|u| u.to_string()).collect(),
            })
            .collect();
        let connected = ble.phone_connected();
        let services = ble
            .discover()
            .map(|svcs| {
                svcs.into_iter()
                    .map(|s| ServiceView {
                        uuid: s.uuid.to_string(),
                        characteristics: s
                            .characteristics
                            .into_iter()
                            .map(|c| CharView {
                                handle: c.handle,
                                uuid: c.uuid.to_string(),
                                props: prop_names(c.props),
                                value_text: String::from_utf8_lossy(&c.value).into_owned(),
                                value: c.value,
                                notifying: c.notifying,
                                description: c.description,
                            })
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        RadioTelemetry {
            cards,
            log,
            conditions: self.space.net.conditions().clone(),
            access_points: self.space.net.access_points().to_vec(),
            has_ble,
            has_lora,
            phone: PhoneView {
                scan,
                connected: connected.map(|c| c.0),
                connected_name: connected.and_then(|c| ble.name(c).map(String::from)),
                services,
                log: self.phone_log.iter().cloned().collect(),
                terminal: String::from_utf8_lossy(&self.terminal).into_owned(),
            },
            mqtt: self.mqtt_log.iter().cloned().collect(),
            explorer_subscriptions: self.subscriptions.clone(),
            lan_bridge: self.space.settings().lan_bridge,
            lan_bridge_status: self.space.lan_bridge_status().map(|s| match s {
                BridgeStatus::Connecting => "connecting".to_string(),
                BridgeStatus::Connected => "connected".to_string(),
                BridgeStatus::Disconnected(w) => format!("disconnected: {w}"),
                BridgeStatus::Stopped => "stopped".to_string(),
            }),
            bench_pos: self.bench_pos,
        }
    }
}

fn prop_names(p: u8) -> Vec<String> {
    let mut v = Vec::new();
    for (bit, name) in [
        (0x02, "read"),
        (0x04, "write-no-response"),
        (0x08, "write"),
        (0x10, "notify"),
        (0x20, "indicate"),
    ] {
        if p & bit != 0 {
            v.push(name.to_string());
        }
    }
    v
}

impl Service for RadioService {
    fn name(&self) -> &str {
        "radio"
    }
    fn next_wake(&self) -> Option<SimTime> {
        if self.active {
            self.space.next_wake()
        } else {
            None
        }
    }
    fn step(&mut self, now: SimTime) {
        self.space.advance(now);
        self.collect(now);
    }
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
    fn host_service(
        &mut self,
        part: &str,
        channel: u8,
        request: &[u8],
        at: SimTime,
    ) -> Option<Vec<u8>> {
        let id = match self.hosts.get(part) {
            Some(id) => *id,
            None => {
                let label = self.reference(part);
                let pos = self.bench_pos;
                self.add_host(part, &label, pos)
            }
        };
        let r = self.space.host_service(id, channel, request, at);
        self.collect(at);
        Some(r)
    }
    fn take_device_wakes(&mut self) -> Vec<(String, SimTime, u64)> {
        std::mem::take(&mut self.wakes)
    }
    fn take_notes(&mut self) -> Vec<ServiceNote> {
        std::mem::take(&mut self.notes)
    }
}
