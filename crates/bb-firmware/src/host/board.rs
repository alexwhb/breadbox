//! [`HostBoard`]: runs a host-compiled sketch (see [`super::compile_host`]) as a child process
//! in lock-step with simulated time.
//!
//! The child only runs between our replies; every message it sends costs a little simulated
//! time (so busy-wait loops on `millis()` make progress), sleeps (`delay`) block until the board
//! clock reaches their deadline, and `run(until)` never lets the board clock pass `until` while
//! the child is waiting — exactly like `bb_avr::Avr::run`. Outputs are timestamped events;
//! inputs are pushed (`set_pin_input`, `schedule_pin_input`, `set_pin_voltage`,
//! `serial_inject`) or pulled through [`HostIo`] (ADC samples, I²C, SPI, radio services).

use super::catalog::HostKind;
use super::protocol::*;
use crate::FirmwareError;
use bb_core::SimTime;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Pull resistor of an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Pull {
    None,
    Up,
    Down,
}

/// Logical drive of a pin (electrical modelling is the co-simulator's job).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum HostPinState {
    Input { pull: Pull },
    Output { high: bool },
    /// Open drain: `low` = actively pulled low, else released.
    OpenDrain { low: bool },
}

/// Something the sketch did, stamped with the board clock.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum HostEvent {
    Pin { time: SimTime, pin: u8, state: HostPinState },
    /// PWM output (`analogWrite`, `ledcWrite`, `tone`): duty 0–1 at `freq_hz` (0 Hz = DC).
    Pwm { time: SimTime, pin: u8, duty: f64, freq_hz: f64 },
    Log { time: SimTime, text: String },
    /// The sketch called `ESP.restart()`; the process was restarted.
    Restarted { time: SimTime },
}

/// Why [`HostBoard::run`] returned.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum HostStop {
    /// The board clock reached `until`.
    ReachedTime,
    /// An output changed (only with [`HostConfig::stop_on_output`]).
    Output,
    /// The sketch called `panic`/`abort`-like code paths of the core.
    Panic(String),
    /// The process ended (crash, `exit`). `stderr` holds the tail of its error output.
    Exited { code: Option<i32>, stderr: String },
    /// The child stopped answering for longer than [`HostConfig::hang_timeout`] of wall time.
    Hung,
}

/// I²C transaction result (`Wire.endTransmission` codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum I2cStatus {
    Ok = 0,
    AddrNack = 2,
    DataNack = 3,
    Other = 4,
}

/// Pulled inputs, answered at the exact board time of the request.
pub trait HostIo {
    /// Voltage on `pin` for `analogRead` (None → last [`HostBoard::set_pin_voltage`], else 0 V).
    fn analog_read(&mut self, pin: u8, at: SimTime) -> Option<f64> {
        let _ = (pin, at);
        None
    }
    fn i2c_write(&mut self, bus: u8, addr: u8, data: &[u8], at: SimTime) -> I2cStatus {
        let _ = (bus, addr, data, at);
        I2cStatus::AddrNack
    }
    /// `None` = address NACK.
    fn i2c_read(&mut self, bus: u8, addr: u8, len: usize, at: SimTime) -> Option<Vec<u8>> {
        let _ = (bus, addr, len, at);
        None
    }
    fn spi_transfer(&mut self, bus: u8, data: &[u8], at: SimTime) -> Vec<u8> {
        let _ = (bus, at);
        vec![0xff; data.len()]
    }
    /// Radio services (Wi-Fi/sockets on channel 1, BLE on channel 2) — forward to
    /// `bb_radio::RadioSpace::host_service`. Default: unavailable (`-1`).
    fn service(&mut self, channel: u8, request: &[u8], at: SimTime) -> Vec<u8> {
        let _ = (channel, request, at);
        (-1i32).to_le_bytes().to_vec()
    }
}

impl HostIo for () {}

/// Board options.
#[derive(Debug, Clone)]
pub struct HostConfig {
    pub kind: HostKind,
    /// Seed for `random()` / `esp_random()`.
    pub seed: u64,
    /// File backing `Preferences` (NVS); persists across restarts and re-flashes.
    pub nvs_path: Option<PathBuf>,
    /// While the sketch uses asynchronous callbacks (BLE), sleeping sketches are woken this
    /// often so callbacks run (≈ FreeRTOS task latency).
    pub poll_interval: SimTime,
    /// Return from `run` after each output change so bit-banged protocols (DHT) can be answered.
    pub stop_on_output: bool,
    /// Wall-clock limit for the child to send its next message.
    pub hang_timeout: Duration,
}

impl HostConfig {
    pub fn new(kind: HostKind) -> Self {
        HostConfig { kind, seed: 1, nvs_path: None, poll_interval: SimTime::from_ms(10), stop_on_output: false, hang_timeout: Duration::from_secs(10) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Blocked {
    Sleep { deadline: SimTime },
    WaitPin { pin: u8, level: bool, deadline: SimTime },
}

#[derive(Debug, Clone, Default)]
struct PinSt {
    mode: u8,
    out: bool,
    ext: Option<bool>,
    volts: Option<f64>,
    irq: Option<u8>,
}

enum Incoming {
    Msg(u8, Vec<u8>),
    Eof,
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Incoming>,
    stderr: Arc<Mutex<String>>,
}

/// A running host-emulated board.
pub struct HostBoard {
    pub config: HostConfig,
    exe: PathBuf,
    proc: Option<Proc>,
    now: SimTime,
    blocked: Option<Blocked>,
    pins: HashMap<u8, PinSt>,
    sched: BTreeMap<(SimTime, u64), (u8, Option<bool>)>,
    sched_seq: u64,
    irqs: Vec<u8>,
    events: Vec<HostEvent>,
    serial_tx: HashMap<u8, Vec<(SimTime, u8)>>,
    serial_rx: HashMap<u8, VecDeque<u8>>,
    serial_baud: HashMap<u8, u32>,
    serial_free: HashMap<u8, SimTime>,
    spin: u32,
    async_on: bool,
    async_next: SimTime,
    stopped: Option<HostStop>,
    messages: u64,
}

impl std::fmt::Debug for HostBoard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostBoard").field("kind", &self.config.kind).field("exe", &self.exe).field("now", &self.now).finish()
    }
}

fn spawn(exe: &Path, cfg: &HostConfig) -> Result<Proc, FirmwareError> {
    let mut cmd = Command::new(exe);
    cmd.env("BB_HOST_BOARD", cfg.kind.id()).env("BB_HOST_SEED", cfg.seed.to_string()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(p) = &cfg.nvs_path {
        cmd.env("BB_HOST_NVS", p);
    }
    let mut child = cmd.spawn().map_err(|e| FirmwareError::Io(format!("cannot start {}: {e}", exe.display())))?;
    let stdin = child.stdin.take().ok_or_else(|| FirmwareError::Io("no stdin".into()))?;
    let mut stdout = child.stdout.take().ok_or_else(|| FirmwareError::Io("no stdout".into()))?;
    let mut err = child.stderr.take().ok_or_else(|| FirmwareError::Io("no stderr".into()))?;
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("bb-host-rx".into())
        .spawn(move || {
            let mut r = std::io::BufReader::with_capacity(1 << 16, &mut stdout);
            loop {
                let mut h = [0u8; 4];
                if r.read_exact(&mut h).is_err() {
                    let _ = tx.send(Incoming::Eof);
                    return;
                }
                let len = u32::from_le_bytes(h) as usize;
                if len == 0 || len > 64 << 20 {
                    let _ = tx.send(Incoming::Eof);
                    return;
                }
                let mut body = vec![0u8; len];
                if r.read_exact(&mut body).is_err() {
                    let _ = tx.send(Incoming::Eof);
                    return;
                }
                let ty = body[0];
                body.remove(0);
                if tx.send(Incoming::Msg(ty, body)).is_err() {
                    return;
                }
            }
        })
        .map_err(|e| FirmwareError::Io(e.to_string()))?;
    let stderr = Arc::new(Mutex::new(String::new()));
    let s2 = stderr.clone();
    std::thread::Builder::new()
        .name("bb-host-err".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(mut s) = s2.lock() {
                    s.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if s.len() > 64 * 1024 {
                        let cut = s.len() - 32 * 1024;
                        let cut = (cut..s.len()).find(|&i| s.is_char_boundary(i)).unwrap_or(s.len());
                        s.drain(..cut);
                    }
                }
            }
        })
        .map_err(|e| FirmwareError::Io(e.to_string()))?;
    Ok(Proc { child, stdin, rx, stderr })
}

impl HostBoard {
    /// Starts the sketch executable at board time 0.
    pub fn spawn(exe: &Path, config: HostConfig) -> Result<HostBoard, FirmwareError> {
        let proc = spawn(exe, &config)?;
        Ok(HostBoard {
            config,
            exe: exe.to_path_buf(),
            proc: Some(proc),
            now: SimTime::ZERO,
            blocked: None,
            pins: HashMap::new(),
            sched: BTreeMap::new(),
            sched_seq: 0,
            irqs: vec![],
            events: vec![],
            serial_tx: HashMap::new(),
            serial_rx: HashMap::new(),
            serial_baud: HashMap::new(),
            serial_free: HashMap::new(),
            spin: 0,
            async_on: false,
            async_next: SimTime::MAX,
            stopped: None,
            messages: 0,
        })
    }

    /// Board clock.
    pub fn time(&self) -> SimTime {
        self.now
    }

    /// Messages exchanged so far (performance diagnostics).
    pub fn message_count(&self) -> u64 {
        self.messages
    }

    /// Restarts the sketch (reset button / re-flash with `exe`), keeping NVS. Pins return to
    /// inputs; the board clock keeps running.
    pub fn reset(&mut self, exe: Option<&Path>) -> Result<(), FirmwareError> {
        if let Some(e) = exe {
            self.exe = e.to_path_buf();
        }
        self.kill();
        self.proc = Some(spawn(&self.exe, &self.config)?);
        self.blocked = None;
        self.stopped = None;
        self.async_on = false;
        self.async_next = SimTime::MAX;
        self.irqs.clear();
        for (pin, p) in self.pins.iter_mut() {
            p.irq = None;
            if p.mode != MODE_INPUT {
                p.mode = MODE_INPUT;
                self.events.push(HostEvent::Pin { time: self.now, pin: *pin, state: HostPinState::Input { pull: Pull::None } });
            }
        }
        Ok(())
    }

    fn kill(&mut self) {
        if let Some(mut p) = self.proc.take() {
            let _ = p.child.kill();
            let _ = p.child.wait();
        }
    }

    // ================================================================ inputs

    /// Line level seen by `pin` from outside (`None` = nothing connected).
    pub fn set_pin_input(&mut self, pin: u8, level: Option<bool>) {
        self.apply_input(pin, level);
    }

    /// Changes an input level at an exact future time (sensor protocols, UART bits, buttons).
    pub fn schedule_pin_input(&mut self, pin: u8, level: Option<bool>, at: SimTime) {
        self.sched_seq += 1;
        self.sched.insert((at.max(self.now), self.sched_seq), (pin, level));
    }

    /// Fallback voltage for `analogRead` when [`HostIo::analog_read`] returns `None`.
    pub fn set_pin_voltage(&mut self, pin: u8, volts: f64) {
        self.pins.entry(pin).or_default().volts = Some(volts);
    }

    /// Bytes arriving on UART `port` (the sketch reads them with `SerialN.read()`).
    pub fn serial_inject(&mut self, port: u8, bytes: &[u8]) {
        self.serial_rx.entry(port).or_default().extend(bytes);
    }

    // ================================================================ outputs

    pub fn take_events(&mut self) -> Vec<HostEvent> {
        std::mem::take(&mut self.events)
    }

    /// Bytes written to UART `port`, stamped with the end of their stop bit (paced at the
    /// configured baud).
    pub fn take_serial(&mut self, port: u8) -> Vec<(SimTime, u8)> {
        self.serial_tx.get_mut(&port).map(std::mem::take).unwrap_or_default()
    }

    pub fn serial_baud(&self, port: u8) -> Option<u32> {
        self.serial_baud.get(&port).copied()
    }

    /// Current logical state of a pin.
    pub fn pin_state(&self, pin: u8) -> HostPinState {
        self.pins.get(&pin).map(state_of).unwrap_or(HostPinState::Input { pull: Pull::None })
    }

    /// The child's stderr so far (sketch `printf`, crash messages).
    pub fn stderr(&self) -> String {
        self.proc.as_ref().and_then(|p| p.stderr.lock().ok().map(|s| s.clone())).unwrap_or_default()
    }

    // ================================================================ running

    fn level(&self, pin: u8) -> bool {
        let Some(p) = self.pins.get(&pin) else { return false };
        if let Some(e) = p.ext {
            return e;
        }
        match state_of(p) {
            HostPinState::Output { high } => high,
            HostPinState::OpenDrain { low } => !low,
            HostPinState::Input { pull } => pull == Pull::Up,
        }
    }

    fn apply_input(&mut self, pin: u8, level: Option<bool>) {
        let before = self.level(pin);
        self.pins.entry(pin).or_default().ext = level;
        let after = self.level(pin);
        if let Some(mode) = self.pins.get(&pin).and_then(|p| p.irq) {
            let fire = match mode {
                IRQ_RISING => !before && after,
                IRQ_FALLING => before && !after,
                IRQ_CHANGE => before != after,
                IRQ_ONLOW => before != after && !after,
                IRQ_ONHIGH => before != after && after,
                _ => false,
            };
            if fire {
                self.irqs.push(pin);
            }
        }
    }

    fn reply(&mut self, payload: &[u8]) -> bool {
        let irqs = std::mem::take(&mut self.irqs);
        let frame = reply_frame(self.now.as_ps() / 1_000_000, &irqs, payload);
        let Some(p) = self.proc.as_mut() else { return false };
        p.stdin.write_all(&frame).and_then(|_| p.stdin.flush()).is_ok()
    }

    /// Applies scheduled inputs up to `t` (inclusive). Stops early (returning that time) at the
    /// first one that satisfies `want` (pin, level) or, with `stop_on_irq`, raises an interrupt.
    fn apply_scheduled(&mut self, t: SimTime, want: Option<(u8, bool)>, stop_on_irq: bool) -> Option<SimTime> {
        while let Some((&(at, seq), _)) = self.sched.iter().next() {
            if at > t {
                break;
            }
            let (pin, level) = self.sched.remove(&(at, seq)).expect("present");
            self.now = self.now.max(at);
            self.apply_input(pin, level);
            if (stop_on_irq && !self.irqs.is_empty()) || want.is_some_and(|(p, l)| self.level(p) == l) {
                return Some(self.now);
            }
        }
        None
    }

    fn cost(&mut self, base_ns: u64) {
        self.now += SimTime::from_ns(base_ns);
    }

    /// Spin-aware cost for time/status polls: cheap at first, growing in long busy-wait loops so
    /// `while (millis() < t) {}` advances quickly.
    fn poll_cost(&mut self, base_ns: u64) {
        self.spin = self.spin.saturating_add(1);
        let k = if self.spin > 1000 { (self.spin / 1000) as u64 + 1 } else { 1 };
        self.cost((base_ns * k).min(50_000));
    }

    /// Runs the board until `until` (or an early stop). Inputs scheduled for times ≤ `until`
    /// are applied at their exact times.
    pub fn run(&mut self, until: SimTime, io: &mut dyn HostIo) -> HostStop {
        if let Some(s) = &self.stopped {
            return s.clone();
        }
        loop {
            if let Some(b) = self.blocked {
                match b {
                    Blocked::Sleep { deadline } => {
                        let mut wake = deadline;
                        if self.async_on {
                            wake = wake.min(self.async_next.max(self.now));
                        }
                        let target = wake.min(until);
                        if let Some(t) = self.apply_scheduled(target, None, true) {
                            self.now = t;
                        } else {
                            self.now = self.now.max(target);
                            if target < wake {
                                return HostStop::ReachedTime;
                            }
                        }
                        if self.async_on && self.now >= self.async_next {
                            self.async_next = self.now + self.config.poll_interval;
                        }
                        self.blocked = None;
                        let d = deadline.as_ps() / 1_000_000;
                        if !self.reply(&Wr::default().u64(d).0) {
                            return self.finish();
                        }
                    }
                    Blocked::WaitPin { pin, level, deadline } => {
                        if self.level(pin) != level {
                            let target = deadline.min(until);
                            match self.apply_scheduled(target, Some((pin, level)), false) {
                                Some(t) => self.now = t,
                                None => {
                                    self.now = self.now.max(target);
                                    if target < deadline {
                                        return HostStop::ReachedTime;
                                    }
                                }
                            }
                        }
                        let ok = self.level(pin) == level;
                        self.blocked = None;
                        if !self.reply(&[ok as u8]) {
                            return self.finish();
                        }
                    }
                }
                continue;
            }
            if self.now >= until {
                return HostStop::ReachedTime;
            }
            // The child is running: wait for its next message.
            let msg = match self.proc.as_ref().map(|p| p.rx.recv_timeout(self.config.hang_timeout)) {
                Some(Ok(m)) => m,
                Some(Err(RecvTimeoutError::Timeout)) => {
                    self.stopped = Some(HostStop::Hung);
                    self.kill();
                    return HostStop::Hung;
                }
                Some(Err(RecvTimeoutError::Disconnected)) | None => Incoming::Eof,
            };
            let (ty, body) = match msg {
                Incoming::Msg(t, b) => (t, b),
                Incoming::Eof => return self.finish(),
            };
            self.messages += 1;
            // Inputs scheduled before the current board time take effect first.
            self.apply_scheduled(self.now, None, false);
            let mut output = false;
            match self.handle(ty, &body, io, &mut output) {
                Ok(()) => {}
                Err(stop) => {
                    self.stopped = Some(stop.clone());
                    return stop;
                }
            }
            if output && self.config.stop_on_output {
                return HostStop::Output;
            }
        }
    }

    fn finish(&mut self) -> HostStop {
        let code = self.proc.as_mut().and_then(|p| p.child.wait().ok()).and_then(|s| s.code());
        let stderr = self.stderr();
        let s = HostStop::Exited { code, stderr };
        self.stopped = Some(s.clone());
        s
    }

    fn handle(&mut self, ty: u8, body: &[u8], io: &mut dyn HostIo, output: &mut bool) -> Result<(), HostStop> {
        let mut r = Rd::new(body);
        let ok = |b: &mut HostBoard, payload: &[u8]| -> Result<(), HostStop> {
            if b.reply(payload) {
                Ok(())
            } else {
                Err(HostStop::Exited { code: None, stderr: b.stderr() })
            }
        };
        if !matches!(ty, MSG_TIME | MSG_YIELD | MSG_DIGITAL_READ | MSG_SERIAL_READ | MSG_SERIAL_AVAILABLE) {
            self.spin = 0;
        }
        match ty {
            MSG_HELLO => ok(self, &[]),
            MSG_TIME => {
                self.poll_cost(250);
                ok(self, &[])
            }
            MSG_YIELD => {
                self.poll_cost(1_000);
                ok(self, &[])
            }
            MSG_SLEEP => {
                let kind = r.u8();
                let v = r.u64();
                let deadline = if kind == 1 { self.now + SimTime::from_us(v) } else { SimTime::from_us(v) };
                self.blocked = Some(Blocked::Sleep { deadline });
                Ok(())
            }
            MSG_PIN_MODE => {
                let (pin, mode) = (r.u8(), r.u8());
                self.cost(100);
                let p = self.pins.entry(pin).or_default();
                let before = state_of(p);
                p.mode = mode;
                let after = state_of(p);
                if before != after {
                    self.events.push(HostEvent::Pin { time: self.now, pin, state: after });
                    *output = true;
                }
                Ok(())
            }
            MSG_DIGITAL_WRITE => {
                let (pin, v) = (r.u8(), r.u8() != 0);
                self.cost(100);
                let p = self.pins.entry(pin).or_default();
                let before = state_of(p);
                p.out = v;
                if p.mode == 0 {
                    // digitalWrite before pinMode: ESP32 leaves the pin an input.
                    return Ok(());
                }
                let after = state_of(p);
                if before != after {
                    self.events.push(HostEvent::Pin { time: self.now, pin, state: after });
                    *output = true;
                }
                Ok(())
            }
            MSG_DIGITAL_READ => {
                let pin = r.u8();
                self.poll_cost(100);
                self.apply_scheduled(self.now, None, false);
                let l = self.level(pin);
                ok(self, &[l as u8])
            }
            MSG_ANALOG_READ => {
                let pin = r.u8();
                let conv = if self.config.kind == HostKind::Pico { 2_000 } else { 10_000 };
                self.cost(conv);
                let v = io.analog_read(pin, self.now).or_else(|| self.pins.get(&pin).and_then(|p| p.volts)).unwrap_or(0.0);
                ok(self, &Wr::default().f64(v).0)
            }
            MSG_PWM => {
                let (pin, duty, full, freq) = (r.u8(), r.u32(), r.u32(), r.u32());
                self.cost(1_000);
                let d = if full == 0 { 0.0 } else { (duty as f64 / full as f64).clamp(0.0, 1.0) };
                self.pins.entry(pin).or_default().mode = MODE_OUTPUT;
                self.events.push(HostEvent::Pwm { time: self.now, pin, duty: d, freq_hz: freq as f64 });
                *output = true;
                Ok(())
            }
            MSG_ATTACH_IRQ => {
                let (pin, mode) = (r.u8(), r.u8());
                self.pins.entry(pin).or_default().irq = Some(mode);
                Ok(())
            }
            MSG_DETACH_IRQ => {
                let pin = r.u8();
                self.pins.entry(pin).or_default().irq = None;
                Ok(())
            }
            MSG_SERIAL_BEGIN => {
                let (port, baud) = (r.u8(), r.u32());
                self.serial_baud.insert(port, baud.max(1));
                Ok(())
            }
            MSG_SERIAL_WRITE => {
                let port = r.u8();
                let data = r.rest();
                let baud = *self.serial_baud.get(&port).unwrap_or(&115_200) as f64;
                let frame = SimTime::from_secs_f64(10.0 / baud);
                let mut t = (*self.serial_free.get(&port).unwrap_or(&SimTime::ZERO)).max(self.now);
                let out = self.serial_tx.entry(port).or_default();
                for &b in data {
                    t += frame;
                    out.push((t, b));
                }
                self.serial_free.insert(port, t);
                Ok(())
            }
            MSG_SERIAL_READ => {
                let (port, max) = (r.u8(), r.u16() as usize);
                self.poll_cost(500);
                let q = self.serial_rx.entry(port).or_default();
                let n = max.min(q.len());
                let data: Vec<u8> = q.drain(..n).collect();
                if !data.is_empty() {
                    self.spin = 0;
                }
                ok(self, &Wr::default().bytes(&data).0)
            }
            MSG_SERIAL_AVAILABLE => {
                let port = r.u8();
                self.poll_cost(500);
                let n = self.serial_rx.get(&port).map(|q| q.len()).unwrap_or(0) as u32;
                ok(self, &n.to_le_bytes())
            }
            MSG_I2C_WRITE => {
                let (bus, addr, freq, _stop) = (r.u8(), r.u8(), r.u32().max(1), r.u8());
                let data = r.bytes().to_vec();
                self.cost((data.len() as u64 + 1) * 9 * 1_000_000_000 / freq as u64);
                let st = io.i2c_write(bus, addr, &data, self.now);
                ok(self, &[st as u8])
            }
            MSG_I2C_READ => {
                let (bus, addr, freq, _stop, n) = (r.u8(), r.u8(), r.u32().max(1), r.u8(), r.u16() as usize);
                self.cost((n as u64 + 1) * 9 * 1_000_000_000 / freq as u64);
                let data = io.i2c_read(bus, addr, n, self.now).unwrap_or_default();
                ok(self, &Wr::default().bytes(&data[..data.len().min(n)]).0)
            }
            MSG_SPI_TRANSFER => {
                let (bus, clock, _mode) = (r.u8(), r.u32().max(1), r.u8());
                let data = r.bytes().to_vec();
                self.cost(data.len() as u64 * 8 * 1_000_000_000 / clock as u64);
                let mut back = io.spi_transfer(bus, &data, self.now);
                back.resize(data.len(), 0xff);
                ok(self, &Wr::default().bytes(&back).0)
            }
            MSG_SERVICE => {
                let ch = r.u8();
                let req = r.rest().to_vec();
                self.cost(20_000);
                let resp = io.service(ch, &req, self.now);
                ok(self, &resp)
            }
            MSG_WAIT_PIN => {
                let (pin, level, timeout) = (r.u8(), r.u8() != 0, r.u32());
                self.blocked = Some(Blocked::WaitPin { pin, level, deadline: self.now + SimTime::from_us(timeout as u64) });
                Ok(())
            }
            MSG_LOG => {
                let text = r.string();
                self.events.push(HostEvent::Log { time: self.now, text });
                Ok(())
            }
            MSG_ASYNC => {
                self.async_on = r.u8() != 0;
                self.async_next = if self.async_on { self.now + self.config.poll_interval } else { SimTime::MAX };
                Ok(())
            }
            MSG_RESTART => {
                // Reply to the trailing YIELD is never read; respawn now.
                let t = self.now;
                self.reset(None).map_err(|e| HostStop::Panic(e.to_string()))?;
                self.events.push(HostEvent::Restarted { time: t });
                Ok(())
            }
            MSG_PANIC => Err(HostStop::Panic(r.string())),
            other => Err(HostStop::Panic(format!("unknown message type 0x{other:02x}"))),
        }
    }
}

impl Drop for HostBoard {
    fn drop(&mut self) {
        self.kill();
    }
}

fn state_of(p: &PinSt) -> HostPinState {
    match p.mode {
        MODE_OUTPUT => HostPinState::Output { high: p.out },
        MODE_OUTPUT_OPEN_DRAIN => HostPinState::OpenDrain { low: !p.out },
        MODE_INPUT_PULLUP => HostPinState::Input { pull: Pull::Up },
        MODE_INPUT_PULLDOWN => HostPinState::Input { pull: Pull::Down },
        _ => HostPinState::Input { pull: Pull::None },
    }
}
