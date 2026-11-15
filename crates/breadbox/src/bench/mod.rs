//! The UI ↔ running-bench boundary.
//!
//! Screens never talk to a simulator directly. They hold a `Box<dyn BenchLink>` and render
//! plain view-model structs ([`DebugViewModel`], [`RadioPanelModel`], [`SerialLine`], …) that
//! the link produces. Two implementations exist:
//!
//! * [`avr::AvrBench`] (this crate) — one `bb_avr::Avr` running real firmware with a
//!   `bb_firmware::Debugger`, optionally wired to an ESP-01 radio module from `bb-radio` (the
//!   garden-node setup). No circuit: analog inputs are fixed voltages.
//! * [`sim::SimBench`] — the app's default: the full co-simulation over `bb_sim::SimHandle`
//!   (analog circuit, MCUs, devices, world, radios, instruments). The same trait, so the
//!   Firmware screen, debugger and Radios panel work unchanged; the Workbench reaches its extra
//!   API (telemetry, world, circuit updates) through [`BenchLink::as_sim`].
//!
//! Contract rules: every method returns quickly (the bench runs on its own thread); `poll`
//! is called once per UI frame while a screen that shows bench state is visible and drains
//! the serial/event queues; the link calls the repaint callback it was created with whenever
//! new data is available, so an idle UI costs nothing.

pub mod audio;
pub mod avr;
pub mod service;
pub mod sim;

use bb_core::SimTime;
use std::path::PathBuf;
use std::sync::Arc;

/// Wakes the UI (wraps `egui::Context::request_repaint`).
pub type Repaint = Arc<dyn Fn() + Send + Sync>;

/// An MCU on the bench that accepts firmware.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchTarget {
    /// Part reference: `"U1"`.
    pub id: String,
    /// Target selector text: `"U1 · UNO R3 (ATmega328P)"`.
    pub label: String,
    /// Board name (`"UNO R3"`).
    pub board: String,
    /// arduino-cli FQBN for AVR boards (`arduino:avr:uno`); `None` for host-emulated boards.
    pub fqbn: Option<String>,
    /// `"cycle-accurate AVR core"` or `"API-level"`.
    pub emulation: String,
    /// Sketch folder of this MCU in the project, if any.
    pub sketch_dir: Option<PathBuf>,
}

/// Firmware to flash.
#[derive(Debug, Clone, Default)]
pub struct Firmware {
    /// ELF (preferred: carries DWARF for source-level debugging).
    pub elf: Option<Arc<Vec<u8>>>,
    /// Intel HEX fallback.
    pub hex: Option<String>,
    /// Where it came from (shown in the output panel).
    pub origin: String,
    /// Built with `-Og -fno-lto` for debugging.
    pub debug_build: bool,
    /// Host-compiled sketch executable for API-level boards (ESP32 / Pico).
    pub host_exe: Option<PathBuf>,
}

/// Result of a successful upload.
#[derive(Debug, Clone, PartialEq)]
pub struct UploadReport {
    pub target: String,
    pub elapsed: std::time::Duration,
    /// `"Flashed virtual U1 in 0.21 s · core reset · bench state kept"`.
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BenchError {
    UnknownTarget(String),
    BadFirmware(String),
    Unsupported(String),
}

impl std::fmt::Display for BenchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BenchError::UnknownTarget(t) => write!(f, "no MCU called {t} on this bench"),
            BenchError::BadFirmware(e) => write!(f, "firmware rejected: {e}"),
            BenchError::Unsupported(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for BenchError {}

/// Simulation speed (`WB-1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SimSpeed {
    /// Multiple of real time (0.1 … 10).
    RealTime(f64),
    /// As fast as possible.
    Max,
}

/// Debugger commands (`MCU-5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugCommand {
    Continue,
    Pause,
    StepOver,
    StepInto,
    StepOut,
    StepInstruction,
}

/// A source breakpoint as the editor knows it.
#[derive(Debug, Clone, PartialEq)]
pub struct BreakpointSpec {
    /// Sketch tab name (`"garden-node.ino"`).
    pub file: String,
    /// 1-based line.
    pub line: u32,
    pub enabled: bool,
    /// Break only on the n-th hit (`None` = always).
    pub hit_count: Option<u64>,
}

/// What `reset` resets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResetScope {
    /// Whole bench (sim time back to 0).
    Bench,
    /// One MCU only ("reset MCU only"; analog/world state kept).
    Mcu(String),
}

/// Commands from the Radios panel (`RF-3`, `RF-6`, `RF-7`).
#[derive(Debug, Clone, PartialEq)]
pub enum RadioCommand {
    SetConditions(NetConditionsView),
    /// `Some((host, port))` bridges the built-in broker to a real LAN broker.
    SetLanBridge(Option<(String, u16)>),
    /// "Add BLE hat" on the empty BLE card.
    AddBleHat,
    MqttPublish {
        topic: String,
        payload: String,
    },
    MqttSubscribe(String),
    /// The virtual phone (BLE central, `RF-6`).
    Phone(PhoneAction),
    /// Moves the bench (and every radio on it) on the floor plan, metres (`RF-1`).
    SetBenchPos {
        x: f64,
        y: f64,
    },
}

/// Virtual-phone actions.
#[derive(Debug, Clone, PartialEq)]
pub enum PhoneAction {
    Connect(u32),
    Disconnect,
    Read(u16),
    Subscribe(u16, bool),
    Write(u16, Vec<u8>),
    TerminalOpen,
    TerminalSend(Vec<u8>),
}

/// Colour class of a serial-monitor line (`WB-11`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// Firmware output.
    Normal,
    /// Simulator / system messages (grey).
    System,
    /// Radio / network annotations (cyan).
    Net,
    /// Warnings (amber).
    Warn,
    /// Errors (red).
    Error,
}

/// One serial-monitor line.
#[derive(Debug, Clone, PartialEq)]
pub struct SerialLine {
    /// MCU it belongs to (`"U1"`).
    pub target: String,
    pub time: SimTime,
    pub text: String,
    pub kind: LineKind,
}

impl SerialLine {
    /// `00:12.500` — the monitor's timestamp column.
    pub fn time_label(&self) -> String {
        clock_label(self.time)
    }
}

/// `mm:ss.mmm`.
pub fn clock_label(t: SimTime) -> String {
    let ms = t.as_millis();
    format!(
        "{:02}:{:02}.{:03}",
        ms / 60_000,
        (ms / 1000) % 60,
        ms % 1000
    )
}

/// Out-of-band bench events for the output panel / toasts.
#[derive(Debug, Clone, PartialEq)]
pub enum BenchEvent {
    /// `"Flashed virtual U1 in 0.21 s · core reset · bench state kept"`.
    Flashed(UploadReport),
    /// A breakpoint was armed / re-armed: `note: line 30 — breakpoint armed on U1 (hits: 24)`.
    BreakpointArmed {
        target: String,
        file: String,
        line: u32,
        hits: u64,
    },
    /// Execution paused (breakpoint, step, user).
    Paused {
        target: String,
        reason: String,
    },
    Note(String),
    Error(String),
}

/// Debugger state of one MCU.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum DebugState {
    /// Nothing flashed yet.
    #[default]
    NoFirmware,
    Running,
    Paused {
        reason: String,
    },
    /// Fault / crashed.
    Halted(String),
}

/// A source position in a sketch tab.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourcePos {
    /// Tab name (`"blink.ino"`), or a path for core/library sources.
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameView {
    pub function: String,
    pub location: Option<SourcePos>,
    pub pc: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarView {
    pub name: String,
    pub type_name: String,
    pub value: String,
    pub location: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RegistersView {
    pub r: Vec<u8>,
    pub sreg: u8,
    pub sp: u16,
    pub pc: u32,
    pub cycle: u64,
}

impl RegistersView {
    /// `ITHSVNZC` with set flags upper-case.
    pub fn sreg_flags(&self) -> String {
        "ITHSVNZC"
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if self.sreg & (0x80 >> i) != 0 {
                    c
                } else {
                    c.to_ascii_lowercase()
                }
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BreakpointView {
    pub file: String,
    /// Requested line.
    pub line: u32,
    /// Line the breakpoint actually landed on (next line with code).
    pub resolved_line: Option<u32>,
    pub hits: u64,
    pub enabled: bool,
}

/// Everything the debug UI shows for one MCU.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DebugViewModel {
    pub target: String,
    pub state: DebugState,
    /// Current line (highlighted in the editor while paused).
    pub location: Option<SourcePos>,
    pub function: Option<String>,
    pub call_stack: Vec<FrameView>,
    pub locals: Vec<VarView>,
    pub globals: Vec<VarView>,
    pub registers: RegistersView,
    pub breakpoints: Vec<BreakpointView>,
    /// Firmware built for debugging (`-Og`)? Release builds show few locals.
    pub debug_build: bool,
    /// SRAM / flash / EEPROM + I/O registers of the AVR core (`None` for API-level boards or
    /// before the first snapshot). Taken at every pause and, while the Debug tab's Memory or
    /// I/O view watches the target, a few times a second while running.
    pub memory: Option<MemoryView>,
}

impl DebugViewModel {
    pub fn is_paused(&self) -> bool {
        matches!(self.state, DebugState::Paused { .. })
    }
}

/// One I/O register in the debugger's I/O view (`PORTB 0x25 0x20 0010 0000`).
#[derive(Debug, Clone, PartialEq)]
pub struct IoRegView {
    pub name: &'static str,
    /// Data-space address.
    pub addr: u16,
    /// `"Port B"`, `"Timer/Counter1"`, `"USART0"`, …
    pub peripheral: &'static str,
    pub value: u8,
    /// Value at the previous pause (`None` before the second pause).
    pub prev: Option<u8>,
}

impl IoRegView {
    /// `0010 0000`.
    pub fn bits(&self) -> String {
        format!("{:04b} {:04b}", self.value >> 4, self.value & 0x0F)
    }
    /// Changed since the previous pause?
    pub fn changed(&self) -> bool {
        self.prev.is_some_and(|p| p != self.value)
    }
}

/// Memory snapshot of one AVR core for the hex views (`MCU-5`). Buffers are shared (`Arc`)
/// so the per-frame view-model copies stay cheap; flash is cached per firmware.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MemoryView {
    /// `"ATmega328P"`.
    pub mcu: String,
    /// Data address of the first SRAM byte (`0x100` on the 328P, `0x200` on the 2560).
    pub sram_start: u16,
    /// SRAM (`sram_start..=RAMEND`).
    pub sram: Arc<Vec<u8>>,
    pub eeprom: Arc<Vec<u8>>,
    /// Program memory (bytes). Fetched once per firmware (`flash_gen`).
    pub flash: Arc<Vec<u8>>,
    /// `bb_avr::Avr::flash_generation` of `flash`.
    pub flash_gen: u64,
    /// Programmed bytes (up to the last non-erased word).
    pub flash_used: usize,
    pub io: Vec<IoRegView>,
    /// SRAM / EEPROM at the previous pause: bytes that differ are highlighted.
    pub prev_sram: Option<Arc<Vec<u8>>>,
    pub prev_eeprom: Option<Arc<Vec<u8>>>,
    /// Stack pointer and the X (r27:r26), Y (r29:r28), Z (r31:r30) pointer registers.
    pub sp: u16,
    pub x: u16,
    pub y: u16,
    pub z: u16,
    /// Core cycle the snapshot was taken at.
    pub cycle: u64,
}

impl MemoryView {
    /// One past the last SRAM data address.
    pub fn sram_end(&self) -> u32 {
        self.sram_start as u32 + self.sram.len() as u32
    }
    /// Pointer registers that point into SRAM: `("SP", addr)`, `("X", addr)`, …
    pub fn pointers(&self) -> [(&'static str, u16); 4] {
        [("SP", self.sp), ("X", self.x), ("Y", self.y), ("Z", self.z)]
    }
}

/// Takes a [`MemoryView`] of `avr` (engine thread). Flash is copied only when its generation
/// differs from `known_flash_gen` (the caller keeps the cached copy).
pub fn memory_snapshot(avr: &bb_avr::Avr, known_flash_gen: Option<u64>) -> MemoryView {
    let mcu = avr.mcu();
    let flash_gen = avr.flash_generation();
    let (flash, flash_used) = if known_flash_gen == Some(flash_gen) {
        (Arc::default(), 0)
    } else {
        let f = avr.flash_bytes();
        let used = f
            .chunks(2)
            .rposition(|w| w.iter().any(|b| *b != 0xFF))
            .map(|i| (i + 1) * 2)
            .unwrap_or(0);
        (Arc::new(f), used)
    };
    let pair = |lo: usize| u16::from_le_bytes([avr.reg(lo), avr.reg(lo + 1)]);
    MemoryView {
        mcu: mcu.name().to_string(),
        sram_start: mcu.sram_start() as u16,
        sram: Arc::new(avr.sram().to_vec()),
        eeprom: Arc::new(avr.eeprom().to_vec()),
        flash,
        flash_gen,
        flash_used,
        io: bb_avr::io_registers(mcu)
            .iter()
            .map(|r| IoRegView {
                name: r.name,
                addr: r.addr,
                peripheral: r.peripheral,
                value: avr.peek(r.addr),
                prev: None,
            })
            .collect(),
        prev_sram: None,
        prev_eeprom: None,
        sp: avr.sp(),
        x: pair(26),
        y: pair(28),
        z: pair(30),
        cycle: avr.cycle(),
    }
}

/// Per-target memory bookkeeping on the UI side of a link: the cached flash copy and the
/// snapshot of the last pause (for "changed since the previous pause").
#[derive(Debug, Default)]
pub struct MemoryHistory {
    flash: std::collections::HashMap<String, (u64, Arc<Vec<u8>>, usize)>,
    last_pause: std::collections::HashMap<String, MemoryView>,
}

impl MemoryHistory {
    /// Flash generation already cached for `target` (pass to [`memory_snapshot`]).
    pub fn known_flash_gen(&self, target: &str) -> Option<u64> {
        self.flash.get(target).map(|f| f.0)
    }

    /// Fills `vm.memory` from the caches (flash, previous pause) and remembers pauses.
    pub fn apply(&mut self, vm: &mut DebugViewModel) {
        let paused = vm.is_paused();
        let Some(m) = vm.memory.as_mut() else {
            return;
        };
        let target = vm.target.clone();
        if m.flash.is_empty() {
            if let Some((g, f, used)) = self.flash.get(&target)
                && *g == m.flash_gen
            {
                m.flash = f.clone();
                m.flash_used = *used;
            }
        } else {
            self.flash
                .insert(target.clone(), (m.flash_gen, m.flash.clone(), m.flash_used));
        }
        let last = self
            .last_pause
            .get(&target)
            .filter(|l| l.flash_gen == m.flash_gen && l.mcu == m.mcu);
        match last {
            // the same pause again (a refresh): keep its comparison base
            Some(l) if paused && l.cycle == m.cycle => {
                m.prev_sram = l.prev_sram.clone();
                m.prev_eeprom = l.prev_eeprom.clone();
                for (r, lr) in m.io.iter_mut().zip(&l.io) {
                    r.prev = lr.prev;
                }
            }
            // a new pause, or running: compare with the last pause
            Some(l) => {
                m.prev_sram = Some(l.sram.clone());
                m.prev_eeprom = Some(l.eeprom.clone());
                for (r, lr) in m.io.iter_mut().zip(&l.io) {
                    r.prev = Some(lr.value);
                }
            }
            None => {}
        }
        if paused {
            let keep = self
                .last_pause
                .get(&target)
                .is_none_or(|l| l.cycle != m.cycle || l.flash_gen != m.flash_gen);
            if keep {
                let mut snap = m.clone();
                snap.flash = Arc::default();
                self.last_pause.insert(target, snap);
            }
        }
    }
}

/// A pin's logical state (for boards without a circuit view).
#[derive(Debug, Clone, PartialEq)]
pub struct PinView {
    /// Arduino name (`"D13"`, `"A0"`).
    pub name: String,
    pub output: bool,
    pub high: bool,
    /// PWM duty 0–1 when a timer drives the pin.
    pub pwm: Option<f32>,
}

// ------------------------------------------------------------------------------------------
// Radios panel view model (`RF-7`)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioKind {
    Wifi,
    Ble,
    Lora,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    Ok,
    Busy,
    Off,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RadioCardView {
    /// `"U2"`.
    pub label: String,
    /// `"ESP8266 Wi-Fi"`.
    pub title: String,
    /// `"connected"`.
    pub status: String,
    pub tone: StatusTone,
    pub kind: RadioKind,
    pub rows: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PacketRow {
    pub seq: u64,
    /// `01.870`.
    pub time: String,
    /// `tx` / `rx` / `!`.
    pub dir: String,
    pub text: String,
    pub dropped: bool,
}

/// Network-conditions panel values.
#[derive(Debug, Clone, PartialEq)]
pub struct NetConditionsView {
    pub latency_ms: f64,
    pub jitter_ms: f64,
    pub loss_pct: f64,
    /// "Drop the AP at t = N s (test reconnect)".
    pub drop_ap_at_s: Option<f64>,
    /// "Bridge to my real LAN broker" (`host:port`).
    pub bridge: Option<String>,
    pub bridge_status: Option<String>,
}

impl Default for NetConditionsView {
    fn default() -> Self {
        NetConditionsView {
            latency_ms: 3.0,
            jitter_ms: 1.0,
            loss_pct: 0.0,
            drop_ap_at_s: None,
            bridge: None,
            bridge_status: None,
        }
    }
}

/// An access point on the floor plan with its link to the bench.
#[derive(Debug, Clone, PartialEq)]
pub struct ApPlacement {
    pub ssid: String,
    pub channel: u8,
    /// Metres.
    pub x: f64,
    pub y: f64,
    /// Bench ↔ AP distance (m), walls crossed, RSSI at the bench (dBm).
    pub distance_m: f64,
    pub walls: u32,
    pub rssi_dbm: f64,
}

/// A wall on the floor plan (metres).
#[derive(Debug, Clone, PartialEq)]
pub struct WallView {
    pub a: (f64, f64),
    pub b: (f64, f64),
    pub loss_db: f64,
    pub label: String,
}

/// Station placement (`RF-1`): where the bench stands relative to the access points.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlacementView {
    /// Bench position, metres.
    pub bench: (f64, f64),
    pub aps: Vec<ApPlacement>,
    pub walls: Vec<WallView>,
}

impl PlacementView {
    /// The access point with the strongest signal at the bench.
    pub fn best_ap(&self) -> Option<&ApPlacement> {
        self.aps
            .iter()
            .max_by(|a, b| a.rssi_dbm.total_cmp(&b.rssi_dbm))
    }
}

/// Bluetooth LE card: devices the virtual phone can see.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BleView {
    pub devices: Vec<(String, String)>,
    /// The virtual phone (co-simulation benches).
    pub phone: Option<PhoneModel>,
}

/// One advertiser in the phone's scan list.
#[derive(Debug, Clone, PartialEq)]
pub struct PhoneDevice {
    pub id: u32,
    pub name: String,
    pub addr: String,
    pub rssi: i32,
}

/// A GATT characteristic of the connected device.
#[derive(Debug, Clone, PartialEq)]
pub struct PhoneChar {
    pub service: String,
    pub handle: u16,
    pub uuid: String,
    pub props: Vec<String>,
    pub value: String,
    pub notifying: bool,
}

/// What the virtual phone shows (`RF-6`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PhoneModel {
    pub scan: Vec<PhoneDevice>,
    pub connected: Option<u32>,
    pub connected_name: Option<String>,
    pub chars: Vec<PhoneChar>,
    /// Bluetooth-serial terminal text (HM-10 / Nordic UART / SPP).
    pub terminal: String,
    pub log: Vec<String>,
}

/// Everything the Radios panel shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RadioPanelModel {
    pub cards: Vec<RadioCardView>,
    /// Newest last.
    pub log: Vec<PacketRow>,
    pub conditions: NetConditionsView,
    /// `None` = no BLE radio on the bench (empty state with "Add BLE hat").
    pub ble: Option<BleView>,
    /// Messages seen by the MQTT explorer.
    pub mqtt: Vec<(String, String)>,
    /// Floor plan: access points, walls, bench position (`RF-1`).
    pub placement: Option<PlacementView>,
}

impl RadioPanelModel {
    /// Builds the model from a live `bb_radio::RadioSpace` (cards, merged log, conditions).
    pub fn from_space(space: &bb_radio::RadioSpace, log_limit: usize) -> Self {
        let cards: Vec<RadioCardView> = space
            .cards()
            .into_iter()
            .map(|c| {
                let lower = c.title.to_ascii_lowercase();
                let kind =
                    if lower.contains("wi-fi") || lower.contains("wifi") || lower.contains("esp") {
                        RadioKind::Wifi
                    } else if lower.contains("ble")
                        || lower.contains("bluetooth")
                        || lower.contains("hc-05")
                    {
                        RadioKind::Ble
                    } else if lower.contains("lora") {
                        RadioKind::Lora
                    } else {
                        RadioKind::Other
                    };
                RadioCardView {
                    tone: status_tone(&c.status),
                    label: c.label,
                    title: c.title,
                    status: c.status,
                    kind,
                    rows: c.rows,
                }
            })
            .collect();
        let rows = space.log_rows();
        let start = rows.len().saturating_sub(log_limit);
        let log = rows[start..]
            .iter()
            .map(|e| PacketRow {
                seq: e.seq,
                time: e.time_label(),
                dir: e.dir.label().to_string(),
                text: e.text(),
                dropped: e.dropped.is_some(),
            })
            .collect();
        let c = space.net.conditions();
        let bridge = space
            .settings()
            .lan_bridge
            .map(|b| format!("{}:{}", b.host, b.port));
        let bridge_status = space.lan_bridge_status().map(|s| format!("{s:?}"));
        let ble = cards
            .iter()
            .any(|c| c.kind == RadioKind::Ble)
            .then(|| BleView {
                phone: None,
                devices: cards
                    .iter()
                    .filter(|c| c.kind == RadioKind::Ble)
                    .map(|c| (format!("{} · {}", c.label, c.title), c.status.clone()))
                    .collect(),
            });
        RadioPanelModel {
            cards,
            log,
            conditions: NetConditionsView {
                latency_ms: c.latency_ms,
                jitter_ms: c.jitter_ms,
                loss_pct: c.loss_pct,
                drop_ap_at_s: c.drop_ap_at_s,
                bridge,
                bridge_status,
            },
            ble,
            mqtt: Vec::new(),
            placement: None,
        }
    }

    pub fn has_radios(&self) -> bool {
        !self.cards.is_empty()
    }
}

/// The floor plan of a live `bb_radio::RadioSpace` with the bench at `bench` (metres).
pub fn placement_from_space(space: &bb_radio::RadioSpace, bench: (f64, f64)) -> PlacementView {
    let net = &space.net;
    let at = bb_radio::geo::Pos::new(bench.0, bench.1);
    PlacementView {
        bench,
        aps: net
            .access_points()
            .iter()
            .map(|a| {
                let l = net.geometry.link(&net.path_loss, a.tx_dbm, a.pos, at);
                ApPlacement {
                    ssid: a.ssid.clone(),
                    channel: a.channel,
                    x: a.pos.x,
                    y: a.pos.y,
                    distance_m: l.distance_m,
                    walls: l.walls,
                    rssi_dbm: l.rssi_dbm,
                }
            })
            .collect(),
        walls: net
            .geometry
            .walls
            .iter()
            .map(|w| WallView {
                a: (w.a.x, w.a.y),
                b: (w.b.x, w.b.y),
                loss_db: w.loss_db,
                label: w.label.clone(),
            })
            .collect(),
    }
}

fn status_tone(s: &str) -> StatusTone {
    let s = s.to_ascii_lowercase();
    if s.contains("connected") && !s.contains("dis") || s == "advertising" || s == "online" {
        StatusTone::Ok
    } else if s.contains("join")
        || s.contains("connecting")
        || s.contains("boot")
        || s.contains("scan")
    {
        StatusTone::Busy
    } else if s.contains("fail") || s.contains("error") || s.contains("lost") {
        StatusTone::Error
    } else {
        StatusTone::Off
    }
}

/// Everything new since the last [`BenchLink::poll`].
#[derive(Debug, Clone, Default)]
pub struct BenchUpdate {
    pub running: bool,
    pub sim_time: SimTime,
    /// New serial lines (all targets), oldest first.
    pub serial: Vec<SerialLine>,
    /// Latest debugger state per target.
    pub debug: Vec<DebugViewModel>,
    /// Latest radio panel (if the bench has a radio space).
    pub radio: Option<RadioPanelModel>,
    /// Latest pin states per target.
    pub pins: Vec<(String, Vec<PinView>)>,
    /// Live probe readings keyed like the Pro `.op` table (`V(A0)`, `I(D1) avg`, `P(R2)`) —
    /// the "bench probe" column. Empty for benches without a circuit.
    pub probes: Vec<(String, f64)>,
    pub events: Vec<BenchEvent>,
}

/// The bench as the UI sees it. Implemented by [`avr::AvrBench`] now and by the `bb-sim`
/// adapter in phase 4.
pub trait BenchLink: Send {
    /// One line for status displays (`"AVR core · U1 · no circuit"`).
    fn describe(&self) -> String;
    /// MCUs that accept firmware, in display order.
    fn targets(&self) -> Vec<BenchTarget>;
    /// Flashes firmware into `target`. Bench state (analog, world, radios) is kept.
    fn upload(&mut self, target: &str, firmware: &Firmware) -> Result<UploadReport, BenchError>;
    fn set_running(&mut self, running: bool);
    fn is_running(&self) -> bool;
    fn reset(&mut self, scope: ResetScope);
    fn set_speed(&mut self, speed: SimSpeed);
    /// Replaces all breakpoints of `target`.
    fn set_breakpoints(&mut self, target: &str, breakpoints: &[BreakpointSpec]);
    fn debug(&mut self, target: &str, cmd: DebugCommand);
    /// Bytes typed into the serial monitor's send box.
    fn send_serial(&mut self, target: &str, data: &[u8]);
    fn radio(&mut self, cmd: RadioCommand);
    /// Drains queued output and returns the latest view models. Never blocks for long.
    fn poll(&mut self) -> BenchUpdate;
    /// The co-simulation bench behind this link (the Workbench's telemetry, world, circuit
    /// updates); `None` for benches without a circuit.
    fn as_sim(&mut self) -> Option<&mut sim::SimBench> {
        None
    }
    /// The Debug tab's Memory / I/O view is showing `target`: keep its [`MemoryView`] fresh
    /// (a few snapshots a second while running). Call every frame the view is visible; the
    /// watch lapses about a second after the last call.
    fn watch_memory(&mut self, _target: &str) {}
}

/// Inputs to a bench factory.
#[derive(Clone)]
pub struct BenchSpec {
    /// Project folder (contains `project.json`), if a project is open.
    pub project_dir: Option<PathBuf>,
    /// MCUs from the project.
    pub targets: Vec<BenchTarget>,
    /// Radio modules from the project (`(ref, part)`, e.g. `("U2", "esp01")`).
    pub radios: Vec<(String, String)>,
    pub repaint: Repaint,
    pub speed: SimSpeed,
    /// The circuit as edited in memory (unsaved edits, read-only examples); `None` loads
    /// `project.json` from `project_dir`.
    pub circuit: Option<bb_circuit::Project>,
    /// Bundled prebuilt firmware (`examples/fixtures`), flashed when the bench starts.
    pub fixtures: Option<PathBuf>,
    /// Play buzzers / relay clicks through the default audio output.
    pub audio: bool,
}

/// Creates the bench for a project. The app's default is [`sim::SimBench::factory`].
pub type BenchFactory = Arc<dyn Fn(&BenchSpec) -> Box<dyn BenchLink> + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sreg_flags_render_like_a_debugger() {
        let r = RegistersView {
            sreg: 0b1000_0011,
            ..Default::default()
        };
        assert_eq!(r.sreg_flags(), "IthsvnZC");
    }

    #[test]
    fn clock_labels() {
        assert_eq!(clock_label(SimTime::from_ms(12_500)), "00:12.500");
        assert_eq!(clock_label(SimTime::from_ms(61_001)), "01:01.001");
    }

    #[test]
    fn memory_history_highlights_changes_since_the_previous_pause() {
        let mut avr = bb_avr::Avr::with_mcu(bb_avr::Mcu::Atmega328p);
        avr.load_program_words(&[0x0000, 0xCFFF]);
        let mut h = MemoryHistory::default();
        let paused = |avr: &bb_avr::Avr, h: &MemoryHistory| DebugViewModel {
            target: "U1".into(),
            state: DebugState::Paused {
                reason: "step".into(),
            },
            memory: Some(memory_snapshot(avr, h.known_flash_gen("U1"))),
            ..Default::default()
        };
        // first pause: flash fetched and cached, nothing to compare with
        let mut a = paused(&avr, &h);
        h.apply(&mut a);
        let m = a.memory.as_ref().unwrap();
        assert_eq!(m.sram_start, 0x100);
        assert_eq!(m.sram.len(), 2048);
        assert_eq!(m.eeprom.len(), 1024);
        assert_eq!(m.flash.len(), 32 * 1024);
        assert_eq!(m.flash_used, 4);
        assert!(m.prev_sram.is_none());
        let portb = m.io.iter().find(|r| r.name == "PORTB").unwrap();
        assert_eq!((portb.addr, portb.value, portb.prev), (0x25, 0, None));
        // second pause: flash comes from the cache, changes are visible
        avr.poke(0x25, 0x20);
        avr.poke(0x123, 0x42);
        avr.set_reg(26, 0x23);
        avr.set_reg(27, 0x01);
        avr.set_pc(2); // a different cycle stands in for "later"
        let mut b = paused(&avr, &h);
        b.memory.as_mut().unwrap().cycle += 100;
        assert!(b.memory.as_ref().unwrap().flash.is_empty(), "not refetched");
        h.apply(&mut b);
        let m = b.memory.as_ref().unwrap();
        assert_eq!(m.flash.len(), 32 * 1024, "filled from the cache");
        assert_eq!(m.x, 0x0123);
        let prev = m.prev_sram.as_ref().unwrap();
        assert_eq!((prev[0x23], m.sram[0x23]), (0, 0x42));
        let portb = m.io.iter().find(|r| r.name == "PORTB").unwrap();
        assert_eq!(portb.bits(), "0010 0000");
        assert!(portb.changed());
        // the same pause refreshed keeps its comparison base
        let mut c = b.clone();
        c.memory.as_mut().unwrap().prev_sram = None;
        h.apply(&mut c);
        assert!(c.memory.as_ref().unwrap().prev_sram.is_some());
        assert!(
            c.memory
                .as_ref()
                .unwrap()
                .io
                .iter()
                .any(|r| r.name == "PORTB" && r.changed())
        );
    }

    #[test]
    fn radio_model_from_a_live_space() {
        use bb_radio::espat::EspAtConfig;
        use bb_radio::geo::Pos;
        let mut space = bb_radio::RadioSpace::default();
        let esp = space.add_esp01("U2", Pos::new(3.2, 0.0), EspAtConfig::with_baud(9600));
        space.power(esp, true, SimTime::ZERO);
        space.advance(SimTime::from_ms(500));
        let m = RadioPanelModel::from_space(&space, 50);
        assert_eq!(m.cards.len(), 1);
        assert_eq!(m.cards[0].label, "U2");
        assert_eq!(m.cards[0].kind, RadioKind::Wifi);
        assert!(m.ble.is_none());
        assert_eq!(m.conditions.loss_pct, 0.0);
    }
}
