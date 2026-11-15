//! API-level boards (ESP32 DevKit, Raspberry Pi Pico) on the bench: [`HostMcu`] adapts
//! `bb_firmware::host::HostBoard` — a host-compiled sketch running as a child process in
//! lock-step with simulated time — to the scheduler's [`McuCore`] trait.
//!
//! * Pin changes come back as timestamped events; `stop_on_output` makes the board return after
//!   each one so bit-banged protocols (DHT22, 1-Wire) see the bench's answer at the right time.
//! * PWM (`analogWrite`, `ledcWrite`, `tone`) is reported by the core as duty + frequency; the
//!   adapter turns it into real edges so LEDs, RC filters, servos and scopes see a waveform.
//! * `analogRead` is answered from the bench's current analog solution (it may lag the board
//!   by one sync quantum — fine for an API-level model).
//! * Radio requests (Wi-Fi, sockets, BLE) go to the bench's services
//!   ([`crate::Service::host_service`]; [`RadioService`] serves them from a `bb_radio::RadioSpace`).
//! * EN / RUN held low and supply brown-out freeze the board; release restarts the sketch.
//!   Board time then runs behind bench time by the time spent in reset (`offset`).

use crate::mcu::{LogicDrive, McuCore, McuIo, McuPinEvent, McuStop};
use bb_avr::FirmwareImage;
use bb_core::SimTime;
use bb_firmware::host::{
    HostBoard, HostBoardSpec, HostConfig, HostEvent, HostIo, HostKind, HostPinState, HostStop,
    I2cStatus, Pull,
};
use bb_radio::uart::{UartConfig, UartDecoder, UartEncoder};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::Path;

/// Supply below which an API-level board stops (brown-out), and above which it restarts.
const BROWN_OUT_V: f64 = 2.3;
const POWER_ON_V: f64 = 2.7;

#[derive(Debug, Clone, Copy)]
struct Pwm {
    duty: f64,
    period: SimTime,
    /// Start of the current cycle (bench time).
    t0: SimTime,
    high: bool,
}

impl Pwm {
    fn next_edge(&self) -> SimTime {
        if self.high {
            self.t0 + SimTime::from_secs_f64(self.period.as_secs_f64() * self.duty)
        } else {
            self.t0 + self.period
        }
    }
}

/// A hardware UART other than the USB serial (`Serial1`, `Serial2`) on its default pins: bytes
/// the sketch writes become a real waveform on TX; edges on RX are decoded into its input.
struct PinUart {
    port: u8,
    rx: Option<usize>,
    tx: Option<usize>,
    enc: UartEncoder,
    dec: UartDecoder,
    baud: u32,
    used: bool,
}

/// An ESP32 / Pico sketch on the bench.
pub struct HostMcu {
    board: HostBoard,
    spec: HostBoardSpec,
    names: Vec<String>,
    gpio: Vec<u8>,
    by_gpio: HashMap<u8, usize>,
    drive: Vec<LogicDrive>,
    pwm: Vec<Option<Pwm>>,
    events: Vec<McuPinEvent>,
    logs: Vec<(SimTime, String)>,
    /// bench time = board time + offset (grows while the board is held in reset).
    offset: SimTime,
    /// While held or stopped, the bench time this core has been advanced to.
    idle_until: SimTime,
    reset_low: bool,
    brown: bool,
    dead: Option<String>,
    resets: u64,
    last_reset: String,
    uarts: Vec<PinUart>,
    /// Scheduled level changes (UART bits) by bench time.
    edges: VecDeque<(SimTime, usize, bool)>,
    /// Bytes of ports ≥ 1 for `take_serial` (UI).
    serial_out: Vec<(usize, SimTime, u8)>,
    sda: Option<usize>,
    sck: Option<usize>,
}

impl HostMcu {
    /// Spawns the sketch executable `exe` built for `board` (`"esp32"`, `"pico"`).
    pub fn spawn(board: &str, exe: &Path, seed: u64) -> Result<HostMcu, String> {
        let kind = HostKind::by_id(board).ok_or_else(|| format!("unknown host board {board}"))?;
        let mut cfg = HostConfig::new(kind);
        cfg.seed = seed;
        cfg.stop_on_output = true;
        let hb = HostBoard::spawn(exe, cfg).map_err(|e| e.to_string())?;
        let spec = kind.spec();
        let names: Vec<String> = spec.pins.iter().map(|p| p.name.clone()).collect();
        let gpio: Vec<u8> = spec.pins.iter().map(|p| p.gpio).collect();
        let by_gpio = gpio.iter().enumerate().map(|(i, g)| (*g, i)).collect();
        let n = names.len();
        let idx = |g: u8| gpio.iter().position(|x| *x == g);
        // UART pins: ESP32 port p ↔ spec.serial[p]; on the Pico port 0 is USB and port p ≥ 1 is
        // spec.serial[p - 1]
        let mut uarts = Vec::new();
        for (i, (rx, tx)) in spec.serial.iter().enumerate() {
            let port = match kind {
                HostKind::Pico => i as u8 + 1,
                HostKind::Esp32DevKit => i as u8,
            };
            if port == 0 {
                continue;
            }
            let cfg = UartConfig::new(115_200);
            uarts.push(PinUart {
                port,
                rx: idx(*rx),
                tx: idx(*tx),
                enc: UartEncoder::new(cfg),
                dec: UartDecoder::new(cfg),
                baud: 115_200,
                used: false,
            });
        }
        let sda = idx(spec.sda);
        let sck = idx(spec.sck);
        Ok(HostMcu {
            board: hb,
            spec,
            names,
            gpio,
            by_gpio,
            drive: vec![LogicDrive::Input { pullup: false }; n],
            pwm: vec![None; n],
            events: Vec::new(),
            logs: Vec::new(),
            offset: SimTime::ZERO,
            idle_until: SimTime::ZERO,
            reset_low: false,
            brown: false,
            dead: None,
            resets: 0,
            last_reset: "power-on".into(),
            uarts,
            edges: VecDeque::new(),
            serial_out: Vec::new(),
            sda,
            sck,
        })
    }

    /// Serial1/2 output → TX waveforms; follows the configured baud. Like a real UART the TX
    /// line idles high from `SerialN.begin()`.
    fn pump_uarts(&mut self) {
        let off = self.offset;
        let now = self.bench_now();
        for u in 0..self.uarts.len() {
            let port = self.uarts[u].port;
            if !self.uarts[u].used
                && self.board.serial_baud(port).is_some()
                && let Some(tx) = self.uarts[u].tx
            {
                self.uarts[u].used = true;
                self.set_drive(tx, LogicDrive::Output { high: true }, now);
            }
            if let Some(b) = self.board.serial_baud(port)
                && b != self.uarts[u].baud
                && b > 0
            {
                let cfg = UartConfig::new(b);
                self.uarts[u].baud = b;
                self.uarts[u].enc = UartEncoder::new(cfg);
                if !self.uarts[u].dec.busy() {
                    self.uarts[u].dec.set_config(cfg);
                }
            }
            let bytes = self.board.take_serial(port);
            if bytes.is_empty() {
                continue;
            }
            let frame = SimTime::from_secs_f64(10.0 / self.uarts[u].baud.max(1) as f64);
            for (t, b) in bytes {
                let t = t + off;
                self.serial_out.push((port as usize, t, b));
                let Some(tx) = self.uarts[u].tx else { continue };
                let mut ready = t.saturating_sub(frame);
                if !self.uarts[u].used {
                    // first byte before we saw begin(): idle high for a frame first
                    self.uarts[u].used = true;
                    self.set_drive(tx, LogicDrive::Output { high: true }, ready);
                    ready += frame;
                }
                for (et, level) in self.uarts[u].enc.encode(&[b], ready) {
                    self.edges.push_back((et, tx, level));
                }
            }
        }
        self.edges.make_contiguous().sort_by_key(|e| e.0);
    }

    /// Delivers decoded Serial1/2 input to the sketch up to `now`.
    fn uart_input(&mut self, now: SimTime) {
        for u in 0..self.uarts.len() {
            let bytes: Vec<u8> = self.uarts[u]
                .dec
                .poll(now)
                .into_iter()
                .filter(|b| b.ok())
                .map(|b| b.data as u8)
                .collect();
            if !bytes.is_empty() {
                let port = self.uarts[u].port;
                self.board.serial_inject(port, &bytes);
            }
        }
    }

    fn next_uart_decision(&self) -> Option<SimTime> {
        self.uarts
            .iter()
            .filter_map(|u| u.dec.next_decision())
            .min()
    }

    fn uart_rx_edge(&mut self, pin: usize, level: Option<bool>, at: SimTime) {
        for u in self.uarts.iter_mut() {
            if u.rx == Some(pin)
                && let Some(l) = level
            {
                u.dec.edge(at, l);
            }
        }
    }

    /// The board catalogue entry (pins, ADC, PWM, buses).
    pub fn board_spec(&self) -> &HostBoardSpec {
        &self.spec
    }

    /// Error output of the sketch process (crash diagnostics).
    pub fn stderr(&self) -> String {
        self.board.stderr()
    }

    fn held(&self) -> bool {
        self.reset_low || self.brown
    }

    fn bench_now(&self) -> SimTime {
        self.board.time() + self.offset
    }

    fn set_drive(&mut self, idx: usize, d: LogicDrive, at: SimTime) {
        if self.drive[idx] != d {
            self.drive[idx] = d;
            self.events.push(McuPinEvent {
                time: at,
                pin: idx,
                drive: d,
            });
        }
    }

    /// Board events → pin events (bench time). Returns whether any pin changed.
    fn collect(&mut self) -> bool {
        let before = self.events.len();
        let off = self.offset;
        for e in self.board.take_events() {
            match e {
                HostEvent::Pin { time, pin, state } => {
                    let Some(&idx) = self.by_gpio.get(&pin) else {
                        continue;
                    };
                    self.pwm[idx] = None;
                    let d = match state {
                        HostPinState::Input { pull } => LogicDrive::Input {
                            pullup: pull == Pull::Up,
                        },
                        HostPinState::Output { high } => LogicDrive::Output { high },
                        HostPinState::OpenDrain { low: true } => LogicDrive::Output { high: false },
                        HostPinState::OpenDrain { low: false } => {
                            LogicDrive::Input { pullup: false }
                        }
                    };
                    self.set_drive(idx, d, time + off);
                }
                HostEvent::Pwm {
                    time,
                    pin,
                    duty,
                    freq_hz,
                } => {
                    let Some(&idx) = self.by_gpio.get(&pin) else {
                        continue;
                    };
                    let at = time + off;
                    let duty = duty.clamp(0.0, 1.0);
                    if freq_hz <= 0.0 || duty <= 0.0 || duty >= 1.0 {
                        self.pwm[idx] = None;
                        self.set_drive(idx, LogicDrive::Output { high: duty >= 0.5 }, at);
                    } else {
                        self.pwm[idx] = Some(Pwm {
                            duty,
                            period: SimTime::from_secs_f64(1.0 / freq_hz),
                            t0: at,
                            high: true,
                        });
                        self.set_drive(idx, LogicDrive::Output { high: true }, at);
                    }
                }
                HostEvent::Log { time, text } => self.logs.push((time + off, text)),
                HostEvent::Restarted { time } => {
                    self.resets += 1;
                    self.last_reset = "software (ESP.restart)".into();
                    self.logs.push((time + off, "sketch restarted".into()));
                }
            }
        }
        self.events.len() > before
    }

    fn next_pwm_edge(&self) -> Option<SimTime> {
        let p = self.pwm.iter().flatten().map(Pwm::next_edge).min();
        let e = self.edges.front().map(|e| e.0);
        match (p, e) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Emits every PWM edge due by `now`. Returns whether any was emitted.
    fn pwm_due(&mut self, now: SimTime) -> bool {
        let mut any = false;
        while let Some(&(t, pin, level)) = self.edges.front() {
            if t > now {
                break;
            }
            self.edges.pop_front();
            self.set_drive(pin, LogicDrive::Output { high: level }, t);
            any = true;
        }
        for idx in 0..self.pwm.len() {
            while let Some(mut p) = self.pwm[idx] {
                let e = p.next_edge();
                if e > now {
                    break;
                }
                if p.high {
                    p.high = false;
                } else {
                    p.high = true;
                    p.t0 = e;
                }
                self.pwm[idx] = Some(p);
                self.set_drive(idx, LogicDrive::Output { high: p.high }, e);
                any = true;
            }
        }
        any
    }

    /// All pins become inputs (reset / brown-out).
    fn release_pins(&mut self, at: SimTime) {
        for idx in 0..self.drive.len() {
            self.pwm[idx] = None;
            self.set_drive(idx, LogicDrive::Input { pullup: false }, at);
        }
    }

    fn hold(&mut self) {
        let now = self.now();
        self.idle_until = now;
        self.release_pins(now);
    }

    /// Restarts the sketch at the current bench time.
    fn restart(&mut self, cause: &str, exe: Option<&Path>) -> Result<(), String> {
        let now = self.now();
        self.offset = now.saturating_sub(self.board.time());
        self.release_pins(now);
        self.board.reset(exe).map_err(|e| e.to_string())?;
        self.collect();
        self.dead = None;
        self.resets += 1;
        self.last_reset = cause.into();
        Ok(())
    }
}

/// `HostIo` for one run: ADC samples from the bench, radio requests to its services.
struct HostIoAdapter<'a> {
    io: &'a mut dyn McuIo,
    by_gpio: &'a HashMap<u8, usize>,
    offset: SimTime,
    sda: Option<usize>,
    sck: Option<usize>,
}

impl HostIo for HostIoAdapter<'_> {
    fn analog_read(&mut self, pin: u8, at: SimTime) -> Option<f64> {
        let idx = *self.by_gpio.get(&pin)?;
        self.io.adc_sample(idx, at + self.offset)
    }
    fn service(&mut self, channel: u8, request: &[u8], at: SimTime) -> Vec<u8> {
        self.io
            .service(channel, request, at + self.offset)
            .unwrap_or_else(|| (-1i32).to_le_bytes().to_vec())
    }
    fn i2c_write(&mut self, bus: u8, addr: u8, data: &[u8], at: SimTime) -> I2cStatus {
        let Some(sda) = self.sda.filter(|_| bus == 0) else {
            return I2cStatus::AddrNack;
        };
        match self.io.i2c(sda, addr, data, 0, at + self.offset) {
            Some(_) => I2cStatus::Ok,
            None => I2cStatus::AddrNack,
        }
    }
    fn i2c_read(&mut self, bus: u8, addr: u8, len: usize, at: SimTime) -> Option<Vec<u8>> {
        let sda = self.sda.filter(|_| bus == 0)?;
        self.io.i2c(sda, addr, &[], len, at + self.offset)
    }
    fn spi_transfer(&mut self, bus: u8, data: &[u8], at: SimTime) -> Vec<u8> {
        let r = match self.sck.filter(|_| bus == 0) {
            Some(sck) => self.io.spi(sck, data, at + self.offset),
            None => None,
        };
        r.unwrap_or_else(|| vec![0xff; data.len()])
    }
}

impl McuCore for HostMcu {
    fn label(&self) -> String {
        format!("{} · {}", self.spec.name, self.spec.chip)
    }
    fn engine(&self) -> &'static str {
        "API-level"
    }
    fn now(&self) -> SimTime {
        self.bench_now().max(self.idle_until)
    }
    fn pin_names(&self) -> Vec<String> {
        self.names.clone()
    }
    fn pin_index(&self, name: &str) -> Option<usize> {
        if let Some(i) = self.names.iter().position(|n| n.eq_ignore_ascii_case(name)) {
            return Some(i);
        }
        let p = self.spec.pin_by_name(name)?;
        self.by_gpio.get(&p.gpio).copied()
    }
    fn run_until(&mut self, t: SimTime, io: &mut dyn McuIo) -> McuStop {
        loop {
            if self.held() || self.dead.is_some() {
                self.idle_until = self.idle_until.max(t);
                return if self.events.is_empty() {
                    McuStop::Reached
                } else {
                    McuStop::PinChange
                };
            }
            let now = self.bench_now();
            self.uart_input(now);
            if self.pwm_due(now) {
                return McuStop::PinChange;
            }
            if now >= t {
                return McuStop::Reached;
            }
            let mut run_to = self.next_pwm_edge().map_or(t, |e| e.min(t));
            if let Some(d) = self.next_uart_decision()
                && d > now
            {
                run_to = run_to.min(d);
            }
            let stop = {
                let mut a = HostIoAdapter {
                    io: &mut *io,
                    by_gpio: &self.by_gpio,
                    offset: self.offset,
                    sda: self.sda,
                    sck: self.sck,
                };
                self.board.run(
                    run_to.saturating_sub(self.offset).max(self.board.time()),
                    &mut a,
                )
            };
            let mut changed = self.collect();
            let before = self.events.len();
            self.pump_uarts();
            changed |= self.events.len() > before;
            match stop {
                HostStop::ReachedTime | HostStop::Output => {
                    if changed {
                        return McuStop::PinChange;
                    }
                    if self.bench_now() < run_to && matches!(stop, HostStop::ReachedTime) {
                        // the board could not reach the target (should not happen)
                        self.idle_until = self.idle_until.max(run_to);
                        return McuStop::Reached;
                    }
                }
                HostStop::Panic(msg) => {
                    let m = format!("sketch panicked: {msg}");
                    self.dead = Some(m.clone());
                    self.idle_until = self.bench_now();
                    return McuStop::Fault(m);
                }
                HostStop::Exited { code, stderr } => {
                    let tail: String = stderr
                        .lines()
                        .rev()
                        .take(3)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join(" / ");
                    let m = match code {
                        Some(c) => format!("sketch exited with code {c}. {tail}"),
                        None => format!("sketch crashed. {tail}"),
                    };
                    self.dead = Some(m.clone());
                    self.idle_until = self.bench_now();
                    return McuStop::Fault(m);
                }
                HostStop::Hung => {
                    let m = "sketch stopped answering (busy loop without delay or yield?)";
                    self.dead = Some(m.into());
                    self.idle_until = self.bench_now();
                    return McuStop::Fault(m.into());
                }
            }
        }
    }
    fn take_pin_events(&mut self) -> Vec<McuPinEvent> {
        let mut v = std::mem::take(&mut self.events);
        v.sort_by_key(|e| e.time);
        v
    }
    fn pin_drive(&self, pin: usize) -> LogicDrive {
        self.drive
            .get(pin)
            .copied()
            .unwrap_or(LogicDrive::Input { pullup: false })
    }
    fn set_input(&mut self, pin: usize, level: Option<bool>) {
        let now = self.now();
        self.uart_rx_edge(pin, level, now);
        if let Some(g) = self.gpio.get(pin) {
            self.board.set_pin_input(*g, level);
        }
    }
    fn schedule_input(&mut self, pin: usize, level: Option<bool>, at: SimTime) {
        self.uart_rx_edge(pin, level, at);
        if let Some(g) = self.gpio.get(pin) {
            let at = at.saturating_sub(self.offset);
            if at <= self.board.time() {
                self.board.set_pin_input(*g, level);
            } else {
                self.board.schedule_pin_input(*g, level, at);
            }
        }
    }
    fn set_analog(&mut self, pin: usize, volts: f64) {
        if let Some(g) = self.gpio.get(pin) {
            self.board.set_pin_voltage(*g, volts);
        }
    }
    fn supply_adc(&mut self, _volts: f64) {}
    fn set_vcc(&mut self, volts: f64) {
        if !self.brown && volts < BROWN_OUT_V {
            self.brown = true;
            if !self.reset_low {
                self.hold();
            }
        } else if self.brown && volts > POWER_ON_V {
            self.brown = false;
            if !self.reset_low && self.restart("power-on", None).is_err() {
                self.dead = Some("could not restart the sketch".into());
            }
        }
    }
    fn set_aref(&mut self, _volts: f64) {}
    fn set_reset_line(&mut self, low: bool) {
        if low && !self.reset_low {
            self.reset_low = true;
            if !self.brown {
                self.hold();
            }
        } else if !low && self.reset_low {
            self.reset_low = false;
            if !self.brown && self.restart("external (EN/RUN)", None).is_err() {
                self.dead = Some("could not restart the sketch".into());
            }
        }
    }
    fn reset(&mut self) {
        if self.restart("external", None).is_err() {
            self.dead = Some("could not restart the sketch".into());
        }
    }
    fn reset_count(&self) -> u64 {
        self.resets
    }
    fn last_reset(&self) -> String {
        self.last_reset.clone()
    }
    fn in_reset(&self) -> bool {
        self.held()
    }
    fn set_temperature(&mut self, _celsius: f64) {}
    fn flash(&mut self, _image: &FirmwareImage) -> Result<(), String> {
        Err(format!(
            "{} runs host-compiled sketches, not AVR images",
            self.spec.name
        ))
    }
    fn flash_exe(&mut self, exe: &Path) -> Result<(), String> {
        self.restart("power-on (flashed)", Some(exe))
    }
    fn take_serial(&mut self) -> Vec<(usize, SimTime, u8)> {
        // port 0 (USB serial) straight from the board; Serial1/2 were turned into pin waveforms
        let mut out: Vec<(usize, SimTime, u8)> = self
            .board
            .take_serial(0)
            .into_iter()
            .map(|(t, b)| (0, t + self.offset, b))
            .collect();
        self.pump_uarts();
        out.append(&mut self.serial_out);
        out
    }
    fn take_log(&mut self) -> Vec<(SimTime, String)> {
        let mut v = std::mem::take(&mut self.logs);
        if let Some(d) = &self.dead
            && !d.is_empty()
        {
            v.push((self.idle_until, d.clone()));
            self.dead = Some(String::new());
        }
        v
    }
    fn serial_inject(&mut self, port: usize, bytes: &[u8]) {
        self.board.serial_inject(port as u8, bytes);
    }
    fn serial_tx_pin(&self, _port: usize) -> Option<usize> {
        None
    }
    fn set_serial_waveform(&mut self, _port: usize, _on: bool) {}
    fn serial_baud(&self, port: usize) -> f64 {
        self.board.serial_baud(port as u8).unwrap_or(115_200) as f64
    }
}

pub use crate::radio::RadioService;
