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
}

impl DebugViewModel {
    pub fn is_paused(&self) -> bool {
        matches!(self.state, DebugState::Paused { .. })
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

/// Bluetooth LE card: devices the virtual phone can see.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BleView {
    pub devices: Vec<(String, String)>,
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
        }
    }

    pub fn has_radios(&self) -> bool {
        !self.cards.is_empty()
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
