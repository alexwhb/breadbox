//! [`AvrBench`]: a [`BenchLink`] backed directly by one `bb_avr::Avr` and a
//! `bb_firmware::Debugger` — real compiled firmware, cycle-accurate, source-level debugging —
//! without a circuit. Analog inputs are fixed voltages (the design's default world: soil 42 %
//! on A0, 180 lux on A1). If the project has an ESP-01 (`esp01`) radio module, it is wired to
//! SoftwareSerial D2 (MCU TX) / D3 (MCU RX) at 9600 baud through `bb_radio::UartWire`, exactly
//! like the garden-node example, so the Radios panel shows a live `RadioSpace`.
//!
//! The co-simulation adapter over `bb_sim::SimHandle` replaces this in phase 4; it implements
//! the same trait.

use super::*;
use bb_avr::{Avr, AvrConfig, AvrIo, Pin, PinOwner, PinState, StopOn, StopReason};
use bb_firmware::{Board, DebugInfo, Debugger, Decision, HitCondition, PauseInfo, PauseReason};
use bb_radio::espat::EspAtConfig;
use bb_radio::geo::{Pos, Wall};
use bb_radio::{RadioSpace, UartWire};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const LOG_LIMIT: usize = 200;
const SERIAL_LIMIT: usize = 5000;

enum Cmd {
    Upload(Firmware, Sender<Result<UploadReport, BenchError>>),
    Run(bool),
    Reset(ResetScope),
    Speed(SimSpeed),
    Breakpoints(Vec<BreakpointSpec>),
    Debug(DebugCommand),
    Serial(Vec<u8>),
    Radio(RadioCommand),
    Analog(String, f64),
    Quit,
}

#[derive(Default)]
struct Shared {
    running: bool,
    sim_time: SimTime,
    serial: Vec<SerialLine>,
    debug: DebugViewModel,
    radio: Option<RadioPanelModel>,
    pins: Vec<PinView>,
    events: Vec<BenchEvent>,
}

/// One AVR board + optional ESP-01, on a worker thread.
pub struct AvrBench {
    tx: Sender<Cmd>,
    shared: Arc<Mutex<Shared>>,
    target: BenchTarget,
    thread: Option<JoinHandle<()>>,
    running: bool,
}

impl AvrBench {
    /// Default factory: the first AVR target of the project (UNO R3 if the project has none).
    pub fn factory() -> BenchFactory {
        Arc::new(|spec: &BenchSpec| Box::new(AvrBench::new(spec)) as Box<dyn BenchLink>)
    }

    pub fn new(spec: &BenchSpec) -> AvrBench {
        let target = spec
            .targets
            .iter()
            .find(|t| t.fqbn.is_some())
            .cloned()
            .unwrap_or_else(default_target);
        let board = target
            .fqbn
            .as_deref()
            .and_then(Board::by_fqbn)
            .or_else(|| Board::by_id("uno"))
            .expect("UNO board definition");
        let esp = spec
            .radios
            .iter()
            .find(|(_, part)| part == "esp01")
            .map(|(r, _)| r.clone());
        let shared = Arc::new(Mutex::new(Shared {
            debug: DebugViewModel {
                target: target.id.clone(),
                ..Default::default()
            },
            ..Default::default()
        }));
        let (tx, rx) = mpsc::channel();
        let worker = Worker::new(
            target.clone(),
            board,
            esp,
            spec.speed,
            shared.clone(),
            spec.repaint.clone(),
        );
        let thread = std::thread::Builder::new()
            .name(format!("bench-{}", target.id))
            .spawn(move || worker.run(rx))
            .ok();
        AvrBench {
            tx,
            shared,
            target,
            thread,
            running: false,
        }
    }

    /// Sets a fixed analog input voltage (`"A0"`, volts).
    pub fn set_analog(&mut self, pin: &str, volts: f64) {
        let _ = self.tx.send(Cmd::Analog(pin.to_string(), volts));
    }
}

fn default_target() -> BenchTarget {
    BenchTarget {
        id: "U1".into(),
        label: "U1 · UNO R3 (ATmega328P)".into(),
        board: "UNO R3".into(),
        fqbn: Some("arduino:avr:uno".into()),
        emulation: "cycle-accurate AVR core".into(),
        sketch_dir: None,
    }
}

impl Drop for AvrBench {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl BenchLink for AvrBench {
    fn describe(&self) -> String {
        format!("AVR core · {} · no circuit", self.target.id)
    }

    fn targets(&self) -> Vec<BenchTarget> {
        vec![self.target.clone()]
    }

    fn upload(&mut self, target: &str, firmware: &Firmware) -> Result<UploadReport, BenchError> {
        if target != self.target.id {
            return Err(BenchError::UnknownTarget(target.to_string()));
        }
        let (rtx, rrx) = mpsc::channel();
        self.tx
            .send(Cmd::Upload(firmware.clone(), rtx))
            .map_err(|_| BenchError::Unsupported("bench thread stopped".into()))?;
        let r = rrx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| BenchError::Unsupported("bench did not answer".into()))?;
        if r.is_ok() {
            self.running = true;
        }
        r
    }

    fn set_running(&mut self, running: bool) {
        self.running = running;
        let _ = self.tx.send(Cmd::Run(running));
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn reset(&mut self, scope: ResetScope) {
        let _ = self.tx.send(Cmd::Reset(scope));
    }

    fn set_speed(&mut self, speed: SimSpeed) {
        let _ = self.tx.send(Cmd::Speed(speed));
    }

    fn set_breakpoints(&mut self, target: &str, breakpoints: &[BreakpointSpec]) {
        if target == self.target.id {
            let _ = self.tx.send(Cmd::Breakpoints(breakpoints.to_vec()));
        }
    }

    fn debug(&mut self, target: &str, cmd: DebugCommand) {
        if target == self.target.id {
            let _ = self.tx.send(Cmd::Debug(cmd));
        }
    }

    fn send_serial(&mut self, target: &str, data: &[u8]) {
        if target == self.target.id {
            let _ = self.tx.send(Cmd::Serial(data.to_vec()));
        }
    }

    fn radio(&mut self, cmd: RadioCommand) {
        let _ = self.tx.send(Cmd::Radio(cmd));
    }

    fn poll(&mut self) -> BenchUpdate {
        let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        self.running = s.running;
        BenchUpdate {
            running: s.running,
            sim_time: s.sim_time,
            serial: std::mem::take(&mut s.serial),
            debug: vec![s.debug.clone()],
            radio: s.radio.clone(),
            pins: vec![(self.target.id.clone(), s.pins.clone())],
            events: std::mem::take(&mut s.events),
        }
    }
}

// ------------------------------------------------------------------------------------------
// Worker

struct RadioRig {
    space: RadioSpace,
    wire: UartWire,
    mcu_tx: Pin,
    mcu_rx: Pin,
}

#[derive(Default, Clone, Copy)]
struct PwmTrack {
    last_rise: Option<u64>,
    prev_rise: Option<u64>,
    last_fall: Option<u64>,
    duty: Option<f32>,
}

struct Io<'a> {
    adc: &'a HashMap<Pin, f64>,
}

impl AvrIo for Io<'_> {
    fn adc_sample(&mut self, pin: Pin, _cycle: u64) -> Option<f64> {
        Some(self.adc.get(&pin).copied().unwrap_or(0.0))
    }
}

struct Worker {
    target: BenchTarget,
    board: Board,
    avr: Avr,
    flashed: bool,
    dbg: Option<Debugger>,
    debug_build: bool,
    bp_specs: Vec<BreakpointSpec>,
    radio: Option<RadioRig>,
    esp_label: Option<String>,
    running: bool,
    paused: Option<PauseInfo>,
    speed: SimSpeed,
    adc: HashMap<Pin, f64>,
    partial: String,
    anchor: (Instant, SimTime),
    pwm: HashMap<Pin, PwmTrack>,
    shared: Arc<Mutex<Shared>>,
    repaint: Repaint,
    out_serial: Vec<SerialLine>,
    out_events: Vec<BenchEvent>,
    last_publish: Instant,
    debug_dirty: bool,
}

fn new_avr(board: &Board) -> Avr {
    let mut cfg = AvrConfig::new(board.mcu);
    cfg.stop_on = StopOn {
        pin_change: true,
        ..StopOn::default()
    };
    let mut avr = Avr::new(cfg);
    avr.set_temperature(21.0);
    avr
}

fn new_radio(label: &str) -> RadioRig {
    let mut space = RadioSpace::default();
    space
        .net
        .geometry
        .walls
        .push(Wall::interior(Pos::new(1.5, -5.0), Pos::new(1.5, 5.0)));
    let module = space.add_esp01(label, Pos::new(3.2, 0.0), EspAtConfig::with_baud(9600));
    space.power(module, true, SimTime::ZERO);
    let wire = UartWire::new(&space, module);
    RadioRig {
        space,
        wire,

        mcu_tx: Pin::new('D', 2),
        mcu_rx: Pin::new('D', 3),
    }
}

impl Worker {
    fn new(
        target: BenchTarget,
        board: Board,
        esp_label: Option<String>,
        speed: SimSpeed,
        shared: Arc<Mutex<Shared>>,
        repaint: Repaint,
    ) -> Worker {
        let avr = new_avr(&board);
        let mut adc = HashMap::new();
        // The design's default world: 42 % VWC → soil_raw ≈ 385, 180 lux → light_raw ≈ 532.
        let v = |raw: f64| raw * 5.0 / 1024.0 + 0.001;
        if let Some(a0) = board.pin_by_name("A0").and_then(|p| p.pin) {
            adc.insert(a0, v(385.0));
        }
        if let Some(a1) = board.pin_by_name("A1").and_then(|p| p.pin) {
            adc.insert(a1, v(532.0));
        }
        let radio = esp_label.as_deref().map(new_radio);
        Worker {
            target,
            board,
            avr,
            flashed: false,
            dbg: None,
            debug_build: false,
            bp_specs: Vec::new(),
            radio,
            esp_label,
            running: false,
            paused: None,
            speed,
            adc,
            partial: String::new(),
            anchor: (Instant::now(), SimTime::ZERO),
            pwm: HashMap::new(),
            shared,
            repaint,
            out_serial: Vec::new(),
            out_events: Vec::new(),
            last_publish: Instant::now(),
            debug_dirty: true,
        }
    }

    fn active(&self) -> bool {
        self.running && self.flashed && self.paused.is_none()
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        self.publish(true);
        loop {
            // drain commands
            let first = if self.active() {
                match rx.try_recv() {
                    Ok(c) => Some(c),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            } else {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(c) => Some(c),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            };
            let mut next = first;
            while let Some(cmd) = next {
                if matches!(cmd, Cmd::Quit) {
                    return;
                }
                self.handle(cmd);
                next = rx.try_recv().ok();
            }
            if self.active() {
                self.slice();
            }
            let due = self.last_publish.elapsed() > Duration::from_millis(33);
            if due || self.debug_dirty || !self.out_events.is_empty() {
                self.publish(false);
            }
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Upload(fw, reply) => {
                let r = self.flash(&fw);
                let _ = reply.send(r);
            }
            Cmd::Run(on) => {
                self.running = on;
                if on {
                    if self.paused.take().is_some() {
                        self.debug_dirty = true;
                    }
                    self.anchor = (Instant::now(), self.avr.time());
                }
                self.debug_dirty = true;
            }
            Cmd::Reset(scope) => {
                match scope {
                    ResetScope::Bench => {
                        self.avr.reset(bb_avr::ResetCause::PowerOn);
                        if let Some(l) = self.esp_label.clone() {
                            self.radio = Some(new_radio(&l));
                        }
                    }
                    ResetScope::Mcu(_) => self.avr.reset(bb_avr::ResetCause::External),
                }
                self.paused = None;
                self.partial.clear();
                self.anchor = (Instant::now(), self.avr.time());
                self.sys_line("core reset");
                self.debug_dirty = true;
            }
            Cmd::Speed(s) => {
                self.speed = s;
                self.anchor = (Instant::now(), self.avr.time());
            }
            Cmd::Breakpoints(specs) => {
                self.bp_specs = specs;
                self.sync_breakpoints();
                self.debug_dirty = true;
            }
            Cmd::Debug(c) => self.debug_cmd(c),
            Cmd::Serial(bytes) => self.avr.usart_inject(0, &bytes),
            Cmd::Radio(c) => self.radio_cmd(c),
            Cmd::Analog(name, v) => {
                if let Some(p) = self.board.pin_by_name(&name).and_then(|p| p.pin) {
                    self.adc.insert(p, v);
                }
            }
            Cmd::Quit => {}
        }
    }

    fn flash(&mut self, fw: &Firmware) -> Result<UploadReport, BenchError> {
        let t0 = Instant::now();
        let image = if let Some(elf) = &fw.elf {
            bb_avr::FirmwareImage::from_elf(elf).map_err(|e| BenchError::BadFirmware(e.to_string()))?
        } else if let Some(hex) = &fw.hex {
            bb_avr::FirmwareImage::from_hex(hex).map_err(|e| BenchError::BadFirmware(e.to_string()))?
        } else {
            return Err(BenchError::BadFirmware("no ELF or HEX".into()));
        };
        let first = !self.flashed;
        self.avr
            .flash(&image)
            .map_err(|e| BenchError::BadFirmware(e.to_string()))?;
        if let Some(r) = &self.radio {
            self.avr.set_pin_input(r.mcu_rx, Some(true));
        }
        self.flashed = true;
        self.debug_build = fw.debug_build;
        self.partial.clear();
        self.pwm.clear();
        // debug info
        let info = fw
            .elf
            .as_ref()
            .and_then(|e| DebugInfo::from_elf(e).ok())
            .map(Arc::new);
        match (info, self.dbg.as_mut()) {
            (Some(info), Some(d)) => d.rebind(info, &mut self.avr),
            (Some(info), None) => self.dbg = Some(Debugger::new(info)),
            (None, _) => self.dbg = None,
        }
        self.sync_breakpoints();
        self.paused = None;
        self.running = true;
        self.anchor = (Instant::now(), self.avr.time());
        let elapsed = t0.elapsed().max(Duration::from_millis(10));
        let message = format!(
            "Flashed virtual {} in {:.2} s · core reset · {}",
            self.target.id,
            elapsed.as_secs_f64(),
            if first {
                "bench started"
            } else {
                "bench state kept"
            }
        );
        self.sys_line(&format!(
            "Breadbox sim · {} @ {} MHz · reset cause: {}",
            self.board.mcu_name,
            self.board.f_cpu / 1_000_000,
            if first { "power-on" } else { "re-flash" }
        ));
        let report = UploadReport {
            target: self.target.id.clone(),
            elapsed,
            message,
        };
        self.out_events.push(BenchEvent::Flashed(report.clone()));
        for b in self.dbg.as_ref().map(|d| d.breakpoints().to_vec()).unwrap_or_default() {
            self.out_events.push(BenchEvent::BreakpointArmed {
                target: self.target.id.clone(),
                file: file_name(&b.file),
                line: b.line,
                hits: b.hits,
            });
        }
        self.debug_dirty = true;
        Ok(report)
    }

    fn sync_breakpoints(&mut self) {
        let Some(dbg) = self.dbg.as_mut() else {
            return;
        };
        // remove the ones no longer wanted
        let existing: Vec<_> = dbg.breakpoints().to_vec();
        for b in &existing {
            let fname = file_name(&b.file);
            let keep = self
                .bp_specs
                .iter()
                .any(|s| same_file(&s.file, &fname) && s.line == b.requested_line);
            if !keep {
                dbg.remove_breakpoint(&mut self.avr, b.id);
            }
        }
        for s in &self.bp_specs {
            let found = dbg
                .breakpoints()
                .iter()
                .find(|b| same_file(&s.file, &file_name(&b.file)) && b.requested_line == s.line)
                .map(|b| b.id);
            let cond = match s.hit_count {
                Some(n) if n > 1 => HitCondition::Equals(n),
                _ => HitCondition::Always,
            };
            let id = match found {
                Some(id) => Some(id),
                None => dbg
                    .add_breakpoint(&mut self.avr, &s.file, s.line, cond)
                    .ok()
                    .map(|b| b.id),
            };
            if let Some(id) = id {
                dbg.set_condition(id, cond);
                dbg.set_enabled(&mut self.avr, id, s.enabled);
            }
        }
    }

    fn debug_cmd(&mut self, c: DebugCommand) {
        if !self.flashed {
            return;
        }
        let Some(dbg) = self.dbg.as_mut() else {
            // no debug info: only run/pause
            match c {
                DebugCommand::Continue => {
                    self.running = true;
                    self.paused = None;
                }
                DebugCommand::Pause => self.running = false,
                _ => {}
            }
            self.debug_dirty = true;
            return;
        };
        match c {
            DebugCommand::Pause => {
                dbg.pause(&mut self.avr);
                self.running = true; // the next run() stops with Paused
            }
            DebugCommand::Continue => {}
            DebugCommand::StepOver => dbg.step_over(&mut self.avr),
            DebugCommand::StepInto => dbg.step_into(&mut self.avr),
            DebugCommand::StepOut => dbg.step_out(&mut self.avr),
            DebugCommand::StepInstruction => dbg.step_instruction(&mut self.avr),
        }
        if c != DebugCommand::Pause {
            self.paused = None;
            self.running = true;
        }
        self.anchor = (Instant::now(), self.avr.time());
        self.debug_dirty = true;
    }

    fn radio_cmd(&mut self, c: RadioCommand) {
        let Some(r) = self.radio.as_mut() else {
            if c == RadioCommand::AddBleHat {
                self.out_events.push(BenchEvent::Note(
                    "BLE hats are placed from the Workbench parts panel (HM-10 BLE).".into(),
                ));
            }
            return;
        };
        match c {
            RadioCommand::SetConditions(v) => {
                let mut cur = r.space.net.conditions().clone();
                cur.latency_ms = v.latency_ms;
                cur.jitter_ms = v.jitter_ms;
                cur.loss_pct = v.loss_pct;
                cur.drop_ap_at_s = v.drop_ap_at_s;
                r.space.net.set_conditions(cur);
            }
            RadioCommand::SetLanBridge(b) => {
                r.space
                    .set_lan_bridge(b.map(|(h, p)| bb_radio::bridge::BridgeConfig::new(&h, p)));
            }
            RadioCommand::AddBleHat => self.out_events.push(BenchEvent::Note(
                "BLE hats are placed from the Workbench parts panel (HM-10 BLE).".into(),
            )),
            RadioCommand::MqttPublish { topic, payload } => {
                r.space
                    .explorer_publish(&topic, payload.as_bytes(), 0, false);
            }
            RadioCommand::MqttSubscribe(f) => r.space.explorer_subscribe(&f),
        }
    }

    /// Runs one paced slice of simulated time.
    fn slice(&mut self) {
        let now_sim = self.avr.time();
        let end = match self.speed {
            SimSpeed::Max => now_sim + SimTime::from_ms(20),
            SimSpeed::RealTime(k) => {
                let wall = self.anchor.0.elapsed().as_secs_f64() * k.max(0.01);
                let mut want = self.anchor.1 + SimTime::from_secs_f64(wall);
                if want > now_sim + SimTime::from_ms(100) {
                    // fell behind (e.g. after a long pause): re-anchor instead of racing
                    self.anchor = (Instant::now(), now_sim);
                    want = now_sim + SimTime::from_ms(5);
                }
                if want <= now_sim {
                    std::thread::sleep(Duration::from_millis(2));
                    return;
                }
                want.min(now_sim + SimTime::from_ms(20))
            }
        };
        let deadline = Instant::now() + Duration::from_millis(25);
        self.run_until(end, deadline);
    }

    fn run_until(&mut self, end: SimTime, deadline: Instant) {
        let f_cpu = self.avr.f_cpu();
        while self.avr.time() < end && self.active() {
            if Instant::now() > deadline {
                break;
            }
            let now = self.avr.time();
            let mut target = (now + SimTime::from_ms(1)).min(end);
            if let Some(r) = &self.radio {
                if let Some(w) = r.space.next_wake() {
                    target = target.min(w.max(now));
                }
                if let Some(d) = r.wire.next_decision() {
                    target = target.min(d.max(now));
                }
            }
            let until = self.avr.cycle_at(target).max(self.avr.cycle() + 1);
            let stop = {
                let mut io = Io { adc: &self.adc };
                self.avr.run(until, &mut io)
            };
            if let Some(dbg) = self.dbg.as_mut()
                && let Decision::Pause(info) = dbg.on_stop(&mut self.avr, &stop)
            {
                let reason = pause_reason(&info.reason);
                self.out_events.push(BenchEvent::Paused {
                    target: self.target.id.clone(),
                    reason: reason.clone(),
                });
                self.paused = Some(info);
                self.debug_dirty = true;
            }
            match stop {
                StopReason::Fault { kind, pc } => {
                    self.out_events.push(BenchEvent::Error(format!(
                        "{}: firmware fault {kind:?} at 0x{pc:04x}",
                        self.target.id
                    )));
                }
                StopReason::AdcSampleNeeded { .. } => self.avr.supply_adc_sample(0.0),
                _ => {}
            }
            // pins
            for e in self.avr.take_pin_events() {
                let t = SimTime::from_cycles(e.cycle, f_cpu);
                if let Some(r) = self.radio.as_mut()
                    && e.pin == r.mcu_tx
                {
                    let high = match e.state {
                        PinState::Output { high } => high,
                        PinState::Input { .. } => true,
                    };
                    r.wire.mcu_edge(t, high);
                    continue;
                }
                let tr = self.pwm.entry(e.pin).or_default();
                match e.state {
                    PinState::Output { high: true } => {
                        tr.prev_rise = tr.last_rise;
                        tr.last_rise = Some(e.cycle);
                        if let (Some(a), Some(f), Some(b)) = (tr.prev_rise, tr.last_fall, tr.last_rise)
                            && a < f
                            && f < b
                        {
                            tr.duty = Some((f - a) as f32 / (b - a) as f32);
                        }
                    }
                    PinState::Output { high: false } => tr.last_fall = Some(e.cycle),
                    _ => {}
                }
            }
            // radio
            if let Some(r) = self.radio.as_mut() {
                let t = self.avr.time();
                r.wire.pump(&mut r.space, t);
                r.space.advance(t);
                for (et, lvl) in r.wire.module_edges(&mut r.space) {
                    let c = self.avr.cycle_at(et);
                    self.avr.schedule_pin_input(r.mcu_rx, Some(lvl), c);
                }
                for a in r.space.take_annotations() {
                    self.out_serial.push(SerialLine {
                        target: self.target.id.clone(),
                        time: a.time,
                        text: a.line(),
                        kind: LineKind::Net,
                    });
                }
            }
            // serial monitor (USART0)
            for b in self.avr.take_usart_tx(0) {
                let c = b.data as u8;
                if c == b'\n' {
                    let text = std::mem::take(&mut self.partial)
                        .trim_end_matches('\r')
                        .to_string();
                    let kind = if text.starts_with("WARN") {
                        LineKind::Warn
                    } else if text.starts_with("ERR") {
                        LineKind::Error
                    } else {
                        LineKind::Normal
                    };
                    self.out_serial.push(SerialLine {
                        target: self.target.id.clone(),
                        time: SimTime::from_cycles(b.cycle, f_cpu),
                        text,
                        kind,
                    });
                } else if self.partial.len() < 4096 {
                    self.partial.push(c as char);
                }
            }
        }
    }

    fn sys_line(&mut self, text: &str) {
        self.out_serial.push(SerialLine {
            target: self.target.id.clone(),
            time: self.avr.time(),
            text: text.to_string(),
            kind: LineKind::System,
        });
    }

    fn debug_vm(&self) -> DebugViewModel {
        let mut vm = DebugViewModel {
            target: self.target.id.clone(),
            debug_build: self.debug_build,
            ..Default::default()
        };
        vm.state = if !self.flashed {
            DebugState::NoFirmware
        } else if let Some(p) = &self.paused {
            DebugState::Paused {
                reason: pause_reason(&p.reason),
            }
        } else if self.running {
            DebugState::Running
        } else {
            DebugState::Paused {
                reason: "paused".into(),
            }
        };
        let regs = RegistersView {
            r: (0..32).map(|i| self.avr.reg(i)).collect(),
            sreg: self.avr.sreg(),
            sp: self.avr.sp(),
            pc: self.avr.pc(),
            cycle: self.avr.cycle(),
        };
        vm.registers = regs;
        if let Some(dbg) = &self.dbg {
            vm.breakpoints = self
                .bp_specs
                .iter()
                .map(|s| {
                    let b = dbg
                        .breakpoints()
                        .iter()
                        .find(|b| same_file(&s.file, &file_name(&b.file)) && b.requested_line == s.line);
                    BreakpointView {
                        file: s.file.clone(),
                        line: s.line,
                        resolved_line: b.map(|b| b.line),
                        hits: b.map(|b| b.hits).unwrap_or(0),
                        enabled: s.enabled,
                    }
                })
                .collect();
            if self.paused.is_some() || (!self.running && self.flashed) {
                vm.location = dbg.location(&self.avr).map(|l| SourcePos {
                    file: file_name(&l.file),
                    line: l.line,
                });
                vm.function = dbg.info().function_name(self.avr.pc());
                vm.call_stack = dbg
                    .call_stack(&self.avr)
                    .into_iter()
                    .map(|f| FrameView {
                        function: f.function.unwrap_or_else(|| format!("0x{:04x}", f.pc)),
                        location: f.location.map(|l| SourcePos {
                            file: file_name(&l.file),
                            line: l.line,
                        }),
                        pc: f.pc,
                    })
                    .collect();
                vm.locals = dbg
                    .locals(&self.avr, 0)
                    .into_iter()
                    .map(var_view)
                    .collect();
                let mut g: Vec<VarView> = dbg
                    .globals(&self.avr)
                    .into_iter()
                    .filter(|v| !v.name.starts_with('_'))
                    .map(var_view)
                    .collect();
                g.truncate(64);
                vm.globals = g;
            }
        } else {
            vm.breakpoints = self
                .bp_specs
                .iter()
                .map(|s| BreakpointView {
                    file: s.file.clone(),
                    line: s.line,
                    resolved_line: None,
                    hits: 0,
                    enabled: s.enabled,
                })
                .collect();
        }
        vm
    }

    fn pins(&self) -> Vec<PinView> {
        self.board
            .pins
            .iter()
            .filter(|p| p.digital)
            .filter_map(|bp| {
                let pin = bp.pin?;
                let info = self.avr.pin_info(pin);
                let (output, high) = match info.state {
                    PinState::Output { high } => (true, high),
                    PinState::Input { .. } => (false, info.level),
                };
                let pwm = matches!(info.owner, PinOwner::OutputCompare { .. })
                    .then(|| self.pwm.get(&pin).and_then(|t| t.duty))
                    .flatten();
                Some(PinView {
                    name: bp.name.clone(),
                    output,
                    high,
                    pwm,
                })
            })
            .collect()
    }

    fn publish(&mut self, force: bool) {
        let debug = (self.debug_dirty || force).then(|| self.debug_vm());
        let pins = self.pins();
        let radio = self
            .radio
            .as_ref()
            .map(|r| RadioPanelModel::from_space(&r.space, LOG_LIMIT));
        {
            let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            s.running = self.active();
            s.sim_time = self.avr.time();
            s.serial.append(&mut self.out_serial);
            if s.serial.len() > SERIAL_LIMIT {
                let n = s.serial.len() - SERIAL_LIMIT;
                s.serial.drain(..n);
            }
            s.events.append(&mut self.out_events);
            if let Some(d) = debug {
                s.debug = d;
            } else if let DebugState::Running = s.debug.state {
                // keep hit counts fresh while running
                if let Some(dbg) = &self.dbg {
                    for b in &mut s.debug.breakpoints {
                        if let Some(x) = dbg
                            .breakpoints()
                            .iter()
                            .find(|x| same_file(&b.file, &file_name(&x.file)) && x.requested_line == b.line)
                        {
                            b.hits = x.hits;
                        }
                    }
                }
            }
            s.pins = pins;
            s.radio = radio;
        }
        self.debug_dirty = false;
        self.last_publish = Instant::now();
        (self.repaint)();
    }
}

fn var_view(v: bb_firmware::VarValue) -> VarView {
    VarView {
        name: v.name,
        type_name: v.type_name,
        value: v.value.to_string(),
        location: v.location,
    }
}

fn pause_reason(r: &PauseReason) -> String {
    match r {
        PauseReason::Breakpoint { hits, .. } => format!("breakpoint (hits: {hits})"),
        PauseReason::Step => "step".into(),
        PauseReason::User => "paused".into(),
        PauseReason::BreakInstruction => "BREAK instruction".into(),
        PauseReason::Fault(f) => format!("fault: {f}"),
        PauseReason::Watchpoint { addr, write } => format!(
            "watchpoint {} 0x{addr:04x}",
            if *write { "write" } else { "read" }
        ),
    }
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

fn same_file(a: &str, b: &str) -> bool {
    let fa = std::path::Path::new(a)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let fb = std::path::Path::new(b)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    fa == fb
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/fixtures")
            .join(name)
    }

    fn spec(radios: Vec<(String, String)>) -> BenchSpec {
        BenchSpec {
            project_dir: None,
            targets: vec![default_target()],
            radios,
            repaint: Arc::new(|| {}),
            speed: SimSpeed::Max,
        }
    }

    fn wait(bench: &mut AvrBench, secs: f64, mut f: impl FnMut(&BenchUpdate) -> bool) -> bool {
        let t0 = Instant::now();
        while t0.elapsed().as_secs_f64() < secs {
            let u = bench.poll();
            if f(&u) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn blink_runs_and_hits_a_breakpoint_then_steps_over() {
        let elf = std::fs::read(fixture("blink.uno.elf")).unwrap();
        let mut b = AvrBench::new(&spec(vec![]));
        b.set_breakpoints(
            "U1",
            &[BreakpointSpec {
                file: "blink.ino".into(),
                line: 8,
                enabled: true,
                hit_count: None,
            }],
        );
        let r = b
            .upload(
                "U1",
                &Firmware {
                    elf: Some(Arc::new(elf)),
                    hex: None,
                    origin: "fixture".into(),
                    debug_build: false,
                },
            )
            .unwrap();
        assert!(r.message.starts_with("Flashed virtual U1 in "), "{}", r.message);
        let mut loc = None;
        assert!(wait(&mut b, 10.0, |u| {
            let d = &u.debug[0];
            if d.is_paused() {
                loc = d.location.clone();
                true
            } else {
                false
            }
        }));
        let loc = loc.unwrap();
        assert_eq!(loc.file, "blink.ino");
        assert_eq!(loc.line, 8);
        b.debug("U1", DebugCommand::StepOver);
        let mut after = None;
        assert!(wait(&mut b, 10.0, |u| {
            let d = &u.debug[0];
            if d.is_paused() && d.location.as_ref().map(|l| l.line) != Some(8) {
                after = d.location.clone();
                true
            } else {
                false
            }
        }));
        assert_eq!(after.unwrap().line, 9, "step over digitalWrite lands on delay()");
    }

    #[test]
    fn garden_node_prints_and_talks_to_the_esp01() {
        let elf = std::fs::read(fixture("garden-node.uno.elf")).unwrap();
        let mut b = AvrBench::new(&spec(vec![("U2".into(), "esp01".into())]));
        b.upload(
            "U1",
            &Firmware {
                elf: Some(Arc::new(elf)),
                hex: None,
                origin: "fixture".into(),
                debug_build: false,
            },
        )
        .unwrap();
        let mut lines: Vec<SerialLine> = Vec::new();
        let mut radio = None;
        assert!(wait(&mut b, 60.0, |u| {
            lines.extend(u.serial.iter().cloned());
            radio = u.radio.clone();
            lines.iter().any(|l| l.text.starts_with("soil_raw="))
                && lines.iter().any(|l| l.kind == LineKind::Net && l.text.starts_with("mqtt: PUB"))
        }), "serial: {lines:#?}");
        let radio = radio.unwrap();
        assert_eq!(radio.cards[0].label, "U2");
        assert!(radio.log.iter().any(|r| r.text.contains("DHCP")));
    }
}
