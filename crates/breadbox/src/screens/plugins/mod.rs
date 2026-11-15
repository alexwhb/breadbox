//! Plugins & instruments (design screen 4, `PLUG-4`): category nav (Instruments, Parts &
//! boards, Radios & networks, World models, Solvers & analyses, Views & themes) + installed
//! list, search, the detail page (front-panel art, kind chip, specs grid, the live loading
//! callout, Place on bench, Edit model, enable / uninstall, theme / analysis / lesson-pack
//! actions), "More to add" catalog cards with Install (permission prompt), the read-only
//! `plugin.json` pane, "Load local plugin…" (folder or `.bbplugin`) and "Plugin SDK docs".

pub mod place;

use crate::app::AppCtx;
use crate::router::{ProTab, Route, Screen};
use crate::theme::{font, pal};
use crate::widgets::{self, Btn, Icon, instrument, markdown};
use bb_plugin::panel::{Panel, PanelState};
use bb_plugin::{CatalogEntry, CatalogStatus, Category, Kind, Manifest, Permission, Source};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Where an install comes from (the permission prompt installs from it).
#[derive(Debug, Clone)]
pub enum InstallSource {
    /// A first-party plugin from the catalog.
    Bundled,
    /// A `.bbplugin` package.
    Package(Vec<u8>),
    /// A development folder ("Load local plugin…").
    Folder(PathBuf),
    /// An installed plugin that asks for more permissions.
    Grant,
}

/// The permission prompt (`PLUG-2`).
#[derive(Debug, Clone)]
pub struct InstallPrompt {
    pub id: String,
    pub name: String,
    pub version: String,
    pub permissions: Vec<Permission>,
    pub source: InstallSource,
}

/// Persisted state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PluginsState {
    pub category: Option<String>,
    pub selected: Option<String>,
    /// Active theme plugin.
    pub theme: Option<String>,
}

pub struct PluginsScreen {
    pub category: Category,
    pub selected: Option<String>,
    pub search: String,
    pub prompt: Option<InstallPrompt>,
    /// "Plugin SDK docs" is open.
    pub docs: Option<String>,
    confirm_uninstall: Option<String>,
    callout: Option<(String, String)>,
    callout_at: Option<Instant>,
    theme_restored: bool,
    saved_theme: Option<String>,
}

impl Default for PluginsScreen {
    fn default() -> Self {
        PluginsScreen {
            category: Category::Instruments,
            selected: None,
            search: String::new(),
            prompt: None,
            docs: None,
            confirm_uninstall: None,
            callout: None,
            callout_at: None,
            theme_restored: false,
            saved_theme: None,
        }
    }
}

fn is_installed(e: &CatalogEntry) -> bool {
    matches!(
        e.status,
        CatalogStatus::Installed
            | CatalogStatus::Disabled
            | CatalogStatus::UpdateAvailable { .. }
            | CatalogStatus::NeedsApproval { .. }
    )
}

fn status_word(e: &CatalogEntry) -> &'static str {
    match &e.status {
        CatalogStatus::Installed => "installed",
        CatalogStatus::Available => "available",
        CatalogStatus::UpdateAvailable { .. } => "update available",
        CatalogStatus::Disabled => "disabled",
        CatalogStatus::NeedsApproval { .. } => "needs approval",
        CatalogStatus::Planned { .. } => "planned",
        CatalogStatus::Error { .. } => "error",
    }
}

fn matches_search(e: &CatalogEntry, q: &str) -> bool {
    let q = q.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    [
        e.name.as_str(),
        e.summary.as_str(),
        e.id.as_str(),
        e.chip.as_str(),
        e.category.label(),
    ]
    .iter()
    .any(|s| s.to_ascii_lowercase().contains(&q))
}

/// `docs/PLUGIN-SDK.md` from the app resources (the bundle, or the repository in development).
fn sdk_doc(app: &AppCtx) -> String {
    app.resources
        .file("docs/PLUGIN-SDK.md")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_else(|| "# Plugin SDK\n\nThe SDK reference (`docs/PLUGIN-SDK.md`) is not bundled with this build.".into())
}

impl PluginsScreen {
    /// Catalog entries (search applied).
    fn catalog(&self, app: &AppCtx) -> Vec<CatalogEntry> {
        app.plugins
            .registry
            .catalog()
            .into_iter()
            .filter(|e| e.category != Category::Lessons || is_installed(e))
            .filter(|e| matches_search(e, &self.search))
            .collect()
    }

    /// The plugin the detail page shows: the selected one, else the first of the category
    /// (installed first).
    fn featured<'a>(&self, cat: &'a [CatalogEntry]) -> Option<&'a CatalogEntry> {
        if let Some(id) = &self.selected
            && let Some(e) = cat.iter().find(|e| &e.id == id)
        {
            return Some(e);
        }
        cat.iter()
            .filter(|e| e.category == self.category)
            .min_by_key(|e| !is_installed(e))
            .or_else(|| cat.first())
    }

    /// Opens the permission prompt for a catalog install.
    pub fn request_install(&mut self, app: &AppCtx, id: &str) {
        let Some(p) = app.plugins.registry.get(id) else {
            return;
        };
        let m = &p.manifest;
        self.prompt = Some(InstallPrompt {
            id: m.id.clone(),
            name: m.display_name(),
            version: m.version.clone(),
            permissions: m.permissions.clone(),
            source: InstallSource::Bundled,
        });
    }

    /// "Load local plugin…" with a known path (folder or `.bbplugin`): opens the prompt.
    pub fn request_load(&mut self, app: &mut AppCtx, path: &std::path::Path) {
        let result = if path.is_dir() {
            Manifest::load(path).map(|m| (m, InstallSource::Folder(path.to_path_buf())))
        } else {
            std::fs::read(path)
                .map_err(|e| bb_plugin::PluginError::Package(e.to_string()))
                .and_then(|bytes| {
                    bb_plugin::package::inspect(&bytes).map(|m| (m, InstallSource::Package(bytes)))
                })
        };
        match result {
            Ok((m, source)) => {
                self.prompt = Some(InstallPrompt {
                    id: m.id.clone(),
                    name: m.display_name(),
                    version: m.version.clone(),
                    permissions: m.permissions.clone(),
                    source,
                });
            }
            Err(e) => app.toasts.error(format!("Can't load that plugin: {e}")),
        }
    }

    /// Installs what the prompt shows (the user granted its permissions).
    pub fn confirm_install(&mut self, app: &mut AppCtx) {
        let Some(pr) = self.prompt.take() else {
            return;
        };
        let reg = &mut app.plugins.registry;
        let r = match &pr.source {
            InstallSource::Bundled => reg.install_bundled(&pr.id, &pr.permissions).map(|_| ()),
            InstallSource::Package(bytes) => reg
                .install_package(bytes, &pr.permissions, false)
                .map(|_| ()),
            InstallSource::Folder(dir) => reg.load_local(dir, &pr.permissions).map(|_| ()),
            InstallSource::Grant => reg.grant(&pr.id, &pr.permissions),
        };
        match r {
            Ok(()) => {
                app.plugins.changed();
                self.selected = Some(pr.id.clone());
                let how = match pr.source {
                    InstallSource::Folder(_) => "Loaded local plugin",
                    InstallSource::Grant => "Approved",
                    _ => "Installed",
                };
                app.toasts
                    .success(format!("{how} {} {}", pr.name, pr.version));
            }
            Err(e) => app.toasts.error(format!("{e}")),
        }
    }

    fn uninstall(&mut self, app: &mut AppCtx, id: &str) {
        let name = app
            .plugins
            .registry
            .get(id)
            .map(|p| p.manifest.display_name())
            .unwrap_or_else(|| id.to_string());
        match app.plugins.registry.uninstall(id) {
            Ok(()) => {
                if app.plugins.theme.as_deref() == Some(id) {
                    let ctx = app.egui.clone();
                    app.plugins.apply_theme(&ctx, None);
                    app.theme.apply(&ctx);
                }
                app.plugins.changed();
                app.toasts.info(format!("Uninstalled {name}"));
            }
            Err(e) => app.toasts.error(format!("{e}")),
        }
    }

    /// "Place on bench": the plugin's (first) part on the open project's bench.
    pub fn place_on_bench(&mut self, app: &mut AppCtx, plugin: &str) -> bool {
        if app.project.is_none() {
            app.toasts
                .info("Open a project or an example first — then place it on its bench.");
            return false;
        }
        let host = app.plugins.host();
        let parts = host.parts_of(plugin);
        let Some(def) = parts.first().cloned() else {
            app.toasts.info("This plugin has no part to place.");
            return false;
        };
        let lib = app.plugins.library();
        let Some(doc) = app.circuit.as_mut() else {
            return false;
        };
        if doc.lib.get(&def).is_none() {
            doc.set_library(lib);
        }
        match place::place(doc, &def) {
            Ok(r) => {
                let how = match (&r.replaced, r.clipped.is_empty()) {
                    (Some(_), _) => format!(
                        "{} is now the {} — its probes stay on the same nodes",
                        r.reference,
                        app.plugins
                            .registry
                            .get(plugin)
                            .map(|p| p.manifest.display_name())
                            .unwrap_or_default()
                    ),
                    (None, false) => format!("{} placed with its probes clipped on", r.reference),
                    (None, true) => format!(
                        "{} placed — drag its probe tips onto any two nodes",
                        r.reference
                    ),
                };
                app.toasts.success(how);
                app.navigate(Route::Workbench);
                self.callout_at = None;
                true
            }
            Err(e) => {
                app.toasts.error(e);
                false
            }
        }
    }

    /// "Edit model": a local, hot-reloaded copy of the plugin in the plugin-dev folder.
    pub fn edit_model(&mut self, app: &mut AppCtx, id: &str, open_folder: bool) -> Option<PathBuf> {
        let p = app.plugins.registry.get(id)?.clone();
        let dir = if p.source == Source::Local {
            p.root.clone()
        } else {
            let dev = app
                .plugins
                .registry
                .user_dir()
                .parent()
                .map(|d| d.join("plugin-dev"))
                .unwrap_or_else(|| PathBuf::from("plugin-dev"))
                .join(id);
            if !dev.join("plugin.json").exists()
                && let Err(e) = copy_dir(&p.root, &dev)
            {
                app.toasts
                    .error(format!("Couldn't copy {}: {e}", p.manifest.display_name()));
                return None;
            }
            let granted: Vec<Permission> = p.manifest.permissions.clone();
            if let Err(e) = app.plugins.registry.load_local(&dev, &granted) {
                app.toasts.error(format!("{e}"));
                return None;
            }
            app.plugins.changed();
            dev
        };
        app.toasts.info(format!(
            "Editing a local copy in {} — saved changes reload on the bench automatically.",
            dir.display()
        ));
        if open_folder {
            app.egui
                .open_url(egui::OpenUrl::new_tab(format!("file://{}", dir.display())));
        }
        Some(dir)
    }

    fn load_local_dialog(&mut self, app: &mut AppCtx, folder: bool) {
        // GUI tests answer the file dialog through the environment
        let path = if let Some(p) = std::env::var_os("BREADBOX_TEST_PICK") {
            Some(PathBuf::from(p))
        } else if folder {
            crate::platform::pick_folder("Load a plugin folder (with plugin.json)")
        } else {
            crate::platform::open_file(
                "Open a plugin package",
                "Breadbox plugin",
                &["bbplugin", "zip"],
            )
        };
        if let Some(p) = path {
            self.request_load(app, &p);
        }
    }

    /// Live loading callout of an instrument on the running bench (refreshed each second).
    fn refresh_callout(&mut self, app: &mut AppCtx, plugin: &str) {
        let due = self
            .callout_at
            .is_none_or(|t| t.elapsed() > Duration::from_millis(1000))
            || self.callout.as_ref().map(|c| c.0.as_str()) != Some(plugin);
        if !due {
            return;
        }
        self.callout_at = Some(Instant::now());
        self.callout = None;
        let Some(part) = instrument_on_bench(app, plugin) else {
            return;
        };
        let Some(sim) = app.sim() else { return };
        let p2 = part.clone();
        if let Some(Some(text)) =
            sim.with_bench(move |b| bb_plugin::host::loading_callout(b, &p2).map(|c| c.text))
        {
            self.callout = Some((plugin.to_string(), text));
        }
    }

    // ------------------------------------------------------------------ drawing

    fn nav(&mut self, ui: &mut Ui, app: &mut AppCtx, cat: &[CatalogEntry]) {
        let p = pal(ui);
        ui.spacing_mut().item_spacing.y = 2.0;
        section(ui, "Extend with", 0.0);
        for c in Category::NAV {
            let on = self.category == c && self.selected.is_none();
            if nav_row(ui, c.label(), on, &format!("Category {}", c.label())).clicked() {
                self.category = c;
                self.selected = None;
            }
        }
        section(ui, "Installed", 18.0);
        let mut any = false;
        for e in cat.iter().filter(|e| is_installed(e)) {
            any = true;
            let on = self.selected.as_deref() == Some(e.id.as_str());
            let r = nav_row(ui, &e.name, on, &format!("Plugin {}", e.name));
            if !matches!(e.status, CatalogStatus::Installed) {
                ui.painter().circle_filled(
                    pos2(r.rect.right() - 10.0, r.rect.center().y),
                    3.0,
                    p.accent,
                );
            }
            if r.clicked() {
                self.selected = Some(e.id.clone());
                self.category = e.category;
            }
        }
        if !any {
            ui.label(
                egui::RichText::new("Nothing installed yet")
                    .font(font::ui(12.0))
                    .color(p.text_faint),
            );
        }
        let lessons: Vec<&CatalogEntry> = cat
            .iter()
            .filter(|e| e.category == Category::Lessons && is_installed(e))
            .collect();
        if !lessons.is_empty() {
            section(ui, "Lesson packs", 18.0);
            for e in lessons {
                if nav_row(
                    ui,
                    &e.name,
                    self.selected.as_deref() == Some(&e.id),
                    &format!("Plugin {}", e.name),
                )
                .clicked()
                {
                    self.selected = Some(e.id.clone());
                }
            }
        }
        let _ = app;
    }

    fn detail(&mut self, ui: &mut Ui, app: &mut AppCtx, e: &CatalogEntry) {
        let p = pal(ui);
        let plugin = app.plugins.registry.get(&e.id).cloned();
        let manifest = plugin.as_ref().map(|x| x.manifest.clone());
        let is_inst = e.kinds.contains(&Kind::Instrument);
        let installed = is_installed(e);
        let active = plugin.as_ref().is_some_and(|x| x.active());
        if is_inst && active {
            self.refresh_callout(app, &e.id);
        }
        let frame = egui::Frame::new()
            .fill(p.bg_header)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(egui::Margin::same(20));
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 24.0;
                // art
                let art_size = vec2(240.0, 376.0);
                let panel = plugin
                    .as_ref()
                    .and_then(panel_of);
                match &panel {
                    Some(panel) => {
                        let state = instrument_on_bench(app, &e.id).and_then(|part| live_panel(app, &part));
                        let name = manifest
                            .as_ref()
                            .and_then(|m| m.provides.instrument.clone())
                            .unwrap_or_else(|| e.name.clone());
                        let inputs = instrument::instrument_widget(ui, panel, state.as_ref(), art_size, &name);
                        if !inputs.is_empty()
                            && let Some(part) = instrument_on_bench(app, &e.id)
                            && let Some(sim) = app.sim()
                        {
                            for (key, value) in inputs {
                                sim.send(bb_sim::Command::Input {
                                    part: part.clone(),
                                    input: bb_core::UserInput::Value { key, value },
                                });
                            }
                        }
                    }
                    None => art_tile(ui, e, art_size),
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 14.0;
                    let mut tag = format!("{} · {}", e.chip, status_word(e));
                    if plugin.as_ref().is_some_and(|x| x.source == Source::Local) {
                        tag.push_str(" · local");
                    }
                    widgets::kind_tag(ui, &tag);
                    ui.label(
                        egui::RichText::new(&e.name)
                            .font(font::heading(28.0))
                            .color(p.text),
                    );
                    let desc = manifest
                        .as_ref()
                        .map(|m| {
                            if m.description.is_empty() {
                                m.summary.clone()
                            } else {
                                m.description.clone()
                            }
                        })
                        .unwrap_or_else(|| e.summary.clone());
                    ui.label(
                        egui::RichText::new(instrument::glyph_safe(&desc))
                            .font(font::ui(14.0))
                            .color(Color32::from_rgb(0xC7, 0xC7, 0xC7))
                            .line_height(Some(22.0)),
                    );
                    // specs grid
                    if let Some(m) = &manifest {
                        let cells: Vec<(String, String)> = if m.highlights.is_empty() {
                            m.specs
                                .iter()
                                .take(4)
                                .map(|(k, v)| (k.replace('_', " "), instrument::glyph_safe(&spec_text(v))))
                                .collect()
                        } else {
                            m.highlights
                                .iter()
                                .map(|h| (h.label.clone(), instrument::glyph_safe(&h.value)))
                                .collect()
                        };
                        specs_grid(ui, &cells);
                    }
                    if let CatalogStatus::Error { message } = &e.status {
                        ui.label(
                            egui::RichText::new(message)
                                .font(font::mono(12.0))
                                .color(p.danger_soft),
                        );
                    }
                    if let CatalogStatus::Planned { note } = &e.status {
                        callout_box(ui, note, false);
                    }
                    // live loading callout
                    if is_inst && manifest.as_ref().is_some_and(|m| m.spice.load.is_some()) {
                        match self.callout.as_ref().filter(|c| c.0 == e.id) {
                            Some((_, text)) => callout_box(ui, text, true),
                            None => callout_box(
                                ui,
                                "Place it on a running bench and clip its probes onto two nodes: this shows how much its input impedance shifts the node, as it would on a real bench.",
                                false,
                            ),
                        }
                    }
                    // actions
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let placeable = active
                            && (is_inst || e.kinds.contains(&Kind::Part) || e.kinds.contains(&Kind::Board) || e.kinds.contains(&Kind::Radio))
                            && !app.plugins.host().parts_of(&e.id).is_empty();
                        if placeable && Btn::amber("Place on bench").show(ui).clicked() {
                            let id = e.id.clone();
                            self.place_on_bench(app, &id);
                        }
                        if installed && manifest.as_ref().is_some_and(|m| m.model_path().is_some() || m.provides.parts.is_some())
                            && Btn::ghost("Edit model").show(ui).clicked()
                        {
                            let id = e.id.clone();
                            self.edit_model(app, &id, true);
                        }
                        match &e.status {
                            CatalogStatus::Available => {
                                let r = Btn::amber("Install").show(ui);
                                let lbl = format!("Install {}", e.name);
                                r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &lbl));
                                if r.clicked() {
                                    self.request_install(app, &e.id);
                                }
                            }
                            CatalogStatus::NeedsApproval { missing } => {
                                if Btn::amber("Review permissions").show(ui).clicked() {
                                    self.prompt = Some(InstallPrompt {
                                        id: e.id.clone(),
                                        name: e.name.clone(),
                                        version: e.version.clone(),
                                        permissions: missing.clone(),
                                        source: InstallSource::Grant,
                                    });
                                }
                            }
                            CatalogStatus::UpdateAvailable { version }
                                if Btn::ghost(&format!("Update to {version}")).show(ui).clicked() => {
                                    self.request_install(app, &e.id);
                                }
                            _ => {}
                        }
                        if e.kinds.contains(&Kind::Theme) && active {
                            let on = app.plugins.theme.as_deref() == Some(e.id.as_str());
                            if Btn::ghost(if on { "Stop using theme" } else { "Apply theme" })
                                .show(ui)
                                .clicked()
                            {
                                let ctx = app.egui.clone();
                                app.plugins.apply_theme(&ctx, (!on).then_some(e.id.as_str()));
                                if on {
                                    // back to the user's own theme choice
                                    app.theme.apply(&ctx);
                                }
                            }
                        }
                        if e.kinds.contains(&Kind::Analysis) && active && app.project.is_some()
                            && Btn::ghost("Open in Pro → Analysis").show(ui).clicked()
                        {
                            app.navigate(Route::Pro(ProTab::Analysis));
                        }
                        if e.kinds.contains(&Kind::LessonPack) && active
                            && Btn::ghost("Open in Lessons").show(ui).clicked()
                        {
                            app.navigate(Route::Lessons);
                        }
                        if installed {
                            let enabled = !matches!(e.status, CatalogStatus::Disabled);
                            if Btn::plain(if enabled { "Disable" } else { "Enable" })
                                .show(ui)
                                .clicked()
                            {
                                match app.plugins.registry.set_enabled(&e.id, !enabled) {
                                    Ok(()) => app.plugins.changed(),
                                    Err(err) => app.toasts.error(format!("{err}")),
                                }
                            }
                            let r = Btn::plain("Uninstall").show(ui);
                            let lbl = format!("Uninstall {}", e.name);
                            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &lbl));
                            if r.clicked() {
                                self.confirm_uninstall = Some(e.id.clone());
                            }
                        }
                    });
                    if let Some(m) = &manifest
                        && !m.permissions.is_empty()
                    {
                        let perms = m
                            .permissions
                            .iter()
                            .map(|x| x.id())
                            .collect::<Vec<_>>()
                            .join(" · ");
                        ui.label(
                            egui::RichText::new(format!("Permissions: {perms}"))
                                .font(font::mono(11.0))
                                .color(p.text_faint),
                        );
                    }
                    if is_inst && !active && installed {
                        ui.label(
                            egui::RichText::new("Enable it to use it on the bench.")
                                .font(font::ui(12.0))
                                .color(p.text_muted),
                        );
                    }
                    if e.kinds.contains(&Kind::WorldModel) && active {
                        ui.label(
                            egui::RichText::new("Runs on every bench while it is enabled.")
                                .font(font::ui(12.0))
                                .color(p.text_muted),
                        );
                    }
                });
            });
        });
    }

    fn more_to_add(
        &mut self,
        ui: &mut Ui,
        app: &mut AppCtx,
        cat: &[CatalogEntry],
        featured: Option<&str>,
    ) {
        let p = pal(ui);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("More to add")
                    .font(font::heading(18.0))
                    .color(p.text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new("Every plugin is a sandboxed package: parts, instruments, world models or solvers")
                        .font(font::ui(12.0))
                        .color(p.text_muted),
                );
            });
        });
        ui.add_space(4.0);
        let mut cards: Vec<&CatalogEntry> = cat
            .iter()
            .filter(|e| Some(e.id.as_str()) != featured)
            .filter(|e| e.category != Category::Lessons)
            .collect();
        // the selected category first, then the catalog order
        cards.sort_by_key(|e| e.category != self.category);
        if cards.is_empty() {
            ui.label(
                egui::RichText::new("No plugins match your search.")
                    .font(font::ui(13.0))
                    .color(p.text_faint),
            );
            return;
        }
        let cols = 3usize;
        let gap = 14.0;
        let w = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
        for row in cards.chunks(cols) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for e in row {
                    self.card(ui, app, e, w);
                }
            });
            ui.add_space(gap - 6.0);
        }
    }

    fn card(&mut self, ui: &mut Ui, app: &mut AppCtx, e: &CatalogEntry, w: f32) {
        let p = pal(ui);
        egui::Frame::new()
            .fill(p.bg_card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(w - 30.0);
                    ui.set_min_height(128.0);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    widgets::kind_tag(ui, &e.chip);
                    let title = ui.add(
                        egui::Label::new(
                            egui::RichText::new(&e.name)
                                .font(font::heading(15.0))
                                .color(p.text),
                        )
                        .sense(Sense::click()),
                    );
                    let t = format!("Show {}", e.name);
                    title.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &t));
                    if title.clicked() {
                        self.selected = Some(e.id.clone());
                        self.category = e.category;
                    }
                    if title.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    ui.label(
                        egui::RichText::new(instrument::glyph_safe(&e.summary))
                            .font(font::ui(13.0))
                            .color(p.text_muted)
                            .line_height(Some(19.5)),
                    );
                    let (label, enabled) = match &e.status {
                        CatalogStatus::Available => ("Install", true),
                        CatalogStatus::Installed => ("Installed", false),
                        CatalogStatus::Disabled => ("Enable", true),
                        CatalogStatus::NeedsApproval { .. } => ("Approve…", true),
                        CatalogStatus::UpdateAvailable { .. } => ("Update", true),
                        CatalogStatus::Planned { .. } => ("Not in this version", false),
                        CatalogStatus::Error { .. } => ("Unavailable", false),
                    };
                    let r = Btn::ghost(label).enabled(enabled).show(ui);
                    let a11y = format!("{label} {}", e.name);
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &a11y));
                    if let CatalogStatus::Planned { note } = &e.status {
                        r.clone().on_hover_text(note);
                    }
                    if r.clicked() {
                        match &e.status {
                            CatalogStatus::Available | CatalogStatus::UpdateAvailable { .. } => {
                                self.request_install(app, &e.id)
                            }
                            CatalogStatus::Disabled => {
                                if app.plugins.registry.set_enabled(&e.id, true).is_ok() {
                                    app.plugins.changed();
                                }
                            }
                            CatalogStatus::NeedsApproval { missing } => {
                                self.prompt = Some(InstallPrompt {
                                    id: e.id.clone(),
                                    name: e.name.clone(),
                                    version: e.version.clone(),
                                    permissions: missing.clone(),
                                    source: InstallSource::Grant,
                                })
                            }
                            _ => {}
                        }
                    }
                });
            });
    }

    fn manifest_pane(&mut self, ui: &mut Ui, app: &AppCtx, e: Option<&CatalogEntry>) {
        let p = pal(ui);
        let r = ui.max_rect();
        let head = Rect::from_min_size(r.min, vec2(r.width(), 38.0));
        ui.painter().rect_filled(head, 0.0, p.bg_header);
        ui.painter().line_segment(
            [head.left_bottom(), head.right_bottom()],
            Stroke::new(1.0, p.border),
        );
        ui.painter().text(
            pos2(head.left() + 14.0, head.center().y),
            Align2::LEFT_CENTER,
            "plugin.json",
            font::ui(12.0),
            p.text_strong,
        );
        ui.painter().text(
            pos2(head.right() - 14.0, head.center().y),
            Align2::RIGHT_CENTER,
            "read-only",
            font::ui(12.0),
            p.text_muted,
        );
        let foot_h = 64.0;
        let foot = Rect::from_min_max(pos2(r.left(), r.bottom() - foot_h), r.max);
        ui.painter().line_segment(
            [foot.left_top(), foot.right_top()],
            Stroke::new(1.0, p.border),
        );
        let mut fc = widgets::child_ui(ui, foot.shrink2(vec2(16.0, 14.0)));
        fc.label(
            egui::RichText::new(
                "Plugins run sandboxed and only touch the circuit through the permissions they declare.",
            )
            .font(font::ui(12.0))
            .color(p.text_muted)
            .line_height(Some(19.0)),
        );
        let body = Rect::from_min_max(pos2(r.left(), head.bottom()), pos2(r.right(), foot.top()));
        let mut c = widgets::child_ui(ui, body.shrink2(vec2(16.0, 14.0)));
        let text = e
            .and_then(|e| app.plugins.registry.get(&e.id))
            .map(|pl| pl.manifest_text())
            .unwrap_or_else(|| match e {
                Some(e) => format!(
                    "{{\n  \"id\": \"{}\",\n  \"name\": \"{}\",\n  \"kind\": [\"{}\"]\n}}",
                    e.id,
                    e.name,
                    e.chip.to_ascii_lowercase().replace(' ', "-")
                ),
                None => "{}".into(),
            });
        egui::ScrollArea::both()
            .id_salt("plugin-json")
            .auto_shrink([false, false])
            .show(&mut c, |ui| {
                // characters the code font can't draw are shown the way JSON may spell
                // them (`⎓` → `\u2393`) instead of as tofu boxes
                let mono = crate::theme::font::mono(12.0);
                let text: String = ui.ctx().fonts_mut(|f| {
                    text.chars()
                        .map(|c| {
                            if c.is_ascii() || f.has_glyph(&mono, c) {
                                c.to_string()
                            } else {
                                format!("\\u{:04x}", c as u32)
                            }
                        })
                        .collect()
                });
                let job = markdown::json_job(text.trim_end(), p);
                let resp = ui.add(egui::Label::new(job).extend());
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Label, true, "plugin.json")
                });
            });
    }

    fn prompts(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        if let Some(pr) = self.prompt.clone() {
            let mut confirm = false;
            let title = match pr.source {
                InstallSource::Folder(_) => format!("Load {}?", pr.name),
                InstallSource::Grant => format!("Approve {}?", pr.name),
                _ => format!("Install {}?", pr.name),
            };
            let res = widgets::modal(ui.ctx(), "plugin-permissions", &title, 440.0, |ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                if pr.permissions.is_empty() {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {} needs no permissions: it cannot read or change your circuit.",
                            pr.name, pr.version
                        ))
                        .font(font::ui(13.0))
                        .color(p.text_dim),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(format!("{} {} asks to:", pr.name, pr.version))
                            .font(font::ui(13.0))
                            .color(p.text_dim),
                    );
                    for perm in &pr.permissions {
                        widgets::inner_card_frame(p, false).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(perm.id())
                                        .font(font::mono_medium(12.0))
                                        .color(p.accent_soft),
                                );
                                ui.label(
                                    egui::RichText::new(perm.describe())
                                        .font(font::ui(12.5))
                                        .color(p.text_dim),
                                );
                            });
                        });
                    }
                }
                ui.label(
                    egui::RichText::new(
                        "Plugins run sandboxed and only touch the circuit through the permissions they declare.",
                    )
                    .font(font::ui(12.0))
                    .color(p.text_muted),
                );
                ui.horizontal(|ui| {
                    let label = match pr.source {
                        InstallSource::Folder(_) => "Allow and load",
                        InstallSource::Grant => "Allow",
                        _ => "Allow and install",
                    };
                    if Btn::amber(label).show(ui).clicked() {
                        confirm = true;
                    }
                    if Btn::ghost("Cancel").show(ui).clicked() {
                        self.prompt = None;
                    }
                });
            });
            if confirm {
                self.confirm_install(app);
            } else if res.close {
                self.prompt = None;
            }
        }
        if let Some(id) = self.confirm_uninstall.clone() {
            let name = app
                .plugins
                .registry
                .get(&id)
                .map(|x| x.manifest.display_name())
                .unwrap_or_else(|| id.clone());
            let mut go = false;
            let res = widgets::modal(
                ui.ctx(),
                "plugin-uninstall",
                &format!("Uninstall {name}?"),
                380.0,
                |ui| {
                    ui.label(
                    egui::RichText::new("Benches that use its parts show them as unknown until you install it again.")
                        .font(font::ui(13.0))
                        .color(p.text_dim),
                );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if Btn::danger("Uninstall").show(ui).clicked() {
                            go = true;
                        }
                        if Btn::ghost("Cancel").show(ui).clicked() {
                            self.confirm_uninstall = None;
                        }
                    });
                },
            );
            if go {
                self.confirm_uninstall = None;
                self.uninstall(app, &id);
                if self.selected.as_deref() == Some(id.as_str()) {
                    self.selected = None;
                }
            } else if res.close {
                self.confirm_uninstall = None;
            }
        }
    }

    fn docs_view(&mut self, ui: &mut Ui, rect: Rect) {
        let p = pal(ui);
        let Some(text) = self.docs.clone() else {
            return;
        };
        let mut c = widgets::child_ui(ui, rect.shrink2(vec2(28.0, 20.0)));
        c.horizontal(|ui| {
            if Btn::ghost("Back to plugins")
                .icon(Icon::ChevronLeft)
                .show(ui)
                .clicked()
            {
                self.docs = None;
            }
            ui.label(
                egui::RichText::new("docs/PLUGIN-SDK.md")
                    .font(font::mono(12.0))
                    .color(p.text_muted),
            );
        });
        c.add_space(8.0);
        egui::ScrollArea::vertical()
            .id_salt("sdk-docs")
            .auto_shrink([false, false])
            .show(&mut c, |ui| {
                ui.set_max_width(820.0);
                markdown::show(ui, &text);
            });
    }
}

/// The instrument part of a plugin currently on the open bench.
fn instrument_on_bench(app: &mut AppCtx, plugin: &str) -> Option<String> {
    let parts = app.plugins.host().parts_of(plugin);
    let doc = app.circuit.as_ref()?;
    doc.project()
        .parts
        .iter()
        .find(|p| parts.contains(&p.part))
        .map(|p| p.id.clone())
}

fn live_panel(app: &mut AppCtx, part: &str) -> Option<PanelState> {
    let tel = app.bench.sim()?.telemetry()?;
    tel.parts
        .iter()
        .find(|p| p.part == part)
        .and_then(|p| p.panel.as_ref())
        .and_then(PanelState::from_json)
}

/// A plugin's panel (installed or not: read from its folder).
fn panel_of(pl: &bb_plugin::Plugin) -> Option<Panel> {
    let rel = pl.manifest.provides.panel.as_ref()?;
    let path = pl.asset(rel)?;
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn spec_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a.iter().map(spec_text).collect::<Vec<_>>().join(" / "),
        other => other.to_string(),
    }
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let name = e.file_name();
        let s = name.to_string_lossy();
        if s.starts_with('.') || s == "target" {
            continue;
        }
        let dst = to.join(&name);
        if e.path().is_dir() {
            copy_dir(&e.path(), &dst)?;
        } else {
            std::fs::copy(e.path(), dst)?;
        }
    }
    Ok(())
}

fn section(ui: &mut Ui, text: &str, top: f32) {
    let p = pal(ui);
    ui.add_space(top);
    let g = widgets::galley_spaced(
        ui,
        &text.to_uppercase(),
        font::semibold(11.0),
        p.text_muted,
        0.9,
    );
    let (r, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), g.size().y + 10.0),
        Sense::hover(),
    );
    ui.painter()
        .galley(pos2(r.left() + 10.0, r.top()), g, p.text_muted);
}

/// A `.cat` row: 7 / 10 px padding, radius 6, `#1F1F1F` when selected.
fn nav_row(ui: &mut Ui, text: &str, on: bool, a11y: &str) -> egui::Response {
    let p = pal(ui);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    if on {
        ui.painter()
            .rect_filled(r, CornerRadius::same(6), p.bg_row_selected);
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(r, CornerRadius::same(6), p.bg_hover);
    }
    ui.painter().text(
        pos2(r.left() + 10.0, r.center().y),
        Align2::LEFT_CENTER,
        text,
        font::ui(13.0),
        p.text_dim,
    );
    let a = a11y.to_string();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, &a));
    resp
}

fn specs_grid(ui: &mut Ui, cells: &[(String, String)]) {
    let p = pal(ui);
    if cells.is_empty() {
        return;
    }
    let gap = 10.0;
    let avail = ui.available_width();
    let cols = ((avail + gap) / (150.0 + gap)).floor().clamp(1.0, 3.0) as usize;
    let w = (avail - gap * (cols as f32 - 1.0)) / cols as f32;
    for row in cells.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (k, v) in row {
                let (r, resp) = ui.allocate_exact_size(vec2(w, 56.0), Sense::hover());
                let a = format!("{k}: {v}");
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &a));
                ui.painter().rect_stroke(
                    r,
                    CornerRadius::same(8),
                    Stroke::new(1.0, p.border),
                    StrokeKind::Inside,
                );
                ui.painter().text(
                    pos2(r.left() + 12.0, r.top() + 11.0),
                    Align2::LEFT_TOP,
                    k,
                    font::ui(11.0),
                    p.text_muted,
                );
                ui.painter().text(
                    pos2(r.left() + 12.0, r.top() + 29.0),
                    Align2::LEFT_TOP,
                    v,
                    font::mono_medium(14.0),
                    p.text,
                );
            }
        });
    }
}

/// The amber monospace callout (`Probing A0 (J1 out): …`); `live` = false is a muted hint.
fn callout_box(ui: &mut Ui, text: &str, live: bool) {
    let p = pal(ui);
    let (fill, stroke, color) = if live {
        (
            p.accent_bg2,
            Color32::from_rgb(0x4A, 0x36, 0x14),
            p.accent_soft,
        )
    } else {
        (p.bg_card, p.border, p.text_muted)
    };
    let resp = egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                egui::RichText::new(text)
                    .font(font::mono(12.5))
                    .color(color)
                    .line_height(Some(20.0)),
            );
        });
    let t = text.to_string();
    resp.response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &t));
}

/// Art for plugins without a front panel: a large icon tile.
fn art_tile(ui: &mut Ui, e: &CatalogEntry, size: egui::Vec2) {
    let p = pal(ui);
    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
    let tile = Rect::from_center_size(r.center() - vec2(0.0, 40.0), vec2(200.0, 200.0));
    ui.painter().rect(
        tile,
        CornerRadius::same(24),
        p.bg_card,
        Stroke::new(1.0, p.border),
        StrokeKind::Inside,
    );
    let icon = match e.kinds.first() {
        Some(Kind::Instrument) => Icon::Multimeter,
        Some(Kind::WorldModel) => Icon::World,
        Some(Kind::Radio) => Icon::Radio,
        Some(Kind::Analysis) => Icon::Analysis,
        Some(Kind::Theme) => Icon::Inspector,
        Some(Kind::LessonPack) => Icon::Book,
        Some(Kind::Board) => Icon::Chip,
        Some(Kind::Generator) => Icon::Waveform,
        _ => Icon::Plug,
    };
    icon.paint_with(ui.painter(), tile.shrink(56.0), p.accent_soft, p.accent);
}

impl Screen for PluginsScreen {
    fn crumb(&self, _app: &AppCtx) -> Option<String> {
        Some("Plugins".into())
    }

    fn header(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        ui.add_space(16.0);
        let edit = egui::TextEdit::singleline(&mut self.search)
            .hint_text(
                egui::RichText::new("Search instruments, parts, world models…")
                    .font(font::ui(13.0))
                    .color(p.text_faint),
            )
            .font(font::ui(13.0))
            .margin(egui::Margin::symmetric(10, 8))
            .desired_width(318.0);
        let r = ui.add(edit);
        r.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::SearchInput, true, "Search plugins")
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if Btn::ghost("Plugin SDK docs").show(ui).clicked() {
                self.docs = if self.docs.is_some() {
                    None
                } else {
                    Some(sdk_doc(app))
                };
            }
            let load = Btn::ghost("Load local plugin…").show(ui);
            egui::Popup::menu(&load).show(|ui| {
                ui.set_min_width(220.0);
                if widgets::menu_item(ui, "Plugin folder…", false, true) {
                    self.load_local_dialog(app, true);
                }
                if widgets::menu_item(ui, ".bbplugin package…", false, true) {
                    self.load_local_dialog(app, false);
                }
            });
        });
    }

    fn ui(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        if !self.theme_restored {
            self.theme_restored = true;
            if let Some(t) = self.saved_theme.clone() {
                let ctx = app.egui.clone();
                app.plugins.apply_theme(&ctx, Some(&t));
            }
        }
        let p = pal(ui);
        let full = ui.max_rect();
        let left = Rect::from_min_size(full.min, vec2(220.0, full.height()));
        let right_w = 356.0;
        let right = Rect::from_min_max(pos2(full.right() - right_w, full.top()), full.max);
        let main = Rect::from_min_max(
            pos2(left.right(), full.top()),
            pos2(right.left(), full.bottom()),
        );
        // panels
        ui.painter()
            .rect_filled(left, 0.0, Color32::from_rgba_unmultiplied(17, 17, 17, 214));
        ui.painter().line_segment(
            [left.right_top(), left.right_bottom()],
            Stroke::new(1.0, p.border),
        );
        ui.painter()
            .rect_filled(right, 0.0, Color32::from_rgba_unmultiplied(6, 6, 6, 224));
        ui.painter().line_segment(
            [right.left_top(), right.left_bottom()],
            Stroke::new(1.0, p.border),
        );
        let cat = self.catalog(app);
        // nav
        let mut nav = widgets::child_ui(ui, left.shrink2(vec2(10.0, 14.0)));
        egui::ScrollArea::vertical()
            .id_salt("plugins-nav")
            .auto_shrink([false, false])
            .show(&mut nav, |ui| self.nav(ui, app, &cat));
        let featured = self.featured(&cat).cloned();
        // manifest pane
        let mut rc = widgets::child_ui(ui, right);
        self.manifest_pane(&mut rc, app, featured.as_ref());
        // main
        if self.docs.is_some() {
            self.docs_view(ui, main);
        } else {
            let mut mc = widgets::child_ui(ui, main);
            egui::ScrollArea::vertical()
                .id_salt("plugins-main")
                .auto_shrink([false, false])
                .show(&mut mc, |ui| {
                    egui::Frame::new()
                        .inner_margin(egui::Margin::same(20))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 20.0;
                            match &featured {
                                Some(e) => self.detail(ui, app, e),
                                None => {
                                    ui.label(
                                        egui::RichText::new("No plugins match your search.")
                                            .font(font::ui(13.0))
                                            .color(p.text_faint),
                                    );
                                }
                            }
                            self.more_to_add(
                                ui,
                                app,
                                &cat,
                                featured.as_ref().map(|e| e.id.as_str()),
                            );
                        });
                });
        }
        self.prompts(ui, app);
        self.saved_theme = app.plugins.theme.clone();
        // a live front panel / callout keeps updating while the bench runs
        if app.bench.running {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    fn save_state(&self) -> Option<serde_json::Value> {
        serde_json::to_value(PluginsState {
            category: Some(format!("{:?}", self.category)),
            selected: self.selected.clone(),
            theme: self.saved_theme.clone(),
        })
        .ok()
    }

    fn load_state(&mut self, state: &serde_json::Value) {
        if let Ok(s) = serde_json::from_value::<PluginsState>(state.clone()) {
            self.selected = s.selected;
            self.saved_theme = s.theme;
            if let Some(c) = s.category {
                self.category = Category::NAV
                    .into_iter()
                    .find(|x| format!("{x:?}") == c)
                    .unwrap_or(Category::Instruments);
            }
        }
    }
}
