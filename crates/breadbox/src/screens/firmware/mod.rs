//! Firmware screen (design screen 2): sketch files + libraries + toolchain (left), the code
//! editor with breakpoints (centre), build output / serial monitor / debugger (bottom), and
//! the Radios panel (right). Verify compiles in the background; Upload flashes the bench
//! through [`crate::bench::BenchLink`]; live debugging drives the link's debugger.

pub mod build;
pub mod debug_panel;
pub mod libraries;
pub mod radio_panel;

use crate::app::AppCtx;
use crate::bench::{BenchEvent, BenchTarget, BreakpointSpec, DebugViewModel, Firmware, LineKind};
use crate::jobs::Job;
use crate::project::{self, Project};
use crate::router::{Command, Screen};
use crate::theme::{Palette, font, pal};
use crate::toolchain::ToolchainState;
use crate::widgets::{
    self, Btn, CodeDoc, CodeEditor, EditorView, Icon, editor_tabs, menu_item, section_header,
};
use bb_firmware::Board;
use bb_firmware::host::HostKind;
use build::{BuildRequest, BuildResult, BuildTarget, LineTone, OutputLine};
use eframe::egui::{self, CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BottomTab {
    #[default]
    Output,
    Serial,
    Debug,
}

struct Sketch {
    target: String,
    dir: PathBuf,
    name: String,
    docs: Vec<CodeDoc>,
    active: usize,
    read_only: bool,
    sketch_yaml: Option<Vec<u8>>,
}

impl Sketch {
    fn versions(&self) -> Vec<u64> {
        self.docs.iter().map(|d| d.version).collect()
    }
    fn bp_key(&self, doc: &CodeDoc) -> String {
        format!("{}/{}", self.dir.display(), doc.name)
    }
}

struct Built {
    result: BuildResult,
    versions: Vec<u64>,
    fqbn: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Persist {
    step_mode: bool,
    bottom: BottomTab,
    breakpoints: HashMap<String, Vec<u32>>,
}

pub struct FirmwareScreen {
    targets: Vec<BenchTarget>,
    target: Option<String>,
    sketch: Option<Sketch>,
    view: EditorView,
    bottom: BottomTab,
    output: Vec<OutputLine>,
    build: Option<(String, Job<BuildResult>, bool)>,
    builds: HashMap<String, Built>,
    board_override: HashMap<String, String>,
    step_mode: bool,
    libs: libraries::LibraryManager,
    install: libraries::InstallDialog,
    radio: radio_panel::RadioPanelState,
    serial_input: String,
    breakpoints: HashMap<String, BTreeSet<u32>>,
    last_edit: Option<Instant>,
    followed_pause: Option<(String, u32, u64)>,
    upload_pending: bool,
}

impl Default for FirmwareScreen {
    fn default() -> Self {
        FirmwareScreen {
            targets: Vec::new(),
            target: None,
            sketch: None,
            view: EditorView::default(),
            bottom: BottomTab::Output,
            output: Vec::new(),
            build: None,
            builds: HashMap::new(),
            board_override: HashMap::new(),
            step_mode: false,
            libs: Default::default(),
            install: Default::default(),
            radio: Default::default(),
            serial_input: String::new(),
            breakpoints: HashMap::new(),
            last_edit: None,
            followed_pause: None,
            upload_pending: false,
        }
    }
}

/// Boards the target selector offers for compiling (`bb_firmware::Board` + host boards).
fn compile_boards() -> Vec<(String, String, bool)> {
    let mut v: Vec<(String, String, bool)> = Board::all()
        .into_iter()
        .map(|b| {
            (
                b.fqbn.to_string(),
                format!("{} ({})", b.name, b.mcu_name),
                true,
            )
        })
        .collect();
    v.push((
        "host:esp32".into(),
        "ESP32 DevKit v1 (API-level)".into(),
        false,
    ));
    v.push((
        "host:pico".into(),
        "Raspberry Pi Pico (API-level)".into(),
        false,
    ));
    v
}

impl FirmwareScreen {
    fn current_target(&self) -> Option<&BenchTarget> {
        let id = self.target.as_ref()?;
        self.targets.iter().find(|t| &t.id == id)
    }

    /// FQBN (or `host:<kind>`) used to compile for the current target.
    fn compile_board(&self) -> Option<String> {
        let t = self.current_target()?;
        if let Some(o) = self.board_override.get(&t.id) {
            return Some(o.clone());
        }
        match &t.fqbn {
            Some(f) => Some(f.clone()),
            None => Some(if t.board.contains("Pico") {
                "host:pico".into()
            } else {
                "host:esp32".into()
            }),
        }
    }

    fn select_target(&mut self, id: &str, project: Option<&Project>) {
        self.target = Some(id.to_string());
        self.sketch = None;
        let Some(t) = self.targets.iter().find(|t| t.id == id).cloned() else {
            return;
        };
        let Some(dir) = t.sketch_dir.clone() else {
            return;
        };
        let read_only = project.map(|p| p.read_only).unwrap_or(false);
        let name = dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sketch".into());
        let mut docs = Vec::new();
        for f in project::sketch_files(&dir) {
            let path = dir.join(&f);
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let mut d = CodeDoc::new(f.clone(), text);
            d.path = Some(path);
            d.read_only = false; // examples are editable in memory; Save As keeps changes
            docs.push(d);
        }
        if docs.is_empty() {
            let mut d = CodeDoc::new(
                format!("{name}.ino"),
                "void setup() {\n}\n\nvoid loop() {\n}\n",
            );
            d.path = Some(dir.join(format!("{name}.ino")));
            docs.push(d);
        }
        let sk = Sketch {
            target: t.id.clone(),
            sketch_yaml: std::fs::read(dir.join("sketch.yaml")).ok(),
            dir,
            name,
            docs,
            active: 0,
            read_only,
        };
        let mut sk = sk;
        for i in 0..sk.docs.len() {
            let key = sk.bp_key(&sk.docs[i]);
            if let Some(b) = self.breakpoints.get(&key) {
                sk.docs[i].breakpoints = b.clone();
            }
        }
        self.sketch = Some(sk);
        self.view = EditorView::default();
    }

    fn breakpoint_specs(&self) -> Vec<BreakpointSpec> {
        let Some(sk) = &self.sketch else {
            return Vec::new();
        };
        if !self.step_mode {
            return Vec::new();
        }
        sk.docs
            .iter()
            .flat_map(|d| {
                d.breakpoints.iter().map(|&l| BreakpointSpec {
                    file: d.name.clone(),
                    line: l,
                    enabled: true,
                    hit_count: None,
                })
            })
            .collect()
    }

    fn push_breakpoints(&self, app: &mut AppCtx) {
        let Some(t) = self.target.clone() else { return };
        if !app.bench.is_active() {
            return;
        }
        let specs = self.breakpoint_specs();
        app.bench_link().set_breakpoints(&t, &specs);
    }

    fn start_verify(&mut self, app: &mut AppCtx, upload_after: bool) {
        if self.build.is_some() {
            return;
        }
        let Some(sk) = &self.sketch else {
            self.output.push(OutputLine::new(
                LineTone::Warning,
                "No sketch for this target.",
            ));
            return;
        };
        let Some(board) = self.compile_board() else {
            return;
        };
        let target = if let Some(kind) = board.strip_prefix("host:") {
            BuildTarget::Host(HostKind::by_id(kind).unwrap_or(HostKind::Esp32DevKit))
        } else {
            match app.toolchain.ready() {
                Some(tc) => BuildTarget::Avr {
                    toolchain: tc,
                    fqbn: board.clone(),
                },
                None => {
                    match &app.toolchain.state {
                        ToolchainState::Detecting => self.output.push(OutputLine::new(
                            LineTone::Muted,
                            "Still looking for the toolchain — try again in a moment.",
                        )),
                        ToolchainState::Disabled => self.output.push(OutputLine::new(
                            LineTone::Warning,
                            "Toolchain detection is off.",
                        )),
                        _ => {
                            self.output.push(OutputLine::new(
                                LineTone::Warning,
                                "No firmware toolchain yet — install it to compile sketches.",
                            ));
                            self.install.open = true;
                        }
                    }
                    if upload_after {
                        self.upload_prebuilt(app);
                    }
                    return;
                }
            }
        };
        let req = BuildRequest {
            sketch_name: sk.name.clone(),
            files: sk
                .docs
                .iter()
                .map(|d| (d.name.clone(), d.text.clone()))
                .collect(),
            extra: sk
                .sketch_yaml
                .iter()
                .map(|y| ("sketch.yaml".to_string(), y.clone()))
                .collect(),
            target,
            debug: self.step_mode,
        };
        let key = format!("{:016x}", fnv(&sk.dir.to_string_lossy()));
        self.output.clear();
        self.bottom = BottomTab::Output;
        self.output.push(OutputLine::new(
            LineTone::Muted,
            format!("Verifying {}…", sk.name),
        ));
        let tid = sk.target.clone();
        self.build = Some((
            tid,
            Job::spawn("verify", app.repaint.clone(), move |_| {
                build::run_build(&key, &req)
            }),
            upload_after,
        ));
    }

    fn finish_build(
        &mut self,
        app: &mut AppCtx,
        target: String,
        r: BuildResult,
        upload_after: bool,
    ) {
        self.output.retain(|l| !l.text.starts_with("Verifying "));
        self.output.extend(r.lines.iter().cloned());
        if let Some(sk) = self.sketch.as_mut() {
            build::apply_diagnostics(&mut sk.docs, &r.diagnostics);
            // jump to the first error
            if let Some((tab, d)) = r
                .diagnostics
                .iter()
                .find(|(_, d)| d.severity == widgets::DiagSeverity::Error)
                && let Some(i) = sk.docs.iter().position(|x| &x.name == tab)
            {
                sk.active = i;
                self.view.scroll_to_line = Some(d.line);
            }
        }
        let success = r.success;
        if success {
            let versions = self
                .sketch
                .as_ref()
                .map(|s| s.versions())
                .unwrap_or_default();
            let fqbn = self.compile_board().unwrap_or_default();
            self.builds.insert(
                target,
                Built {
                    result: r,
                    versions,
                    fqbn,
                },
            );
            if upload_after {
                self.upload(app);
            }
        } else if upload_after {
            self.output.push(OutputLine::new(
                LineTone::Error,
                "Upload skipped: the sketch did not compile.",
            ));
        }
    }

    /// Upload: the current build if it matches the editor, else Verify first, else the
    /// project's prebuilt image when no toolchain is installed.
    fn upload(&mut self, app: &mut AppCtx) {
        let Some(t) = self.current_target().cloned() else {
            return;
        };
        if t.fqbn.is_none() {
            self.output.push(OutputLine::new(
                LineTone::Note,
                format!("{} runs API-level firmware on the Workbench bench (bb-sim). Verify compiles it here.", t.label),
            ));
            return;
        }
        let versions = self
            .sketch
            .as_ref()
            .map(|s| s.versions())
            .unwrap_or_default();
        let fresh = self.builds.get(&t.id).is_some_and(|b| {
            b.versions == versions
                && b.result.debug == self.step_mode
                && Some(&b.fqbn) == self.compile_board().as_ref()
        });
        if !fresh {
            if app.toolchain.ready().is_some() {
                self.start_verify(app, true);
            } else {
                self.upload_prebuilt(app);
            }
            return;
        }
        let Some(b) = self.builds.get(&t.id) else {
            return;
        };
        let Some(out) = b.result.avr.clone() else {
            return;
        };
        let fw = Firmware {
            elf: out.elf.clone().map(Arc::new),
            hex: out.hex.clone(),
            origin: format!(
                "build of {}",
                self.sketch
                    .as_ref()
                    .map(|s| s.name.as_str())
                    .unwrap_or("sketch")
            ),
            debug_build: b.result.debug,
        };
        self.flash(app, &t.id, fw);
    }

    fn upload_prebuilt(&mut self, app: &mut AppCtx) {
        let Some(t) = self.current_target().cloned() else {
            return;
        };
        let Some(proj) = app.project.clone() else {
            return;
        };
        let Some(mcu) = proj.mcus.iter().find(|m| m.reference == t.id) else {
            return;
        };
        let fixtures = app.resources.fixtures();
        match proj.prebuilt_firmware(mcu, fixtures.as_deref()) {
            Some(path) => {
                let is_elf = path.extension().is_some_and(|e| e == "elf");
                let fw = Firmware {
                    elf: if is_elf { std::fs::read(&path).ok().map(Arc::new) } else { None },
                    hex: if is_elf { None } else { std::fs::read_to_string(&path).ok() },
                    origin: format!("prebuilt {}", path.display()),
                    debug_build: false,
                };
                let shown = path
                    .strip_prefix(app.resources.root.clone().unwrap_or_default())
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| path.display().to_string());
                self.output.push(OutputLine::new(LineTone::Muted, format!("Using prebuilt firmware {shown} (toolchain not installed)")));
                self.flash(app, &t.id, fw);
            }
            None => self.output.push(OutputLine::new(
                LineTone::Error,
                "No toolchain and no prebuilt firmware for this sketch — install the toolchain to compile it.",
            )),
        }
    }

    fn flash(&mut self, app: &mut AppCtx, target: &str, fw: Firmware) {
        let specs = self.breakpoint_specs();
        let link = app.bench_link();
        link.set_breakpoints(target, &specs);
        match link.upload(target, &fw) {
            Ok(_) => {} // the Flashed event carries the message
            Err(e) => self.output.push(OutputLine::new(
                LineTone::Error,
                format!("Upload failed: {e}"),
            )),
        }
    }

    fn handle_events(&mut self, app: &mut AppCtx) {
        for ev in app.bench.take_events() {
            match ev {
                BenchEvent::Flashed(r) => self
                    .output
                    .push(OutputLine::new(LineTone::Success, r.message)),
                BenchEvent::BreakpointArmed {
                    target, line, hits, ..
                } => self.output.push(OutputLine::new(
                    LineTone::Note,
                    format!("note: line {line} — breakpoint armed on {target} (hits: {hits})"),
                )),
                BenchEvent::Paused { .. } => {}
                BenchEvent::Note(n) => self.output.push(OutputLine::new(LineTone::Note, n)),
                BenchEvent::Error(e) => self.output.push(OutputLine::new(LineTone::Error, e)),
            }
        }
    }

    fn debug_vm<'a>(&self, app: &'a AppCtx) -> Option<&'a DebugViewModel> {
        app.bench.debug.get(self.target.as_ref()?)
    }

    fn follow_pause(&mut self, app: &AppCtx) {
        let Some(vm) = self.debug_vm(app).cloned() else {
            return;
        };
        if !vm.is_paused() {
            self.followed_pause = None;
            return;
        }
        let Some(loc) = vm.location.clone() else {
            return;
        };
        let key = (loc.file.clone(), loc.line, vm.registers.cycle);
        if self.followed_pause.as_ref() == Some(&key) {
            return;
        }
        self.followed_pause = Some(key);
        if let Some(sk) = self.sketch.as_mut()
            && let Some(i) = sk.docs.iter().position(|d| d.name == loc.file)
        {
            sk.active = i;
            self.view.scroll_to_line = Some(loc.line);
        }
        if self.bottom == BottomTab::Output {
            self.bottom = BottomTab::Debug;
        }
    }

    fn autosave(&mut self, app: &mut AppCtx) {
        let Some(sk) = self.sketch.as_mut() else {
            return;
        };
        if sk.read_only || !sk.docs.iter().any(|d| d.dirty) {
            return;
        }
        let Some(t) = self.last_edit else { return };
        let wait = Duration::from_millis(1200);
        if t.elapsed() < wait {
            app.egui
                .request_repaint_after(wait - t.elapsed() + Duration::from_millis(20));
            return;
        }
        for d in sk.docs.iter_mut().filter(|d| d.dirty) {
            if let Some(p) = &d.path {
                match std::fs::write(p, &d.text) {
                    Ok(()) => d.dirty = false,
                    Err(e) => app
                        .toasts
                        .error(format!("Autosave failed for {}: {e}", d.name)),
                }
            }
        }
    }

    fn save_as_copy(&mut self, app: &mut AppCtx) {
        let Some(proj) = app.project.clone() else {
            return;
        };
        let Some(dest) = crate::platform::save_folder(
            "Save a copy of this project",
            &format!("{}.breadbox", proj.name),
        ) else {
            return;
        };
        match proj.save_as(&dest) {
            Ok(np) => {
                // carry the editor's buffers over
                if let Some(sk) = &self.sketch
                    && let Ok(rel) = sk.dir.strip_prefix(&proj.dir)
                {
                    let ndir = np.dir.join(rel);
                    for d in &sk.docs {
                        let _ = std::fs::write(ndir.join(&d.name), &d.text);
                    }
                }
                app.toasts.success(format!("Saved to {}", np.dir.display()));
                app.set_project(Some(np));
            }
            Err(e) => app.toasts.error(format!("{e:#}")),
        }
    }
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

impl Screen for FirmwareScreen {
    fn crumb(&self, _app: &AppCtx) -> Option<String> {
        Some(match &self.target {
            Some(t) => format!("Firmware · {t}"),
            None => "Firmware".into(),
        })
    }

    fn project_changed(&mut self, app: &mut AppCtx) {
        self.targets = app
            .project
            .as_ref()
            .map(|p| p.bench_targets())
            .unwrap_or_default();
        self.builds.clear();
        self.output.clear();
        self.sketch = None;
        self.target = None;
        self.radio = Default::default();
        self.followed_pause = None;
        if let Some(first) = self.targets.first().map(|t| t.id.clone()) {
            let proj = app.project.clone();
            self.select_target(&first, proj.as_ref());
        }
    }

    fn header(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let has_target = self.current_target().is_some();
            if Btn::ghost("Step · breakpoints")
                .selected(self.step_mode)
                .enabled(has_target)
                .tooltip("Debug builds (-Og) + source breakpoints: click the gutter to set one")
                .show(ui)
                .clicked()
            {
                self.step_mode = !self.step_mode;
                let n: usize = self.sketch.as_ref().map(|s| s.docs.iter().map(|d| d.breakpoints.len()).sum()).unwrap_or(0);
                self.output.push(OutputLine::new(
                    LineTone::Note,
                    if self.step_mode {
                        format!("Step mode on — {n} breakpoint{} armed; Upload rebuilds with -Og for source-level debugging", if n == 1 { "" } else { "s" })
                    } else {
                        "Step mode off — breakpoints disarmed".into()
                    },
                ));
                self.push_breakpoints(app);
                if !self.step_mode
                    && let (Some(t), true) = (self.target.clone(), app.bench.is_active())
                {
                    app.bench_link().debug(&t, crate::bench::DebugCommand::Continue);
                }
            }
            let busy = self.build.is_some();
            if Btn::amber("Upload to sim").enabled(has_target && !busy).show(ui).clicked() {
                self.upload(app);
            }
            if Btn::ghost("Verify").enabled(has_target && !busy).show(ui).clicked() {
                self.start_verify(app, false);
            }
            // target selector
            let label = self.current_target().map(|t| t.label.clone()).unwrap_or_else(|| "no MCU in this project".into());
            let targets = self.targets.clone();
            let radios = app.project.as_ref().map(|p| p.radios.clone()).unwrap_or_default();
            let current = self.target.clone();
            let board = self.compile_board();
            let (_, picked) = widgets::select(ui, "Target", &label, 210.0, |ui| {
                let mut pick: Option<(Option<String>, Option<String>)> = None;
                for t in &targets {
                    if menu_item(ui, &t.label, current.as_ref() == Some(&t.id), true) {
                        pick = Some((Some(t.id.clone()), None));
                    }
                }
                for (r, part) in &radios {
                    let what = match part.as_str() {
                        "esp01" => "ESP8266 (AT firmware)",
                        "hm10" => "HM-10 (AT firmware)",
                        "hc05" => "HC-05 (AT firmware)",
                        _ => "radio module",
                    };
                    menu_item(ui, &format!("{r} · {what}"), false, false);
                }
                if !targets.is_empty() {
                    ui.separator();
                    ui.label(egui::RichText::new("Compile for board").font(font::ui(11.0)).color(p.text_muted));
                    for (fqbn, name, _) in compile_boards() {
                        if menu_item(ui, &name, board.as_deref() == Some(fqbn.as_str()), true) {
                            pick = Some((None, Some(fqbn)));
                        }
                    }
                }
                pick
            });
            if let Some(Some((tid, board))) = picked {
                if let Some(tid) = tid {
                    let proj = app.project.clone();
                    self.select_target(&tid, proj.as_ref());
                }
                if let (Some(b), Some(t)) = (board, self.target.clone()) {
                    self.board_override.insert(t, b);
                }
            }
            ui.label(egui::RichText::new("Target").font(font::ui(12.0)).color(p.text_muted));
        });
    }

    fn command(&mut self, cmd: &Command, app: &mut AppCtx) -> bool {
        match cmd {
            Command::Verify => {
                self.start_verify(app, false);
                true
            }
            Command::Upload => {
                if self.build.is_some() {
                    self.upload_pending = true;
                } else {
                    self.upload(app);
                }
                true
            }
            Command::Find => {
                self.view.open_find();
                true
            }
            Command::Save | Command::SaveAs => {
                let ro = self.sketch.as_ref().is_some_and(|s| s.read_only);
                if ro || matches!(cmd, Command::SaveAs) {
                    self.save_as_copy(app);
                    return true;
                }
                if let Some(sk) = self.sketch.as_mut() {
                    for d in sk.docs.iter_mut() {
                        if let Some(p) = &d.path
                            && std::fs::write(p, &d.text).is_ok()
                        {
                            d.dirty = false;
                        }
                    }
                }
                app.toasts.success("Saved");
                true
            }
            _ => false,
        }
    }

    fn save_state(&self) -> Option<serde_json::Value> {
        let mut bps = self.breakpoints.clone();
        if let Some(sk) = &self.sketch {
            for d in &sk.docs {
                bps.insert(sk.bp_key(d), d.breakpoints.clone());
            }
        }
        serde_json::to_value(Persist {
            step_mode: self.step_mode,
            bottom: self.bottom,
            breakpoints: bps
                .into_iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| (k, v.into_iter().collect()))
                .collect(),
        })
        .ok()
    }

    fn load_state(&mut self, state: &serde_json::Value) {
        if let Ok(p) = serde_json::from_value::<Persist>(state.clone()) {
            self.step_mode = p.step_mode;
            self.bottom = p.bottom;
            self.breakpoints = p
                .breakpoints
                .into_iter()
                .map(|(k, v)| (k, v.into_iter().collect()))
                .collect();
        }
    }

    fn ui(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        // background work
        if let Some((target, job, upload_after)) = self.build.as_mut()
            && let Some(r) = job.poll()
        {
            let (t, ua) = (target.clone(), *upload_after || self.upload_pending);
            self.build = None;
            self.upload_pending = false;
            self.finish_build(app, t, r, ua);
        }
        self.handle_events(app);
        self.follow_pause(app);
        self.autosave(app);

        if self.targets.is_empty() {
            empty_state(ui, p, app);
            return;
        }

        egui::Panel::left("fw-files")
            .exact_size(220.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(p.bg_panel)
                    .inner_margin(egui::Margin::symmetric(10, 14)),
            )
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().line_segment(
                    [
                        pos2(r.right() + 10.0, r.top() - 14.0),
                        pos2(r.right() + 10.0, r.bottom() + 14.0),
                    ],
                    Stroke::new(1.0, p.border),
                );
                self.files_panel(ui, app);
            });
        egui::Panel::right("fw-radios")
            .exact_size(340.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.bg_panel).inner_margin(egui::Margin::same(16)))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().line_segment([pos2(r.left() - 16.0, r.top() - 16.0), pos2(r.left() - 16.0, r.bottom() + 16.0)], Stroke::new(1.0, p.border));
                egui::ScrollArea::vertical().id_salt("radios-scroll").auto_shrink([false, false]).show(ui, |ui| {
                    let model = app.bench.radio.clone();
                    let cmds = radio_panel::radio_panel(ui, model.as_ref(), &mut self.radio);
                    for c in cmds {
                        if app.bench.is_active() {
                            if let Some(l) = app.bench.existing() {
                                l.radio(c);
                            }
                        } else if c == crate::bench::RadioCommand::AddBleHat {
                            app.toasts.info("Add an HM-10 BLE module on the Workbench; the virtual phone appears here.");
                        }
                    }
                });
            });
        egui::Panel::bottom("fw-output")
            .exact_size(210.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(p.bg_app)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().line_segment(
                    [
                        pos2(r.left() - 16.0, r.top() - 10.0),
                        pos2(r.right() + 16.0, r.top() - 10.0),
                    ],
                    Stroke::new(1.0, p.border),
                );
                self.bottom_panel(ui, app);
            });
        egui::CentralPanel::no_frame().show(ui, |ui| self.editor(ui, app));
        self.libs.show(app);
        self.install.show(app);
    }
}

impl FirmwareScreen {
    fn files_panel(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            section_header(ui, "Sketch");
        });
        ui.add_space(4.0);
        let mut clicked = None;
        if let Some(sk) = &self.sketch {
            for (i, d) in sk.docs.iter().enumerate() {
                let label = if d.dirty {
                    format!("{} •", d.name)
                } else {
                    d.name.clone()
                };
                if file_row(ui, p, &label, i == sk.active).clicked() {
                    clicked = Some(i);
                }
            }
        }
        if let (Some(i), Some(sk)) = (clicked, self.sketch.as_mut()) {
            sk.active = i;
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            section_header(ui, "Libraries");
        });
        ui.add_space(4.0);
        let libs = self
            .sketch
            .as_ref()
            .map(|s| build::included_libraries(&s.docs))
            .unwrap_or_default();
        for l in &libs {
            let text = match app.toolchain.user_library_version(l) {
                Some(v) => format!("{l} {}", v.split('.').next().unwrap_or(v)),
                None => l.clone(),
            };
            file_row(ui, p, &text, false);
        }
        if libs.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("core only")
                        .font(font::ui(12.0))
                        .color(p.text_faint),
                );
            });
        }
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            if Btn::plain("Manage libraries…")
                .icon(Icon::Library)
                .font_size(12.0)
                .show(ui)
                .clicked()
            {
                self.libs.open = true;
            }
        });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            section_header(ui, "Toolchain");
        });
        ui.add_space(4.0);
        let emulation = self
            .current_target()
            .map(|t| t.emulation.clone())
            .unwrap_or_default();
        let board = self.compile_board().unwrap_or_default();
        let compiler = if board.starts_with("host:") {
            "host clang++ · hostcore".to_string()
        } else {
            app.toolchain.compiler_label()
        };
        let mut lines = vec![compiler, emulation];
        if let Some(cli) = app.toolchain.cli_label() {
            lines.push(cli);
        }
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(lines.join("\n"))
                    .font(font::mono(12.0))
                    .line_height(Some(20.0))
                    .color(p.text_muted),
            );
        });
        let installable = matches!(
            app.toolchain.state,
            ToolchainState::Missing { .. } | ToolchainState::Failed(_) | ToolchainState::Installing
        );
        if installable {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                if Btn::amber("Install toolchain")
                    .icon(Icon::Download)
                    .small()
                    .show(ui)
                    .clicked()
                {
                    self.install.open = true;
                    if !app.toolchain.is_installing() {
                        app.toolchain.start_install();
                    }
                }
            });
            if let Some(pr) = app.toolchain.install_progress() {
                widgets::progress(ui, pr.fraction, &pr.step);
            }
        }
        if let Some(sk) = &self.sketch
            && sk.read_only
        {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "Example project — edits stay in memory. ⌘S saves a copy.",
                        )
                        .font(font::ui(11.0))
                        .color(p.text_faint),
                    )
                    .wrap(),
                );
            });
        }
    }

    fn editor(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        let vm = self.debug_vm(app).cloned();
        let Some(sk) = self.sketch.as_mut() else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new("This MCU has no sketch folder in the project.")
                        .color(p.text_muted),
                );
            });
            return;
        };
        let lang = sk
            .docs
            .get(sk.active)
            .map(|d| d.lang.label())
            .unwrap_or("C++");
        let status = format!(
            "Ln {}, Col {} · {}",
            self.view.cursor.0.max(1),
            self.view.cursor.1.max(1),
            lang
        );
        let names: Vec<(String, bool)> =
            sk.docs.iter().map(|d| (d.name.clone(), d.dirty)).collect();
        if let Some(i) = editor_tabs(ui, &names, sk.active, &status) {
            sk.active = i;
        }
        // debug strip
        let debugging = self.step_mode || vm.as_ref().is_some_and(|v| v.is_paused());
        if debugging {
            let rect = ui.available_rect_before_wrap();
            let strip = Rect::from_min_size(rect.min, vec2(rect.width(), 34.0));
            ui.painter().rect_filled(strip, 0.0, p.bg_panel);
            ui.painter().line_segment(
                [strip.left_bottom(), strip.right_bottom()],
                Stroke::new(1.0, p.border),
            );
            let mut c = widgets::child_ui(ui, strip.shrink2(vec2(12.0, 4.0)));
            if let Some(cmd) = debug_panel::debug_toolbar(&mut c, vm.as_ref())
                && let Some(t) = self.target.clone()
            {
                app.bench_link().debug(&t, cmd);
            }
            ui.allocate_rect(strip, Sense::hover());
        }
        let exec_line = vm
            .as_ref()
            .filter(|v| v.is_paused())
            .and_then(|v| v.location.as_ref())
            .filter(|l| sk.docs.get(sk.active).is_some_and(|d| d.name == l.file))
            .map(|l| l.line);
        let active = sk.active.min(sk.docs.len().saturating_sub(1));
        let doc = &mut sk.docs[active];
        let id = format!("fw-{}", sk.target);
        let out = CodeEditor::new(&id)
            .breakpoints(true)
            .exec_line(exec_line)
            .show(ui, doc, &mut self.view);
        if out.changed {
            self.last_edit = Some(Instant::now());
            doc.diagnostics.clear();
        }
        if let Some(line) = out.toggled_breakpoint {
            let key = format!("{}/{}", sk.dir.display(), doc.name);
            self.breakpoints.insert(key, doc.breakpoints.clone());
            let on = doc.breakpoints.contains(&line);
            if on && self.step_mode {
                let hits = vm
                    .as_ref()
                    .and_then(|v| {
                        v.breakpoints
                            .iter()
                            .find(|b| b.line == line)
                            .map(|b| b.hits)
                    })
                    .unwrap_or(0);
                self.output.push(OutputLine::new(
                    LineTone::Note,
                    format!(
                        "note: line {line} — breakpoint armed on {} (hits: {hits})",
                        sk.target
                    ),
                ));
            }
            if !self.step_mode && on {
                self.output.push(OutputLine::new(
                    LineTone::Muted,
                    format!(
                        "Breakpoint on line {line} saved — turn on Step · breakpoints to arm it"
                    ),
                ));
            }
            self.push_breakpoints(app);
        }
    }

    fn bottom_panel(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for (tab, label) in [
                (BottomTab::Output, "Output"),
                (BottomTab::Serial, "Serial monitor"),
                (BottomTab::Debug, "Debug"),
            ] {
                let on = self.bottom == tab;
                let up = label.to_uppercase();
                let g = widgets::galley_spaced(
                    ui,
                    &up,
                    font::semibold(11.0),
                    if on { p.text } else { p.text_faint },
                    0.88,
                );
                let (r, resp) = ui.allocate_exact_size(vec2(g.size().x, 18.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, on, label));
                ui.painter()
                    .galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, p.text);
                if on {
                    ui.painter().rect_filled(
                        Rect::from_min_max(
                            pos2(r.left(), r.bottom() + 1.0),
                            pos2(r.right(), r.bottom() + 3.0),
                        ),
                        0.0,
                        p.accent,
                    );
                }
                if resp.clicked() {
                    self.bottom = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.build.is_some() {
                    widgets::spinner(ui, 14.0);
                    ui.label(
                        egui::RichText::new("compiling…")
                            .font(font::mono(12.0))
                            .color(p.text_muted),
                    );
                }
                match self.bottom {
                    BottomTab::Output => {
                        if Btn::plain("Clear output").small().show(ui).clicked() {
                            self.output.clear();
                        }
                    }
                    BottomTab::Serial => {
                        if Btn::plain("Clear serial").small().show(ui).clicked() {
                            app.bench.serial.clear();
                        }
                        let t = app.bench.sim_time;
                        let state = if app.bench.running {
                            "running"
                        } else {
                            "stopped"
                        };
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · 115200 baud · NL · t = {} · {state}",
                                self.target.as_deref().unwrap_or("-"),
                                crate::bench::clock_label(t)
                            ))
                            .font(font::mono(12.0))
                            .color(p.text_muted),
                        );
                    }
                    BottomTab::Debug => {}
                }
            });
        });
        ui.add_space(8.0);
        match self.bottom {
            BottomTab::Output => {
                egui::ScrollArea::vertical().id_salt("fw-output-scroll").stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if self.output.is_empty() {
                        ui.label(egui::RichText::new("Verify compiles the sketch; Upload to sim flashes it into the bench.").font(font::mono(12.5)).color(p.text_faint));
                    }
                    for l in &self.output {
                        let c = match l.kind {
                            LineTone::Muted => p.text_muted,
                            LineTone::Normal => p.text_dim,
                            LineTone::Note | LineTone::Warning => p.accent,
                            LineTone::Error => p.danger_soft,
                            LineTone::Success => p.ok,
                        };
                        ui.add(egui::Label::new(egui::RichText::new(&l.text).font(font::mono(12.5)).line_height(Some(21.0)).color(c)).wrap());
                    }
                });
            }
            BottomTab::Serial => {
                let h = ui.available_height() - 36.0;
                egui::ScrollArea::vertical()
                    .id_salt("fw-serial-scroll")
                    .stick_to_bottom(true)
                    .max_height(h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let target = self.target.clone().unwrap_or_default();
                        let lines: Vec<_> = app
                            .bench
                            .serial
                            .iter()
                            .filter(|l| l.target == target)
                            .collect();
                        if lines.is_empty() {
                            ui.label(
                                egui::RichText::new(
                                    "Serial output appears here after Upload to sim.",
                                )
                                .font(font::mono(12.5))
                                .color(p.text_faint),
                            );
                        }
                        let start = lines.len().saturating_sub(800);
                        for l in &lines[start..] {
                            let c = match l.kind {
                                LineKind::Normal => p.text_dim,
                                LineKind::System => p.text_muted,
                                LineKind::Net => p.info,
                                LineKind::Warn => p.accent,
                                LineKind::Error => p.danger_soft,
                            };
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 16.0;
                                ui.label(
                                    egui::RichText::new(l.time_label())
                                        .font(font::mono(12.5))
                                        .color(p.text_faint),
                                );
                                ui.label(
                                    egui::RichText::new(&l.text).font(font::mono(12.5)).color(c),
                                );
                            });
                        }
                    });
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.serial_input)
                            .hint_text("Send to the board (Enter)")
                            .font(font::mono(12.5))
                            .desired_width(ui.available_width() - 80.0),
                    );
                    let send = (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        | Btn::ghost("Send").small().show(ui).clicked();
                    if send && !self.serial_input.is_empty() {
                        let mut data = std::mem::take(&mut self.serial_input).into_bytes();
                        data.push(b'\n');
                        if let (Some(t), true) = (self.target.clone(), app.bench.is_active()) {
                            app.bench_link().send_serial(&t, &data);
                        }
                        r.request_focus();
                    }
                });
            }
            BottomTab::Debug => {
                let vm = self.debug_vm(app).cloned();
                debug_panel::debug_view(ui, vm.as_ref());
            }
        }
    }
}

fn file_row(ui: &mut Ui, p: &Palette, text: &str, selected: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 27.0), Sense::click());
    let t = text.to_string();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &t));
    if selected {
        ui.painter()
            .rect_filled(r, CornerRadius::same(5), p.bg_row_selected);
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(r, CornerRadius::same(5), p.bg_hover);
    }
    ui.painter().text(
        pos2(r.left() + 8.0, r.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        font::ui(13.0),
        p.text_dim,
    );
    resp
}

fn empty_state(ui: &mut Ui, p: &Palette, app: &mut AppCtx) {
    let area = ui.max_rect();
    let card = Rect::from_center_size(area.center(), vec2(460.0, 190.0));
    ui.painter().rect(
        card,
        CornerRadius::same(12),
        p.bg_panel,
        Stroke::new(1.0, p.border),
        egui::StrokeKind::Inside,
    );
    let mut c = widgets::child_ui(ui, card.shrink(24.0));
    c.spacing_mut().item_spacing.y = 10.0;
    c.label(
        egui::RichText::new("No microcontroller on this bench")
            .font(font::heading(18.0))
            .color(p.text_strong),
    );
    c.label(egui::RichText::new("Place an UNO, Nano, Mega, ESP32 or Pico on the Workbench and its sketch opens here. Or open an example with firmware.").font(font::ui(13.0)).color(p.text_muted));
    c.horizontal(|ui| {
        if Btn::ghost("Open blink example").show(ui).clicked()
            && let Some(ex) = app.examples.iter().find(|e| e.name == "blink")
        {
            app.command(Command::OpenPath(ex.dir.clone()));
            app.navigate(crate::router::Route::Firmware);
        }
    });
}
