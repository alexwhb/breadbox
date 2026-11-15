//! Pro → Analysis tab (design screen 3): netlist editor (left), plot panels (centre), the
//! Analyses sidebar with config popovers + mixed-signal notes (right). "Run analyses" runs
//! every enabled analysis on a background thread with progress and cancel.

pub mod cards;
pub mod demo;
pub mod model;
pub mod pcb;
pub mod plugin_analyses;
pub mod schematic;
pub mod sync;

use crate::app::AppCtx;
use crate::jobs::Job;
use crate::router::{Command, Screen};
use crate::theme::{font, pal};
use crate::widgets::plot::{
    self, AxisSpec, Cursors, LinePlot, Marker, PngResult, Trace, fmt_readout, histogram_plot,
};
use crate::widgets::{
    self, Btn, Cell, CodeDoc, CodeEditor, Col, DiagSeverity, EditorDiagnostic, EditorView, Icon,
    Row, TableStyle, data_table, inner_card_frame, legend_swatch, section_header,
};
use bb_spice::CancelToken;
use cards::AnalysisKind;
use eframe::egui::{self, Rect, Sense, Stroke, Ui, pos2, vec2};
use model::{Profile, RunOutcome, ViewData};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where the netlist came from.
#[derive(Debug, Clone, PartialEq)]
pub enum NetlistSource {
    /// The built-in garden-node demo.
    Demo,
    /// A `.cir` file (project `analyses/` or opened).
    File(PathBuf),
    /// Generated from the shared circuit (`PRO-2` "synced from schematic").
    Synced,
    /// A synced netlist edited in ways the circuit cannot represent (Re-sync restores it).
    Detached,
}

/// Export menu choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportKind {
    Csv,
    Png,
    Svg,
}

pub struct AnalysisScreen {
    doc: CodeDoc,
    source: NetlistSource,
    source_label: String,
    profile: Profile,
    view: EditorView,
    job: Option<Job<RunOutcome>>,
    cancel: Option<CancelToken>,
    result: Option<Arc<ViewData>>,
    error: Option<String>,
    cursors: Cursors,
    editing: Option<(AnalysisKind, Vec<String>, bool)>,
    editing_anchor: Rect,
    last_edit: Option<Instant>,
    parsed_version: u64,
    png: Option<PngResult>,
    plot_rects: Vec<(String, Rect)>,
    show_all_op: bool,
    auto_ran: bool,
    pub runs_started: u32,
    /// Document generation the synced text was made from.
    synced_gen: Option<u64>,
    /// The synced text as generated (edits are compared against it).
    synced_text: String,
}

impl Default for AnalysisScreen {
    fn default() -> Self {
        let mut doc = CodeDoc::new(demo::FILE_NAME, demo::NETLIST);
        doc.lang = widgets::Lang::Spice;
        AnalysisScreen {
            doc,
            source: NetlistSource::Demo,
            source_label: "built-in demo · editable".into(),
            profile: demo::profile(),
            view: EditorView::default(),
            job: None,
            cancel: None,
            result: None,
            error: None,
            cursors: Cursors::default(),
            editing: None,
            editing_anchor: Rect::NOTHING,
            last_edit: None,
            parsed_version: 0,
            png: None,
            plot_rects: Vec::new(),
            show_all_op: false,
            auto_ran: false,
            runs_started: 0,
            synced_gen: None,
            synced_text: String::new(),
        }
    }
}

impl AnalysisScreen {
    /// Loads a netlist file.
    pub fn load_file(&mut self, path: PathBuf) -> std::io::Result<()> {
        let text = std::fs::read_to_string(&path)?;
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "netlist.cir".into());
        let mut doc = CodeDoc::new(name, text);
        doc.lang = widgets::Lang::Spice;
        doc.path = Some(path.clone());
        self.doc = doc;
        self.source = NetlistSource::File(path);
        self.source_label = "editable · autosaved".into();
        self.profile = Profile::default();
        self.result = None;
        self.error = None;
        Ok(())
    }

    /// Loads (or refreshes) the netlist synced from the shared circuit.
    pub fn load_synced(&mut self, app: &mut AppCtx) -> bool {
        let tel = app.bench.sim().and_then(|s| s.telemetry());
        let Some(doc) = app.circuit.as_ref() else {
            return false;
        };
        if doc.project().parts.is_empty() {
            return false;
        }
        let text = match sync::synced_netlist(doc, tel.as_deref()) {
            Ok(t) => t,
            Err(e) => {
                self.error = Some(format!("Netlister: {e}"));
                return false;
            }
        };
        let name = format!("{}.cir", doc.project().name);
        let gen_ = doc.generation();
        let profile = sync_profile(doc);
        if let Some(detached) = doc.detached_netlist.clone() {
            let mut cd = CodeDoc::new(name, detached);
            cd.lang = widgets::Lang::Spice;
            self.doc = cd;
            self.source = NetlistSource::Detached;
            self.source_label = "detached from the schematic".into();
        } else {
            if self.source == NetlistSource::Synced && self.doc.text == text {
                self.synced_gen = Some(gen_);
                return true;
            }
            let mut cd = CodeDoc::new(name, text.clone());
            cd.lang = widgets::Lang::Spice;
            self.doc = cd;
            self.source = NetlistSource::Synced;
            self.source_label = "synced from schematic".into();
        }
        self.synced_text = text;
        self.synced_gen = Some(gen_);
        self.profile = profile;
        self.parsed_version = 0;
        true
    }

    /// Keeps a synced netlist current and maps edits back into the circuit.
    fn sync_tick(&mut self, app: &mut AppCtx) {
        match self.source {
            NetlistSource::Synced => {
                // user edits: map value changes back once typing pauses
                if self.doc.text != self.synced_text {
                    let idle = self
                        .last_edit
                        .is_none_or(|t| t.elapsed() > Duration::from_millis(700));
                    if !idle {
                        app.egui.request_repaint_after(Duration::from_millis(720));
                        return;
                    }
                    let Some(doc) = app.circuit.as_mut() else {
                        return;
                    };
                    match sync::map_edit(doc.project(), &self.synced_text, &self.doc.text) {
                        sync::EditMapping::Props(props) => {
                            if let Err(e) = sync::apply(doc, &props) {
                                app.toasts.error(format!("{e}"));
                            }
                            // the edited text now describes the circuit: later edits compare
                            // against it (not the text before this one)
                            self.synced_text = self.doc.text.clone();
                            self.synced_gen = Some(doc.generation());
                        }
                        sync::EditMapping::Cards { props, cards } => {
                            if let Err(e) = sync::apply(doc, &props) {
                                app.toasts.error(format!("{e}"));
                            }
                            sync::apply_cards(doc, &cards);
                            self.synced_text = self.doc.text.clone();
                            self.synced_gen = Some(doc.generation());
                        }
                        sync::EditMapping::Detach => {
                            doc.detached_netlist = Some(self.doc.text.clone());
                            doc.touched();
                            self.source = NetlistSource::Detached;
                            self.source_label = "detached from the schematic".into();
                        }
                        sync::EditMapping::None => {}
                    }
                    return;
                }
                let g = app.circuit.as_ref().map(|d| d.generation());
                if g != self.synced_gen {
                    self.load_synced(app);
                }
            }
            NetlistSource::Detached => {
                if let Some(doc) = app.circuit.as_mut()
                    && doc.detached_netlist.as_deref() != Some(self.doc.text.as_str())
                {
                    doc.detached_netlist = Some(self.doc.text.clone());
                    doc.touched();
                }
            }
            _ => {}
        }
    }

    /// Drops a detached netlist and regenerates it from the schematic.
    pub fn resync(&mut self, app: &mut AppCtx) {
        if let Some(doc) = app.circuit.as_mut() {
            doc.detached_netlist = None;
            doc.touched();
        }
        self.source = NetlistSource::Synced;
        self.synced_gen = None;
        self.load_synced(app);
    }

    fn load_demo(&mut self) {
        *self = AnalysisScreen {
            runs_started: self.runs_started,
            ..Default::default()
        };
    }

    pub fn start_run(&mut self, app: &mut AppCtx) {
        if let Some(c) = &self.cancel {
            c.cancel();
        }
        let text = self.doc.text.clone();
        let file = self.doc.name.clone();
        let base = match &self.source {
            NetlistSource::File(p) => p.parent().map(|d| d.to_path_buf()),
            NetlistSource::Demo | NetlistSource::Synced | NetlistSource::Detached => None,
        };
        let profile = self.profile.clone();
        let cancel = CancelToken::new();
        self.cancel = Some(cancel.clone());
        self.error = None;
        self.runs_started += 1;
        self.job = Some(Job::spawn("spice-run", app.repaint.clone(), move |p| {
            let pc = p.clone();
            model::run(
                &text,
                &file,
                base,
                &profile,
                cancel,
                Arc::new(move |f| pc.set_fraction(Some(f))),
            )
        }));
    }

    pub fn is_running(&self) -> bool {
        self.job.is_some()
    }

    pub fn result(&self) -> Option<&Arc<ViewData>> {
        self.result.as_ref()
    }

    fn poll(&mut self) {
        if let Some(j) = self.job.as_mut()
            && let Some(r) = j.poll()
        {
            self.job = None;
            self.cancel = None;
            match r {
                Ok(v) => {
                    if let Some(t) = &v.tran
                        && let Some((a, b, _)) = t.cursors
                    {
                        self.cursors = Cursors {
                            a,
                            b,
                            enabled: true,
                        };
                    }
                    self.result = Some(v);
                    self.doc
                        .diagnostics
                        .retain(|d| d.severity != DiagSeverity::Error);
                }
                Err(e) => {
                    let cancelled = e.message.to_ascii_lowercase().contains("cancel");
                    self.error = Some(if cancelled {
                        "Run cancelled.".into()
                    } else {
                        e.message.clone()
                    });
                    if let Some(l) = e.line {
                        self.doc.diagnostics = vec![EditorDiagnostic {
                            line: l,
                            col: None,
                            severity: DiagSeverity::Error,
                            message: e.message,
                        }];
                        self.view.scroll_to_line = Some(l);
                    }
                }
            }
        }
    }

    /// Debounced parse check for editor diagnostics; autosave for file netlists.
    fn after_edit(&mut self, app: &AppCtx) {
        let Some(t) = self.last_edit else { return };
        if t.elapsed() < Duration::from_millis(400) {
            app.egui.request_repaint_after(Duration::from_millis(420));
            return;
        }
        if self.parsed_version == self.doc.version {
            return;
        }
        self.parsed_version = self.doc.version;
        let (prepared, lib) = model::prepare(&self.doc.text);
        let opts = bb_spice::ParseOptions {
            file_name: Some(self.doc.name.clone()),
            libraries: lib.into_iter().collect(),
            ..Default::default()
        };
        self.doc.diagnostics = match bb_spice::Netlist::parse_with(&prepared, &opts) {
            Ok(nl) => nl
                .warnings
                .iter()
                .map(|w| EditorDiagnostic {
                    line: w.span.line,
                    col: Some(w.span.col),
                    severity: DiagSeverity::Warning,
                    message: w.msg.clone(),
                })
                .collect(),
            Err(e) => vec![EditorDiagnostic {
                line: e.span.line,
                col: Some(e.span.col),
                severity: DiagSeverity::Error,
                message: e.msg.clone(),
            }],
        };
        if let NetlistSource::File(p) = &self.source
            && self.doc.dirty
            && std::fs::write(p, &self.doc.text).is_ok()
        {
            self.doc.dirty = false;
        }
    }

    fn export_cir(&mut self, app: &mut AppCtx) {
        let Some(path) =
            crate::platform::save_file("Export netlist", &self.doc.name, "SPICE netlist", "cir")
        else {
            return;
        };
        match std::fs::write(&path, export_text(&self.doc.text)) {
            Ok(()) => app.toasts.success(format!("Exported {}", path.display())),
            Err(e) => app.toasts.error(format!("Export failed: {e}")),
        }
    }

    fn export_plot(&mut self, app: &mut AppCtx, which: &str, kind: ExportKind) {
        let Some(v) = self.result.clone() else { return };
        let stem = format!("{}-{which}", self.doc.name.trim_end_matches(".cir"));
        let (ext, name) = match kind {
            ExportKind::Csv => ("csv", "CSV"),
            ExportKind::Png => ("png", "PNG image"),
            ExportKind::Svg => ("svg", "SVG image"),
        };
        let Some(path) =
            crate::platform::save_file("Export plot", &format!("{stem}.{ext}"), name, ext)
        else {
            return;
        };
        let (traces, x_name, x_axis, y_axis, title): (
            Vec<Trace>,
            &str,
            AxisSpec,
            AxisSpec,
            String,
        ) = match which {
            "tran" => match &v.tran {
                Some(t) => (
                    t.traces.clone(),
                    "time",
                    AxisSpec::linear("s"),
                    AxisSpec::linear(&t.y_unit),
                    t.title.clone(),
                ),
                None => return,
            },
            "dc" => match &v.dc {
                Some(d) => (
                    d.traces.clone(),
                    d.x_name.as_str(),
                    if d.x_log {
                        AxisSpec::log(&d.x_unit)
                    } else {
                        AxisSpec::linear(&d.x_unit)
                    },
                    AxisSpec::linear("V"),
                    d.title.clone(),
                ),
                None => return,
            },
            "ac" => match &v.ac {
                Some(a) => (
                    a.mag.clone(),
                    "frequency",
                    AxisSpec::log("Hz"),
                    AxisSpec::linear("dB"),
                    a.title.clone(),
                ),
                None => return,
            },
            "mc" => match &v.mc {
                Some(m) => {
                    let pts: Vec<[f64; 2]> = m
                        .hist
                        .bins
                        .iter()
                        .map(|(a, b, n)| [(a + b) / 2.0, *n as f64])
                        .collect();
                    (
                        vec![Trace::new(m.vector.clone(), crate::theme::DARK.accent, pts)],
                        "bin",
                        AxisSpec::linear("V"),
                        AxisSpec::linear(""),
                        m.title.clone(),
                    )
                }
                None => return,
            },
            _ => return,
        };
        let r = match kind {
            ExportKind::Csv => {
                let text = if which == "ac" {
                    v.ac.as_ref()
                        .map(|a| {
                            bb_spice::export::plot_to_csv(
                                &a.plot,
                                bb_spice::export::ComplexCsv::DbPhase,
                            )
                        })
                        .unwrap_or_default()
                } else {
                    plot::traces_to_csv(x_name, &traces)
                };
                std::fs::write(&path, text)
                    .map(|_| path.clone())
                    .map_err(|e| e.to_string())
            }
            ExportKind::Svg => std::fs::write(
                &path,
                plot::traces_to_svg(&title, &traces, &x_axis, &y_axis, 900.0, 300.0),
            )
            .map(|_| path.clone())
            .map_err(|e| e.to_string()),
            ExportKind::Png => {
                if let Some((_, rect)) = self.plot_rects.iter().find(|(n, _)| n == which) {
                    self.png = Some(plot::export_png(&app.egui, *rect, path));
                }
                return;
            }
        };
        match r {
            Ok(p) => app.toasts.success(format!("Exported {}", p.display())),
            Err(e) => app.toasts.error(format!("Export failed: {e}")),
        }
    }
}

/// ngspice-compatible export: stand-ins included so ngspice can run it; Breadbox-only cards
/// become `*!bb` directives.
fn sync_profile(doc: &crate::circuit::CircuitDoc) -> Profile {
    sync::profile_for(doc)
}

pub fn export_text(text: &str) -> String {
    let (prepared, lib) = model::prepare(text);
    let mut with_stubs = prepared.clone();
    if lib.is_some() {
        let stubs: String = demo::STUB_PARTS
            .iter()
            .filter(|(n, _)| text.to_ascii_lowercase().contains(&n.to_ascii_lowercase()))
            .map(|(_, b)| *b)
            .collect();
        match with_stubs.to_ascii_lowercase().rfind("\n.end") {
            Some(i) => with_stubs.insert_str(i + 1, &stubs),
            None => with_stubs.push_str(&stubs),
        }
    }
    match bb_spice::Netlist::parse(&with_stubs) {
        Ok(nl) => nl.to_ngspice_text(&[]),
        Err(_) => text.to_string(),
    }
}

impl Screen for AnalysisScreen {
    fn crumb(&self, _app: &AppCtx) -> Option<String> {
        Some("Analysis".into())
    }

    fn flush(&mut self, _app: &mut AppCtx) {
        if let NetlistSource::File(p) = &self.source
            && self.doc.dirty
            && std::fs::write(p, &self.doc.text).is_ok()
        {
            self.doc.dirty = false;
        }
    }

    fn project_changed(&mut self, app: &mut AppCtx) {
        // the previous project's file netlist keeps its last edits
        self.flush(app);
        // a project's own netlist (analyses/*.cir) wins over the demo
        let file = app.project.as_ref().and_then(|p| {
            let dir = p.dir.join("analyses");
            let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "cir"))
                .collect();
            v.sort();
            v.into_iter().next()
        });
        match file {
            Some(f) => {
                if self.load_file(f.clone()).is_err() {
                    self.load_demo();
                }
            }
            None => {
                // the circuit's own netlist, synced from the schematic (PRO-2)
                self.source = NetlistSource::Demo;
                if !self.load_synced(app) {
                    self.load_demo();
                }
            }
        }
        self.result = None;
        self.auto_ran = false;
    }

    fn on_enter(&mut self, app: &mut AppCtx) {
        if !self.auto_ran && self.result.is_none() && self.job.is_none() {
            self.auto_ran = true;
            self.start_run(app);
        }
    }

    fn command(&mut self, cmd: &Command, app: &mut AppCtx) -> bool {
        match cmd {
            Command::RunAnalyses => {
                self.start_run(app);
                true
            }
            Command::ExportCir => {
                self.export_cir(app);
                true
            }
            Command::Find => {
                self.view.open_find();
                true
            }
            Command::Save => {
                if let NetlistSource::File(p) = &self.source {
                    if std::fs::write(p, &self.doc.text).is_ok() {
                        self.doc.dirty = false;
                        app.toasts.success("Saved");
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn header(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if Btn::ghost("Export .cir").show(ui).clicked() {
                self.export_cir(app);
            }
            if self.job.is_some() {
                if Btn::ghost("Cancel").show(ui).clicked()
                    && let Some(c) = &self.cancel
                {
                    c.cancel();
                }
                let frac = self.job.as_ref().and_then(|j| j.progress().fraction);
                ui.allocate_ui(vec2(140.0, 20.0), |ui| {
                    widgets::progress(ui, frac, "");
                });
                ui.label(
                    egui::RichText::new("solving…")
                        .font(font::mono(12.0))
                        .color(p.text_muted),
                );
            } else {
                if Btn::amber("Run analyses").show(ui).clicked() {
                    self.start_run(app);
                }
                let s = match (&self.result, &self.error) {
                    (_, Some(e)) => e
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(90)
                        .collect::<String>(),
                    (Some(r), None) => r.summary.clone(),
                    _ => String::new(),
                };
                let c = if self.error.is_some() {
                    p.danger_soft
                } else {
                    p.text_muted
                };
                ui.label(egui::RichText::new(s).font(font::mono(12.0)).color(c));
            }
        });
    }

    fn ui(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        self.poll();
        self.after_edit(app);
        self.sync_tick(app);
        if let Some(slot) = &self.png {
            let done = slot.lock().ok().and_then(|mut g| g.take());
            if let Some(r) = done {
                match r {
                    Ok(path) => app.toasts.success(format!("Exported {}", path.display())),
                    Err(e) => app.toasts.error(format!("PNG export failed: {e}")),
                }
                self.png = None;
            }
        }

        egui::Panel::left("pro-netlist")
            .exact_size(380.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.bg_canvas))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().line_segment(
                    [r.right_top(), r.right_bottom()],
                    Stroke::new(1.0, p.border),
                );
                self.netlist_panel(ui, app);
            });
        egui::Panel::right("pro-analyses")
            .exact_size(280.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(p.bg_panel)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().line_segment(
                    [
                        pos2(r.left() - 16.0, r.top() - 16.0),
                        pos2(r.left() - 16.0, r.bottom() + 16.0),
                    ],
                    Stroke::new(1.0, p.border),
                );
                self.sidebar(ui, app);
            });
        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("pro-main")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(egui::Margin::same(16))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 16.0;
                            self.plots(ui, app);
                        });
                });
        });
        self.popover(app);
    }
}

impl AnalysisScreen {
    fn netlist_panel(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        let rect = ui.available_rect_before_wrap();
        let bar = Rect::from_min_size(rect.min, vec2(rect.width(), 38.0));
        ui.painter().rect_filled(bar, 0.0, p.bg_header);
        ui.painter().line_segment(
            [bar.left_bottom(), bar.right_bottom()],
            Stroke::new(1.0, p.border),
        );
        let mut c = widgets::child_ui(ui, bar.shrink2(vec2(14.0, 0.0)));
        c.horizontal_centered(|ui| {
            let name = if self.doc.dirty {
                format!("{} •", self.doc.name)
            } else {
                self.doc.name.clone()
            };
            ui.label(
                egui::RichText::new(name)
                    .font(font::ui(12.0))
                    .color(p.text_strong),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if Btn::plain("Open .cir…")
                    .icon_only(Icon::Folder)
                    .height(24.0)
                    .show(ui)
                    .clicked()
                    && let Some(f) = crate::platform::open_file(
                        "Open a SPICE netlist",
                        "SPICE netlist",
                        &["cir", "sp", "spice", "net"],
                    )
                {
                    match self.load_file(f) {
                        Ok(()) => self.start_run(app),
                        Err(e) => app.toasts.error(format!("{e}")),
                    }
                }
                if self.source == NetlistSource::Detached
                    && Btn::mini("Re-sync").icon(Icon::Refresh).show(ui).clicked()
                {
                    self.resync(app);
                    self.start_run(app);
                }
                if matches!(self.source, NetlistSource::File(_))
                    && Btn::plain("Load the demo netlist")
                        .icon_only(Icon::Restart)
                        .height(24.0)
                        .show(ui)
                        .clicked()
                {
                    self.load_demo();
                    self.start_run(app);
                }
                ui.label(
                    egui::RichText::new(&self.source_label)
                        .font(font::ui(12.0))
                        .color(p.text_muted),
                );
            });
        });
        ui.allocate_rect(bar, Sense::hover());
        let out =
            CodeEditor::new("pro-netlist")
                .font(12.5, 19.0)
                .show(ui, &mut self.doc, &mut self.view);
        if out.changed {
            self.last_edit = Some(Instant::now());
        }
    }

    fn sidebar(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        ui.spacing_mut().item_spacing.y = 10.0;
        section_header(ui, "Analyses");
        ui.add_space(-2.0);
        let found = cards::scan(&self.doc.text);
        let w = ui.available_width();
        for kind in AnalysisKind::ALL {
            let card = found.iter().find(|c| c.kind == kind).cloned();
            // the sidebar shows .noise / .op only when present, like the design
            if matches!(kind, AnalysisKind::Noise | AnalysisKind::Op) && card.is_none() {
                continue;
            }
            let on = card.as_ref().is_some_and(|c| c.enabled);
            let summary = cards::summary(card.as_ref());
            let resp = inner_card_frame(p, on)
                .show(ui, |ui| {
                    ui.set_width(w - 26.0);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.label(
                        egui::RichText::new(kind.title())
                            .font(font::semibold(13.0))
                            .color(p.text),
                    );
                    ui.label(
                        egui::RichText::new(&summary)
                            .font(font::mono(12.0))
                            .color(p.text_muted),
                    );
                })
                .response;
            let resp = ui.interact(
                resp.rect,
                ui.scope_id().with(("an-card", kind.keyword())),
                Sense::click(),
            );
            let label = format!("{} — {}", kind.title(), summary);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, &label));
            if resp.hovered() {
                ui.painter().rect_stroke(
                    resp.rect,
                    8.0,
                    Stroke::new(1.0, p.accent_border),
                    egui::StrokeKind::Inside,
                );
            }
            if resp.clicked() {
                let args = card.as_ref().map(|c| c.args.clone()).unwrap_or_else(|| {
                    kind.default_args()
                        .split_whitespace()
                        .map(str::to_string)
                        .collect()
                });
                self.editing = Some((kind, kind.to_fields(&args), on));
                self.editing_anchor = resp.rect;
            }
            let _ = app;
        }
        // analysis plugins (PLUG-7)
        plugin_analyses::section(ui, app, &self.doc.text);
        ui.add_space(4.0);
        widgets::hline(ui, p.border);
        ui.add_space(4.0);
        section_header(ui, "Mixed-signal");
        let mi = model::mixed_info(&self.doc.text);
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "The AVR core runs the real compiled firmware and hands pin states to the analog solver every {}; ADC reads sample the solved node voltage.",
                    mi.sync
                ))
                .font(font::ui(13.0))
                .line_height(Some(20.0))
                .color(p.text_dim),
            )
            .wrap(),
        );
        widgets::kv_grid(
            ui,
            "mixed",
            &[
                ("Sync step".into(), mi.sync.clone()),
                ("Solver".into(), mi.solver.clone()),
                ("Integration".into(), mi.integration.clone()),
            ],
            90.0,
        );
        let warnings = self
            .result
            .as_ref()
            .map(|r| r.warnings.clone())
            .unwrap_or_default();
        if !warnings.is_empty() {
            ui.add_space(6.0);
            section_header(ui, "Warnings");
            for w in warnings.iter().take(6) {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(w)
                            .font(font::mono(11.0))
                            .color(p.accent),
                    )
                    .wrap(),
                );
            }
        }
    }

    fn popover(&mut self, app: &mut AppCtx) {
        let Some((kind, fields, enabled)) = self.editing.as_mut() else {
            return;
        };
        let ctx = app.egui.clone();
        let p = crate::theme::pal_ctx(&ctx);
        let w = 300.0;
        let pos = pos2(
            self.editing_anchor.left() - w - 12.0,
            self.editing_anchor.top(),
        );
        let mut apply = false;
        let mut close = false;
        let area = egui::Area::new(egui::Id::unique("analysis-popover"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(&ctx, |ui| {
                egui::Frame::new()
                    .fill(p.bg_panel)
                    .stroke(Stroke::new(1.0, p.border_strong))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::same(14))
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        ui.set_width(w - 28.0);
                        ui.spacing_mut().item_spacing.y = 8.0;
                        ui.label(
                            egui::RichText::new(kind.title())
                                .font(font::heading(15.0))
                                .color(p.text_strong),
                        );
                        widgets::checkbox(ui, enabled, "Enabled");
                        let defs = kind.fields();
                        egui::Grid::new("an-fields")
                            .num_columns(2)
                            .spacing(vec2(10.0, 6.0))
                            .show(ui, |ui| {
                                for (i, (label, hint)) in defs.iter().enumerate() {
                                    if fields.len() <= i {
                                        fields.push(String::new());
                                    }
                                    ui.label(
                                        egui::RichText::new(*label)
                                            .font(font::ui(12.0))
                                            .color(p.text_muted),
                                    );
                                    ui.add(
                                        egui::TextEdit::singleline(&mut fields[i])
                                            .hint_text(*hint)
                                            .font(font::mono(12.0))
                                            .desired_width(150.0),
                                    );
                                    ui.end_row();
                                }
                            });
                        if defs.is_empty() {
                            ui.label(
                                egui::RichText::new(
                                    "Solves the DC bias point; results fill the .op table.",
                                )
                                .font(font::ui(12.0))
                                .color(p.text_muted),
                            );
                        }
                        let preview = format!("{} {}", kind.keyword(), kind.from_fields(fields));
                        ui.label(
                            egui::RichText::new(if *enabled {
                                preview
                            } else {
                                format!("*off {preview}")
                            })
                            .font(font::mono(11.5))
                            .color(p.text_faint),
                        );
                        ui.horizontal(|ui| {
                            if Btn::amber("Apply").small().show(ui).clicked() {
                                apply = true;
                            }
                            if Btn::ghost("Cancel").small().show(ui).clicked() {
                                close = true;
                            }
                        });
                    });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
            && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|pt| {
                !area.response.rect.contains(pt) && !self.editing_anchor.contains(pt)
            });
        if apply {
            let (kind, fields, enabled) = self.editing.take().expect("editing");
            let mut text = cards::set_card(&self.doc.text, kind, &kind.from_fields(&fields));
            if !enabled {
                text = cards::set_enabled(&text, kind, false);
            }
            if text != self.doc.text {
                self.doc.text = text;
                self.doc.version += 1;
                self.doc.dirty = true;
                self.last_edit = Some(Instant::now());
                self.start_run(app);
            }
        } else if close || clicked_outside {
            self.editing = None;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn plot_card<R>(
        &mut self,
        ui: &mut Ui,
        id: &str,
        title: &str,
        content: impl FnOnce(&mut Ui, &mut Self) -> R,
        exports: bool,
        app: &mut AppCtx,
        extra: impl FnOnce(&mut Ui),
    ) -> R {
        let p = pal(ui);
        let mut export = None;
        let r = widgets::card_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                ui.label(
                    egui::RichText::new(title)
                        .font(font::heading(15.0))
                        .color(p.text),
                );
                extra(ui);
                if exports {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let resp = Btn::plain(&format!("Export {id}"))
                            .icon_only(Icon::Dots)
                            .height(22.0)
                            .show(ui);
                        egui::Popup::menu(&resp).show(|ui| {
                            if widgets::menu_item(ui, "Export CSV…", false, true) {
                                export = Some(ExportKind::Csv);
                            }
                            if widgets::menu_item(ui, "Export PNG…", false, true) {
                                export = Some(ExportKind::Png);
                            }
                            if widgets::menu_item(ui, "Export SVG…", false, true) {
                                export = Some(ExportKind::Svg);
                            }
                        });
                    });
                }
            });
            content(ui, self)
        });
        self.plot_rects.retain(|(n, _)| n != id);
        self.plot_rects.push((id.to_string(), r.response.rect));
        if let Some(k) = export {
            self.export_plot(app, id, k);
        }
        r.inner
    }

    fn plots(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        let Some(v) = self.result.clone() else {
            let msg = if self.job.is_some() {
                "Solving the netlist…".to_string()
            } else {
                self.error
                    .clone()
                    .unwrap_or_else(|| "Run analyses to see plots.".into())
            };
            ui.label(
                egui::RichText::new(msg)
                    .font(font::mono(12.5))
                    .color(p.text_muted),
            );
            return;
        };
        let w = ui.available_width();
        // ---- .tran
        if let Some(t) = v.tran.clone() {
            let readout = if self.cursors.enabled {
                let dt = self.cursors.delta();
                let duty = t
                    .traces
                    .first()
                    .and_then(|tr| {
                        duty_between(
                            &tr.points,
                            self.cursors.a.min(self.cursors.b),
                            self.cursors.a.max(self.cursors.b),
                        )
                    })
                    .map(|d| format!(" · duty {:.1}%", d * 100.0))
                    .unwrap_or_default();
                format!(
                    "cursor Δt {} · f {}{}",
                    fmt_readout(dt, "s", 3),
                    fmt_readout(1.0 / dt.max(1e-30), "Hz", 1),
                    duty
                )
            } else {
                String::new()
            };
            let traces = t.traces.clone();
            self.plot_card(
                ui,
                "tran",
                &t.title,
                |ui, me| {
                    let h = ((w - 32.0) * 250.0 / 900.0).clamp(180.0, 320.0);
                    let mut lp = LinePlot::new("tran", &traces);
                    lp.x = AxisSpec::linear("s");
                    lp.y = AxisSpec::linear(&t.y_unit);
                    lp.y2 = t
                        .y2_unit
                        .as_ref()
                        .map(|_| AxisSpec::linear("A").display(1e3));
                    if let Some(y2) = lp.y2.as_mut() {
                        y2.unit = "mA".into();
                    }
                    lp.height = h;
                    lp.cursors = Some(&mut me.cursors);
                    lp.show(ui);
                },
                true,
                app,
                |ui| {
                    for tr in &t.traces {
                        legend_swatch(ui, tr.color, &tr.name);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(30.0);
                        ui.label(
                            egui::RichText::new(&readout)
                                .font(font::mono(12.0))
                                .color(p.text_muted),
                        );
                    });
                },
            );
        }
        // ---- .dc + .step  |  .mc
        let half = (w - 16.0) / 2.0;
        let has_dc = v.dc.is_some();
        let has_mc = v.mc.is_some();
        if has_dc || has_mc {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                let cw = if has_dc && has_mc { half } else { w };
                if let Some(d) = v.dc.clone() {
                    ui.vertical(|ui| {
                        ui.set_width(cw);
                        self.plot_card(
                            ui,
                            "dc",
                            &d.title,
                            |ui, _| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 14.0;
                                    for tr in &d.traces {
                                        legend_swatch(ui, tr.color, &tr.name);
                                    }
                                });
                                let h = ((cw - 32.0) * 210.0 / 440.0).clamp(150.0, 260.0);
                                let mut lp = LinePlot::new("dc", &d.traces);
                                lp.x = if d.x_log {
                                    AxisSpec::log(&d.x_unit)
                                } else {
                                    AxisSpec::linear(&d.x_unit)
                                };
                                lp.y = AxisSpec::linear("V");
                                lp.height = h;
                                if let Some((x, y, label)) = &d.marker {
                                    lp.markers.push(Marker::Point {
                                        x: *x,
                                        y: *y,
                                        label: label.clone(),
                                        color: p.accent,
                                    });
                                }
                                lp.show(ui);
                            },
                            true,
                            app,
                            |_| {},
                        );
                    });
                }
                if let Some(m) = v.mc.clone() {
                    ui.vertical(|ui| {
                        ui.set_width(cw);
                        self.plot_card(
                            ui,
                            "mc",
                            &m.title,
                            |ui, _| {
                                ui.label(
                                    egui::RichText::new(&m.stats)
                                        .font(font::mono(12.0))
                                        .color(p.text_muted),
                                );
                                let h = ((cw - 32.0) * 210.0 / 440.0).clamp(150.0, 260.0);
                                histogram_plot(ui, "mc", &m.hist, h);
                            },
                            true,
                            app,
                            |_| {},
                        );
                    });
                }
            });
        }
        // ---- .ac
        if let Some(a) = v.ac.clone() {
            self.plot_card(
                ui,
                "ac",
                &a.title,
                |ui, _| {
                    plot::bode_plot(ui, "ac", &a.mag, &a.phase, 300.0);
                },
                true,
                app,
                |ui| {
                    legend_swatch(ui, a.mag[0].color, &a.mag[0].name);
                    legend_swatch(ui, a.phase[0].color, &a.phase[0].name);
                },
            );
        }
        // ---- .noise
        if let Some(n) = v.noise.clone() {
            self.plot_card(
                ui,
                "noise",
                &n.title,
                |ui, _| {
                    if let Some(t) = &n.total {
                        ui.label(
                            egui::RichText::new(t)
                                .font(font::mono(12.0))
                                .color(p.text_muted),
                        );
                    }
                    let mut lp = LinePlot::new("noise", &n.traces);
                    lp.x = AxisSpec::log("Hz");
                    lp.y = AxisSpec::linear("V/√Hz");
                    lp.height = 200.0;
                    lp.show(ui);
                },
                false,
                app,
                |_| {},
            );
        }
        // ---- .op
        if !v.op.is_empty() {
            self.op_table(ui, app, &v);
        }
    }

    fn op_table(&mut self, ui: &mut Ui, app: &mut AppCtx, v: &ViewData) {
        let p = pal(ui);
        let rows_all = &v.op;
        let shown = if self.show_all_op {
            rows_all.len()
        } else if self.profile.op_rows.is_empty() {
            rows_all.len().min(8)
        } else {
            rows_all.len().min(self.profile.op_rows.len())
        };
        egui::Frame::new()
            .fill(p.bg_header)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(10)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.add_space(0.0);
                    ui.vertical(|ui| {
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new(".op — operating point")
                                .font(font::heading(15.0))
                                .color(p.text),
                        );
                        ui.add_space(12.0);
                    });
                    if rows_all.len() > shown || self.show_all_op {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(12.0);
                            let label = if self.show_all_op {
                                "Fewer rows"
                            } else {
                                "All rows"
                            };
                            if Btn::plain(label).small().show(ui).clicked() {
                                self.show_all_op = !self.show_all_op;
                            }
                        });
                    }
                });
                let rows: Vec<Row> = rows_all
                    .iter()
                    .take(shown)
                    .map(|r| {
                        let probe = app
                            .bench
                            .probes
                            .get(&r.quantity)
                            .map(|v| plot::fmt_si(*v, unit_of(&r.quantity)))
                            .unwrap_or_else(|| "—".into());
                        Row::new(vec![
                            Cell::new(&r.quantity),
                            Cell::new(&r.value),
                            Cell::new(probe).color(p.text_muted),
                        ])
                    })
                    .collect();
                data_table(
                    ui,
                    "op",
                    &[
                        Col::Fr("quantity", 1.4),
                        Col::Fr("value", 1.0),
                        Col::Fr("bench probe", 1.0),
                    ],
                    &rows,
                    &TableStyle::default(),
                );
            });
    }
}

fn unit_of(q: &str) -> &'static str {
    match q.chars().next() {
        Some('V') => "V",
        Some('I') => "A",
        Some('P') => "W",
        _ => "",
    }
}

/// Fraction of time above the mid level between `a` and `b`.
fn duty_between(points: &[[f64; 2]], a: f64, b: f64) -> Option<f64> {
    let seg: Vec<&[f64; 2]> = points.iter().filter(|p| p[0] >= a && p[0] <= b).collect();
    if seg.len() < 2 {
        return None;
    }
    let (lo, hi) = seg
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), p| {
            (l.min(p[1]), h.max(p[1]))
        });
    let mid = (lo + hi) / 2.0;
    let mut high = 0.0;
    for w in seg.windows(2) {
        if (w[0][1] + w[1][1]) / 2.0 >= mid {
            high += w[1][0] - w[0][0];
        }
    }
    Some(high / (b - a).max(1e-30))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_is_ngspice_runnable_text() {
        let out = export_text(demo::NETLIST);
        assert!(
            out.to_ascii_lowercase().contains(".subckt atmega328p"),
            "{out}"
        );
        assert!(!out.contains("fw="));
        assert!(out.contains("*!bb .step"), "{out}");
        assert!(bb_spice::Netlist::parse(&out).is_ok());
    }

    #[test]
    fn duty_of_a_square_wave() {
        let pts: Vec<[f64; 2]> = (0..1000)
            .map(|i| [i as f64, if i % 100 < 30 { 5.0 } else { 0.0 }])
            .collect();
        let d = duty_between(&pts, 0.0, 999.0).unwrap();
        assert!((d - 0.3).abs() < 0.02, "{d}");
    }
}
