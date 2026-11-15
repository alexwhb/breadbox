//! Home (`APP-2`): new project (Playground / Pro), open a project folder, recent projects,
//! the examples gallery, and entry points to Lessons and Plugins.

use crate::app::AppCtx;
use crate::project::{self, ExampleInfo, Project, ProjectMode};
use crate::router::{Command, Route, Screen};
use crate::theme::{Palette, font, pal};
use crate::toolchain::ToolchainState;
use crate::widgets::{self, Btn, Icon, Seg, Tone, card_frame, section_header};
use eframe::egui::{self, CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};
use std::path::PathBuf;

#[derive(Default)]
pub struct HomeScreen {
    new_dialog: Option<NewProjectDialog>,
}

struct NewProjectDialog {
    name: String,
    mode: ProjectMode,
    parent: PathBuf,
    error: Option<String>,
    /// The suggested name has been focused + selected once.
    selected: bool,
}

impl Screen for HomeScreen {
    fn crumb(&self, _app: &AppCtx) -> Option<String> {
        Some("Home".into())
    }

    fn header(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if Btn::amber("New project")
                .icon(Icon::Plus)
                .show(ui)
                .clicked()
            {
                self.open_new(ProjectMode::Playground);
            }
            if Btn::ghost("Open project…")
                .icon(Icon::Folder)
                .show(ui)
                .clicked()
            {
                app.command(Command::OpenProject);
            }
            if let Some(p) = &app.project {
                let label = format!("Back to {}", p.name);
                if Btn::plain(&label)
                    .icon(Icon::ChevronLeft)
                    .show(ui)
                    .clicked()
                {
                    let r = match p.mode {
                        ProjectMode::Pro => Route::Pro(crate::router::ProTab::Analysis),
                        ProjectMode::Playground => Route::Workbench,
                    };
                    app.navigate(r);
                }
            }
        });
    }

    fn command(&mut self, cmd: &Command, _app: &mut AppCtx) -> bool {
        if let Command::NewProject(mode) = cmd {
            self.open_new(*mode);
            return true;
        }
        false
    }

    fn ui(&mut self, ui: &mut Ui, app: &mut AppCtx) {
        let p = pal(ui);
        let area = ui.max_rect();
        egui::ScrollArea::vertical()
            .id_salt("home-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let w = (area.width() - 96.0).clamp(760.0, 1180.0);
                let left = area.left() + (area.width() - w) / 2.0;
                let mut child = widgets::child_ui(
                    ui,
                    Rect::from_min_size(pos2(left, area.top() + 40.0), vec2(w, 10_000.0)),
                );
                let ui = &mut child;
                ui.spacing_mut().item_spacing.y = 14.0;
                self.hero(ui, app, p, w);
                ui.add_space(18.0);
                let side = 300.0;
                let main_w = w - side - 24.0;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 24.0;
                    ui.vertical(|ui| {
                        ui.set_width(main_w);
                        ui.spacing_mut().item_spacing.y = 12.0;
                        self.recent(ui, app, p);
                        ui.add_space(10.0);
                        self.examples(ui, app, p, main_w);
                    });
                    ui.vertical(|ui| {
                        ui.set_width(side);
                        ui.spacing_mut().item_spacing.y = 12.0;
                        self.side(ui, app, p);
                    });
                });
                ui.add_space(40.0);
                let used = ui.min_rect();
                let _ = used;
            });
        self.new_project_modal(app);
    }
}

impl HomeScreen {
    fn open_new(&mut self, mode: ProjectMode) {
        let parent = project::default_projects_dir();
        self.new_dialog = Some(NewProjectDialog {
            name: unique_name(&parent, default_name(mode)),
            mode,
            parent,
            error: None,
            selected: false,
        });
    }

    fn hero(&mut self, ui: &mut Ui, app: &mut AppCtx, p: &Palette, w: f32) {
        ui.label(
            egui::RichText::new("Start something")
                .font(font::brand(28.0))
                .color(p.text_strong),
        );
        ui.label(
            egui::RichText::new("A breadboard bench where Arduinos run real firmware — and a SPICE workbench over the same circuit.")
                .font(font::ui(14.0))
                .color(p.text_muted),
        );
        ui.add_space(6.0);
        let card_w = (w - 16.0) / 2.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for (mode, icon, title, blurb, button) in [
                (
                    ProjectMode::Playground,
                    Icon::Breadboard,
                    "Playground",
                    "Breadboards, jumper wires, sensors in a pot of soil, radios — and an Arduino running your sketch.",
                    "New Playground project",
                ),
                (
                    ProjectMode::Pro,
                    Icon::Analysis,
                    "Pro",
                    "Netlists, transient / DC / AC / Monte Carlo analyses, operating points against bench probes.",
                    "New Pro project",
                ),
            ] {
                let (r, _) = ui.allocate_exact_size(vec2(card_w, 132.0), Sense::hover());
                ui.painter().rect(r, CornerRadius::same(10), p.bg_header, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
                let mut c = widgets::child_ui(ui, r.shrink2(vec2(18.0, 16.0)));
                c.spacing_mut().item_spacing.y = 8.0;
                c.horizontal(|ui| {
                    let (ir, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
                    icon.paint_with(ui.painter(), ir, p.text_dim, p.accent);
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(title).font(font::heading(18.0)).color(p.text_strong));
                    if mode == ProjectMode::Pro {
                        widgets::tag(ui, "SPICE", Tone::Info);
                    }
                });
                widgets::wrapped_text(&mut c, blurb, font::ui(13.0), p.text_muted, 2);
                c.horizontal(|ui| {
                    if Btn::amber(button).icon(Icon::Plus).show(ui).clicked() {
                        self.open_new(mode);
                    }
                    if mode == ProjectMode::Playground && Btn::ghost("Open project…").icon(Icon::Folder).show(ui).clicked() {
                        app.command(Command::OpenProject);
                    }
                    if mode == ProjectMode::Pro && Btn::ghost("Import .cir…").icon(Icon::Folder).show(ui).clicked() {
                        app.command(Command::ImportCir);
                    }
                });
            }
        });
    }

    fn recent(&mut self, ui: &mut Ui, app: &mut AppCtx, p: &Palette) {
        section_header(ui, "Recent projects");
        let recent: Vec<_> = app.recent.iter().take(6).cloned().collect();
        if recent.is_empty() {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::hover());
            widgets::controls::dashed_rect(ui.painter(), r, 8.0, p.border_strong);
            ui.painter().text(
                pos2(r.left() + 16.0, r.center().y),
                egui::Align2::LEFT_CENTER,
                "Nothing here yet — open an example below or start a new project.",
                font::ui(13.0),
                p.text_muted,
            );
            return;
        }
        let w = ui.available_width();
        for rp in recent {
            let (r, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
            let label = format!("Open recent {}", rp.name);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
            let fill = if resp.hovered() {
                p.bg_hover
            } else {
                p.bg_header
            };
            ui.painter().rect(
                r,
                CornerRadius::same(8),
                fill,
                Stroke::new(1.0, p.border),
                egui::StrokeKind::Inside,
            );
            let icon = if rp.mode == ProjectMode::Pro {
                Icon::Analysis
            } else {
                Icon::Breadboard
            };
            icon.paint_with(
                ui.painter(),
                Rect::from_center_size(pos2(r.left() + 22.0, r.center().y), vec2(20.0, 20.0)),
                p.text_dim,
                p.accent,
            );
            ui.painter().text(
                pos2(r.left() + 44.0, r.center().y - 8.0),
                egui::Align2::LEFT_CENTER,
                &rp.name,
                font::medium(13.0),
                p.text,
            );
            ui.painter().text(
                pos2(r.left() + 44.0, r.center().y + 9.0),
                egui::Align2::LEFT_CENTER,
                rp.path.display().to_string(),
                font::mono(11.0),
                p.text_faint,
            );
            ui.painter().text(
                pos2(r.right() - 14.0, r.center().y),
                egui::Align2::RIGHT_CENTER,
                format!("{} · {}", rp.mode.label(), ago(rp.opened)),
                font::ui(12.0),
                p.text_muted,
            );
            if resp.clicked() {
                if rp.path.join("project.json").exists() {
                    app.command(Command::OpenPath(rp.path.clone()));
                } else {
                    app.toasts
                        .warn(format!("{} was moved or deleted.", rp.path.display()));
                    app.recent.retain(|x| x.path != rp.path);
                }
            }
        }
    }

    fn examples(&mut self, ui: &mut Ui, app: &mut AppCtx, p: &Palette, w: f32) {
        ui.horizontal(|ui| {
            section_header(ui, "Examples");
            ui.label(
                egui::RichText::new(format!(
                    "{} projects · open read-only, Save As to keep changes",
                    app.examples.len()
                ))
                .font(font::ui(12.0))
                .color(p.text_muted),
            );
        });
        let cols = 3usize;
        let gap = 12.0;
        let cw = (w - gap * (cols as f32 - 1.0)) / cols as f32;
        let examples: Vec<ExampleInfo> = app.examples.clone();
        for row in examples.chunks(cols) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for ex in row {
                    let (r, resp) = ui.allocate_exact_size(vec2(cw, 128.0), Sense::click());
                    let label = format!("Open example {}", ex.name);
                    resp.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, &label)
                    });
                    let hot = resp.hovered();
                    ui.painter().rect(
                        r,
                        CornerRadius::same(10),
                        if hot { p.accent_hover } else { p.bg_card },
                        Stroke::new(1.0, if hot { p.accent_border } else { p.border }),
                        egui::StrokeKind::Inside,
                    );
                    let mut c = widgets::child_ui(ui, r.shrink(14.0));
                    c.spacing_mut().item_spacing.y = 6.0;
                    c.horizontal(|ui| {
                        let icon = if ex.mode == ProjectMode::Pro {
                            Icon::Analysis
                        } else if ex.boards.iter().any(|b| b.contains("ESP32")) {
                            Icon::Esp32
                        } else if ex.boards.is_empty() {
                            Icon::Breadboard
                        } else {
                            Icon::Uno
                        };
                        let (ir, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                        icon.paint_with(ui.painter(), ir, p.text_dim, p.accent);
                        ui.label(
                            egui::RichText::new(&ex.name)
                                .font(font::heading(15.0))
                                .color(p.text_strong),
                        );
                    });
                    widgets::wrapped_text(&mut c, &ex.description, font::ui(12.0), p.text_muted, 3);
                    let mut tags = widgets::child_ui(
                        ui,
                        Rect::from_min_max(
                            pos2(r.left() + 14.0, r.bottom() - 30.0),
                            pos2(r.right() - 14.0, r.bottom() - 8.0),
                        ),
                    );
                    tags.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        widgets::kind_tag(ui, &ex.mode.label().to_uppercase());
                        for b in &ex.boards {
                            widgets::tag(ui, b, Tone::Neutral);
                        }
                    });
                    if resp.clicked() {
                        app.command(Command::OpenPath(ex.dir.clone()));
                    }
                }
            });
        }
        if app.examples.is_empty() {
            ui.label(
                egui::RichText::new("No examples folder found (set BREADBOX_RESOURCES).")
                    .color(p.text_muted),
            );
        }
    }

    fn side(&mut self, ui: &mut Ui, app: &mut AppCtx, p: &Palette) {
        section_header(ui, "Learn & extend");
        for (icon, title, blurb, button, route) in [
            (
                Icon::Book,
                "Lessons",
                "Electricity you can see, sensors, microcontrollers from zero, radios & IoT, SPICE analysis — do the math yourself.",
                "Open lessons",
                Route::Lessons,
            ),
            (
                Icon::Plug,
                "Plugins",
                "Instruments, parts, radios and world models in a WebAssembly sandbox.",
                "Browse plugins",
                Route::Plugins,
            ),
        ] {
            card_frame(p).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.horizontal(|ui| {
                    icon.show(ui, 18.0, p.text_dim);
                    ui.label(
                        egui::RichText::new(title)
                            .font(font::heading(15.0))
                            .color(p.text_strong),
                    );
                });
                ui.label(
                    egui::RichText::new(blurb)
                        .font(font::ui(12.5))
                        .color(p.text_muted),
                );
                if Btn::ghost(button).show(ui).clicked() {
                    app.navigate(route);
                }
            });
        }
        card_frame(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.horizontal(|ui| {
                Icon::Wrench.show(ui, 16.0, p.text_dim);
                ui.label(
                    egui::RichText::new("Firmware toolchain")
                        .font(font::heading(15.0))
                        .color(p.text_strong),
                );
            });
            let (text, tone) = match &app.toolchain.state {
                ToolchainState::Ready(_) => (app.toolchain.compiler_label(), Tone::Ok),
                ToolchainState::Detecting => ("detecting…".to_string(), Tone::Neutral),
                ToolchainState::Installing => ("installing…".to_string(), Tone::Warn),
                ToolchainState::Missing { reason, .. } => (reason.clone(), Tone::Warn),
                ToolchainState::Failed(e) => (e.clone(), Tone::Danger),
                ToolchainState::Disabled => ("off".to_string(), Tone::Neutral),
            };
            ui.add(
                egui::Label::new(egui::RichText::new(text).font(font::mono(12.0)).color(
                    match tone {
                        Tone::Ok => p.ok,
                        Tone::Warn => p.accent,
                        Tone::Danger => p.danger_soft,
                        _ => p.text_muted,
                    },
                ))
                .wrap(),
            );
            if let Some(cli) = app.toolchain.cli_label() {
                ui.label(
                    egui::RichText::new(cli)
                        .font(font::mono(12.0))
                        .color(p.text_muted),
                );
            }
            if matches!(
                app.toolchain.state,
                ToolchainState::Missing { .. } | ToolchainState::Failed(_)
            ) && Btn::amber("Install toolchain")
                .icon(Icon::Download)
                .show(ui)
                .clicked()
            {
                app.toolchain.start_install();
            }
            if let Some(pr) = app.toolchain.install_progress() {
                widgets::progress(ui, pr.fraction, &pr.step);
            }
        });
    }

    fn new_project_modal(&mut self, app: &mut AppCtx) {
        let Some(d) = self.new_dialog.as_mut() else {
            return;
        };
        let p = crate::theme::pal_ctx(&app.egui);
        let mut create = false;
        let mut cancel = false;
        let r = widgets::modal(&app.egui, "new-project", "New project", 440.0, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            let mode_before = d.mode;
            widgets::segmented(
                ui,
                "Project mode",
                &mut d.mode,
                &[
                    Seg::new(ProjectMode::Playground, "Playground").icon(Icon::SegBreadboard),
                    Seg::new(ProjectMode::Pro, "Pro").icon(Icon::SegSchematic),
                ],
            );
            // switching the mode keeps a typed name but swaps the suggestion
            if d.mode != mode_before && d.name == unique_name(&d.parent, default_name(mode_before))
            {
                d.name = unique_name(&d.parent, default_name(d.mode));
            }
            ui.label(
                egui::RichText::new("Name")
                    .font(font::ui(12.0))
                    .color(p.text_muted),
            );
            let mut out = egui::TextEdit::singleline(&mut d.name)
                .font(font::ui(13.0))
                .desired_width(f32::INFINITY)
                .show(ui);
            let resp = out.response.clone();
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Project name")
            });
            // the suggested name is focused and selected: typing replaces it
            if !d.selected {
                d.selected = true;
                resp.request_focus();
                let n = d.name.chars().count();
                out.state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(n),
                    )));
                out.state.store(ui.ctx(), resp.id);
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                create = true;
            }
            ui.label(
                egui::RichText::new("Location")
                    .font(font::ui(12.0))
                    .color(p.text_muted),
            );
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(d.parent.display().to_string())
                        .font(font::mono(12.0))
                        .color(p.text_dim),
                );
                if Btn::ghost("Choose…").small().show(ui).clicked()
                    && let Some(dir) =
                        crate::platform::pick_folder("Where should the project live?")
                {
                    d.parent = dir;
                }
            });
            if let Some(e) = &d.error {
                ui.label(
                    egui::RichText::new(e)
                        .font(font::ui(12.0))
                        .color(p.danger_soft),
                );
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if Btn::amber("Create project").show(ui).clicked() {
                    create = true;
                }
                if Btn::ghost("Cancel").show(ui).clicked() {
                    cancel = true;
                }
            });
        });
        if r.close || cancel {
            self.new_dialog = None;
            return;
        }
        if create {
            let _ = std::fs::create_dir_all(&d.parent);
            match Project::create(&d.parent, d.name.trim(), d.mode) {
                Ok(pr) => {
                    let dir = pr.dir.clone();
                    self.new_dialog = None;
                    app.command(Command::OpenPath(dir));
                }
                Err(e) => d.error = Some(format!("{e:#}")),
            }
        }
    }
}

fn default_name(mode: ProjectMode) -> &'static str {
    match mode {
        ProjectMode::Playground => "my-bench",
        ProjectMode::Pro => "my-analysis",
    }
}

/// `base`, or `base-2`, `base-3`, … — the first name without a project folder in `parent`.
fn unique_name(parent: &std::path::Path, base: &str) -> String {
    let mut name = base.to_string();
    let mut n = 2;
    while parent.join(format!("{name}.breadbox")).exists() {
        name = format!("{base}-{n}");
        n += 1;
    }
    name
}

fn ago(unix: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(unix);
    let s = now.saturating_sub(unix);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}
