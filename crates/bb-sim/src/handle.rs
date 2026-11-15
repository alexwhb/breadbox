//! The threaded engine handle the app uses (SIM-5): a [`Bench`] on its own thread, paced
//! against the wall clock at 0.1×–10× or as fast as possible (SIM-3); commands in over a
//! channel; telemetry out as a lock-free snapshot (`ArcSwap`, published ≤ 60 Hz) plus bounded
//! channels for serial lines, events and audio.

use crate::bench::{Bench, BenchOptions};
use crate::serial::SerialLine;
use crate::telemetry::{RunStatus, Telemetry};
use crate::{SimError, SimEvent};
use arc_swap::ArcSwap;
use bb_avr::FirmwareImage;
use bb_circuit::project::ProbeDef;
use bb_circuit::{Library, Project};
use bb_core::{AudioChunk, DeviceEvent, SimTime, UserInput};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Simulation speed (WB-1: 0.1×, 0.5×, 1×, 2×, 10×, Max).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Speed {
    Factor(f64),
    Max,
}

/// Debugger operations (bb_firmware::Debugger under the scheduler).
#[derive(Debug, Clone)]
pub enum DebugCommand {
    /// Attach source-level debugging with the ELF of the running firmware.
    Attach { part: String, elf: Vec<u8> },
    AddBreakpoint {
        part: String,
        file: String,
        line: u32,
        /// Pause on the n-th hit (`HitCondition::Equals`), every hit when `None`.
        hits: Option<u64>,
    },
    RemoveBreakpoint { part: String, id: u32 },
    StepOver { part: String },
    StepInto { part: String },
    StepOut { part: String },
    StepInstruction { part: String },
    Pause { part: String },
    Continue,
}

/// Commands into the engine.
#[derive(Debug, Clone)]
pub enum Command {
    Play,
    Pause,
    /// Run exactly this much simulated time, then pause (single quantum, single ms, …).
    Step(SimTime),
    SetSpeed(Speed),
    /// Rebuild the whole bench from its project (time back to 0).
    Reset,
    /// Reset one MCU only (analog, world and devices keep running).
    ResetMcu(String),
    /// Re-flash an MCU; the bench keeps its state (SIM-7).
    Flash { part: String, image: Box<FirmwareImage> },
    SetWorld { key: String, value: f64 },
    Water { pot: String, ml: f64 },
    SetPartParam { part: String, key: String, value: f64 },
    Input { part: String, input: UserInput },
    AddProbe(ProbeDef),
    RemoveProbe(String),
    Scope {
        id: String,
        timebase: Option<f64>,
        vdiv: Option<[f64; 2]>,
        trigger: Option<crate::instruments::Trigger>,
    },
    SerialSend { part: String, port: usize, bytes: Vec<u8> },
    /// A circuit edit: rebuild/patch keeping MCU, world and device state.
    UpdateProject(Box<Project>),
    RepairAll,
    Debug(DebugCommand),
    Shutdown,
}

/// Handle to a bench running on its own thread.
pub struct SimHandle {
    tx: Sender<Command>,
    telemetry: Arc<ArcSwap<Telemetry>>,
    serial: Receiver<SerialLine>,
    events: Receiver<SimEvent>,
    audio: Receiver<AudioChunk>,
    thread: Option<JoinHandle<()>>,
}

const FRAME: Duration = Duration::from_millis(16);

impl SimHandle {
    /// Starts the engine thread (paused).
    pub fn spawn(project: Project, lib: Arc<Library>, opts: BenchOptions) -> Result<SimHandle, SimError> {
        let bench = Bench::new(project.clone(), lib.clone(), opts.clone())?;
        let telemetry = Arc::new(ArcSwap::from_pointee(Telemetry::default()));
        let (tx, rx) = unbounded::<Command>();
        let (stx, srx) = bounded::<SerialLine>(8192);
        let (etx, erx) = bounded::<SimEvent>(4096);
        let (atx, arx) = bounded::<AudioChunk>(256);
        let tel = telemetry.clone();
        let thread = std::thread::Builder::new()
            .name("bb-sim".into())
            .spawn(move || {
                let mut eng = Engine {
                    bench,
                    project,
                    lib,
                    opts,
                    running: false,
                    speed: Speed::Factor(1.0),
                    step_until: None,
                    error: None,
                    tel,
                    stx,
                    etx,
                    atx,
                    rate: 0.0,
                };
                eng.run(rx);
            })
            .map_err(|e| SimError::Build(e.to_string()))?;
        Ok(SimHandle {
            tx,
            telemetry,
            serial: srx,
            events: erx,
            audio: arx,
            thread: Some(thread),
        })
    }

    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }
    /// Latest snapshot (never blocks).
    pub fn telemetry(&self) -> Arc<Telemetry> {
        self.telemetry.load_full()
    }
    /// Serial lines since the last call.
    pub fn serial_lines(&self) -> Vec<SerialLine> {
        self.serial.try_iter().collect()
    }
    pub fn events(&self) -> Vec<SimEvent> {
        self.events.try_iter().collect()
    }
    pub fn audio(&self) -> Vec<AudioChunk> {
        self.audio.try_iter().collect()
    }
    /// Stops the engine thread.
    pub fn shutdown(mut self) {
        self.stop();
    }
    fn stop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for SimHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Engine {
    bench: Bench,
    project: Project,
    lib: Arc<Library>,
    opts: BenchOptions,
    running: bool,
    speed: Speed,
    step_until: Option<SimTime>,
    error: Option<String>,
    tel: Arc<ArcSwap<Telemetry>>,
    stx: Sender<SerialLine>,
    etx: Sender<SimEvent>,
    atx: Sender<AudioChunk>,
    rate: f64,
}

impl Engine {
    fn run(&mut self, rx: Receiver<Command>) {
        let mut last_pub = Instant::now() - FRAME;
        let mut wall_anchor = Instant::now();
        let mut sim_anchor = self.bench.time();
        let mut rate_wall = Instant::now();
        let mut rate_sim = self.bench.time();
        loop {
            // commands
            let timeout = if self.running { Duration::ZERO } else { FRAME };
            let first = if timeout.is_zero() {
                rx.try_recv().ok()
            } else {
                rx.recv_timeout(timeout).ok()
            };
            let mut cmds: Vec<Command> = first.into_iter().collect();
            cmds.extend(rx.try_iter());
            for c in cmds {
                if matches!(c, Command::Shutdown) {
                    return;
                }
                let was_running = self.running;
                self.handle(c);
                if self.running && !was_running {
                    wall_anchor = Instant::now();
                    sim_anchor = self.bench.time();
                }
            }
            // simulate one slice
            if self.running {
                let slice_end = Instant::now() + Duration::from_millis(8);
                let target = match self.speed {
                    Speed::Max => self.bench.time() + SimTime::from_secs(10),
                    Speed::Factor(f) => {
                        let wall = wall_anchor.elapsed().as_secs_f64();
                        sim_anchor + SimTime::from_secs_f64(wall * f)
                    }
                };
                let target = match self.step_until {
                    Some(s) => target.min(s),
                    None => target,
                };
                while self.bench.time() < target && Instant::now() < slice_end {
                    let chunk = (self.bench.time() + SimTime::from_ms(1)).min(target);
                    if let Err(e) = self.bench.run_until(chunk) {
                        self.error = Some(e.to_string());
                        self.running = false;
                        break;
                    }
                    if self.bench.paused().is_some() {
                        self.running = false;
                        break;
                    }
                }
                if let Some(s) = self.step_until
                    && self.bench.time() >= s
                {
                    self.running = false;
                    self.step_until = None;
                }
                // falling behind by more than 250 ms: re-anchor instead of racing to catch up
                if let Speed::Factor(f) = self.speed {
                    let wall = wall_anchor.elapsed().as_secs_f64();
                    let want = sim_anchor + SimTime::from_secs_f64(wall * f);
                    if want.saturating_sub(self.bench.time()) > SimTime::from_ms(250) {
                        wall_anchor = Instant::now();
                        sim_anchor = self.bench.time();
                    }
                    if self.bench.time() >= want && self.running {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            }
            if last_pub.elapsed() >= FRAME {
                let dw = rate_wall.elapsed().as_secs_f64();
                if dw > 0.25 {
                    self.rate = (self.bench.time() - rate_sim).as_secs_f64() / dw;
                    rate_wall = Instant::now();
                    rate_sim = self.bench.time();
                }
                self.publish();
                last_pub = Instant::now();
            }
        }
    }

    fn handle(&mut self, c: Command) {
        let r: Result<(), SimError> = match c {
            Command::Play => {
                self.running = self.error.is_none();
                self.bench.resume();
                Ok(())
            }
            Command::Pause => {
                self.running = false;
                Ok(())
            }
            Command::Step(dt) => {
                self.step_until = Some(self.bench.time() + dt);
                self.running = true;
                self.bench.resume();
                Ok(())
            }
            Command::SetSpeed(s) => {
                self.speed = s;
                Ok(())
            }
            Command::Reset => match Bench::new(self.project.clone(), self.lib.clone(), self.opts.clone()) {
                Ok(b) => {
                    self.bench = b;
                    self.error = None;
                    Ok(())
                }
                Err(e) => Err(e),
            },
            Command::ResetMcu(part) => {
                let t = self.bench.time();
                if let Some(m) = self.bench.mcu(&part) {
                    m.reset();
                }
                if let Some(s) = self.bench.serial_mut(&part) {
                    s.system(t, "MCU reset (bench state kept)");
                }
                Ok(())
            }
            Command::Flash { part, image } => {
                let t = self.bench.time();
                let r = match self.bench.mcu(&part) {
                    Some(m) => m.flash(&image).map_err(SimError::Firmware),
                    None => Err(SimError::NotFound(part.clone())),
                };
                if r.is_ok() {
                    self.opts.firmware.insert(part.clone(), (*image).clone());
                    let reference = self
                        .project
                        .part(&part)
                        .map(|p| p.reference.clone())
                        .unwrap_or_default();
                    if let Some(s) = self.bench.serial_mut(&part) {
                        s.system(t, &format!("Flashed virtual {reference} · core reset · bench state kept"));
                    }
                }
                r
            }
            Command::SetWorld { key, value } => {
                self.bench.set_world(&key, value);
                Ok(())
            }
            Command::Water { pot, ml } => {
                self.bench.water(&pot, ml);
                Ok(())
            }
            Command::SetPartParam { part, key, value } => {
                if let Some(p) = self.project.part_mut(&part) {
                    p.props.insert(key.clone(), bb_circuit::PropValue::Num(value));
                }
                self.bench.set_part_param(&part, &key, value)
            }
            Command::Input { part, input } => self.bench.input(&part, &input),
            Command::AddProbe(def) => {
                let built = self.bench.netlist().clone();
                self.bench.instruments_mut().add_probe(def, &built);
                Ok(())
            }
            Command::RemoveProbe(id) => {
                self.bench.instruments_mut().remove_probe(&id);
                Ok(())
            }
            Command::Scope {
                id,
                timebase,
                vdiv,
                trigger,
            } => {
                self.bench.instruments_mut().set_scope(&id, timebase, vdiv, trigger);
                Ok(())
            }
            Command::SerialSend { part, port, bytes } => {
                if let Some(m) = self.bench.mcu(&part) {
                    m.serial_inject(port, &bytes);
                }
                Ok(())
            }
            Command::UpdateProject(p) => {
                self.project = (*p).clone();
                self.bench.update_project(*p)
            }
            Command::RepairAll => self.bench.repair_all(),
            Command::Debug(d) => self.debug(d),
            Command::Shutdown => Ok(()),
        };
        if let Err(e) = r {
            self.error = Some(e.to_string());
            self.running = false;
        }
    }

    fn debug(&mut self, d: DebugCommand) -> Result<(), SimError> {
        use bb_firmware::{DebugInfo, Debugger, HitCondition};
        let part_of = |d: &DebugCommand| -> Option<String> {
            match d {
                DebugCommand::Attach { part, .. }
                | DebugCommand::AddBreakpoint { part, .. }
                | DebugCommand::RemoveBreakpoint { part, .. }
                | DebugCommand::StepOver { part }
                | DebugCommand::StepInto { part }
                | DebugCommand::StepOut { part }
                | DebugCommand::StepInstruction { part }
                | DebugCommand::Pause { part } => Some(part.clone()),
                DebugCommand::Continue => None,
            }
        };
        if let DebugCommand::Continue = d {
            self.bench.resume();
            self.running = true;
            return Ok(());
        }
        let part = part_of(&d).unwrap_or_default();
        let Some(m) = self.bench.mcu(&part) else {
            return Err(SimError::NotFound(part));
        };
        let Some((dbg, avr)) = m.avr_debug() else {
            return Err(SimError::Firmware("this board has no source-level debugger yet".into()));
        };
        match d {
            DebugCommand::Attach { elf, .. } => {
                let info = DebugInfo::from_elf(&elf).map_err(|e| SimError::Firmware(e.to_string()))?;
                match dbg {
                    Some(existing) => existing.rebind(Arc::new(info), avr),
                    None => *dbg = Some(Debugger::new(Arc::new(info))),
                }
            }
            DebugCommand::AddBreakpoint { file, line, hits, .. } => {
                let Some(dbg) = dbg.as_mut() else {
                    return Err(SimError::Firmware("attach the debugger first".into()));
                };
                let cond = match hits {
                    Some(n) => HitCondition::Equals(n),
                    None => HitCondition::Always,
                };
                dbg.add_breakpoint(avr, &file, line, cond)
                    .map_err(|e| SimError::Firmware(e.to_string()))?;
            }
            DebugCommand::RemoveBreakpoint { id, .. } => {
                if let Some(dbg) = dbg.as_mut() {
                    dbg.remove_breakpoint(avr, id);
                }
            }
            DebugCommand::StepOver { .. } => step(dbg, avr, |d, a| d.step_over(a)),
            DebugCommand::StepInto { .. } => step(dbg, avr, |d, a| d.step_into(a)),
            DebugCommand::StepOut { .. } => step(dbg, avr, |d, a| d.step_out(a)),
            DebugCommand::StepInstruction { .. } => step(dbg, avr, |d, a| d.step_instruction(a)),
            DebugCommand::Pause { .. } => step(dbg, avr, |d, a| d.pause(a)),
            DebugCommand::Continue => {}
        }
        if !matches!(self.bench.paused(), None) {
            // stepping continues from a pause
        }
        Ok(())
    }

    fn publish(&mut self) {
        let mut t = self.bench.telemetry();
        t.status = match (&self.error, self.running, self.bench.paused()) {
            (Some(e), _, _) => RunStatus::Error(e.clone()),
            (None, true, _) => RunStatus::Running,
            (None, false, _) => RunStatus::Paused,
        };
        t.speed = match self.speed {
            Speed::Factor(f) => f,
            Speed::Max => 0.0,
        };
        t.realtime_factor = self.rate;
        self.tel.store(Arc::new(t));
        for part in self.bench.mcu_parts() {
            if let Some(s) = self.bench.serial_mut(&part) {
                for l in s.take_new() {
                    let _ = self.stx.try_send(l);
                }
            }
        }
        for e in self.bench.take_events() {
            if let DeviceEvent::Audio(a) = &e.event {
                let _ = self.atx.try_send(a.clone());
            } else {
                let _ = self.etx.try_send(e);
            }
        }
    }
}

fn step(
    dbg: &mut Option<bb_firmware::Debugger>,
    avr: &mut bb_avr::Avr,
    f: impl FnOnce(&mut bb_firmware::Debugger, &mut bb_avr::Avr),
) {
    if let Some(d) = dbg.as_mut() {
        f(d, avr);
    }
}
