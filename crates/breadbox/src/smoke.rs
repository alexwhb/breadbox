//! `breadbox --smoke <project | example name | lesson:<id>> [--seconds N] [--speed max|<factor>]
//! [--headless] [--warmup S] [--place <plugin id>]` — a measurable start-up + run path for
//! `scripts/test-all.sh` and `scripts/check-bundle.sh` (`PLAT-4`, `SIM-4`): the real
//! [`BreadboxApp`] opens the project on the Workbench (or the lesson on the Lessons screen, with
//! its own bench), runs its bench with the GUI drawing every frame, and the process prints
//! one line
//!
//! ```text
//! SMOKE project=garden-node mode=window first_frame_ms=412 sim_s=10.00 wall_s=0.61 realtime=16.4 frames=37 frame_ms_avg=1.2 frame_ms_max=6.3 status=ok
//! ```
//!
//! `--place <plugin id>` puts the plugin's instrument on the bench first (like "Place on
//! bench"); instruments with a display add `instruments=DMM1:1.882V` before `status=` and the
//! run fails if a placed instrument never shows a reading.
//!
//! and exits non-zero when the bench reports an error, a part is damaged, the ERC has errors
//! or the run does not finish in time. With `--headless` (or without a display) the same app
//! draws into an egui context without a window (layout + tessellation, no GPU).
//!
//! The smoke run never touches the user's saved app state (no persistence, separate storage).

use crate::app::{AppOptions, BreadboxApp, PersistedState};
use crate::bench::SimSpeed;
use crate::router::{Route, ScreenSlot};
use crate::screens::lessons::LessonsScreen;
use eframe::egui;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Parsed `--smoke` arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct SmokeArgs {
    /// The project folder (empty for a lesson).
    pub project: PathBuf,
    /// `lesson:<id>`: run that lesson's own bench on the Lessons screen instead.
    pub lesson: Option<String>,
    /// A plugin whose instrument is placed on the bench before the run.
    pub place: Option<String>,
    /// Simulated seconds measured after the warm-up.
    pub seconds: f64,
    /// Simulated seconds before the measurement starts (boot, Wi-Fi join).
    pub warmup: f64,
    pub speed: SimSpeed,
    pub headless: bool,
}

impl SmokeArgs {
    /// `None` when `--smoke` is not among the arguments.
    pub fn parse(args: &[String]) -> Option<Result<SmokeArgs, String>> {
        let i = args.iter().position(|a| a == "--smoke")?;
        Some(Self::parse_after(&args[i + 1..]))
    }

    fn parse_after(rest: &[String]) -> Result<SmokeArgs, String> {
        let mut it = rest.iter();
        let project = it
            .next()
            .filter(|p| !p.starts_with("--"))
            .ok_or("usage: breadbox --smoke <project folder | example name | lesson:<id>> [--seconds N] [--speed max|<factor>] [--headless] [--warmup S] [--place <plugin id>]")?;
        let lesson = project.strip_prefix("lesson:").map(str::to_string);
        let mut a = SmokeArgs {
            project: match &lesson {
                Some(_) => PathBuf::new(),
                None => resolve_project(project)?,
            },
            lesson,
            place: None,
            seconds: 5.0,
            warmup: 1.0,
            speed: SimSpeed::Max,
            headless: false,
        };
        while let Some(flag) = it.next() {
            let mut value = || {
                it.next()
                    .cloned()
                    .ok_or_else(|| format!("{flag} needs a value"))
            };
            match flag.as_str() {
                "--seconds" => a.seconds = num(&value()?)?,
                "--warmup" => a.warmup = num(&value()?)?,
                "--speed" => {
                    let v = value()?;
                    a.speed = if v.eq_ignore_ascii_case("max") {
                        SimSpeed::Max
                    } else {
                        SimSpeed::RealTime(num(v.trim_end_matches(['x', '×']))?)
                    };
                }
                "--headless" => a.headless = true,
                "--place" => a.place = Some(value()?),
                other => return Err(format!("unknown smoke option {other}")),
            }
        }
        if a.seconds <= 0.0 {
            return Err("--seconds must be positive".into());
        }
        Ok(a)
    }
}

fn num(s: &str) -> Result<f64, String> {
    s.parse::<f64>().map_err(|_| format!("not a number: {s}"))
}

/// A project folder, or the name of a bundled example (`garden-node`).
fn resolve_project(p: &str) -> Result<PathBuf, String> {
    let direct = PathBuf::from(p);
    if direct.join("project.json").exists() {
        return Ok(direct);
    }
    if let Some(ex) = crate::project::Resources::locate().examples() {
        let d = ex.join(p);
        if d.join("project.json").exists() {
            return Ok(d);
        }
    }
    Err(format!(
        "{p}: no project.json (not a project folder or example name)"
    ))
}

/// What a smoke run measured.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SmokeReport {
    pub project: String,
    pub mode: String,
    /// Process start → first frame of the app (`PLAT-4`).
    pub first_frame_ms: f64,
    /// Simulated seconds in the measured window.
    pub sim_s: f64,
    pub wall_s: f64,
    /// Simulated seconds per wall second while the GUI runs (`SIM-4`).
    pub realtime: f64,
    pub frames: u64,
    pub frame_ms_avg: f64,
    pub frame_ms_max: f64,
    /// Instrument displays at the end of the run: (reference, reading).
    pub instruments: Vec<(String, String)>,
    pub errors: Vec<String>,
}

impl SmokeReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// The one-line summary `scripts/test-all.sh` parses.
    pub fn line(&self) -> String {
        let instruments = if self.instruments.is_empty() {
            String::new()
        } else {
            let v: Vec<String> = self
                .instruments
                .iter()
                .map(|(r, t)| format!("{r}:{}", t.replace(' ', "")))
                .collect();
            format!(" instruments={}", v.join(","))
        };
        format!(
            "SMOKE project={} mode={} first_frame_ms={:.0} sim_s={:.2} wall_s={:.2} realtime={:.2} frames={} frame_ms_avg={:.2} frame_ms_max={:.2}{instruments} status={}",
            self.project,
            self.mode,
            self.first_frame_ms,
            self.sim_s,
            self.wall_s,
            self.realtime,
            self.frames,
            self.frame_ms_avg,
            self.frame_ms_max,
            if self.ok() {
                "ok".to_string()
            } else {
                format!("error: {}", self.errors.join("; "))
            }
        )
    }
}

/// Drives the app through one smoke run, frame by frame.
struct Driver {
    args: SmokeArgs,
    started: Instant,
    opened: bool,
    measure: Option<(Instant, f64)>,
    frame_sum: f64,
    frame_max: f64,
    frames: u64,
    deadline: Instant,
    report: SmokeReport,
    done: bool,
    /// Reference of the instrument `--place` put on the bench.
    placed: Option<String>,
}

impl Driver {
    fn new(args: SmokeArgs, mode: &str) -> Driver {
        // generous: debug builds, slow CI machines; the realtime figure is the real gate
        let budget = 120.0 + (args.seconds + args.warmup) * 20.0;
        let project = match &args.lesson {
            Some(id) => format!("lesson:{id}"),
            None => args
                .project
                .file_name()
                .map(|n| {
                    n.to_string_lossy()
                        .trim_end_matches(".breadbox")
                        .to_string()
                })
                .unwrap_or_default(),
        };
        Driver {
            started: Instant::now(),
            opened: false,
            measure: None,
            frame_sum: 0.0,
            frame_max: 0.0,
            frames: 0,
            deadline: Instant::now() + Duration::from_secs_f64(budget),
            report: SmokeReport {
                project,
                mode: mode.into(),
                ..Default::default()
            },
            done: false,
            placed: None,
            args,
        }
    }

    /// Before the app draws a frame.
    fn before(&mut self, app: &mut BreadboxApp) {
        if self.opened {
            return;
        }
        self.opened = true;
        self.report.first_frame_ms = crate::STARTED
            .get()
            .unwrap_or(&self.started)
            .elapsed()
            .as_secs_f64()
            * 1e3;
        if let Some(id) = self.args.lesson.clone() {
            app.ctx.navigate(Route::Lessons);
            let Some(lessons) = lessons_screen(app) else {
                self.fail("no Lessons screen".into());
                return;
            };
            // `open_lesson` needs the screen and the app context at once
            let mut screen = std::mem::take(lessons);
            screen.open_lesson(&mut app.ctx, &id);
            let error = screen.error.clone();
            let bench = screen.run.as_ref().map(|r| r.bench.is_some());
            if let Some(l) = lessons_screen(app) {
                *l = screen;
            }
            match (error, bench) {
                (Some(e), _) => self.fail(e),
                (None, Some(true)) => {}
                (None, Some(false)) => self.fail(format!("lesson {id} has no bench (Pro)")),
                (None, None) => self.fail(format!("lesson {id} did not open")),
            }
            return;
        }
        app.open_path(&self.args.project.clone());
        if app.ctx.project.is_none() {
            self.fail("the project did not open".into());
            return;
        }
        app.ctx.navigate(Route::Workbench);
        if let Some(plugin) = self.args.place.clone() {
            match place_plugin(app, &plugin) {
                Ok(reference) => self.placed = Some(reference),
                Err(e) => {
                    self.fail(format!("place {plugin}: {e}"));
                    return;
                }
            }
        }
        app.ctx.bench_link().set_running(true);
    }

    /// The running bench: (host builds pending, latest telemetry).
    fn bench(&self, app: &mut BreadboxApp) -> (bool, Option<Arc<bb_sim::Telemetry>>) {
        if self.args.lesson.is_some() {
            let tel = lessons_screen(app)
                .and_then(|l| l.run.as_ref())
                .and_then(|r| r.telemetry());
            return (false, tel);
        }
        let pending = app.ctx.sim().is_some_and(|s| s.host_build_pending());
        (pending, app.ctx.sim().and_then(|s| s.telemetry()))
    }

    /// After the app drew a frame that took `dt`.
    fn after(&mut self, app: &mut BreadboxApp, dt: Duration) {
        if self.done {
            return;
        }
        let (pending, tel) = self.bench(app);
        let Some(tel) = tel else {
            if Instant::now() > self.deadline {
                self.fail("the bench never started".into());
            }
            return;
        };
        let t = tel.time.as_secs_f64();
        if let bb_sim::RunStatus::Error(e) = &tel.status {
            self.fail(format!("bench error: {e}"));
            return;
        }
        if self.measure.is_some() {
            let ms = dt.as_secs_f64() * 1e3;
            self.frames += 1;
            self.frame_sum += ms;
            self.frame_max = self.frame_max.max(ms);
        }
        // API-level boards are compiled in the background and the bench is rebuilt around
        // them: measure only once every MCU runs its firmware (and restart after a rebuild)
        let loaded = !pending;
        if self.measure.is_some_and(|(_, t0)| t < t0) {
            self.measure = None;
            self.frames = 0;
            self.frame_sum = 0.0;
            self.frame_max = 0.0;
        }
        match self.measure {
            None if loaded && t >= self.args.warmup => self.measure = Some((Instant::now(), t)),
            Some((w0, t0)) if t - t0 >= self.args.seconds => {
                let wall = w0.elapsed().as_secs_f64();
                let r = &mut self.report;
                r.sim_s = t - t0;
                r.wall_s = wall;
                r.realtime = (t - t0) / wall.max(1e-9);
                r.frames = self.frames;
                r.frame_ms_avg = self.frame_sum / self.frames.max(1) as f64;
                r.frame_ms_max = self.frame_max;
                for p in &tel.parts {
                    if let bb_core::Health::Damaged { reason } = &p.visual.health {
                        r.errors.push(format!("{} damaged: {reason}", p.reference));
                    }
                    if let Some(d) = p
                        .panel
                        .as_ref()
                        .and_then(bb_plugin::panel::PanelState::from_json)
                        .and_then(|s| s.displays.into_values().next())
                    {
                        r.instruments
                            .push((p.reference.clone(), format!("{} {}", d.text, d.unit)));
                    }
                }
                if let Some(placed) = &self.placed {
                    let reading = r.instruments.iter().find(|(rf, _)| rf == placed);
                    let shows = reading.is_some_and(|(_, t)| {
                        t.trim()
                            .split(' ')
                            .next()
                            .is_some_and(|v| v.parse::<f64>().is_ok())
                    });
                    if !shows {
                        r.errors.push(format!(
                            "{placed} shows no reading ({})",
                            reading.map(|(_, t)| t.as_str()).unwrap_or("no panel")
                        ));
                    }
                }
                for e in &tel.erc {
                    if e.severity == bb_circuit::erc::ErcSeverity::Error {
                        r.errors.push(format!("ERC: {}", e.message));
                    }
                }
                self.done = true;
            }
            _ => {}
        }
        if !self.done && Instant::now() > self.deadline {
            self.fail(format!("timed out at t = {t:.2} s"));
        }
    }

    fn fail(&mut self, why: String) {
        self.report.errors.push(why);
        self.done = true;
    }
}

fn lessons_screen(app: &mut BreadboxApp) -> Option<&mut LessonsScreen> {
    app.screens
        .get_mut(ScreenSlot::Lessons)?
        .as_any_mut()?
        .downcast_mut::<LessonsScreen>()
}

/// "Place on bench" without the UI: the plugin's first part on the open circuit. Returns the
/// new part's reference (`DMM1`).
fn place_plugin(app: &mut BreadboxApp, plugin: &str) -> Result<String, String> {
    let host = app.ctx.plugins.host();
    let def = host
        .parts_of(plugin)
        .first()
        .cloned()
        .ok_or("not installed, or it has no part")?;
    let lib = app.ctx.plugins.library();
    let doc = app.ctx.circuit.as_mut().ok_or("no circuit")?;
    if doc.lib.get(&def).is_none() {
        doc.set_library(lib);
    }
    let r = crate::screens::plugins::place::place(doc, &def)?;
    Ok(r.reference)
}

fn app_options(speed: SimSpeed) -> AppOptions {
    AppOptions {
        load_persisted: false,
        native_menu: false,
        detect_toolchain: false,
        sim_speed: speed,
        audio: false,
        // opening a lesson saves its progress: keep the user's file out of it
        progress_path: Some(std::env::temp_dir().join(format!(
            "breadbox-smoke-progress-{}.json",
            std::process::id()
        ))),
        ..AppOptions::default()
    }
}

/// The window around the app for a smoke run.
struct SmokeApp {
    app: BreadboxApp,
    driver: Driver,
    out: Arc<Mutex<Option<SmokeReport>>>,
}

impl eframe::App for SmokeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.driver.before(&mut self.app);
        let t0 = Instant::now();
        self.app.frame(ui);
        self.driver.after(&mut self.app, t0.elapsed());
        if self.driver.done {
            *self.out.lock().unwrap() = Some(self.driver.report.clone());
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            // a GUI at 60 Hz (the bench's own watcher repaints ≤ 30 Hz on top)
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
    }
    // never write the user's saved state
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {}
    fn persist_egui_memory(&self) -> bool {
        false
    }
}

/// Runs a smoke test in a real window (wgpu); `None` if no window could be opened.
pub fn run_windowed(args: SmokeArgs) -> Option<SmokeReport> {
    crate::STARTED.get_or_init(Instant::now);
    let out = Arc::new(Mutex::new(None));
    let out2 = out.clone();
    let mut native = crate::native_options();
    native.persist_window = false;
    native.persistence_path = Some(std::env::temp_dir().join("breadbox-smoke-state"));
    let speed = args.speed;
    let r = eframe::run_native(
        "Breadbox smoke",
        native,
        Box::new(move |cc| {
            let app = BreadboxApp::with_context(
                &cc.egui_ctx,
                app_options(speed),
                PersistedState::default(),
            );
            Ok(Box::new(SmokeApp {
                app,
                driver: Driver::new(args, "window"),
                out: out2,
            }))
        }),
    );
    if let Err(e) = r {
        log::warn!("smoke: no window ({e}); running headless");
        return None;
    }
    let report = out.lock().unwrap().take();
    Some(report.unwrap_or_else(|| SmokeReport {
        errors: vec!["the window closed before the run finished".into()],
        ..Default::default()
    }))
}

/// Runs a smoke test without a window: the app draws into an egui context at ~60 Hz (layout
/// and tessellation on the CPU).
pub fn run_headless(args: SmokeArgs) -> SmokeReport {
    let opts = app_options(args.speed);
    run_headless_with(args, opts)
}

/// [`run_headless`] with explicit app options (tests: temporary plugin + progress folders).
pub fn run_headless_with(args: SmokeArgs, opts: AppOptions) -> SmokeReport {
    crate::STARTED.get_or_init(Instant::now);
    let ctx = egui::Context::default();
    let mut app = BreadboxApp::with_context(&ctx, opts, PersistedState::default());
    let mut d = Driver::new(args, "headless");
    let frame = Duration::from_millis(16);
    let raw = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1440.0, 900.0),
        )),
        ..Default::default()
    };
    while !d.done {
        let t0 = Instant::now();
        d.before(&mut app);
        let mut out = ctx.run_ui(raw(), |ui| app.frame(ui));
        let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        out.textures_delta.clear();
        let dt = t0.elapsed();
        d.after(&mut app, dt);
        if let Some(rest) = frame.checked_sub(dt) {
            std::thread::sleep(rest);
        }
    }
    d.report
}

/// `main`'s entry: runs the smoke test, prints the report line, returns the exit code.
pub fn main(args: SmokeArgs) -> i32 {
    let headless = args.headless || std::env::var_os("BREADBOX_HEADLESS").is_some();
    let report = if headless {
        run_headless(args)
    } else {
        match run_windowed(args.clone()) {
            Some(r) => r,
            None => run_headless(args),
        }
    };
    println!("{}", report.line());
    if report.ok() { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_smoke_arguments() {
        assert_eq!(SmokeArgs::parse(&s(&["breadbox"])), None);
        let a = SmokeArgs::parse(&s(&[
            "breadbox",
            "--smoke",
            "garden-node",
            "--seconds",
            "3",
            "--speed",
            "1",
            "--headless",
        ]))
        .unwrap()
        .unwrap();
        assert!(a.project.ends_with("examples/garden-node"));
        assert_eq!((a.lesson.as_deref(), a.place.as_deref()), (None, None));
        assert_eq!(a.seconds, 3.0);
        assert_eq!(a.speed, SimSpeed::RealTime(1.0));
        assert!(a.headless);
        assert!(
            SmokeArgs::parse(&s(&["breadbox", "--smoke", "no-such-project"]))
                .unwrap()
                .is_err()
        );
        assert!(
            SmokeArgs::parse(&s(&["breadbox", "--smoke", "blink", "--bogus"]))
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn parses_lesson_and_place_options() {
        let a = SmokeArgs::parse(&s(&["breadbox", "--smoke", "lesson:ohms-law-led"]))
            .unwrap()
            .unwrap();
        assert_eq!(a.lesson.as_deref(), Some("ohms-law-led"));
        assert_eq!(a.project, PathBuf::new());
        let a = SmokeArgs::parse(&s(&[
            "breadbox",
            "--smoke",
            "garden-node",
            "--place",
            "community.dmm-handheld",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(a.place.as_deref(), Some("community.dmm-handheld"));
        assert!(
            SmokeArgs::parse(&s(&["breadbox", "--smoke", "blink", "--place"]))
                .unwrap()
                .is_err()
        );
    }

    fn test_options() -> AppOptions {
        let t = AppOptions::test();
        AppOptions {
            plugin_dir: t.plugin_dir,
            progress_path: t.progress_path,
            ..app_options(SimSpeed::Max)
        }
    }

    #[test]
    fn headless_smoke_runs_a_lesson_bench_with_its_meter() {
        let a = SmokeArgs::parse(&s(&[
            "breadbox",
            "--smoke",
            "lesson:ohms-law-led",
            "--seconds",
            "0.5",
            "--warmup",
            "0.2",
            "--headless",
        ]))
        .unwrap()
        .unwrap();
        let r = run_headless_with(a, test_options());
        assert!(r.ok(), "{}", r.line());
        assert!(r.line().starts_with("SMOKE project=lesson:ohms-law-led"));
        // the lesson's meter is the Handheld DMM plugin
        assert!(!r.instruments.is_empty(), "{}", r.line());
    }

    #[test]
    fn headless_smoke_places_the_plugin_dmm_on_garden_node() {
        let a = SmokeArgs::parse(&s(&[
            "breadbox",
            "--smoke",
            "garden-node",
            "--place",
            "community.dmm-handheld",
            "--seconds",
            "0.5",
            "--warmup",
            "0.5",
            "--headless",
        ]))
        .unwrap()
        .unwrap();
        let r = run_headless_with(a, test_options());
        assert!(r.ok(), "{}", r.line());
        // DMM1 took over the built-in meter's probes: A0 = the soil sensor ≈ 1.88 V
        let (_, reading) = r
            .instruments
            .iter()
            .find(|(rf, _)| rf.starts_with("DMM"))
            .unwrap_or_else(|| panic!("{}", r.line()));
        let v: f64 = reading.split(' ').next().unwrap().parse().unwrap();
        assert!((v - 1.88).abs() < 0.1, "{}", r.line());
        assert!(r.line().contains(" instruments=DMM"), "{}", r.line());
    }

    #[test]
    fn headless_smoke_runs_blink() {
        let a = SmokeArgs::parse(&s(&[
            "breadbox",
            "--smoke",
            "blink",
            "--seconds",
            "1",
            "--warmup",
            "0.2",
            "--headless",
        ]))
        .unwrap()
        .unwrap();
        let r = run_headless(a);
        assert!(r.ok(), "{}", r.line());
        assert!(r.sim_s >= 1.0 && r.frames > 0, "{}", r.line());
        assert!(r.line().starts_with("SMOKE project=blink mode=headless"));
    }
}
