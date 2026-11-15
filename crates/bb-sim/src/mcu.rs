//! Microcontrollers on the bench, behind one trait so cycle-accurate AVRs and API-level host
//! boards (ESP32 / Pico via `bb_firmware::host::HostBoard`, see [`crate::host`]) plug into the
//! same scheduler.
//!
//! The electrical pin model (SIM-2) lives in [`PinModel`]: push-pull outputs are Thevenin
//! sources (≈ 25 Ω high side, 22 Ω low side → the datasheet's VOH ≈ 4.5 V at 20 mA), internal
//! pull-ups ≈ 35 kΩ, inputs have Schmitt thresholds 0.3·Vcc / 0.6·Vcc and ~1 nA leakage.

use bb_avr::{Avr, AvrConfig, AvrIo, FirmwareImage, Pin, PinState, ResetCause, StopReason};
use bb_core::{PinDrive, SimTime};
use bb_firmware::{Board, Debugger, Decision, PauseInfo};

/// What an MCU does to one of its pins (logical; the electrical drive comes from [`PinModel`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicDrive {
    Input { pullup: bool },
    Output { high: bool },
}

impl From<PinState> for LogicDrive {
    fn from(s: PinState) -> Self {
        match s {
            PinState::Input { pullup } => LogicDrive::Input { pullup },
            PinState::Output { high } => LogicDrive::Output { high },
        }
    }
}

/// A pin drive change with its exact time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct McuPinEvent {
    pub time: SimTime,
    /// Index into the MCU's pin list ([`McuCore::pin_names`]).
    pub pin: usize,
    pub drive: LogicDrive,
}

/// Why [`McuCore::run_until`] returned.
#[derive(Debug, Clone, PartialEq)]
pub enum McuStop {
    Reached,
    PinChange,
    /// The ADC samples `pin` at `time` and needs the voltage (see [`McuCore::supply_adc`]).
    AdcSample {
        pin: usize,
        time: SimTime,
    },
    /// The debugger paused the core (breakpoint, step done, pause request).
    Paused(String),
    /// Firmware fault (stack overflow, invalid opcode) — the core keeps running if resumed.
    Fault(String),
}

/// Analog callbacks during a run.
pub trait McuIo {
    /// Voltage on `pin` at `time` if the caller can answer without advancing the circuit.
    fn adc_sample(&mut self, pin: usize, time: SimTime) -> Option<f64>;
    /// Radio service request from an API-level board (Wi-Fi/sockets on channel 1, BLE on 2);
    /// `None` = nobody serves it.
    fn service(&mut self, channel: u8, request: &[u8], at: SimTime) -> Option<Vec<u8>> {
        let _ = (channel, request, at);
        None
    }
}

/// One MCU (cycle-accurate or API-level).
pub trait McuCore: Send {
    /// `"ATmega328P @ 16 MHz"`.
    fn label(&self) -> String;
    /// `"cycle-accurate AVR core"` / `"API-level"`.
    fn engine(&self) -> &'static str;
    fn now(&self) -> SimTime;
    /// Pins by name, matching the board part's `mcu` pin names (`"D9"`, `"A0"`).
    fn pin_names(&self) -> Vec<String>;
    fn pin_index(&self, name: &str) -> Option<usize>;
    /// Runs until `t` or an early stop (pin change, ADC sample, debugger pause).
    fn run_until(&mut self, t: SimTime, io: &mut dyn McuIo) -> McuStop;
    fn take_pin_events(&mut self) -> Vec<McuPinEvent>;
    fn pin_drive(&self, pin: usize) -> LogicDrive;
    /// Resolved line level seen by the input buffer from now (`None` = nothing connected).
    fn set_input(&mut self, pin: usize, level: Option<bool>);
    /// Same, at an exact future time (≥ now).
    fn schedule_input(&mut self, pin: usize, level: Option<bool>, at: SimTime);
    /// Analog comparator / fallback voltage.
    fn set_analog(&mut self, pin: usize, volts: f64);
    /// Answers [`McuStop::AdcSample`].
    fn supply_adc(&mut self, volts: f64);
    /// Supply voltage (brown-out / power-on reset follow it).
    fn set_vcc(&mut self, volts: f64);
    fn set_aref(&mut self, volts: f64);
    /// External reset line (true = held low).
    fn set_reset_line(&mut self, low: bool);
    fn reset(&mut self);
    fn reset_count(&self) -> u64;
    fn last_reset(&self) -> String;
    fn in_reset(&self) -> bool;
    fn set_temperature(&mut self, celsius: f64);
    /// Programs firmware and resets the core (EEPROM is kept).
    fn flash(&mut self, image: &FirmwareImage) -> Result<(), String>;
    /// Loads a host-compiled sketch executable (API-level boards) and restarts the board.
    fn flash_exe(&mut self, exe: &std::path::Path) -> Result<(), String> {
        let _ = exe;
        Err("this board runs AVR firmware images".into())
    }
    /// Serial bytes sent since the last call: (port, end time, byte).
    fn take_serial(&mut self) -> Vec<(usize, SimTime, u8)>;
    /// Core diagnostics since the last call (crashes, restarts), shown as system lines.
    fn take_log(&mut self) -> Vec<(SimTime, String)> {
        Vec::new()
    }
    /// Types bytes into a serial port's receiver (serial monitor send box).
    fn serial_inject(&mut self, port: usize, bytes: &[u8]);
    /// The pin a serial port transmits on, if its waveform matters (wired TX pin).
    fn serial_tx_pin(&self, port: usize) -> Option<usize>;
    /// Turns the bit-level TX waveform of a port on/off (off when nothing is wired to TX).
    fn set_serial_waveform(&mut self, port: usize, on: bool);
    fn serial_baud(&self, port: usize) -> f64;
    /// The cycle-accurate core, for the debugger and register views.
    fn as_avr(&mut self) -> Option<&mut Avr> {
        None
    }
    fn as_avr_ref(&self) -> Option<&Avr> {
        None
    }
    /// The source-level debugger slot and its core (AVR boards).
    fn avr_debug(&mut self) -> Option<(&mut Option<Debugger>, &mut Avr)> {
        None
    }
}

/// Electrical model of MCU pins (SIM-2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PinModel {
    /// Output high-side resistance (Ω).
    pub r_high: f64,
    /// Output low-side resistance (Ω).
    pub r_low: f64,
    /// Internal pull-up (Ω).
    pub pullup: f64,
    /// Input leakage modelled as a resistance to ground (Ω).
    pub leak: f64,
    /// Schmitt thresholds as fractions of Vcc.
    pub vil: f64,
    pub vih: f64,
    /// Absolute maximum pin current (A) before damage counts.
    pub max_current: f64,
}

impl PinModel {
    pub const AVR: PinModel = PinModel {
        r_high: 25.0,
        r_low: 22.0,
        pullup: 35e3,
        leak: 1e9,
        vil: 0.3,
        vih: 0.6,
        max_current: 0.040,
    };
    pub const ESP32: PinModel = PinModel {
        r_high: 30.0,
        r_low: 30.0,
        pullup: 45e3,
        leak: 1e9,
        vil: 0.25,
        vih: 0.75,
        max_current: 0.040,
    };

    /// Port drives (high side to Vcc, low side to ground) for a logical drive.
    pub fn port_drives(&self, d: LogicDrive) -> (PinDrive, PinDrive) {
        match d {
            LogicDrive::Output { high: true } => (
                PinDrive::Thevenin {
                    v: 0.0,
                    r: self.r_high,
                },
                PinDrive::HighZ,
            ),
            LogicDrive::Output { high: false } => (
                PinDrive::HighZ,
                PinDrive::Thevenin {
                    v: 0.0,
                    r: self.r_low,
                },
            ),
            LogicDrive::Input { pullup: true } => (
                PinDrive::Pull {
                    v: 0.0,
                    r: self.pullup,
                },
                PinDrive::Pull {
                    v: 0.0,
                    r: self.leak,
                },
            ),
            LogicDrive::Input { pullup: false } => (
                PinDrive::HighZ,
                PinDrive::Pull {
                    v: 0.0,
                    r: self.leak,
                },
            ),
        }
    }

    /// Single drive against ground for closed-form (digital) nets, given the supply voltage.
    pub fn net_drive(&self, d: LogicDrive, vcc: f64) -> PinDrive {
        match d {
            LogicDrive::Output { high: true } => PinDrive::Thevenin {
                v: vcc,
                r: self.r_high,
            },
            LogicDrive::Output { high: false } => PinDrive::Thevenin {
                v: 0.0,
                r: self.r_low,
            },
            LogicDrive::Input { pullup: true } => PinDrive::Pull {
                v: vcc,
                r: self.pullup,
            },
            LogicDrive::Input { pullup: false } => PinDrive::Pull {
                v: 0.0,
                r: self.leak,
            },
        }
    }
}

// ---------------------------------------------------------------------------------- AVR

/// A cycle-accurate ATmega328P / ATmega2560 on an Arduino board.
pub struct AvrMcu {
    pub avr: Avr,
    pub board: Board,
    /// Source-level debugger policy (breakpoints by line, stepping); see `bb_firmware`.
    pub debugger: Option<Debugger>,
    /// Why the debugger last paused the core.
    pub last_pause: Option<PauseInfo>,
    pins: Vec<Pin>,
    names: Vec<String>,
    f_cpu: u64,
    analog_pins: Vec<Option<usize>>,
}

struct AvrIoAdapter<'a> {
    io: &'a mut dyn McuIo,
    pins: &'a [Pin],
    f_cpu: u64,
}

impl AvrIo for AvrIoAdapter<'_> {
    fn adc_sample(&mut self, pin: Pin, cycle: u64) -> Option<f64> {
        let idx = self.pins.iter().position(|p| *p == pin)?;
        self.io
            .adc_sample(idx, SimTime::from_cycles(cycle, self.f_cpu))
    }
}

impl AvrMcu {
    /// An AVR for a board (`"uno"`, `"nano"`, `"mega"`).
    pub fn new(board_id: &str, seed: u64, adc_noise_lsb: f64) -> Result<AvrMcu, String> {
        let board = Board::by_id(board_id).ok_or_else(|| format!("unknown board `{board_id}`"))?;
        let mut cfg = AvrConfig::new(board.mcu);
        cfg.f_cpu = board.f_cpu;
        cfg.seed = seed;
        cfg.adc_noise_lsb = adc_noise_lsb;
        cfg.stop_on.reset = false;
        let avr = Avr::new(cfg);
        let mut pins = Vec::new();
        let mut names = Vec::new();
        let mut analog_pins = Vec::new();
        for bp in &board.pins {
            if let Some(p) = bp.pin {
                pins.push(p);
                names.push(bp.name.clone());
                analog_pins.push(bp.adc.map(|a| a as usize));
            }
        }
        Ok(AvrMcu {
            f_cpu: board.f_cpu,
            debugger: None,
            last_pause: None,
            avr,
            board,
            pins,
            names,
            analog_pins,
        })
    }

    pub fn pin(&self, idx: usize) -> Option<Pin> {
        self.pins.get(idx).copied()
    }

    fn usart_pins(&self, port: usize) -> Option<(u8, u8)> {
        self.board.serial.get(port).copied()
    }
}

impl McuCore for AvrMcu {
    fn label(&self) -> String {
        format!("{} @ {} MHz", self.board.mcu_name, self.f_cpu / 1_000_000)
    }
    fn engine(&self) -> &'static str {
        "cycle-accurate AVR core"
    }
    fn now(&self) -> SimTime {
        self.avr.time()
    }
    fn pin_names(&self) -> Vec<String> {
        self.names.clone()
    }
    fn pin_index(&self, name: &str) -> Option<usize> {
        if let Some(i) = self.names.iter().position(|n| n.eq_ignore_ascii_case(name)) {
            return Some(i);
        }
        let bp = self.board.pin_by_name(name)?;
        self.names.iter().position(|n| *n == bp.name)
    }
    fn run_until(&mut self, t: SimTime, io: &mut dyn McuIo) -> McuStop {
        let until = t.to_cycles(self.f_cpu);
        let mut adapter = AvrIoAdapter {
            io,
            pins: &self.pins,
            f_cpu: self.f_cpu,
        };
        loop {
            let r = self.avr.run(until, &mut adapter);
            if let Some(dbg) = self.debugger.as_mut()
                && matches!(
                    r,
                    StopReason::Breakpoint { .. }
                        | StopReason::StepDone
                        | StopReason::Paused
                        | StopReason::Break { .. }
                        | StopReason::Watchpoint { .. }
                        | StopReason::Fault { .. }
                )
            {
                match dbg.on_stop(&mut self.avr, &r) {
                    Decision::Continue => {
                        if self.avr.cycle() >= until {
                            return McuStop::Reached;
                        }
                        continue;
                    }
                    Decision::Pause(info) => {
                        let what = format!("{:?}", info.reason);
                        self.last_pause = Some(info);
                        return McuStop::Paused(what);
                    }
                }
            }
            return match r {
                StopReason::ReachedCycle => McuStop::Reached,
                StopReason::PinChange => McuStop::PinChange,
                StopReason::AdcSampleNeeded { pin, cycle } => {
                    let idx = self.pins.iter().position(|p| *p == pin).unwrap_or(0);
                    McuStop::AdcSample {
                        pin: idx,
                        time: SimTime::from_cycles(cycle, self.f_cpu),
                    }
                }
                StopReason::Breakpoint { pc } => {
                    McuStop::Paused(format!("breakpoint at 0x{pc:04x}"))
                }
                StopReason::Watchpoint { addr, .. } => {
                    McuStop::Paused(format!("watchpoint 0x{addr:04x}"))
                }
                StopReason::Break { pc } => McuStop::Paused(format!("BREAK at 0x{pc:04x}")),
                StopReason::StepDone => McuStop::Paused("step".into()),
                StopReason::Paused => McuStop::Paused("paused".into()),
                StopReason::Fault { kind, pc } => McuStop::Fault(format!("{kind:?} at 0x{pc:04x}")),
                StopReason::Sleep { .. } | StopReason::Reset { .. } => continue,
            };
        }
    }
    fn take_pin_events(&mut self) -> Vec<McuPinEvent> {
        let f = self.f_cpu;
        self.avr
            .take_pin_events()
            .into_iter()
            .filter_map(|e| {
                let idx = self.pins.iter().position(|p| *p == e.pin)?;
                Some(McuPinEvent {
                    time: SimTime::from_cycles(e.cycle, f),
                    pin: idx,
                    drive: e.state.into(),
                })
            })
            .collect()
    }
    fn pin_drive(&self, pin: usize) -> LogicDrive {
        self.pins
            .get(pin)
            .map(|p| self.avr.pin_info(*p).state.into())
            .unwrap_or(LogicDrive::Input { pullup: false })
    }
    fn set_input(&mut self, pin: usize, level: Option<bool>) {
        if let Some(p) = self.pins.get(pin) {
            self.avr.set_pin_input(*p, level);
        }
    }
    fn schedule_input(&mut self, pin: usize, level: Option<bool>, at: SimTime) {
        if let Some(p) = self.pins.get(pin) {
            let c = at.to_cycles(self.f_cpu);
            self.avr.schedule_pin_input(*p, level, c);
        }
    }
    fn set_analog(&mut self, pin: usize, volts: f64) {
        // ADC channels are always sampled from the bench (inline answer or a stop): a pushed
        // value would be used as a stale fallback for every later conversion
        if self.analog_pins.get(pin).copied().flatten().is_some() {
            return;
        }
        if let Some(p) = self.pins.get(pin) {
            self.avr.set_pin_voltage(*p, volts);
        }
    }
    fn supply_adc(&mut self, volts: f64) {
        self.avr.supply_adc_sample(volts);
    }
    fn set_vcc(&mut self, volts: f64) {
        if (self.avr.vcc() - volts).abs() > 1e-4 {
            self.avr.set_vcc(volts);
        }
    }
    fn set_aref(&mut self, volts: f64) {
        self.avr.set_aref(volts);
    }
    fn set_reset_line(&mut self, low: bool) {
        self.avr.set_reset_pin(low);
    }
    fn reset(&mut self) {
        self.avr.reset(ResetCause::External);
    }
    fn reset_count(&self) -> u64 {
        self.avr.reset_count()
    }
    fn last_reset(&self) -> String {
        match self.avr.last_reset_cause() {
            ResetCause::PowerOn => "power-on",
            ResetCause::External => "external",
            ResetCause::BrownOut => "brown-out",
            ResetCause::Watchdog => "watchdog",
        }
        .into()
    }
    fn in_reset(&self) -> bool {
        self.avr.in_reset()
    }
    fn set_temperature(&mut self, celsius: f64) {
        self.avr.set_temperature(celsius);
    }
    fn flash(&mut self, image: &FirmwareImage) -> Result<(), String> {
        self.avr.flash(image).map_err(|e| e.to_string())
    }
    fn take_serial(&mut self) -> Vec<(usize, SimTime, u8)> {
        let mut out = Vec::new();
        for port in 0..self.board.serial.len() {
            for b in self.avr.take_usart_tx(port) {
                out.push((
                    port,
                    SimTime::from_cycles(b.cycle, self.f_cpu),
                    b.data as u8,
                ));
            }
        }
        out
    }
    fn serial_inject(&mut self, port: usize, bytes: &[u8]) {
        self.avr.usart_inject(port, bytes);
    }
    fn serial_tx_pin(&self, port: usize) -> Option<usize> {
        let (_, tx) = self.usart_pins(port)?;
        let bp = self.board.pin(tx)?;
        self.names.iter().position(|n| *n == bp.name)
    }
    fn set_serial_waveform(&mut self, port: usize, on: bool) {
        self.avr.set_usart_waveform(port, on);
    }
    fn serial_baud(&self, port: usize) -> f64 {
        self.avr.usart_baud(port)
    }
    fn as_avr(&mut self) -> Option<&mut Avr> {
        Some(&mut self.avr)
    }
    fn as_avr_ref(&self) -> Option<&Avr> {
        Some(&self.avr)
    }
    fn avr_debug(&mut self) -> Option<(&mut Option<Debugger>, &mut Avr)> {
        Some((&mut self.debugger, &mut self.avr))
    }
}

impl AvrMcu {
    /// ADC channel of a pin index (for probes / the inspector).
    pub fn adc_channel(&self, pin: usize) -> Option<usize> {
        self.analog_pins.get(pin).copied().flatten()
    }
}

// ---------------------------------------------------------------------------------- placeholder

/// Stand-in for boards whose emulation is not available in this build (ESP32 / Pico until the
/// host-core glue lands): time advances, pins stay inputs, the serial monitor says why.
pub struct IdleMcu {
    label: String,
    names: Vec<String>,
    now: SimTime,
    notice: Option<String>,
}

impl IdleMcu {
    pub fn new(label: &str, names: Vec<String>, notice: &str) -> IdleMcu {
        IdleMcu {
            label: label.into(),
            names,
            now: SimTime::ZERO,
            notice: Some(notice.into()),
        }
    }
}

impl McuCore for IdleMcu {
    fn label(&self) -> String {
        self.label.clone()
    }
    fn engine(&self) -> &'static str {
        "API-level"
    }
    fn now(&self) -> SimTime {
        self.now
    }
    fn pin_names(&self) -> Vec<String> {
        self.names.clone()
    }
    fn pin_index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n.eq_ignore_ascii_case(name))
    }
    fn run_until(&mut self, t: SimTime, _io: &mut dyn McuIo) -> McuStop {
        self.now = self.now.max(t);
        McuStop::Reached
    }
    fn take_pin_events(&mut self) -> Vec<McuPinEvent> {
        Vec::new()
    }
    fn pin_drive(&self, _pin: usize) -> LogicDrive {
        LogicDrive::Input { pullup: false }
    }
    fn set_input(&mut self, _pin: usize, _level: Option<bool>) {}
    fn schedule_input(&mut self, _pin: usize, _level: Option<bool>, _at: SimTime) {}
    fn set_analog(&mut self, _pin: usize, _volts: f64) {}
    fn supply_adc(&mut self, _volts: f64) {}
    fn set_vcc(&mut self, _volts: f64) {}
    fn set_aref(&mut self, _volts: f64) {}
    fn set_reset_line(&mut self, _low: bool) {}
    fn reset(&mut self) {}
    fn reset_count(&self) -> u64 {
        1
    }
    fn last_reset(&self) -> String {
        "power-on".into()
    }
    fn in_reset(&self) -> bool {
        false
    }
    fn set_temperature(&mut self, _c: f64) {}
    fn flash(&mut self, _image: &FirmwareImage) -> Result<(), String> {
        Err("this board runs host-compiled sketches (flash_exe)".into())
    }
    fn take_serial(&mut self) -> Vec<(usize, SimTime, u8)> {
        Vec::new()
    }
    fn take_log(&mut self) -> Vec<(SimTime, String)> {
        self.notice
            .take()
            .map(|n| (self.now, n))
            .into_iter()
            .collect()
    }
    fn serial_inject(&mut self, _port: usize, _bytes: &[u8]) {}
    fn serial_tx_pin(&self, _port: usize) -> Option<usize> {
        None
    }
    fn set_serial_waveform(&mut self, _port: usize, _on: bool) {}
    fn serial_baud(&self, _port: usize) -> f64 {
        115_200.0
    }
}
