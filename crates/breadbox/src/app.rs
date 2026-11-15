//! The eframe app: shared services ([`AppCtx`]), the top bar (`APP-1`), screen routing,
//! menu + keyboard commands (`APP-2`, `APP-4`), persistence (`APP-5`).

use crate::bench::avr::AvrBench;
use crate::bench::service::BenchService;
use crate::bench::{BenchFactory, BenchLink, Repaint, SimSpeed};
use crate::platform;
use crate::project::{self, ExampleInfo, Project, ProjectMode, RecentProject, Resources};
use crate::router::{Command, ProTab, Route, ScreenRegistry, ScreenSlot};
use crate::screens;
use crate::theme::{self, ThemeChoice, font, pal};
use crate::toolchain::ToolchainService;
use crate::widgets::{self, Btn, Icon, Toasts, Tone};
use eframe::egui::{
    self, Color32, CornerRadius, Key, KeyboardShortcut, Modifiers, Rect, Sense, Stroke, Ui, pos2,
    vec2,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Storage key for [`PersistedState`].
pub const STORAGE_KEY: &str = "breadbox-app";

/// How the app is started (real app vs. GUI tests).
#[derive(Clone)]
pub struct AppOptions {
    pub load_persisted: bool,
    pub native_menu: bool,
    pub detect_toolchain: bool,
    pub resources: Option<Resources>,
    pub bench_factory: Option<BenchFactory>,
    pub sim_speed: SimSpeed,
}

impl Default for AppOptions {
    fn default() -> Self {
        AppOptions {
            load_persisted: true,
            native_menu: true,
            detect_toolchain: true,
            resources: None,
            bench_factory: None,
            sim_speed: SimSpeed::RealTime(1.0),
        }
    }
}

impl AppOptions {
    /// Deterministic options for `egui_kittest`: no persistence, no native menu, the bench
    /// runs as fast as possible.
    pub fn test() -> Self {
        AppOptions {
            load_persisted: false,
            native_menu: false,
            detect_toolchain: true,
            resources: None,
            bench_factory: None,
            sim_speed: SimSpeed::Max,
        }
    }
}

/// What survives a relaunch.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PersistedState {
    pub route: Route,
    pub theme: ThemeChoice,
    pub recent: Vec<RecentProject>,
    pub last_project: Option<PathBuf>,
    pub screens: HashMap<String, serde_json::Value>,
}

/// Services shared by all screens.
pub struct AppCtx {
    pub egui: egui::Context,
    pub route: Route,
    pub project: Option<Project>,
    pub resources: Resources,
    pub examples: Vec<ExampleInfo>,
    pub recent: Vec<RecentProject>,
    pub toasts: Toasts,
    pub toolchain: ToolchainService,
    pub bench: BenchService,
    pub theme: ThemeChoice,
    pub repaint: Repaint,
    /// Bumped whenever the open project changes.
    pub project_generation: u64,
    pending: Vec<Command>,
    nav: Option<Route>,
    native_menu: bool,
}

impl AppCtx {
    pub fn navigate(&mut self, route: Route) {
        self.nav = Some(route);
    }

    /// Queues a command (handled after the current frame's UI).
    pub fn command(&mut self, cmd: Command) {
        self.pending.push(cmd);
    }

    /// Opens a project folder.
    pub fn open_project(&mut self, dir: &Path) -> anyhow::Result<()> {
        let mut p = Project::load(dir)?;
        p.read_only = self.resources.is_example(dir);
        self.set_project(Some(p));
        Ok(())
    }

    pub fn set_project(&mut self, p: Option<Project>) {
        self.bench.shutdown();
        if let Some(p) = &p {
            project::touch_recent(&mut self.recent, p);
        }
        self.project = p;
        self.project_generation += 1;
    }

    /// The bench link for the open project (created on first use).
    pub fn bench_link(&mut self) -> &mut dyn BenchLink {
        self.bench.link(self.project.as_ref(), &self.repaint)
    }

    /// True if keyboard focus is in a text field (editor shortcuts then belong to it).
    pub fn text_has_focus(&self) -> bool {
        self.egui.memory(|m| m.focused().is_some()) && self.egui.egui_wants_keyboard_input()
    }

    pub fn has_native_menu(&self) -> bool {
        self.native_menu
    }
}

pub struct BreadboxApp {
    pub ctx: AppCtx,
    pub screens: ScreenRegistry,
    menu: Option<platform::NativeMenu>,
    /// Project generation each screen last saw.
    seen: HashMap<ScreenSlot, u64>,
    entered: Option<Route>,
    /// Key events to inject next frame (menu Undo/Redo → text editors).
    inject: Vec<egui::Event>,
    first_frame: bool,
}

impl BreadboxApp {
    pub fn new(cc: &eframe::CreationContext<'_>, opts: AppOptions) -> BreadboxApp {
        let persisted = if opts.load_persisted {
            cc.storage
                .and_then(|s| eframe::get_value::<PersistedState>(s, STORAGE_KEY))
                .unwrap_or_default()
        } else {
            PersistedState::default()
        };
        Self::with_context(&cc.egui_ctx, opts, persisted)
    }

    /// Builds the app on an existing egui context (tests, previews).
    pub fn with_context(ctx: &egui::Context, opts: AppOptions, persisted: PersistedState) -> Self {
        theme::install(ctx);
        persisted.theme.apply(ctx);
        let ectx = ctx.clone();
        let repaint: Repaint = Arc::new(move || ectx.request_repaint());
        let resources = opts.resources.clone().unwrap_or_else(Resources::locate);
        let examples = resources
            .examples()
            .map(|d| project::scan_examples(&d))
            .unwrap_or_default();
        let factory = opts.bench_factory.clone().unwrap_or_else(AvrBench::factory);
        let menu = if opts.native_menu {
            platform::NativeMenu::install(ctx)
        } else {
            None
        };
        let mut app_ctx = AppCtx {
            egui: ctx.clone(),
            route: Route::Home,
            project: None,
            examples,
            recent: persisted.recent.clone(),
            toasts: Toasts::default(),
            toolchain: ToolchainService::new(repaint.clone(), opts.detect_toolchain),
            bench: BenchService::new(factory, opts.sim_speed),
            theme: persisted.theme,
            repaint,
            project_generation: 0,
            pending: Vec::new(),
            nav: None,
            native_menu: menu.is_some(),
            resources,
        };
        let mut screens = screens::default_registry();
        for slot in ScreenSlot::ALL {
            if let (Some(s), Some(v)) = (screens.get_mut(slot), persisted.screens.get(slot.key())) {
                s.load_state(v);
            }
        }
        // restore the last project + screen
        if let Some(dir) = &persisted.last_project
            && dir.join("project.json").exists()
            && let Err(e) = app_ctx.open_project(dir)
        {
            log::warn!("could not reopen {}: {e:#}", dir.display());
        }
        let route = persisted.route;
        app_ctx.route = if route.needs_project() && app_ctx.project.is_none() {
            Route::Home
        } else {
            route
        };
        BreadboxApp {
            ctx: app_ctx,
            screens,
            menu,
            seen: HashMap::new(),
            entered: None,
            inject: Vec::new(),
            first_frame: true,
        }
    }

    pub fn persisted(&self) -> PersistedState {
        PersistedState {
            route: self.ctx.route,
            theme: self.ctx.theme,
            recent: self.ctx.recent.clone(),
            last_project: self.ctx.project.as_ref().map(|p| p.dir.clone()),
            screens: self
                .screens
                .slots()
                .filter_map(|s| {
                    self.screens
                        .get(s)
                        .and_then(|x| x.save_state())
                        .map(|v| (s.key().to_string(), v))
                })
                .collect(),
        }
    }

    /// The whole frame (also used directly by tests).
    pub fn frame(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if std::mem::take(&mut self.first_frame)
            && let Some(t0) = crate::STARTED.get()
        {
            log::info!("first frame {} ms after start", t0.elapsed().as_millis());
        }
        self.ctx.toolchain.poll();
        self.ctx.bench.poll();
        if let Some(m) = &self.menu {
            for cmd in m.take_commands() {
                self.ctx.pending.push(cmd);
            }
        }
        self.shortcuts(&ctx);
        self.route_changes();

        let slot = self.ctx.route.slot();
        // project change notifications (lazily, for the visible screen)
        let generation = self.ctx.project_generation;
        if self.seen.get(&slot) != Some(&generation) {
            self.seen.insert(slot, generation);
            if let Some(s) = self.screens.get_mut(slot) {
                s.project_changed(&mut self.ctx);
            }
        }
        if self.entered != Some(self.ctx.route) {
            self.entered = Some(self.ctx.route);
            if let Some(s) = self.screens.get_mut(slot) {
                s.on_enter(&mut self.ctx);
            }
        }

        let p = pal(ui);
        egui::Panel::top("top-bar")
            .exact_size(theme::size::HEADER)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(p.bg_header)
                    .inner_margin(egui::Margin::symmetric(16, 0)),
            )
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().line_segment(
                    [
                        pos2(r.left() - 16.0, r.bottom() - 0.5),
                        pos2(r.right() + 16.0, r.bottom() - 0.5),
                    ],
                    Stroke::new(1.0, p.border),
                );
                ui.horizontal_centered(|ui| self.top_bar(ui));
            });
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let r = ui.max_rect();
            theme::paint_paper(ui.painter(), r, p);
            if let Some(s) = self.screens.get_mut(slot) {
                s.ui(ui, &mut self.ctx);
            }
        });
        self.process_commands();
        self.route_changes();
        self.ctx.toasts.show(&ctx);
        if let Some(m) = &self.menu {
            m.sync(&self.ctx);
        }
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        ui.spacing_mut().item_spacing.x = 10.0;
        // logo + wordmark → Home
        let (lr, logo_resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
        widgets::paint_logo(ui.painter(), lr);
        let wm = widgets::galley(ui, "Breadbox", font::brand(16.0), p.text);
        let (wr, wm_resp) = ui.allocate_exact_size(wm.size(), Sense::click());
        ui.painter().galley(wr.min, wm, p.text);
        wm_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, "Breadbox"));
        if logo_resp.clicked() || wm_resp.clicked() {
            self.ctx.navigate(Route::Home);
        }
        let _ = wm_resp.on_hover_cursor(egui::CursorIcon::PointingHand);

        let slot = self.ctx.route.slot();
        let crumb = self.screens.get(slot).and_then(|s| s.crumb(&self.ctx));
        if self.ctx.route != Route::Home {
            ui.add_space(4.0);
            if let Some(proj) = &self.ctx.project {
                slash(ui, p);
                ui.label(
                    egui::RichText::new(&proj.name)
                        .font(font::ui(13.0))
                        .color(p.text_dim),
                );
            }
            // section crumb with a screen switcher
            let section = crumb.unwrap_or_else(|| self.ctx.route.title().to_string());
            if self.ctx.route.slot() != ScreenSlot::ProAnalysis
                && self.ctx.route.slot() != ScreenSlot::ProSchematic
                && self.ctx.route.slot() != ScreenSlot::ProPcb
            {
                slash(ui, p);
            }
            let is_pro = matches!(self.ctx.route, Route::Pro(_));
            if !is_pro {
                self.section_switcher(ui, &section);
            }
            let mode = if is_pro {
                Some(ProjectMode::Pro)
            } else {
                self.ctx.project.as_ref().map(|p| p.mode)
            };
            if let Some(m) = mode {
                let tone = if m == ProjectMode::Pro {
                    Tone::Info
                } else {
                    Tone::Neutral
                };
                widgets::tag(ui, m.label(), tone);
            }
            if is_pro {
                ui.add_space(12.0);
                let mut tab = match self.ctx.route {
                    Route::Pro(t) => t,
                    _ => ProTab::Analysis,
                };
                if widgets::segmented(
                    ui,
                    "Workspace",
                    &mut tab,
                    &[
                        widgets::Seg::new(ProTab::Schematic, "Schematic"),
                        widgets::Seg::new(ProTab::Analysis, "Analysis"),
                        widgets::Seg::new(ProTab::Pcb, "PCB"),
                    ],
                ) {
                    self.ctx.navigate(Route::Pro(tab));
                }
                self.section_menu_button(ui);
            }
        }
        if let Some(s) = self.screens.get_mut(slot) {
            s.header(ui, &mut self.ctx);
        }
    }

    /// The breadcrumb's section text; clicking it opens the screen switcher.
    fn section_switcher(&mut self, ui: &mut Ui, section: &str) {
        let p = pal(ui);
        let g = widgets::galley(ui, section, font::ui(13.0), p.text_dim);
        let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 18.0, 26.0), Sense::click());
        if resp.hovered() {
            ui.painter()
                .rect_filled(r.expand2(vec2(6.0, 0.0)), CornerRadius::same(6), p.bg_hover);
        }
        ui.painter().galley(
            pos2(r.left(), r.center().y - g.size().y / 2.0),
            g,
            p.text_dim,
        );
        Icon::ChevronDown.paint(
            ui.painter(),
            Rect::from_center_size(pos2(r.right() - 6.0, r.center().y), vec2(11.0, 11.0)),
            p.text_faint,
        );
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Switch screen"));
        self.screen_menu(&resp);
    }

    fn section_menu_button(&mut self, ui: &mut Ui) {
        let resp = Btn::plain("Switch screen")
            .icon_only(Icon::ChevronDown)
            .height(28.0)
            .show(ui);
        self.screen_menu(&resp);
    }

    fn screen_menu(&mut self, resp: &egui::Response) {
        let mut go = None;
        let current = self.ctx.route;
        egui::Popup::menu(resp).show(|ui| {
            ui.set_min_width(180.0);
            for r in Route::SECTIONS {
                let on = std::mem::discriminant(&r) == std::mem::discriminant(&current);
                let icon = match r {
                    Route::Workbench => Icon::Breadboard,
                    Route::Firmware => Icon::Chip,
                    Route::Pro(_) => Icon::Analysis,
                    Route::Plugins => Icon::Plug,
                    _ => Icon::Book,
                };
                if Btn::plain(r.title())
                    .icon(icon)
                    .selected(on)
                    .full_width()
                    .show(ui)
                    .clicked()
                {
                    go = Some(r);
                }
            }
            ui.separator();
            if Btn::plain("Home")
                .icon(Icon::Home)
                .full_width()
                .show(ui)
                .clicked()
            {
                go = Some(Route::Home);
            }
        });
        if let Some(r) = go {
            self.ctx.navigate(r);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let text_focus = self.ctx.text_has_focus();
        let mut push = |c| self.ctx.pending.push(c);
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let consume = |m: Modifiers, k: Key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)))
        };
        // ⌘⇧ variants first (consume_shortcut ignores extra shift otherwise)
        if consume(cmd_shift, Key::S) {
            push(Command::SaveAs);
        }
        if !text_focus && consume(cmd_shift, Key::Z) {
            push(Command::Redo);
        }
        if consume(cmd, Key::N) {
            push(Command::NewProject(ProjectMode::Playground));
        }
        if consume(cmd, Key::O) {
            push(Command::OpenProject);
        }
        if consume(cmd, Key::S) {
            push(Command::Save);
        }
        if consume(cmd, Key::W) {
            push(Command::Close);
        }
        if !text_focus && consume(cmd, Key::Z) {
            push(Command::Undo);
        }
        if consume(cmd, Key::B) {
            push(Command::Verify);
        }
        if consume(cmd, Key::U) {
            push(Command::Upload);
        }
        if consume(cmd, Key::R) {
            push(Command::RunPause);
        }
        if consume(cmd, Key::Num1) {
            push(Command::Navigate(Route::Workbench));
        }
        if consume(cmd, Key::Num2) {
            push(Command::Navigate(Route::Firmware));
        }
        if consume(cmd, Key::Num3) {
            push(Command::Navigate(Route::Pro(ProTab::Analysis)));
        }
        if consume(cmd, Key::Num4) {
            push(Command::Navigate(Route::Plugins));
        }
        if consume(cmd, Key::Num5) {
            push(Command::Navigate(Route::Lessons));
        }
        if consume(cmd_shift, Key::H) {
            push(Command::Navigate(Route::Home));
        }
    }

    fn process_commands(&mut self) {
        let cmds = std::mem::take(&mut self.ctx.pending);
        for c in cmds {
            let slot = self.ctx.route.slot();
            // the visible screen gets the first chance
            if let Some(s) = self.screens.get_mut(slot)
                && s.command(&c, &mut self.ctx)
            {
                continue;
            }
            self.app_command(c);
        }
    }

    fn app_command(&mut self, c: Command) {
        match c {
            Command::NewProject(mode) => {
                if let Some(s) = self.screens.get_mut(ScreenSlot::Home) {
                    self.ctx.navigate(Route::Home);
                    s.command(&Command::NewProject(mode), &mut self.ctx);
                }
            }
            Command::OpenProject => {
                if let Some(dir) = platform::pick_folder("Open a Breadbox project") {
                    self.open_path(&dir);
                }
            }
            Command::OpenPath(dir) => self.open_path(&dir),
            Command::Close => {
                if self.ctx.project.is_some() {
                    self.ctx.set_project(None);
                    self.ctx.navigate(Route::Home);
                } else {
                    self.ctx
                        .egui
                        .send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Command::Navigate(r) => {
                if r.needs_project() && self.ctx.project.is_none() {
                    self.ctx.toasts.info("Open a project or an example first.");
                    self.ctx.navigate(Route::Home);
                } else {
                    self.ctx.navigate(r);
                }
            }
            Command::SetTheme(t) => {
                self.ctx.theme = t;
                t.apply(&self.ctx.egui);
            }
            Command::Undo => self.inject_key(Key::Z, Modifiers::COMMAND),
            Command::Redo => self.inject_key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT),
            Command::Find => self.inject_key(Key::F, Modifiers::COMMAND),
            Command::About => self.ctx.toasts.info(format!(
                "Breadbox {} — a circuit playground and SPICE workbench.",
                env!("CARGO_PKG_VERSION")
            )),
            Command::OpenDocs => {
                if let Some(root) = &self.ctx.resources.root {
                    let doc = root.join("docs");
                    if doc.is_dir() {
                        self.ctx
                            .egui
                            .open_url(egui::OpenUrl::new_tab(format!("file://{}", doc.display())));
                    }
                }
            }
            Command::Save | Command::SaveAs => {
                if let Some(p) = &self.ctx.project
                    && (p.read_only || c == Command::SaveAs)
                {
                    let suggested = format!("{}-copy.breadbox", p.name);
                    if let Some(dest) = platform::save_folder("Save project as", &suggested) {
                        match p.save_as(&dest) {
                            Ok(np) => {
                                self.ctx
                                    .toasts
                                    .success(format!("Saved a copy to {}", np.dir.display()));
                                let route = self.ctx.route;
                                self.ctx.set_project(Some(np));
                                self.ctx.navigate(route);
                            }
                            Err(e) => self.ctx.toasts.error(format!("{e:#}")),
                        }
                    }
                } else if self.ctx.project.is_some() {
                    self.ctx.toasts.success("Saved");
                }
            }
            Command::RunPause => {
                if self.ctx.project.is_some() {
                    let running = self.ctx.bench.running;
                    self.ctx.bench_link().set_running(!running);
                }
            }
            Command::ResetSim => {
                if let Some(l) = self.ctx.bench.existing() {
                    l.reset(crate::bench::ResetScope::Bench);
                }
            }
            Command::Verify | Command::Upload => {
                if self.ctx.project.is_some() {
                    self.ctx.navigate(Route::Firmware);
                    self.ctx.pending.push(c);
                }
            }
            Command::RunAnalyses | Command::ExportCir => {
                if self.ctx.project.is_some() {
                    self.ctx.navigate(Route::Pro(ProTab::Analysis));
                    self.ctx.pending.push(c);
                }
            }
        }
    }

    fn inject_key(&mut self, key: Key, modifiers: Modifiers) {
        for pressed in [true, false] {
            self.inject.push(egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat: false,
                modifiers,
            });
        }
        self.ctx.egui.request_repaint();
    }

    pub fn open_path(&mut self, dir: &Path) {
        match self.ctx.open_project(dir) {
            Ok(()) => {
                let p = self.ctx.project.as_ref().expect("project");
                let route = match p.mode {
                    ProjectMode::Pro => Route::Pro(ProTab::Analysis),
                    ProjectMode::Playground => Route::Workbench,
                };
                self.ctx.navigate(route);
            }
            Err(e) => self.ctx.toasts.error(format!("{e:#}")),
        }
    }

    fn route_changes(&mut self) {
        if let Some(r) = self.ctx.nav.take() {
            let r = if r.needs_project() && self.ctx.project.is_none() {
                Route::Home
            } else {
                r
            };
            if r != self.ctx.route {
                self.ctx.route = r;
                self.ctx.egui.request_repaint();
            }
        }
    }
}

fn slash(ui: &mut Ui, p: &theme::Palette) {
    ui.label(
        egui::RichText::new("/")
            .font(font::ui(13.0))
            .color(p.text_ghost),
    );
}

impl eframe::App for BreadboxApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.persisted());
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        raw_input.events.append(&mut self.inject);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = theme::pal_ctx(&self.ctx.egui).bg_app;
        Color32::to_normalized_gamma_f32(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> AppOptions {
        AppOptions {
            detect_toolchain: false,
            native_menu: false,
            load_persisted: false,
            ..AppOptions::test()
        }
    }

    fn frame(app: &mut BreadboxApp, ctx: &egui::Context) {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.frame(ui));
        out.textures_delta.clear();
    }

    #[test]
    fn persisted_state_restores_project_route_theme_and_recents() {
        let ctx = egui::Context::default();
        let mut app = BreadboxApp::with_context(&ctx, opts(), PersistedState::default());
        let dir = app
            .ctx
            .resources
            .examples()
            .expect("examples")
            .join("blink");
        app.open_path(&dir);
        frame(&mut app, &ctx);
        app.ctx.command(Command::Navigate(Route::Firmware));
        app.ctx.command(Command::SetTheme(ThemeChoice::Light));
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert_eq!(app.ctx.route, Route::Firmware);
        let saved = serde_json::to_string(&app.persisted()).unwrap();
        let back: PersistedState = serde_json::from_str(&saved).unwrap();
        let ctx2 = egui::Context::default();
        let app2 = BreadboxApp::with_context(&ctx2, opts(), back);
        assert_eq!(
            app2.ctx.project.as_ref().map(|p| p.name.as_str()),
            Some("blink")
        );
        assert!(
            app2.ctx.project.as_ref().unwrap().read_only,
            "examples open read-only"
        );
        assert_eq!(app2.ctx.route, Route::Firmware);
        assert_eq!(app2.ctx.theme, ThemeChoice::Light);
        assert_eq!(app2.ctx.recent[0].name, "blink");
        assert!(
            saved.contains("\"firmware\""),
            "screen state saved: {saved}"
        );
    }

    #[test]
    fn routes_needing_a_project_fall_back_to_home() {
        let ctx = egui::Context::default();
        let state = PersistedState {
            route: Route::Firmware,
            last_project: Some("/nonexistent/x.breadbox".into()),
            ..Default::default()
        };
        let mut app = BreadboxApp::with_context(&ctx, opts(), state);
        assert_eq!(app.ctx.route, Route::Home);
        app.ctx
            .command(Command::Navigate(Route::Pro(ProTab::Analysis)));
        frame(&mut app, &ctx);
        frame(&mut app, &ctx);
        assert_eq!(app.ctx.route, Route::Home);
    }
}
