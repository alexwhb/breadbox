//! Bluetooth (`RF-6`): a GATT model, the BLE air between peripherals on the bench (HM-10,
//! ESP32 BLE servers) and the **virtual phone** acting as central — scan with RSSI, connect,
//! discover services, read / write / notify, plus a Bluetooth serial terminal that talks to the
//! HM-10's transparent UART characteristic (FFE1), the Nordic UART service, or an HC-05's SPP
//! link (classic Bluetooth, paired with its PIN).
//!
//! Timing: connection setup ~ 40–90 ms, one connection event every 30 ms (writes, reads and
//! notifications are delivered at connection events), ATT MTU 23 → 20-byte payloads.

mod hc05;
mod hm10;

pub use hc05::Hc05;
pub use hm10::Hm10;

use crate::geo::{Geometry, PathLoss, Pos};
use crate::log::{Dir, LogEntry, PacketLog, Proto};
use crate::rng::Pcg32;
use bb_core::SimTime;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};
use std::fmt;
use thiserror::Error;

/// A Bluetooth UUID (16-bit SIG-assigned or 128-bit custom).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Uuid {
    U16(u16),
    U128([u8; 16]),
}

impl Uuid {
    const BASE: [u8; 16] = [
        0, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0x80, 0x5f, 0x9b, 0x34, 0xfb,
    ];

    /// Parses `"FFE1"`, `"0xFFE1"` or a 128-bit `"6e400001-b5a3-f393-e0a9-e50e24dcca9e"`.
    pub fn parse(s: &str) -> Option<Uuid> {
        let t = s.trim().trim_start_matches("0x").trim_start_matches("0X");
        if t.len() <= 4 {
            return u16::from_str_radix(t, 16).ok().map(Uuid::U16);
        }
        let hex: String = t.chars().filter(|c| *c != '-').collect();
        if hex.len() != 32 {
            return None;
        }
        let mut b = [0u8; 16];
        for (i, byte) in b.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        }
        // A 128-bit form of a SIG UUID collapses to 16 bits.
        if b[0] == 0 && b[1] == 0 && b[4..] == Self::BASE[4..] {
            return Some(Uuid::U16(u16::from_be_bytes([b[2], b[3]])));
        }
        Some(Uuid::U128(b))
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Uuid::U16(v) => write!(f, "{v:04X}"),
            Uuid::U128(b) => write!(
                f,
                "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                b[0],
                b[1],
                b[2],
                b[3],
                b[4],
                b[5],
                b[6],
                b[7],
                b[8],
                b[9],
                b[10],
                b[11],
                b[12],
                b[13],
                b[14],
                b[15]
            ),
        }
    }
}

/// Characteristic properties (bit flags as in the GATT declaration).
pub mod props {
    pub const READ: u8 = 0x02;
    pub const WRITE_NO_RESPONSE: u8 = 0x04;
    pub const WRITE: u8 = 0x08;
    pub const NOTIFY: u8 = 0x10;
    pub const INDICATE: u8 = 0x20;
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Characteristic {
    pub handle: u16,
    pub uuid: Uuid,
    pub props: u8,
    pub value: Vec<u8>,
    /// Client Characteristic Configuration (notifications enabled by the phone).
    pub notifying: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Service {
    pub uuid: Uuid,
    pub characteristics: Vec<Characteristic>,
}

/// A peripheral's attribute table.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Gatt {
    pub services: Vec<Service>,
    next_handle: u16,
}

impl Gatt {
    pub fn add_service(&mut self, uuid: Uuid) -> usize {
        self.services.push(Service {
            uuid,
            characteristics: vec![],
        });
        self.services.len() - 1
    }

    /// Adds a characteristic to service `svc`; returns its value handle.
    pub fn add_characteristic(&mut self, svc: usize, uuid: Uuid, props: u8, value: &[u8]) -> u16 {
        self.next_handle += 3; // declaration, value, CCCD
        let handle = self.next_handle;
        if let Some(s) = self.services.get_mut(svc) {
            s.characteristics.push(Characteristic {
                handle,
                uuid,
                props,
                value: value.to_vec(),
                notifying: false,
                description: None,
            });
        }
        handle
    }

    pub fn char(&self, handle: u16) -> Option<&Characteristic> {
        self.services
            .iter()
            .flat_map(|s| &s.characteristics)
            .find(|c| c.handle == handle)
    }

    pub fn char_mut(&mut self, handle: u16) -> Option<&mut Characteristic> {
        self.services
            .iter_mut()
            .flat_map(|s| &mut s.characteristics)
            .find(|c| c.handle == handle)
    }

    /// Value handle of the first characteristic with `uuid`.
    pub fn find(&self, uuid: Uuid) -> Option<u16> {
        self.services
            .iter()
            .flat_map(|s| &s.characteristics)
            .find(|c| c.uuid == uuid)
            .map(|c| c.handle)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PeripheralId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BtKind {
    /// Bluetooth Low Energy peripheral (GATT).
    Le,
    /// Classic Bluetooth SPP (HC-05).
    Classic,
}

/// What the phone did to a peripheral (consumed by the module / host board owning it).
#[derive(Debug, Clone, PartialEq)]
pub enum PeripheralEvent {
    Connected,
    Disconnected,
    /// The phone wrote `data` to the characteristic `handle`.
    Write {
        handle: u16,
        data: Vec<u8>,
    },
    /// The phone enabled/disabled notifications.
    Subscribed {
        handle: u16,
        on: bool,
    },
    /// The phone read `handle` (value already served from the table).
    Read {
        handle: u16,
    },
    /// Classic SPP bytes from the phone.
    SppData(Vec<u8>),
}

#[derive(Debug, Clone)]
struct Peripheral {
    label: String,
    kind: BtKind,
    pos: Pos,
    name: String,
    addr: [u8; 6],
    tx_dbm: f64,
    advertising: bool,
    powered: bool,
    gatt: Gatt,
    connected: bool,
    pin: String,
    paired: bool,
    events: VecDeque<PeripheralEvent>,
}

/// One row of the phone's scan list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanEntry {
    pub id: PeripheralId,
    pub name: String,
    pub addr: String,
    pub rssi: i32,
    pub kind: BtKind,
    /// Advertised service UUIDs.
    pub services: Vec<Uuid>,
}

/// Results the phone UI shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum PhoneEvent {
    Connected {
        id: PeripheralId,
    },
    ConnectFailed {
        id: PeripheralId,
        reason: String,
    },
    Disconnected {
        id: PeripheralId,
    },
    Notification {
        id: PeripheralId,
        handle: u16,
        data: Vec<u8>,
    },
    ReadResult {
        id: PeripheralId,
        handle: u16,
        data: Vec<u8>,
    },
    WriteDone {
        id: PeripheralId,
        handle: u16,
    },
    /// Bytes arrived in the Bluetooth serial terminal.
    Terminal {
        id: PeripheralId,
        data: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BleError {
    #[error("no such device")]
    NoDevice,
    #[error("device out of range")]
    OutOfRange,
    #[error("not connected")]
    NotConnected,
    #[error("no such characteristic")]
    NoCharacteristic,
    #[error("operation not permitted by the characteristic")]
    NotPermitted,
    #[error("wrong PIN")]
    WrongPin,
}

#[derive(Debug, Clone)]
enum BleEv {
    ConnectDone {
        p: PeripheralId,
    },
    Write {
        p: PeripheralId,
        handle: u16,
        data: Vec<u8>,
        response: bool,
    },
    Read {
        p: PeripheralId,
        handle: u16,
    },
    Notify {
        p: PeripheralId,
        handle: u16,
        data: Vec<u8>,
    },
    Subscribe {
        p: PeripheralId,
        handle: u16,
        on: bool,
    },
    SppToPeripheral {
        p: PeripheralId,
        data: Vec<u8>,
    },
    SppToPhone {
        p: PeripheralId,
        data: Vec<u8>,
    },
}

#[derive(Debug)]
struct Sched {
    at: SimTime,
    seq: u64,
    ev: BleEv,
}
impl PartialEq for Sched {
    fn eq(&self, o: &Self) -> bool {
        self.at == o.at && self.seq == o.seq
    }
}
impl Eq for Sched {}
impl PartialOrd for Sched {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Sched {
    fn cmp(&self, o: &Self) -> Ordering {
        o.at.cmp(&self.at).then(o.seq.cmp(&self.seq))
    }
}

/// The BLE air + the virtual phone.
#[derive(Debug)]
pub struct BleSpace {
    peripherals: Vec<Peripheral>,
    pub geometry: Geometry,
    pub path_loss: PathLoss,
    /// Where the phone is (metres).
    pub phone_pos: Pos,
    /// Phone's receive sensitivity (dBm).
    pub sensitivity_dbm: f64,
    /// Connection interval.
    pub conn_interval: SimTime,
    phone_conn: Option<PeripheralId>,
    phone_events: Vec<(SimTime, PhoneEvent)>,
    queue: BinaryHeap<Sched>,
    seq: u64,
    rng: Pcg32,
    now: SimTime,
    pub log: PacketLog,
}

/// HM-10 transparent UART service / characteristic.
pub const HM10_SERVICE: Uuid = Uuid::U16(0xFFE0);
pub const HM10_CHAR: Uuid = Uuid::U16(0xFFE1);
/// Nordic UART Service (ESP32 BLE-UART sketches): service, RX (phone writes), TX (notify).
pub const NUS_SERVICE: Uuid = Uuid::U128([
    0x6e, 0x40, 0x00, 0x01, 0xb5, 0xa3, 0xf3, 0x93, 0xe0, 0xa9, 0xe5, 0x0e, 0x24, 0xdc, 0xca, 0x9e,
]);
pub const NUS_RX: Uuid = Uuid::U128([
    0x6e, 0x40, 0x00, 0x02, 0xb5, 0xa3, 0xf3, 0x93, 0xe0, 0xa9, 0xe5, 0x0e, 0x24, 0xdc, 0xca, 0x9e,
]);
pub const NUS_TX: Uuid = Uuid::U128([
    0x6e, 0x40, 0x00, 0x03, 0xb5, 0xa3, 0xf3, 0x93, 0xe0, 0xa9, 0xe5, 0x0e, 0x24, 0xdc, 0xca, 0x9e,
]);

impl BleSpace {
    pub fn new(seed: u64) -> Self {
        BleSpace {
            peripherals: vec![],
            geometry: Geometry::default(),
            path_loss: PathLoss::BLE_ROOM,
            phone_pos: Pos::new(0.8, 0.5),
            sensitivity_dbm: -95.0,
            conn_interval: SimTime::from_ms(30),
            phone_conn: None,
            phone_events: vec![],
            queue: BinaryHeap::new(),
            seq: 0,
            rng: Pcg32::new(seed),
            now: SimTime::ZERO,
            log: PacketLog::default(),
        }
    }

    fn schedule(&mut self, at: SimTime, ev: BleEv) {
        self.seq += 1;
        self.queue.push(Sched {
            at: at.max(self.now),
            seq: self.seq,
            ev,
        });
    }

    /// Next connection event at or after `t` (phase-locked to the connection interval).
    fn conn_event(&mut self, t: SimTime) -> SimTime {
        let ci = self.conn_interval.as_ps().max(1);
        let next = (t.as_ps() / ci + 1) * ci;
        SimTime::from_ps(next) + SimTime::from_us(150 + self.rng.below(300) as u64)
    }

    fn per(&self, id: PeripheralId) -> Result<&Peripheral, BleError> {
        self.peripherals
            .get(id.0 as usize)
            .ok_or(BleError::NoDevice)
    }

    fn per_mut(&mut self, id: PeripheralId) -> Result<&mut Peripheral, BleError> {
        self.peripherals
            .get_mut(id.0 as usize)
            .ok_or(BleError::NoDevice)
    }

    fn log_row(&mut self, id: PeripheralId, dir: Dir, summary: String, bytes: usize) {
        let label = self.per(id).map(|p| p.label.clone()).ok();
        self.log.push(LogEntry {
            seq: 0,
            time: self.now,
            dir,
            radio: label,
            proto: Proto::Ble,
            summary,
            bytes,
            src: None,
            dst: None,
            dropped: None,
            ack_only: false,
            frame: None,
        });
    }

    // ================================================================ peripherals

    pub fn add_peripheral(&mut self, label: &str, pos: Pos) -> PeripheralId {
        self.add(label, pos, BtKind::Le)
    }

    pub fn add_classic(&mut self, label: &str, pos: Pos) -> PeripheralId {
        self.add(label, pos, BtKind::Classic)
    }

    fn add(&mut self, label: &str, pos: Pos, kind: BtKind) -> PeripheralId {
        let n = self.peripherals.len() as u8;
        let addr = match kind {
            BtKind::Le => [0xa4, 0xc1, 0x38, 0x12, 0x34, 0xa0 + n],
            BtKind::Classic => [0x98, 0xd3, 0x31, 0xfb, 0x12, 0x30 + n],
        };
        self.peripherals.push(Peripheral {
            label: label.into(),
            kind,
            pos,
            name: label.into(),
            addr,
            tx_dbm: 0.0,
            advertising: false,
            powered: false,
            gatt: Gatt::default(),
            connected: false,
            pin: "1234".into(),
            paired: false,
            events: VecDeque::new(),
        });
        PeripheralId(self.peripherals.len() as u32 - 1)
    }

    pub fn set_position(&mut self, id: PeripheralId, pos: Pos) {
        if let Ok(p) = self.per_mut(id) {
            p.pos = pos;
        }
    }

    pub fn set_name(&mut self, id: PeripheralId, name: &str) {
        if let Ok(p) = self.per_mut(id) {
            p.name = name.into();
        }
    }

    pub fn name(&self, id: PeripheralId) -> Option<&str> {
        self.per(id).ok().map(|p| p.name.as_str())
    }

    pub fn address(&self, id: PeripheralId) -> Option<[u8; 6]> {
        self.per(id).ok().map(|p| p.addr)
    }

    pub fn set_pin(&mut self, id: PeripheralId, pin: &str) {
        if let Ok(p) = self.per_mut(id) {
            p.pin = pin.into();
            p.paired = false;
        }
    }

    pub fn set_tx_power(&mut self, id: PeripheralId, dbm: f64) {
        if let Ok(p) = self.per_mut(id) {
            p.tx_dbm = dbm;
        }
    }

    /// Powers the radio; power-off drops the connection.
    pub fn set_powered(&mut self, id: PeripheralId, on: bool, now: SimTime) {
        self.advance(now);
        let Ok(p) = self.per_mut(id) else { return };
        p.powered = on;
        if !on {
            p.advertising = false;
            if p.connected {
                self.drop_link(id);
            }
        }
    }

    pub fn set_advertising(&mut self, id: PeripheralId, on: bool) {
        if let Ok(p) = self.per_mut(id) {
            p.advertising = on;
            if on {
                p.powered = true;
            }
        }
    }

    pub fn gatt(&self, id: PeripheralId) -> Option<&Gatt> {
        self.per(id).ok().map(|p| &p.gatt)
    }

    pub fn gatt_mut(&mut self, id: PeripheralId) -> Option<&mut Gatt> {
        self.per_mut(id).ok().map(|p| &mut p.gatt)
    }

    pub fn is_connected(&self, id: PeripheralId) -> bool {
        self.per(id).is_ok_and(|p| p.connected)
    }

    /// Sets a characteristic's value (served to subsequent reads).
    pub fn set_value(&mut self, id: PeripheralId, handle: u16, data: &[u8]) {
        if let Some(c) = self.gatt_mut(id).and_then(|g| g.char_mut(handle)) {
            c.value = data.to_vec();
        }
    }

    /// Sends a notification (if the phone subscribed). Values longer than 20 bytes are split.
    pub fn notify(&mut self, id: PeripheralId, handle: u16, data: &[u8], now: SimTime) -> bool {
        self.advance(now);
        let ok = self
            .per(id)
            .is_ok_and(|p| p.connected && p.gatt.char(handle).is_some_and(|c| c.notifying));
        if let Some(c) = self.gatt_mut(id).and_then(|g| g.char_mut(handle)) {
            c.value = data.to_vec();
        }
        if !ok {
            return false;
        }
        let mut t = self.conn_event(now);
        for chunk in data.chunks(20) {
            self.schedule(
                t,
                BleEv::Notify {
                    p: id,
                    handle,
                    data: chunk.to_vec(),
                },
            );
            t += SimTime::from_us(1_500); // several packets per connection event
        }
        true
    }

    /// Classic SPP: bytes from the module to the phone's terminal.
    pub fn spp_send(&mut self, id: PeripheralId, data: &[u8], now: SimTime) {
        self.advance(now);
        if !self.is_connected(id) || data.is_empty() {
            return;
        }
        let at = now + SimTime::from_ms(8 + self.rng.below(6) as u64);
        self.schedule(
            at,
            BleEv::SppToPhone {
                p: id,
                data: data.to_vec(),
            },
        );
    }

    /// The peripheral drops the link (module command / reset).
    pub fn peripheral_disconnect(&mut self, id: PeripheralId, now: SimTime) {
        self.advance(now);
        if self.is_connected(id) {
            self.drop_link(id);
        }
    }

    /// True when a peripheral has events its module has not taken yet (the space must poll the
    /// module now, e.g. `OK+LOST` after the phone hung up).
    pub fn has_peripheral_events(&self, id: PeripheralId) -> bool {
        self.per(id).is_ok_and(|p| !p.events.is_empty())
    }

    pub fn take_peripheral_events(&mut self, id: PeripheralId) -> Vec<PeripheralEvent> {
        self.per_mut(id)
            .map(|p| p.events.drain(..).collect())
            .unwrap_or_default()
    }

    fn drop_link(&mut self, id: PeripheralId) {
        if let Ok(p) = self.per_mut(id) {
            p.connected = false;
            for s in &mut p.gatt.services {
                for c in &mut s.characteristics {
                    c.notifying = false;
                }
            }
            p.events.push_back(PeripheralEvent::Disconnected);
        }
        if self.phone_conn == Some(id) {
            self.phone_conn = None;
            self.phone_events
                .push((self.now, PhoneEvent::Disconnected { id }));
        }
        self.log_row(id, Dir::Rx, "BLE disconnect".into(), 0);
    }

    // ================================================================ the phone

    /// RSSI the phone measures from a peripheral.
    pub fn rssi(&self, id: PeripheralId) -> Option<f64> {
        let p = self.per(id).ok()?;
        Some(
            self.geometry
                .link(&self.path_loss, p.tx_dbm, p.pos, self.phone_pos)
                .rssi_dbm,
        )
    }

    /// Devices the phone can see right now (strongest first): advertising LE peripherals and
    /// discoverable classic modules.
    pub fn scan(&self) -> Vec<ScanEntry> {
        let mut v: Vec<ScanEntry> = self
            .peripherals
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.powered && (p.advertising || p.kind == BtKind::Classic && !p.connected)
            })
            .filter_map(|(i, p)| {
                let id = PeripheralId(i as u32);
                let rssi = self.rssi(id)?;
                (rssi >= self.sensitivity_dbm).then(|| ScanEntry {
                    id,
                    name: p.name.clone(),
                    addr: crate::wire::mac_string(p.addr).to_ascii_uppercase(),
                    rssi: rssi.round() as i32,
                    kind: p.kind,
                    services: p.gatt.services.iter().map(|s| s.uuid).collect(),
                })
            })
            .collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.rssi));
        v
    }

    /// Connects the phone (LE: to an advertising peripheral; classic: pairs with `pin`).
    pub fn connect(
        &mut self,
        id: PeripheralId,
        pin: Option<&str>,
        now: SimTime,
    ) -> Result<(), BleError> {
        self.advance(now);
        if let Some(old) = self.phone_conn {
            self.drop_link(old);
        }
        let in_range = self.rssi(id).is_some_and(|r| r >= self.sensitivity_dbm);
        let p = self.per(id)?;
        if !p.powered || !(p.advertising || p.kind == BtKind::Classic) || !in_range {
            return Err(BleError::OutOfRange);
        }
        if p.kind == BtKind::Classic && !p.paired {
            if pin != Some(p.pin.as_str()) {
                return Err(BleError::WrongPin);
            }
            self.per_mut(id)?.paired = true;
        }
        self.phone_conn = Some(id);
        let at = now + SimTime::from_ms(40 + self.rng.below(50) as u64);
        self.log_row(id, Dir::Tx, "BLE CONNECT_IND from phone".into(), 0);
        self.schedule(at, BleEv::ConnectDone { p: id });
        Ok(())
    }

    pub fn disconnect(&mut self, now: SimTime) {
        self.advance(now);
        if let Some(id) = self.phone_conn {
            self.drop_link(id);
        }
    }

    /// The peripheral the phone is connected to (once the link is up).
    pub fn phone_connected(&self) -> Option<PeripheralId> {
        self.phone_conn.filter(|id| self.is_connected(*id))
    }

    /// Service discovery on the connected peripheral.
    pub fn discover(&self) -> Result<Vec<Service>, BleError> {
        let id = self.phone_connected().ok_or(BleError::NotConnected)?;
        Ok(self.per(id)?.gatt.services.clone())
    }

    fn connected_char(&self, handle: u16) -> Result<(PeripheralId, &Characteristic), BleError> {
        let id = self.phone_connected().ok_or(BleError::NotConnected)?;
        let c = self
            .per(id)?
            .gatt
            .char(handle)
            .ok_or(BleError::NoCharacteristic)?;
        Ok((id, c))
    }

    /// Reads a characteristic; result arrives as `PhoneEvent::ReadResult`.
    pub fn read(&mut self, handle: u16, now: SimTime) -> Result<(), BleError> {
        self.advance(now);
        let (id, c) = self.connected_char(handle)?;
        if c.props & props::READ == 0 {
            return Err(BleError::NotPermitted);
        }
        let at = self.conn_event(now);
        self.schedule(at, BleEv::Read { p: id, handle });
        Ok(())
    }

    /// Writes a characteristic (with or without response).
    pub fn write(&mut self, handle: u16, data: &[u8], now: SimTime) -> Result<(), BleError> {
        self.advance(now);
        let (id, c) = self.connected_char(handle)?;
        let response = c.props & props::WRITE != 0;
        if c.props & (props::WRITE | props::WRITE_NO_RESPONSE) == 0 {
            return Err(BleError::NotPermitted);
        }
        let mut at = self.conn_event(now);
        for chunk in data.chunks(20) {
            self.schedule(
                at,
                BleEv::Write {
                    p: id,
                    handle,
                    data: chunk.to_vec(),
                    response,
                },
            );
            at += SimTime::from_us(1_500);
        }
        Ok(())
    }

    /// Enables/disables notifications (writes the CCCD).
    pub fn subscribe(&mut self, handle: u16, on: bool, now: SimTime) -> Result<(), BleError> {
        self.advance(now);
        let (id, c) = self.connected_char(handle)?;
        if c.props & (props::NOTIFY | props::INDICATE) == 0 {
            return Err(BleError::NotPermitted);
        }
        let at = self.conn_event(now);
        self.schedule(at, BleEv::Subscribe { p: id, handle, on });
        Ok(())
    }

    /// The characteristic the serial terminal uses on the connected peripheral: HM-10 FFE1,
    /// Nordic UART TX/RX, or `None` for classic SPP.
    fn terminal_chars(&self, id: PeripheralId) -> Option<(u16, u16)> {
        let g = &self.per(id).ok()?.gatt;
        if let Some(h) = g.find(HM10_CHAR) {
            return Some((h, h));
        }
        match (g.find(NUS_RX), g.find(NUS_TX)) {
            (Some(rx), Some(tx)) => Some((rx, tx)),
            _ => None,
        }
    }

    /// Bluetooth serial terminal: subscribes to the serial characteristic (LE) — call after
    /// connecting.
    pub fn terminal_open(&mut self, now: SimTime) -> Result<(), BleError> {
        let id = self.phone_connected().ok_or(BleError::NotConnected)?;
        if self.per(id)?.kind == BtKind::Classic {
            return Ok(());
        }
        let (_, tx) = self.terminal_chars(id).ok_or(BleError::NoCharacteristic)?;
        self.subscribe(tx, true, now)
    }

    /// Bluetooth serial terminal: sends bytes to the connected device.
    pub fn terminal_send(&mut self, data: &[u8], now: SimTime) -> Result<(), BleError> {
        self.advance(now);
        let id = self.phone_connected().ok_or(BleError::NotConnected)?;
        if self.per(id)?.kind == BtKind::Classic {
            let at = now + SimTime::from_ms(8 + self.rng.below(6) as u64);
            self.schedule(
                at,
                BleEv::SppToPeripheral {
                    p: id,
                    data: data.to_vec(),
                },
            );
            return Ok(());
        }
        let (rx, _) = self.terminal_chars(id).ok_or(BleError::NoCharacteristic)?;
        self.write(rx, data, now)
    }

    /// Phone-side events since the last call.
    pub fn take_phone_events(&mut self) -> Vec<(SimTime, PhoneEvent)> {
        std::mem::take(&mut self.phone_events)
    }

    // ================================================================ time

    pub fn next_event_time(&self) -> Option<SimTime> {
        self.queue.peek().map(|s| s.at)
    }

    pub fn advance(&mut self, now: SimTime) {
        while let Some(s) = self.queue.peek() {
            if s.at > now {
                break;
            }
            let s = self.queue.pop().expect("peeked");
            self.now = self.now.max(s.at);
            self.process(s.ev);
        }
        self.now = self.now.max(now);
    }

    fn process(&mut self, ev: BleEv) {
        let now = self.now;
        match ev {
            BleEv::ConnectDone { p } => {
                if self.phone_conn != Some(p) {
                    return;
                }
                let Ok(per) = self.per_mut(p) else { return };
                if !per.powered {
                    self.phone_conn = None;
                    self.phone_events.push((
                        now,
                        PhoneEvent::ConnectFailed {
                            id: p,
                            reason: "device went away".into(),
                        },
                    ));
                    return;
                }
                per.connected = true;
                per.advertising = false;
                per.events.push_back(PeripheralEvent::Connected);
                self.phone_events
                    .push((now, PhoneEvent::Connected { id: p }));
                self.log_row(p, Dir::Rx, "BLE connected (interval 30 ms)".into(), 0);
            }
            BleEv::Write {
                p,
                handle,
                data,
                response,
            } => {
                if !self.is_connected(p) {
                    return;
                }
                let n = data.len();
                if let Some(c) = self.gatt_mut(p).and_then(|g| g.char_mut(handle)) {
                    c.value = data.clone();
                }
                let shown = printable(&data);
                self.log_row(
                    p,
                    Dir::Rx,
                    format!("ATT WRITE 0x{handle:04X} \"{shown}\""),
                    n,
                );
                if let Ok(per) = self.per_mut(p) {
                    per.events
                        .push_back(PeripheralEvent::Write { handle, data });
                }
                if response {
                    self.phone_events
                        .push((now, PhoneEvent::WriteDone { id: p, handle }));
                }
            }
            BleEv::Read { p, handle } => {
                if !self.is_connected(p) {
                    return;
                }
                let data = self
                    .gatt(p)
                    .and_then(|g| g.char(handle))
                    .map(|c| c.value.clone())
                    .unwrap_or_default();
                if let Ok(per) = self.per_mut(p) {
                    per.events.push_back(PeripheralEvent::Read { handle });
                }
                self.log_row(
                    p,
                    Dir::Tx,
                    format!("ATT READ RSP 0x{handle:04X} · {} B", data.len()),
                    data.len(),
                );
                self.phone_events.push((
                    now,
                    PhoneEvent::ReadResult {
                        id: p,
                        handle,
                        data: data[..data.len().min(22)].to_vec(),
                    },
                ));
            }
            BleEv::Notify { p, handle, data } => {
                if !self.is_connected(p) || self.phone_conn != Some(p) {
                    return;
                }
                let shown = printable(&data);
                self.log_row(
                    p,
                    Dir::Tx,
                    format!("ATT NOTIFY 0x{handle:04X} \"{shown}\""),
                    data.len(),
                );
                let term = self.terminal_chars(p).is_some_and(|(_, tx)| tx == handle);
                if term {
                    self.phone_events.push((
                        now,
                        PhoneEvent::Terminal {
                            id: p,
                            data: data.clone(),
                        },
                    ));
                }
                self.phone_events.push((
                    now,
                    PhoneEvent::Notification {
                        id: p,
                        handle,
                        data,
                    },
                ));
            }
            BleEv::Subscribe { p, handle, on } => {
                if !self.is_connected(p) {
                    return;
                }
                if let Some(c) = self.gatt_mut(p).and_then(|g| g.char_mut(handle)) {
                    c.notifying = on;
                }
                self.log_row(
                    p,
                    Dir::Rx,
                    format!(
                        "ATT WRITE CCCD 0x{handle:04X} = {}",
                        if on { "notify" } else { "off" }
                    ),
                    2,
                );
                if let Ok(per) = self.per_mut(p) {
                    per.events
                        .push_back(PeripheralEvent::Subscribed { handle, on });
                }
            }
            BleEv::SppToPeripheral { p, data } => {
                if !self.is_connected(p) {
                    return;
                }
                self.log_row(
                    p,
                    Dir::Rx,
                    format!("SPP \"{}\"", printable(&data)),
                    data.len(),
                );
                if let Ok(per) = self.per_mut(p) {
                    per.events.push_back(PeripheralEvent::SppData(data));
                }
            }
            BleEv::SppToPhone { p, data } => {
                if !self.is_connected(p) || self.phone_conn != Some(p) {
                    return;
                }
                self.log_row(
                    p,
                    Dir::Tx,
                    format!("SPP \"{}\"", printable(&data)),
                    data.len(),
                );
                self.phone_events
                    .push((now, PhoneEvent::Terminal { id: p, data }));
            }
        }
    }
}

/// Bytes as text for log rows (non-printables as `·`).
pub(crate) fn printable(d: &[u8]) -> String {
    d.iter()
        .map(|&b| {
            if (0x20..0x7f).contains(&b) {
                b as char
            } else {
                '·'
            }
        })
        .collect()
}
