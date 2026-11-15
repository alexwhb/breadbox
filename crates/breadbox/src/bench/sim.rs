//! [`SimBench`]: the app's bench — a [`BenchLink`] over `bb_sim::SimHandle`, the full
//! mixed-signal co-simulation (analog circuit + cycle-accurate AVRs / API-level host boards +
//! devices + world + radios + instruments) running the open project on its own thread.
//!
//! Besides the `BenchLink` contract (Firmware screen, debugger, Radios panel, Pro bench-probe
//! column) it exposes what the Workbench needs: the live [`Telemetry`] snapshot, world and part
//! commands, circuit updates (`UpdateProject` keeps MCU / world / device state) and bench audio.
//!
//! Debugging runs on the engine thread: `Command::With` closures attach a
//! `bb_firmware::Debugger` to an AVR core, keep its breakpoints in sync with the editor and
//! build the [`DebugViewModel`]s, which come back over a channel.

use super::audio::{AudioMsg, AudioOut};
use super::*;
use bb_circuit::{Library, Project as CircuitProject};
use bb_firmware::{DebugInfo, Debugger, HitCondition};
use bb_sim::handle::BenchFn;
use bb_sim::{BenchOptions, Command, SimEvent, SimHandle, Speed, Telemetry};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Wakes the UI while the bench changes (≤ 30 Hz) and briefly after every command.
struct Watcher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    fn spawn(handle: Arc<SimHandle>, kick: Arc<AtomicU64>, repaint: Repaint) -> Watcher {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::Builder::new()
            .name("bench-watch".into())
            .spawn(move || {
                let t0 = Instant::now();
                let mut last = 0u64;
                while !s.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(33));
                    let t = handle.telemetry();
                    let sig = signature(&t);
                    let now_ms = t0.elapsed().as_millis() as u64;
                    let kicked = kick.load(Ordering::Relaxed) > now_ms;
                    if sig != last || kicked {
                        last = sig;
                        repaint();
                    }
                }
            })
            .ok();
        Watcher { stop, thread }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Cheap change detector for a telemetry snapshot.
fn signature(t: &Telemetry) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.time.hash(&mut h);
    t.status.label().hash(&mut h);
    t.erc_summary.hash(&mut h);
    for m in &t.mcus {
        m.paused.hash(&mut h);
        m.reset_count.hash(&mut h);
    }
    for p in &t.parts {
        p.visual.health.is_damaged().hash(&mut h);
    }
    if let Some(r) = &t.radio {
        r.log.len().hash(&mut h);
    }
    h.finish()
}

/// Pause / step bookkeeping per MCU.
#[derive(Default, Clone)]
struct McuState {
    part: String,
    flashed: bool,
    debug_build: bool,
    has_debugger: bool,
    paused: Option<String>,
}

/// The co-simulation bench.
pub struct SimBench {
    handle: Option<Arc<SimHandle>>,
    watcher: Option<Watcher>,
    kick: Arc<AtomicU64>,
    epoch: Instant,
    build_error: Option<String>,
    lib: Arc<Library>,
    project: CircuitProject,
    dir: Option<PathBuf>,
    targets: Vec<BenchTarget>,
    /// Designator → state.
    mcus: HashMap<String, McuState>,
    breakpoints: HashMap<String, Vec<BreakpointSpec>>,
    speed: SimSpeed,
    running: bool,
    /// Ignore the telemetry run state until it caught up with our last Play/Pause.
    run_cmd_at: Option<Instant>,
    debug: HashMap<String, DebugViewModel>,
    dbg_tx: Sender<DebugViewModel>,
    dbg_rx: Receiver<DebugViewModel>,
    last_debug_refresh: Instant,
    events: Vec<BenchEvent>,
    /// Recent raw simulator events (damage, sounds, logs) for the Workbench.
    recent: Vec<SimEvent>,
    audio: Option<AudioOut>,
    repaint: Repaint,
    last_status: String,
    host_build: Option<JoinHandle<()>>,
    host_notes: Arc<Mutex<Vec<BenchEvent>>>,
}

const RECENT_EVENTS: usize = 200;

impl SimBench {
    /// Factory registered as the app's default bench.
    pub fn factory() -> BenchFactory {
        Arc::new(|spec: &BenchSpec| Box::new(SimBench::new(spec)) as Box<dyn BenchLink>)
    }

    /// Builds the bench for `spec` (paused). A circuit that cannot be simulated still yields
    /// a link: its error shows in the Workbench and every command is a no-op until an edit
    /// fixes the circuit.
    pub fn new(spec: &BenchSpec) -> SimBench {
        let lib = Library::builtin();
        let project = spec
            .circuit
            .clone()
            .or_else(|| {
                spec.project_dir
                    .as_ref()
                    .and_then(|d| CircuitProject::load(d).ok())
            })
            .unwrap_or_else(|| {
                CircuitProject::new("untitled", bb_circuit::project::Mode::Playground)
            });
        let (dbg_tx, dbg_rx) = mpsc::channel();
        let mut b = SimBench {
            handle: None,
            watcher: None,
            kick: Arc::new(AtomicU64::new(0)),
            epoch: Instant::now(),
            build_error: None,
            lib,
            project,
            dir: spec.project_dir.clone(),
            targets: spec.targets.clone(),
            mcus: HashMap::new(),
            breakpoints: HashMap::new(),
            speed: spec.speed,
            running: false,
            run_cmd_at: None,
            debug: HashMap::new(),
            dbg_tx,
            dbg_rx,
            last_debug_refresh: Instant::now(),
            events: Vec::new(),
            recent: Vec::new(),
            audio: spec.audio.then(AudioOut::new),
            repaint: spec.repaint.clone(),
            last_status: String::new(),
            host_build: None,
            host_notes: Arc::new(Mutex::new(Vec::new())),
        };
        b.refresh_targets();
        b.spawn(spec.fixtures.as_deref());
        b
    }

    fn refresh_targets(&mut self) {
        // targets come from the circuit (a board added on the bench becomes a target)
        let mut targets = Vec::new();
        for p in &self.project.parts {
            let Some(def) = self.lib.get(&p.part) else {
                continue;
            };
            if !def.is_mcu() {
                continue;
            }
            let Some(k) = crate::project::board_kind(&p.part) else {
                continue;
            };
            let sketch_dir = p
                .firmware
                .as_ref()
                .and_then(|f| self.dir.as_ref().map(|d| d.join(&f.sketch)))
                .or_else(|| {
                    self.targets
                        .iter()
                        .find(|t| t.id == p.reference)
                        .and_then(|t| t.sketch_dir.clone())
                });
            targets.push(BenchTarget {
                id: p.reference.clone(),
                label: format!("{} · {} ({})", p.reference, k.name, k.mcu),
                board: k.name.to_string(),
                fqbn: k.fqbn.map(str::to_string),
                emulation: k.emulation.to_string(),
                sketch_dir,
            });
            let st = self.mcus.entry(p.reference.clone()).or_default();
            st.part = p.id.clone();
        }
        if !targets.is_empty() || self.targets.is_empty() {
            self.targets = targets;
        }
        let live: HashSet<String> = self.targets.iter().map(|t| t.id.clone()).collect();
        self.mcus.retain(|r, _| live.contains(r));
    }

    fn spawn(&mut self, fixtures: Option<&Path>) {
        self.watcher = None;
        self.handle = None;
        let mut opts = BenchOptions::default();
        // prebuilt images so Run works straight away (Upload replaces them)
        for p in &self.project.parts {
            let Some(def) = self.lib.get(&p.part) else {
                continue;
            };
            let Some(mcu) = &def.model.mcu else { continue };
            if mcu.core != "avr" {
                continue;
            }
            if let Some(path) = prebuilt_image(&self.project, p, self.dir.as_deref(), fixtures)
                && let Ok(img) = bb_sim::firmware::load_image(&path)
            {
                opts.firmware.insert(p.id.clone(), img);
                if let Some(st) = self.mcus.get_mut(&p.reference) {
                    st.flashed = true;
                }
            }
        }
        match SimHandle::spawn(self.project.clone(), self.lib.clone(), opts) {
            Ok(h) => {
                let h = Arc::new(h);
                h.send(Command::SetSpeed(to_speed(self.speed)));
                self.watcher = Some(Watcher::spawn(
                    h.clone(),
                    self.kick.clone(),
                    self.repaint.clone(),
                ));
                self.handle = Some(h);
                self.build_error = None;
                self.build_host_boards();
            }
            Err(e) => {
                self.build_error = Some(e.to_string());
                self.events.push(BenchEvent::Error(format!("Bench: {e}")));
            }
        }
    }

    /// Compiles API-level boards' sketches (ESP32 / Pico) in the background and loads them.
    fn build_host_boards(&mut self) {
        let Some(h) = self.handle.clone() else { return };
        let Some(dir) = self.dir.clone() else { return };
        let hosts: Vec<_> = self
            .project
            .parts
            .iter()
            .filter(|p| {
                self.lib
                    .get(&p.part)
                    .and_then(|d| d.model.mcu.as_ref())
                    .is_some_and(|m| m.core == "host")
                    && p.firmware.is_some()
            })
            .cloned()
            .collect();
        if hosts.is_empty() {
            return;
        }
        let lib = self.lib.clone();
        let notes = self.host_notes.clone();
        let project = self.project.clone();
        self.host_build = std::thread::Builder::new()
            .name("host-build".into())
            .spawn(
                move || match bb_sim::firmware::compile_host_project(&project, &dir, &lib) {
                    Ok(exes) => {
                        for (part, exe) in exes {
                            h.send(Command::FlashHost { part, exe });
                        }
                    }
                    Err(e) => {
                        if let Ok(mut n) = notes.lock() {
                            n.push(BenchEvent::Note(format!(
                                "API-level boards stay idle until their sketch builds: {e}"
                            )));
                        }
                    }
                },
            )
            .ok();
    }

    fn kick(&self) {
        let until = self.epoch.elapsed().as_millis() as u64 + 400;
        self.kick.store(until, Ordering::Relaxed);
    }

    /// Sends a raw engine command (Workbench: world, inputs, params, probes, scopes…).
    pub fn send(&self, cmd: Command) {
        if let Some(h) = &self.handle {
            h.send(cmd);
            self.kick();
        }
    }

    /// The latest telemetry snapshot (never blocks).
    pub fn telemetry(&self) -> Option<Arc<Telemetry>> {
        self.handle.as_ref().map(|h| h.telemetry())
    }

    /// Why the bench could not be built (shown in the Workbench status).
    pub fn build_error(&self) -> Option<&str> {
        self.build_error.as_deref()
    }

    /// The circuit the bench is simulating.
    pub fn project(&self) -> &CircuitProject {
        &self.project
    }

    /// Applies a circuit edit, keeping MCU firmware, world and device state (SIM-7):
    /// electrical changes rebuild the netlist (`UpdateProject`), probe changes add / remove
    /// live probes, instrument changes re-clip the bench instruments.
    pub fn update_project(&mut self, project: CircuitProject) {
        if project == self.project {
            return;
        }
        let boards_changed = mcu_set(&project) != mcu_set(&self.project);
        let electrical = project.parts != self.project.parts
            || project.wires != self.project.wires
            || project.schematic != self.project.schematic;
        let instruments = project.instruments != self.project.instruments;
        let old_probes = std::mem::take(&mut self.project.probes);
        let new_probes = project.probes.clone();
        self.project = project;
        self.refresh_targets();
        let Some(h) = self.handle.clone() else {
            self.spawn(None);
            return;
        };
        if boards_changed {
            // a board was added / removed: start over with the new circuit
            let was_running = self.running;
            self.spawn(None);
            if was_running {
                self.set_running(true);
            }
            return;
        }
        for p in &old_probes {
            if !new_probes.iter().any(|q| q == p) {
                h.send(Command::RemoveProbe(p.id.clone()));
            }
        }
        for p in &new_probes {
            if !old_probes.iter().any(|q| q == p) {
                if old_probes.iter().any(|q| q.id == p.id) {
                    h.send(Command::RemoveProbe(p.id.clone()));
                }
                h.send(Command::AddProbe(p.clone()));
            }
        }
        if instruments {
            let defs = self.project.instruments.clone();
            h.send(Command::With(BenchFn(Arc::new(move |b| {
                b.instruments_mut().set_instruments(&defs);
            }))));
        }
        if electrical || instruments {
            h.send(Command::UpdateProject(Box::new(self.project.clone())));
        }
        self.kick();
    }

    /// Records a circuit revision whose changes already reached the engine (live
    /// `SetPartParam` edits) without rebuilding.
    pub fn adopt_project(&mut self, project: CircuitProject) {
        self.project = project;
    }

    /// Part id ↔ designator.
    pub fn part_id(&self, reference: &str) -> Option<String> {
        self.project.part_by_ref(reference).map(|p| p.id.clone())
    }

    /// Raw simulator events seen recently (newest last).
    pub fn recent_events(&self) -> &[SimEvent] {
        &self.recent
    }

    /// Exports the radio packet log (`pcapng` / `csv`).
    pub fn export_packet_log(&self, format: &str) -> Option<Vec<u8>> {
        let h = self.handle.as_ref()?;
        let (tx, rx) = crossbeam_reply();
        h.send(Command::ExportPacketLog {
            format: format.into(),
            reply: tx,
        });
        rx.recv_timeout(Duration::from_secs(5)).ok()
    }

    /// Runs a closure on the engine thread and waits for its result (lessons, tests).
    pub fn with_bench<R: Send + 'static>(
        &self,
        f: impl Fn(&mut bb_sim::Bench) -> R + Send + Sync + 'static,
    ) -> Option<R> {
        let h = self.handle.as_ref()?;
        let (tx, rx) = mpsc::channel::<R>();
        let tx = Mutex::new(tx);
        h.send(Command::With(BenchFn(Arc::new(move |b| {
            let r = f(b);
            if let Ok(t) = tx.lock() {
                let _ = t.send(r);
            }
        }))));
        rx.recv_timeout(Duration::from_secs(10)).ok()
    }

    fn mcu_part(&self, target: &str) -> Option<String> {
        self.mcus
            .get(target)
            .map(|m| m.part.clone())
            .filter(|p| !p.is_empty())
            .or_else(|| self.part_id(target))
    }

    /// Asks the engine thread for a fresh debugger view of `target`.
    fn request_debug(&mut self, target: &str) {
        let Some(part) = self.mcu_part(target) else {
            return;
        };
        let Some(h) = &self.handle else { return };
        let st = self.mcus.get(target).cloned().unwrap_or_default();
        let specs = self.breakpoints.get(target).cloned().unwrap_or_default();
        let tx = Mutex::new(self.dbg_tx.clone());
        let target = target.to_string();
        let running = self.running;
        h.send(Command::With(BenchFn(Arc::new(move |b| {
            let paused = b.paused().map(str::to_string);
            let vm = debug_vm(b, &part, &target, &st, &specs, running, paused);
            if let Ok(t) = tx.lock() {
                let _ = t.send(vm);
            }
        }))));
        self.last_debug_refresh = Instant::now();
        self.kick();
    }

    /// Re-arms `target`'s breakpoints on the engine thread (keeps hit counts).
    fn sync_breakpoints(&mut self, target: &str) {
        let Some(part) = self.mcu_part(target) else {
            return;
        };
        let specs = self.breakpoints.get(target).cloned().unwrap_or_default();
        self.send(Command::With(BenchFn(Arc::new(move |b| {
            if let Some(m) = b.mcu(&part)
                && let Some((Some(dbg), avr)) = m.avr_debug()
            {
                sync_breakpoints(dbg, avr, &specs);
            }
        }))));
    }

    fn convert_serial(&self, l: bb_sim::SerialLine) -> SerialLine {
        let kind = match l.kind {
            bb_sim::LineKind::Data => {
                if l.text.starts_with("WARN") {
                    LineKind::Warn
                } else if l.text.starts_with("ERR") {
                    LineKind::Error
                } else {
                    LineKind::Normal
                }
            }
            bb_sim::LineKind::System => LineKind::System,
            bb_sim::LineKind::Net => LineKind::Net,
            bb_sim::LineKind::Warn => LineKind::Warn,
        };
        SerialLine {
            target: l.mcu,
            time: l.time,
            text: l.text,
            kind,
        }
    }

    fn handle_event(&mut self, e: &SimEvent) {
        use bb_core::{DeviceEvent, LogLevel, Sound};
        let src = &e.source;
        match &e.event {
            DeviceEvent::Damage { reason } => {
                self.events
                    .push(BenchEvent::Error(format!("{src} damaged: {reason}")));
                if let Some(a) = &self.audio {
                    a.send(AudioMsg::Pop);
                }
            }
            DeviceEvent::Stress { reason } => self
                .events
                .push(BenchEvent::Note(format!("{src} stressed: {reason}"))),
            DeviceEvent::Log {
                level: LogLevel::Warn | LogLevel::Error,
                text,
            } => self.events.push(BenchEvent::Note(format!("{src}: {text}"))),
            DeviceEvent::Log { .. } => {}
            DeviceEvent::Erc { message, .. } => self
                .events
                .push(BenchEvent::Note(format!("ERC: {message}"))),
            DeviceEvent::Sound(s) => {
                if let Some(a) = &self.audio {
                    a.send(match s {
                        Sound::Click => AudioMsg::Click,
                        Sound::Pop => AudioMsg::Pop,
                        Sound::Beep { freq_hz, secs } => AudioMsg::Beep {
                            freq_hz: *freq_hz,
                            secs: *secs,
                        },
                    });
                }
            }
            DeviceEvent::Audio(_) | DeviceEvent::Annotation { .. } => {}
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }
}

impl Drop for SimBench {
    fn drop(&mut self) {
        self.watcher = None;
        self.handle = None;
        if let Some(t) = self.host_build.take() {
            // the build thread holds a handle clone; it finishes on its own
            drop(t);
        }
    }
}

fn crossbeam_reply() -> (
    crossbeam_channel::Sender<Vec<u8>>,
    crossbeam_channel::Receiver<Vec<u8>>,
) {
    crossbeam_channel::bounded(1)
}

fn to_speed(s: SimSpeed) -> Speed {
    match s {
        SimSpeed::RealTime(k) => Speed::Factor(k.clamp(0.01, 100.0)),
        SimSpeed::Max => Speed::Max,
    }
}

fn mcu_set(p: &CircuitProject) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = p
        .parts
        .iter()
        .filter(|x| crate::project::board_kind(&x.part).is_some())
        .map(|x| (x.id.clone(), x.part.clone()))
        .collect();
    v.sort();
    v
}

/// The prebuilt firmware for an MCU part: `<sketch>.<board>.elf` in the bundled fixtures, the
/// project's `firmware.image` (ELF next to a HEX preferred, for debugging).
pub fn prebuilt_image(
    project: &CircuitProject,
    part: &bb_circuit::PartInstance,
    dir: Option<&Path>,
    fixtures: Option<&Path>,
) -> Option<PathBuf> {
    let _ = project;
    let fw = part.firmware.as_ref()?;
    let tag = crate::project::board_kind(&part.part)?.fixture_tag;
    let sketch = Path::new(&fw.sketch)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())?;
    if let Some(f) = fixtures {
        let elf = f.join(format!("{sketch}.{tag}.elf"));
        if elf.exists() {
            return Some(elf);
        }
    }
    let img = dir?.join(fw.image.as_ref()?);
    let elf = img.with_extension("elf");
    if elf.exists() {
        return Some(elf);
    }
    img.exists().then_some(img)
}

impl BenchLink for SimBench {
    fn describe(&self) -> String {
        match &self.build_error {
            Some(e) => format!("bench error: {e}"),
            None => format!(
                "co-simulation · {} part{} · {} MCU{}",
                self.project.parts.len(),
                if self.project.parts.len() == 1 {
                    ""
                } else {
                    "s"
                },
                self.targets.len(),
                if self.targets.len() == 1 { "" } else { "s" }
            ),
        }
    }

    fn targets(&self) -> Vec<BenchTarget> {
        self.targets.clone()
    }

    fn upload(&mut self, target: &str, fw: &Firmware) -> Result<UploadReport, BenchError> {
        let part = self
            .mcu_part(target)
            .ok_or_else(|| BenchError::UnknownTarget(target.to_string()))?;
        let Some(h) = self.handle.clone() else {
            return Err(BenchError::Unsupported(
                self.build_error
                    .clone()
                    .unwrap_or_else(|| "the bench is not running".into()),
            ));
        };
        let t0 = Instant::now();
        let first = !self.mcus.get(target).is_some_and(|m| m.flashed);
        if let Some(exe) = &fw.host_exe {
            h.send(Command::FlashHost {
                part: part.clone(),
                exe: exe.clone(),
            });
        } else {
            let image = if let Some(elf) = &fw.elf {
                bb_avr::FirmwareImage::from_elf(elf)
                    .map_err(|e| BenchError::BadFirmware(e.to_string()))?
            } else if let Some(hex) = &fw.hex {
                bb_avr::FirmwareImage::from_hex(hex)
                    .map_err(|e| BenchError::BadFirmware(e.to_string()))?
            } else {
                return Err(BenchError::BadFirmware("no ELF or HEX".into()));
            };
            h.send(Command::Flash {
                part: part.clone(),
                image: Box::new(image),
            });
        }
        // attach the source-level debugger (ELF) and arm the breakpoints, then acknowledge
        let elf = fw.elf.clone();
        let specs = self.breakpoints.get(target).cloned().unwrap_or_default();
        let (atx, arx) = mpsc::channel::<(bool, Vec<(String, u32, u64)>)>();
        let atx = Mutex::new(atx);
        let p2 = part.clone();
        h.send(Command::With(BenchFn(Arc::new(move |b| {
            let mut armed = Vec::new();
            let mut attached = false;
            if let Some(m) = b.mcu(&p2)
                && let Some((slot, avr)) = m.avr_debug()
            {
                match elf.as_ref().and_then(|e| DebugInfo::from_elf(e).ok()) {
                    Some(info) => {
                        let info = Arc::new(info);
                        match slot.as_mut() {
                            Some(d) => d.rebind(info, avr),
                            None => *slot = Some(Debugger::new(info)),
                        }
                        if let Some(d) = slot.as_mut() {
                            sync_breakpoints(d, avr, &specs);
                            attached = true;
                            for bp in d.breakpoints() {
                                armed.push((file_name(&bp.file), bp.line, bp.hits));
                            }
                        }
                    }
                    None => *slot = None,
                }
            }
            if let Ok(t) = atx.lock() {
                let _ = t.send((attached, armed));
            }
        }))));
        let (attached, armed) = arx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| BenchError::Unsupported("the bench did not answer".into()))?;
        {
            let st = self.mcus.entry(target.to_string()).or_default();
            st.part = part.clone();
            st.flashed = true;
            st.debug_build = fw.debug_build;
            st.has_debugger = attached;
            st.paused = None;
        }
        self.set_running(true);
        let elapsed = t0.elapsed().max(Duration::from_millis(10));
        let report = UploadReport {
            target: target.to_string(),
            elapsed,
            message: format!(
                "Flashed virtual {target} in {:.2} s · core reset · {}",
                elapsed.as_secs_f64(),
                if first {
                    "bench started"
                } else {
                    "bench state kept"
                }
            ),
        };
        self.events.push(BenchEvent::Flashed(report.clone()));
        for (file, line, hits) in armed {
            self.events.push(BenchEvent::BreakpointArmed {
                target: target.to_string(),
                file,
                line,
                hits,
            });
        }
        self.request_debug(target);
        Ok(report)
    }

    fn set_running(&mut self, running: bool) {
        self.running = running;
        self.run_cmd_at = Some(Instant::now());
        if running {
            self.send(Command::Play);
        } else {
            self.send(Command::Pause);
            if let Some(a) = &self.audio {
                a.send(AudioMsg::Flush);
            }
        }
        let targets: Vec<String> = self.targets.iter().map(|t| t.id.clone()).collect();
        for t in targets {
            if self.mcus.get(&t).is_some_and(|m| m.has_debugger) {
                self.request_debug(&t);
            }
        }
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn reset(&mut self, scope: ResetScope) {
        match scope {
            ResetScope::Bench => {
                self.send(Command::Reset);
                for m in self.mcus.values_mut() {
                    m.paused = None;
                }
                if let Some(a) = &self.audio {
                    a.send(AudioMsg::Flush);
                }
            }
            ResetScope::Mcu(t) => {
                if let Some(p) = self.mcu_part(&t) {
                    self.send(Command::ResetMcu(p));
                }
            }
        }
    }

    fn set_speed(&mut self, speed: SimSpeed) {
        self.speed = speed;
        self.send(Command::SetSpeed(to_speed(speed)));
    }

    fn set_breakpoints(&mut self, target: &str, breakpoints: &[BreakpointSpec]) {
        self.breakpoints
            .insert(target.to_string(), breakpoints.to_vec());
        if self.mcus.get(target).is_some_and(|m| m.has_debugger) {
            self.sync_breakpoints(target);
            self.request_debug(target);
        } else {
            // no debug info yet: report the requested breakpoints unresolved
            let vm = self
                .debug
                .entry(target.to_string())
                .or_insert_with(|| DebugViewModel {
                    target: target.to_string(),
                    ..Default::default()
                });
            vm.breakpoints = breakpoints
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
    }

    fn debug(&mut self, target: &str, cmd: DebugCommand) {
        let Some(part) = self.mcu_part(target) else {
            return;
        };
        let has_dbg = self.mcus.get(target).is_some_and(|m| m.has_debugger);
        match cmd {
            DebugCommand::Continue => {
                self.send(Command::Debug(bb_sim::DebugCommand::Continue));
                self.running = true;
                self.run_cmd_at = Some(Instant::now());
            }
            DebugCommand::Pause => {
                if has_dbg {
                    self.send(Command::Debug(bb_sim::DebugCommand::Pause { part }));
                } else {
                    self.set_running(false);
                }
            }
            DebugCommand::StepOver
            | DebugCommand::StepInto
            | DebugCommand::StepOut
            | DebugCommand::StepInstruction => {
                if !has_dbg {
                    return;
                }
                let c = match cmd {
                    DebugCommand::StepOver => bb_sim::DebugCommand::StepOver { part },
                    DebugCommand::StepInto => bb_sim::DebugCommand::StepInto { part },
                    DebugCommand::StepOut => bb_sim::DebugCommand::StepOut { part },
                    _ => bb_sim::DebugCommand::StepInstruction { part },
                };
                self.send(Command::Debug(c));
                // the step completes when the engine runs again; it pauses by itself
                self.send(Command::Play);
                self.running = true;
                self.run_cmd_at = Some(Instant::now());
            }
        }
        if let Some(st) = self.mcus.get_mut(target) {
            st.paused = None;
        }
        self.request_debug(target);
    }

    fn send_serial(&mut self, target: &str, data: &[u8]) {
        if let Some(part) = self.mcu_part(target) {
            self.send(Command::SerialSend {
                part,
                port: 0,
                bytes: data.to_vec(),
            });
        }
    }

    fn radio(&mut self, cmd: RadioCommand) {
        use bb_sim::RadioCommand as R;
        let c = match cmd {
            RadioCommand::SetConditions(v) => {
                let mut cur = self
                    .telemetry()
                    .and_then(|t| t.radio.as_ref().map(|r| r.conditions.clone()))
                    .unwrap_or_default();
                cur.latency_ms = v.latency_ms;
                cur.jitter_ms = v.jitter_ms;
                cur.loss_pct = v.loss_pct;
                cur.drop_ap_at_s = v.drop_ap_at_s;
                R::SetConditions(cur)
            }
            RadioCommand::SetLanBridge(b) => {
                R::SetLanBridge(b.map(|(h, p)| bb_radio::bridge::BridgeConfig::new(&h, p)))
            }
            RadioCommand::AddBleHat => {
                self.events.push(BenchEvent::Note(
                    "Drag an HM-10 BLE module from the Workbench parts panel onto the bench; the virtual phone sees it right away.".into(),
                ));
                return;
            }
            RadioCommand::MqttPublish { topic, payload } => R::ExplorerPublish {
                topic,
                payload: payload.into_bytes(),
                qos: 0,
                retain: false,
            },
            RadioCommand::MqttSubscribe(f) => R::ExplorerSubscribe(f),
        };
        self.send(Command::Radio(c));
    }

    fn poll(&mut self) -> BenchUpdate {
        let mut u = BenchUpdate::default();
        if let Ok(mut n) = self.host_notes.lock() {
            self.events.append(&mut n);
        }
        let Some(h) = self.handle.clone() else {
            u.events = std::mem::take(&mut self.events);
            return u;
        };
        let tel = h.telemetry();
        // run state
        let tel_running = tel.status == bb_sim::RunStatus::Running;
        let settled = self
            .run_cmd_at
            .is_none_or(|t| t.elapsed() > Duration::from_millis(150));
        if settled {
            self.running = tel_running;
            self.run_cmd_at = None;
        }
        let status = tel.status.label().to_string();
        if status != self.last_status {
            if let bb_sim::RunStatus::Error(e) = &tel.status {
                self.events
                    .push(BenchEvent::Error(format!("Simulation stopped: {e}")));
            }
            self.last_status = status;
        }
        u.running = self.running;
        u.sim_time = tel.time;
        // serial
        u.serial = h
            .serial_lines()
            .into_iter()
            .map(|l| self.convert_serial(l))
            .collect();
        // events + audio
        for e in h.events() {
            self.handle_event(&e);
            self.recent.push(e);
        }
        if self.recent.len() > RECENT_EVENTS {
            let n = self.recent.len() - RECENT_EVENTS;
            self.recent.drain(..n);
        }
        let chunks = h.audio();
        if let Some(a) = &self.audio {
            for c in chunks {
                a.send(AudioMsg::Pcm {
                    sample_rate: c.sample_rate,
                    samples: c.samples,
                });
            }
        }
        // debugger: pause transitions + periodic hit counts
        let mut refresh = Vec::new();
        for m in &tel.mcus {
            let Some(st) = self.mcus.get_mut(&m.reference) else {
                continue;
            };
            if st.paused != m.paused {
                st.paused = m.paused.clone();
                if let Some(why) = &m.paused {
                    self.events.push(BenchEvent::Paused {
                        target: m.reference.clone(),
                        reason: pause_reason(why),
                    });
                }
                refresh.push(m.reference.clone());
            } else if st.has_debugger
                && self.running
                && self.last_debug_refresh.elapsed() > Duration::from_millis(500)
                && self
                    .breakpoints
                    .get(&m.reference)
                    .is_some_and(|b| !b.is_empty())
            {
                refresh.push(m.reference.clone());
            }
        }
        for r in refresh {
            self.request_debug(&r);
        }
        while let Ok(vm) = self.dbg_rx.try_recv() {
            self.debug.insert(vm.target.clone(), vm);
        }
        // keep the run state of debug views current without waiting for the engine
        for (t, vm) in self.debug.iter_mut() {
            let st = self.mcus.get(t);
            if !st.is_some_and(|s| s.flashed) {
                vm.state = DebugState::NoFirmware;
            } else if let Some(p) = st.and_then(|s| s.paused.as_ref()) {
                if !vm.is_paused() {
                    vm.state = DebugState::Paused {
                        reason: pause_reason(p),
                    };
                }
            } else if self.running {
                vm.state = DebugState::Running;
            }
        }
        u.debug = self.debug.values().cloned().collect();
        u.radio = tel.radio.as_ref().map(radio_model);
        u.probes = probe_values(&tel);
        u.events = std::mem::take(&mut self.events);
        u
    }

    fn as_sim(&mut self) -> Option<&mut SimBench> {
        Some(self)
    }
}

/// Pro `.op` keys for the live bench (`V(A0)`, `I(D1) avg`, `P(R2)`), plus the bb-circuit
/// element-name variants (`I(D_D1) avg`, `P(R_R2)`).
pub fn probe_values(t: &Telemetry) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for (name, v) in &t.nets {
        if name == "GND" || name == "0" {
            continue;
        }
        out.push((format!("V({})", name.to_ascii_uppercase()), *v));
    }
    for p in &t.parts {
        if let bb_core::Look::Led { current, .. } = &p.visual.look {
            let r = p.reference.to_ascii_uppercase();
            out.push((format!("I({r}) avg"), *current));
            out.push((format!("I(D_{r}) avg"), *current));
        }
    }
    for (r, w) in &t.power.dissipation {
        let r = r.to_ascii_uppercase();
        out.push((format!("P({r})"), *w));
        out.push((format!("P(R_{r})"), *w));
    }
    for p in &t.probes {
        out.push((format!("probe:{}", p.label), p.value));
    }
    out
}

/// Radios panel view model from the engine's radio telemetry.
pub fn radio_model(r: &bb_sim::RadioTelemetry) -> RadioPanelModel {
    let cards: Vec<RadioCardView> = r
        .cards
        .iter()
        .map(|c| {
            let lower = c.title.to_ascii_lowercase();
            let kind = if lower.contains("wi-fi") || lower.contains("wifi") || lower.contains("esp")
            {
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
                label: c.label.clone(),
                title: c.title.clone(),
                status: c.status.clone(),
                tone: status_tone(&c.status),
                kind,
                rows: c.rows.clone(),
            }
        })
        .collect();
    let log = r
        .log
        .iter()
        .map(|e| PacketRow {
            seq: e.seq,
            time: e.time_label.clone(),
            dir: e.dir.clone(),
            text: e.text.clone(),
            dropped: e.dropped,
        })
        .collect();
    let ble = (r.has_ble || cards.iter().any(|c| c.kind == RadioKind::Ble)).then(|| BleView {
        devices: if r.phone.scan.is_empty() {
            cards
                .iter()
                .filter(|c| c.kind == RadioKind::Ble)
                .map(|c| (format!("{} · {}", c.label, c.title), c.status.clone()))
                .collect()
        } else {
            r.phone
                .scan
                .iter()
                .map(|s| {
                    (
                        format!("{} · {}", s.name, s.addr),
                        format!("{} dBm", s.rssi),
                    )
                })
                .collect()
        },
    });
    RadioPanelModel {
        cards,
        log,
        conditions: NetConditionsView {
            latency_ms: r.conditions.latency_ms,
            jitter_ms: r.conditions.jitter_ms,
            loss_pct: r.conditions.loss_pct,
            drop_ap_at_s: r.conditions.drop_ap_at_s,
            bridge: r
                .lan_bridge
                .as_ref()
                .map(|b| format!("{}:{}", b.host, b.port)),
            bridge_status: r.lan_bridge_status.clone(),
        },
        ble,
        mqtt: r
            .mqtt
            .iter()
            .map(|m| (m.topic.clone(), m.payload.clone()))
            .collect(),
    }
}

/// `Breakpoint { id: 1, hits: 3 }` → `breakpoint (hits: 3)`.
fn pause_reason(raw: &str) -> String {
    let r = raw.trim();
    if r.starts_with("Breakpoint") {
        let hits = r
            .split("hits:")
            .nth(1)
            .map(|s| s.trim().trim_end_matches(['}', ' ']).trim().to_string())
            .unwrap_or_default();
        if hits.is_empty() {
            "breakpoint".into()
        } else {
            format!("breakpoint (hits: {hits})")
        }
    } else if r.starts_with("Step") || r == "step" {
        "step".into()
    } else if r.starts_with("User") || r == "paused" {
        "paused".into()
    } else if let Some(f) = r.strip_prefix("Fault") {
        format!("fault{f}")
    } else {
        r.to_string()
    }
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

fn same_file(a: &str, b: &str) -> bool {
    let f = |s: &str| {
        Path::new(s)
            .file_name()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    f(a) == f(b)
}

/// Makes the debugger's breakpoints match the editor's (same rules as `AvrBench`).
fn sync_breakpoints(dbg: &mut Debugger, avr: &mut bb_avr::Avr, specs: &[BreakpointSpec]) {
    let existing: Vec<_> = dbg.breakpoints().to_vec();
    for b in &existing {
        let fname = file_name(&b.file);
        let keep = specs
            .iter()
            .any(|s| same_file(&s.file, &fname) && s.line == b.requested_line);
        if !keep {
            dbg.remove_breakpoint(avr, b.id);
        }
    }
    for s in specs {
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
                .add_breakpoint(avr, &s.file, s.line, cond)
                .ok()
                .map(|b| b.id),
        };
        if let Some(id) = id {
            dbg.set_condition(id, cond);
            dbg.set_enabled(avr, id, s.enabled);
        }
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

/// Builds the debugger view of one MCU on the engine thread.
fn debug_vm(
    b: &mut bb_sim::Bench,
    part: &str,
    target: &str,
    st: &McuState,
    specs: &[BreakpointSpec],
    running: bool,
    bench_paused: Option<String>,
) -> DebugViewModel {
    let mut vm = DebugViewModel {
        target: target.to_string(),
        debug_build: st.debug_build,
        ..Default::default()
    };
    let paused_here = bench_paused
        .as_deref()
        .filter(|p| p.starts_with(&format!("{target}:")))
        .map(|p| p[target.len() + 1..].trim().to_string());
    vm.state = if !st.flashed {
        DebugState::NoFirmware
    } else if let Some(p) = &paused_here {
        DebugState::Paused {
            reason: pause_reason(p),
        }
    } else if running {
        DebugState::Running
    } else {
        DebugState::Paused {
            reason: "paused".into(),
        }
    };
    let Some(m) = b.mcu(part) else { return vm };
    let Some((slot, avr)) = m.avr_debug() else {
        return vm;
    };
    vm.registers = RegistersView {
        r: (0..32).map(|i| avr.reg(i)).collect(),
        sreg: avr.sreg(),
        sp: avr.sp(),
        pc: avr.pc(),
        cycle: avr.cycle(),
    };
    match slot.as_ref() {
        Some(dbg) => {
            vm.breakpoints = specs
                .iter()
                .map(|s| {
                    let bp = dbg.breakpoints().iter().find(|b| {
                        same_file(&s.file, &file_name(&b.file)) && b.requested_line == s.line
                    });
                    BreakpointView {
                        file: s.file.clone(),
                        line: s.line,
                        resolved_line: bp.map(|b| b.line),
                        hits: bp.map(|b| b.hits).unwrap_or(0),
                        enabled: s.enabled,
                    }
                })
                .collect();
            if vm.is_paused() {
                vm.location = dbg.location(avr).map(|l| SourcePos {
                    file: file_name(&l.file),
                    line: l.line,
                });
                vm.function = dbg.info().function_name(avr.pc());
                vm.call_stack = dbg
                    .call_stack(avr)
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
                vm.locals = dbg.locals(avr, 0).into_iter().map(var_view).collect();
                let mut g: Vec<VarView> = dbg
                    .globals(avr)
                    .into_iter()
                    .filter(|v| !v.name.starts_with('_'))
                    .map(var_view)
                    .collect();
                g.truncate(64);
                vm.globals = g;
            }
        }
        None => {
            vm.breakpoints = specs
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
    }
    vm
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn spec(example: &str) -> BenchSpec {
        let dir = repo().join("examples").join(example);
        let p = crate::project::Project::load(&dir).unwrap();
        BenchSpec {
            project_dir: Some(dir),
            targets: p.bench_targets(),
            radios: p.radios.clone(),
            repaint: Arc::new(|| {}),
            speed: SimSpeed::Max,
            circuit: None,
            fixtures: Some(repo().join("examples/fixtures")),
            audio: false,
        }
    }

    fn wait(b: &mut SimBench, secs: f64, mut f: impl FnMut(&BenchUpdate) -> bool) -> bool {
        let t0 = Instant::now();
        while t0.elapsed().as_secs_f64() < secs {
            let u = b.poll();
            if f(&u) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn pause_reasons_read_like_the_design() {
        assert_eq!(
            pause_reason("Breakpoint { id: 1, hits: 3 }"),
            "breakpoint (hits: 3)"
        );
        assert_eq!(pause_reason("Step"), "step");
        assert_eq!(pause_reason("User"), "paused");
    }

    #[test]
    fn garden_node_runs_from_the_prebuilt_image_with_probes_and_radio() {
        let mut b = SimBench::new(&spec("garden-node"));
        assert!(b.build_error().is_none(), "{:?}", b.build_error());
        assert_eq!(b.targets()[0].id, "U1");
        b.set_running(true);
        let mut lines: Vec<SerialLine> = Vec::new();
        let mut probes = Vec::new();
        let mut radio = None;
        assert!(
            wait(&mut b, 120.0, |u| {
                lines.extend(u.serial.iter().cloned());
                if !u.probes.is_empty() {
                    probes = u.probes.clone();
                }
                if u.radio.is_some() {
                    radio = u.radio.clone();
                }
                lines.iter().any(|l| l.text.starts_with("soil_raw="))
                    && lines.iter().any(|l| l.text.starts_with("mqtt: PUB"))
            }),
            "serial: {lines:#?}"
        );
        assert!(lines.iter().all(|l| l.target == "U1"));
        let a0 = probes.iter().find(|(k, _)| k == "V(A0)").map(|x| x.1);
        assert!(a0.is_some_and(|v| v > 1.0 && v < 3.5), "{probes:?}");
        assert_eq!(radio.unwrap().cards[0].label, "U2");
    }

    #[test]
    fn blink_breakpoint_hit_and_step_over_through_the_engine() {
        let elf = std::fs::read(repo().join("examples/fixtures/blink.uno.elf")).unwrap();
        let mut b = SimBench::new(&spec("blink"));
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
                    origin: "fixture".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            r.message.starts_with("Flashed virtual U1 in "),
            "{}",
            r.message
        );
        let paused_line = |u: &BenchUpdate| {
            u.debug
                .iter()
                .find(|d| d.target == "U1" && d.is_paused())
                .and_then(|d| d.location.as_ref().map(|l| l.line))
        };
        assert!(
            wait(&mut b, 30.0, |u| paused_line(u) == Some(8)),
            "breakpoint never hit"
        );
        b.debug("U1", DebugCommand::StepOver);
        assert!(
            wait(&mut b, 30.0, |u| paused_line(u) == Some(9)),
            "step over did not land on line 9"
        );
        b.debug("U1", DebugCommand::Continue);
        assert!(wait(&mut b, 30.0, |u| paused_line(u) == Some(8)));
    }
}
