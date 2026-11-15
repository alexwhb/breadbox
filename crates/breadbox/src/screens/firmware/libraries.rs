//! Library manager (search + install with progress) and the one-click toolchain installer.

use crate::app::AppCtx;
use crate::jobs::Job;
use crate::theme::{font, pal_ctx};
use crate::toolchain::ToolchainState;
use crate::widgets::{self, Btn, Icon, progress};
use bb_firmware::{FirmwareError, InstallProgress, LibraryInfo};
use eframe::egui;

#[derive(Default)]
pub struct LibraryManager {
    pub open: bool,
    query: String,
    search: Option<Job<Result<Vec<LibraryInfo>, FirmwareError>>>,
    results: Vec<LibraryInfo>,
    install: Option<(String, Job<Result<(), FirmwareError>>)>,
    message: Option<(bool, String)>,
}

impl LibraryManager {
    pub fn show(&mut self, app: &mut AppCtx) {
        if !self.open {
            return;
        }
        if let Some(j) = self.search.as_mut()
            && let Some(r) = j.poll()
        {
            self.search = None;
            match r {
                Ok(v) => self.results = v,
                Err(e) => self.message = Some((false, e.to_string())),
            }
        }
        if let Some((name, j)) = self.install.as_mut()
            && let Some(r) = j.poll()
        {
            let name = name.clone();
            self.install = None;
            match r {
                Ok(()) => {
                    self.message = Some((true, format!("Installed {name}")));
                    app.toolchain.refresh_libraries();
                }
                Err(e) => self.message = Some((false, e.to_string())),
            }
        }
        let p = pal_ctx(&app.egui);
        let tc = app.toolchain.any();
        let repaint = app.repaint.clone();
        let mut close = false;
        let r = widgets::modal(&app.egui, "library-manager", "Library manager", 560.0, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            let Some(tc) = tc.clone() else {
                ui.label(egui::RichText::new("Install the toolchain first — the library index comes with arduino-cli.").color(p.text_muted));
                return;
            };
            ui.horizontal(|ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Search the Arduino Library Manager (e.g. ArduinoJson)")
                        .font(font::ui(13.0))
                        .desired_width(400.0),
                );
                let go = (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    | Btn::ghost("Search").icon(Icon::Search).show(ui).clicked();
                if go && !self.query.trim().is_empty() && self.search.is_none() {
                    let q = self.query.trim().to_string();
                    let tc = tc.clone();
                    self.search = Some(Job::spawn("lib-search", repaint.clone(), move |_| tc.lib_search(&q)));
                }
            });
            if self.search.is_some() {
                progress(ui, None, "Searching…");
            }
            if let Some((name, j)) = &self.install {
                let pr = j.progress();
                progress(ui, pr.fraction, &format!("Installing {name} — {}", pr.step));
            }
            if let Some((ok, m)) = &self.message {
                ui.label(egui::RichText::new(m).font(font::ui(12.0)).color(if *ok { p.ok } else { p.danger_soft }));
            }
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for lib in self.results.iter().take(40) {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(400.0);
                            ui.label(egui::RichText::new(format!("{} {}", lib.name, lib.version)).font(font::medium(13.0)).color(p.text));
                            ui.label(egui::RichText::new(&lib.sentence).font(font::ui(12.0)).color(p.text_muted));
                            ui.label(egui::RichText::new(&lib.author).font(font::ui(11.0)).color(p.text_faint));
                        });
                        let installed = app.toolchain.library_version(&lib.name).map(str::to_string);
                        let label = match &installed {
                            Some(v) if v == &lib.version => "Installed".to_string(),
                            Some(_) => "Update".to_string(),
                            None => "Install".to_string(),
                        };
                        let enabled = self.install.is_none() && label != "Installed";
                        let btn_label = format!("{label} {}", lib.name);
                        if Btn::ghost(&btn_label).enabled(enabled).small().show(ui).clicked() {
                            let tc = tc.clone();
                            let name = lib.name.clone();
                            let n2 = name.clone();
                            self.message = None;
                            self.install = Some((
                                name,
                                Job::spawn("lib-install", repaint.clone(), move |p| {
                                    tc.lib_install(&n2, None, &mut |ev| match ev {
                                        InstallProgress::Step(s) => p.step(s),
                                        InstallProgress::Log(l) => p.log(l),
                                        InstallProgress::Download { bytes, total } => {
                                            p.set_fraction(total.map(|t| bytes as f32 / t.max(1) as f32))
                                        }
                                        InstallProgress::Done => p.set_fraction(Some(1.0)),
                                    })
                                }),
                            ));
                        }
                    });
                    ui.separator();
                }
            });
            ui.add_space(4.0);
            widgets::section_header(ui, "Installed");
            let names: Vec<String> = app
                .toolchain
                .libraries
                .iter()
                .filter(|l| l.location == "user")
                .map(|l| format!("{} {}", l.name, l.version))
                .collect();
            ui.label(
                egui::RichText::new(if names.is_empty() { "none yet".into() } else { names.join(" · ") })
                    .font(font::mono(12.0))
                    .color(p.text_muted),
            );
            if Btn::ghost("Done").show(ui).clicked() {
                close = true;
            }
        });
        if r.close || close {
            self.open = false;
        }
    }
}

/// One-click toolchain install dialog (`MCU-4`).
#[derive(Default)]
pub struct InstallDialog {
    pub open: bool,
}

impl InstallDialog {
    pub fn show(&mut self, app: &mut AppCtx) {
        if !self.open {
            return;
        }
        let p = pal_ctx(&app.egui);
        let mut close = false;
        let r = widgets::modal(&app.egui, "toolchain-install", "Install the firmware toolchain", 520.0, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            let reason = match &app.toolchain.state {
                ToolchainState::Missing { reason, .. } => format!("Breadbox compiles sketches with arduino-cli + avr-gcc — {reason}."),
                ToolchainState::Failed(e) => format!("The last install failed: {e}"),
                ToolchainState::Ready(t) => format!("Ready: {} · arduino-cli {}", t.compiler_label(), t.cli_version),
                ToolchainState::Installing => "Installing…".into(),
                ToolchainState::Detecting => "Looking for an existing install…".into(),
                ToolchainState::Disabled => "Toolchain detection is off.".into(),
            };
            ui.add(egui::Label::new(egui::RichText::new(reason).font(font::ui(13.0)).color(p.text_dim)).wrap());
            ui.label(
                egui::RichText::new("Downloads arduino-cli and the arduino:avr core into ~/Library/Application Support/Breadbox/toolchain (≈ 150 MB). On Apple Silicon without Rosetta a native avr-gcc is installed with Homebrew.")
                    .font(font::ui(12.0))
                    .color(p.text_muted),
            );
            if let Some(pr) = app.toolchain.install_progress() {
                progress(ui, pr.fraction, &pr.step);
                let tail: Vec<String> = pr.log.iter().rev().take(6).rev().cloned().collect();
                ui.label(egui::RichText::new(tail.join("\n")).font(font::mono(11.0)).color(p.text_faint));
            }
            ui.horizontal(|ui| {
                let installing = app.toolchain.is_installing();
                let ready = app.toolchain.ready().is_some();
                if !ready && Btn::amber("Install toolchain").icon(Icon::Download).enabled(!installing).show(ui).clicked() {
                    app.toolchain.start_install();
                }
                if Btn::ghost(if ready { "Done" } else { "Later" }).show(ui).clicked() {
                    close = true;
                }
            });
        });
        if r.close || close {
            self.open = false;
        }
    }
}
