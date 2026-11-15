//! # breadbox-e2e — headless end-to-end scenarios (`TEST-4`)
//!
//! The scenario tests in `tests/` drive the whole stack through the entry points the app uses:
//! projects loaded from `examples/`, the threaded [`bb_sim::SimHandle`] (commands in, telemetry +
//! serial/event streams out), `bb_plugin::{Registry, PluginHost}` for plugins, `bb_lessons`
//! sessions + checkers, `bb_firmware::Toolchain` when it is installed (prebuilt fixtures
//! otherwise), and the app crate's own project / export code.
//!
//! [`Sim`] makes the threaded engine deterministic for tests: every run is a
//! [`bb_sim::Command::Step`] at maximum speed, and the driver waits until the engine has paused
//! at exactly the requested simulated time before the next command, so commands land at exact
//! simulated times and nothing depends on the wall clock.
//!
//! Adding a scenario: load a project ([`example`]), build options ([`bench_options`] or a
//! `PluginHost`), [`Sim::spawn`], then `run_for` / `send` / `tel` / `with`.

use bb_avr::FirmwareImage;
use bb_circuit::{Library, Project};
use bb_core::{DeviceEvent, SimTime};
use bb_sim::handle::BenchFn;
use bb_sim::{
    Bench, BenchOptions, Command, LineKind, PartTelemetry, ProbeReading, RunStatus, SerialLine,
    SimEvent, SimHandle, Speed, Telemetry,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub use bb_core::SimTime as Time;

/// Repository root.
pub fn root() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    p.canonicalize().unwrap_or(p)
}

pub fn examples_dir() -> PathBuf {
    root().join("examples")
}

/// The built-in parts library (shared).
pub fn lib() -> Arc<Library> {
    Library::builtin()
}

/// Loads `examples/<name>` → (project, folder).
pub fn example(name: &str) -> (Project, PathBuf) {
    let dir = examples_dir().join(name);
    let p = Project::load(&dir).unwrap_or_else(|e| panic!("load example {name}: {e}"));
    (p, dir)
}

/// Every example project folder (those with a `project.json`), sorted.
pub fn example_names() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(examples_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("project.json").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

pub fn secs(s: f64) -> SimTime {
    SimTime::from_secs_f64(s)
}
pub fn ms(v: u64) -> SimTime {
    SimTime::from_ms(v)
}

// ---------------------------------------------------------------------------- temp dirs

/// A scratch folder removed on drop (unless `BREADBOX_E2E_KEEP=1`).
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let base = std::env::var_os("BREADBOX_E2E_TMP")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let p = base.join(format!("bb-e2e-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if std::env::var_os("BREADBOX_E2E_KEEP").is_none() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

// ---------------------------------------------------------------------------- firmware

/// Where a firmware image came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FwSource {
    /// Compiled now by arduino-cli / avr-gcc.
    Toolchain,
    /// The checked-in prebuilt fixture (`examples/fixtures/*.hex|elf`).
    Fixture,
}

/// The installed Arduino toolchain, if it is ready (detected once per test binary).
pub fn toolchain() -> Option<&'static bb_firmware::Toolchain> {
    static TC: OnceLock<Option<bb_firmware::Toolchain>> = OnceLock::new();
    TC.get_or_init(|| {
        if std::env::var_os("BREADBOX_E2E_NO_TOOLCHAIN").is_some() {
            return None;
        }
        bb_firmware::Toolchain::detect().ok().filter(|t| t.ready())
    })
    .as_ref()
}

/// True when host boards (ESP32 / Pico) can be compiled (a C++ compiler is present).
pub fn have_cxx() -> bool {
    bb_firmware::host::cxx().is_some()
}

/// True when ngspice is on PATH.
pub fn ngspice() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("ngspice"))
        .find(|p| p.is_file())
}

/// An AVR image (+ ELF) for an example's MCU: compiled from its sketch with the real toolchain
/// when available, else its prebuilt fixture.
pub fn avr_firmware(
    project: &Project,
    dir: &Path,
    part: &str,
    debug: bool,
) -> (FirmwareImage, Option<Vec<u8>>, FwSource) {
    let inst = project
        .part(part)
        .unwrap_or_else(|| panic!("no part {part}"));
    let fw = inst.firmware.as_ref().expect("part has firmware");
    if let Some(tc) = toolchain() {
        let fqbn = fw.fqbn.clone().unwrap_or_else(|| "arduino:avr:uno".into());
        let mut opts = bb_firmware::CompileOptions::new(&fqbn);
        opts.debug = debug;
        match tc.compile(&dir.join(&fw.sketch), &opts) {
            Ok(out) if out.success => {
                let img = out.image().expect("image from a successful build");
                return (img, out.elf.clone(), FwSource::Toolchain);
            }
            Ok(out) => panic!(
                "{part}: sketch does not compile:\n{}",
                out.log.lines().take(40).collect::<Vec<_>>().join("\n")
            ),
            Err(e) => eprintln!("toolchain error ({e}); falling back to the fixture"),
        }
    }
    let img_rel = fw.image.as_ref().expect("prebuilt image path");
    let img_path = dir.join(img_rel);
    let img = bb_sim::firmware::load_image(&img_path).unwrap();
    let elf = std::fs::read(img_path.with_extension("elf")).ok();
    (img, elf, FwSource::Fixture)
}

/// Bench options for an example project the way the app starts it: prebuilt AVR images for
/// every MCU (Run works at once) and host-compiled executables for ESP32 / Pico boards.
/// Returns `None` when the project needs a host board but no C++ compiler exists.
pub fn bench_options(project: &Project, dir: &Path) -> Option<BenchOptions> {
    let mut opts = BenchOptions {
        firmware: bb_sim::firmware::project_images(project, dir),
        ..Default::default()
    };
    let needs_host = project.parts.iter().any(|p| {
        p.firmware.is_some()
            && lib()
                .get(&p.part)
                .and_then(|d| d.model.mcu.as_ref())
                .is_some_and(|m| m.core == "host")
    });
    if needs_host {
        if !have_cxx() {
            return None;
        }
        opts.host_firmware = host_exes(project, dir);
    }
    Some(opts)
}

/// Host builds for a project's ESP32 / Pico boards (cached per test binary + project).
pub fn host_exes(project: &Project, dir: &Path) -> HashMap<String, PathBuf> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, HashMap<String, PathBuf>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut c = cache.lock().unwrap();
    c.entry(dir.to_path_buf())
        .or_insert_with(|| {
            bb_sim::firmware::compile_host_project(project, dir, &lib())
                .unwrap_or_else(|e| panic!("host build of {}: {e}", dir.display()))
        })
        .clone()
}

// ---------------------------------------------------------------------------- driver

/// Why a [`Sim::run_for`] returned.
#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    /// Reached the requested time.
    Reached,
    /// An MCU paused (breakpoint / step) before the requested time: (part, reason).
    Paused(String, String),
}

/// Deterministic driver over the threaded engine.
pub struct Sim {
    pub handle: SimHandle,
    /// Every serial line received from the engine's stream so far.
    pub serial: Vec<SerialLine>,
    /// Every event received so far.
    pub events: Vec<SimEvent>,
    /// Audio chunks received.
    pub audio_chunks: usize,
    last: Arc<Telemetry>,
    /// Wall-clock budget for one `run_for` (debug builds are slow; generous by default).
    pub timeout: Duration,
}

impl Sim {
    /// Spawns the engine thread (paused) at maximum speed.
    pub fn spawn(project: Project, lib: Arc<Library>, opts: BenchOptions) -> Sim {
        let handle = SimHandle::spawn(project, lib, opts).unwrap_or_else(|e| panic!("{e}"));
        handle.send(Command::SetSpeed(Speed::Max));
        let last = handle.telemetry();
        Sim {
            handle,
            serial: Vec::new(),
            events: Vec::new(),
            audio_chunks: 0,
            last,
            timeout: Duration::from_secs(300),
        }
    }

    /// An example project with its firmware, as the app opens it.
    pub fn example(name: &str) -> Option<Sim> {
        let (p, dir) = example(name);
        let opts = bench_options(&p, &dir)?;
        Some(Sim::spawn(p, lib(), opts))
    }

    pub fn send(&self, c: Command) {
        self.handle.send(c);
    }

    /// Runs `f` on the engine thread with the bench and returns its result (a fence: every
    /// command sent before has been applied).
    pub fn with<R: Send + 'static>(&self, f: impl FnOnce(&mut Bench) -> R + Send + 'static) -> R {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let cell = Mutex::new(Some(f));
        let bf = BenchFn(Arc::new(move |b: &mut Bench| {
            if let Some(f) = cell.lock().unwrap().take() {
                let _ = tx.send(f(b));
            }
        }));
        self.handle.send(Command::With(bf));
        rx.recv_timeout(Duration::from_secs(120))
            .expect("engine thread answered Command::With")
    }

    /// Exact engine time.
    pub fn time(&self) -> SimTime {
        self.with(|b| b.time())
    }

    fn drain(&mut self) {
        self.serial.extend(self.handle.serial_lines());
        self.events.extend(self.handle.events());
        self.audio_chunks += self.handle.audio().len();
    }

    /// Runs `dt` of simulated time (as fast as possible) and waits until the engine paused
    /// there. Panics on an engine error or the wall-clock timeout.
    pub fn run_for(&mut self, dt: SimTime) -> Stop {
        let start = self.time();
        let target = start + dt;
        self.handle.send(Command::Step(dt));
        let t0 = Instant::now();
        loop {
            self.drain();
            let tel = self.handle.telemetry();
            if let RunStatus::Error(e) = &tel.status {
                panic!(
                    "engine error at t={:.3}s: {e}\nlast serial: {:#?}",
                    tel.time.as_secs_f64(),
                    self.serial.iter().rev().take(10).collect::<Vec<_>>()
                );
            }
            let paused_mcu = tel
                .mcus
                .iter()
                .find_map(|m| m.paused.clone().map(|r| (m.part.clone(), r)));
            let done = tel.time >= target && tel.status != RunStatus::Running;
            let paused =
                tel.status == RunStatus::Paused && tel.time > start && paused_mcu.is_some();
            if done || paused {
                // fence: the publish that stored this snapshot has also pushed its stream items
                self.with(|_| ());
                self.drain();
                self.last = tel.clone();
                return match paused_mcu {
                    Some((p, r)) if !done => Stop::Paused(p, r),
                    _ => Stop::Reached,
                };
            }
            if t0.elapsed() > self.timeout {
                panic!(
                    "run_for({:.3}s) timed out at t={:.3}s (status {:?})",
                    dt.as_secs_f64(),
                    tel.time.as_secs_f64(),
                    tel.status
                );
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn run_secs(&mut self, s: f64) -> Stop {
        self.run_for(secs(s))
    }

    /// Runs until the absolute simulated time `t`.
    pub fn run_to(&mut self, t: SimTime) -> Stop {
        let now = self.time();
        if t > now {
            self.run_for(t - now)
        } else {
            Stop::Reached
        }
    }

    /// Telemetry at the end of the last run (the engine is paused there).
    pub fn tel(&self) -> Arc<Telemetry> {
        self.last.clone()
    }

    /// A fresh telemetry snapshot built on the engine thread now.
    pub fn tel_now(&mut self) -> Arc<Telemetry> {
        let t = Arc::new(self.with(|b| b.telemetry()));
        self.last = t.clone();
        t
    }

    pub fn probe(&self, label: &str) -> ProbeReading {
        self.last
            .probes
            .iter()
            .find(|p| p.label == label)
            .unwrap_or_else(|| {
                panic!(
                    "no probe {label:?}: {:?}",
                    self.last
                        .probes
                        .iter()
                        .map(|p| &p.label)
                        .collect::<Vec<_>>()
                )
            })
            .clone()
    }

    pub fn part(&self, id: &str) -> PartTelemetry {
        self.last
            .parts
            .iter()
            .find(|p| p.part == id)
            .unwrap_or_else(|| panic!("no part {id} in telemetry"))
            .clone()
    }

    pub fn reading(&self, part: &str, key: &str) -> Option<f64> {
        self.part(part)
            .readings
            .iter()
            .find(|(k, _)| k == key)
            .map(|x| x.1)
    }

    /// Firmware output lines of one MCU (designator, e.g. `U1`) received from the stream.
    pub fn data_lines(&self, mcu: &str) -> Vec<&SerialLine> {
        self.serial
            .iter()
            .filter(|l| l.mcu == mcu && l.kind == LineKind::Data)
            .collect()
    }

    pub fn lines_of(&self, mcu: &str, kind: LineKind) -> Vec<&SerialLine> {
        self.serial
            .iter()
            .filter(|l| l.mcu == mcu && l.kind == kind)
            .collect()
    }

    /// Damage events (source, reason).
    pub fn damage(&self) -> Vec<(String, String)> {
        self.events
            .iter()
            .filter_map(|e| match &e.event {
                DeviceEvent::Damage { reason } => Some((e.source.clone(), reason.clone())),
                _ => None,
            })
            .collect()
    }

    /// Error-level logs and damage events — what a clean run must not have.
    pub fn problems(&self) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|e| match &e.event {
                DeviceEvent::Damage { reason } => Some(format!("{}: damage: {reason}", e.source)),
                DeviceEvent::Log {
                    level: bb_core::LogLevel::Error,
                    text,
                } => Some(format!("{}: {text}", e.source)),
                _ => None,
            })
            .collect()
    }

    pub fn shutdown(self) {
        self.handle.shutdown();
    }
}

/// Value of `key=` in a `k=v k2=v2` line (`%` stripped).
pub fn field(line: &str, key: &str) -> Option<f64> {
    line.split_whitespace()
        .find_map(|tok| tok.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.trim_end_matches(['%', ',']).parse().ok())
}

/// Plugins for tests: a registry over the repository's bundled plugins with a scratch user
/// folder (first run → the preinstalled first-party plugins are installed and approved), and
/// the host built from it — exactly how the app starts.
pub fn plugin_registry(user_dir: &Path) -> bb_plugin::Registry {
    let bundled = root().join("plugins");
    bb_plugin::Registry::open(Some(&bundled), user_dir)
}
