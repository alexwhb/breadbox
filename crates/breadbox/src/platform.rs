//! OS-specific glue (`PLAT-1`): the native menu bar (`APP-4`) and file dialogs. Everything
//! platform-dependent lives here; other modules stay portable.

use crate::app::AppCtx;
use crate::project::ProjectMode;
use crate::router::{Command, ProTab, Route};
use crate::theme::ThemeChoice;
use std::path::PathBuf;

/// Folder picker (Open project).
pub fn pick_folder(title: &str) -> Option<PathBuf> {
    rfd::FileDialog::new().set_title(title).pick_folder()
}

/// "Save as" for a project folder: returns the destination folder path.
pub fn save_folder(title: &str, suggested: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(title)
        .set_file_name(suggested)
        .save_file()
}

/// Save dialog for a file with one extension.
pub fn save_file(title: &str, suggested: &str, ext_name: &str, ext: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(title)
        .set_file_name(suggested)
        .add_filter(ext_name, &[ext])
        .save_file()
}

/// Open dialog for one file type.
pub fn open_file(title: &str, ext_name: &str, exts: &[&str]) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(title)
        .add_filter(ext_name, exts)
        .pick_file()
}

/// Menu ids → commands.
fn command_for(id: &str) -> Option<Command> {
    Some(match id {
        "new-playground" => Command::NewProject(ProjectMode::Playground),
        "new-pro" => Command::NewProject(ProjectMode::Pro),
        "open" => Command::OpenProject,
        "save" => Command::Save,
        "save-as" => Command::SaveAs,
        "close" => Command::Close,
        "undo" => Command::Undo,
        "redo" => Command::Redo,
        "find" => Command::Find,
        "home" => Command::Navigate(Route::Home),
        "workbench" => Command::Navigate(Route::Workbench),
        "firmware" => Command::Navigate(Route::Firmware),
        "pro" => Command::Navigate(Route::Pro(ProTab::Analysis)),
        "plugins" => Command::Navigate(Route::Plugins),
        "lessons" => Command::Navigate(Route::Lessons),
        "theme-dark" => Command::SetTheme(ThemeChoice::Dark),
        "theme-light" => Command::SetTheme(ThemeChoice::Light),
        "run" => Command::RunPause,
        "reset" => Command::ResetSim,
        "reset-mcu" => Command::ResetMcu,
        "step-quantum" => Command::StepSim(crate::router::StepSize::Quantum),
        "step-ms" => Command::StepSim(crate::router::StepSize::Millisecond),
        "import-cir" => Command::ImportCir,
        "verify" => Command::Verify,
        "upload" => Command::Upload,
        "analyses" => Command::RunAnalyses,
        "export-cir" => Command::ExportCir,
        "about" => Command::About,
        "docs" => Command::OpenDocs,
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use muda::accelerator::{Accelerator, Code, Modifiers};
    use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, Sender, channel};

    pub struct NativeMenu {
        _menu: Menu,
        rx: Receiver<Command>,
        theme_dark: CheckMenuItem,
        theme_light: CheckMenuItem,
    }

    fn item(id: &str, text: &str, accel: Option<(Modifiers, Code)>) -> MenuItem {
        MenuItem::with_id(id, text, true, accel.map(|(m, c)| Accelerator::new(m, c)))
    }

    impl NativeMenu {
        pub fn install(ctx: &eframe::egui::Context) -> Option<NativeMenu> {
            let cmd = Modifiers::META;
            let cmd_shift = Modifiers::META | Modifiers::SHIFT;
            let menu = Menu::new();
            let app = Submenu::with_items(
                "Breadbox",
                true,
                &[
                    &item("about", "About Breadbox", None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::services(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::hide(None),
                    &PredefinedMenuItem::hide_others(None),
                    &PredefinedMenuItem::show_all(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::quit(None),
                ],
            )
            .ok()?;
            let file = Submenu::with_items(
                "File",
                true,
                &[
                    &item(
                        "new-playground",
                        "New Playground Project…",
                        Some((cmd, Code::KeyN)),
                    ),
                    &item("new-pro", "New Pro Project…", None),
                    &item("open", "Open Project…", Some((cmd, Code::KeyO))),
                    &item("import-cir", "Import .cir Netlist…", None),
                    &PredefinedMenuItem::separator(),
                    &item("save", "Save", Some((cmd, Code::KeyS))),
                    &item("save-as", "Save As…", Some((cmd_shift, Code::KeyS))),
                    &PredefinedMenuItem::separator(),
                    &item("export-cir", "Export .cir…", None),
                    &PredefinedMenuItem::separator(),
                    &item("close", "Close", Some((cmd, Code::KeyW))),
                ],
            )
            .ok()?;
            let edit = Submenu::with_items(
                "Edit",
                true,
                &[
                    &item("undo", "Undo", Some((cmd, Code::KeyZ))),
                    &item("redo", "Redo", Some((cmd_shift, Code::KeyZ))),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::cut(None),
                    &PredefinedMenuItem::copy(None),
                    &PredefinedMenuItem::paste(None),
                    &PredefinedMenuItem::select_all(None),
                    &PredefinedMenuItem::separator(),
                    &item("find", "Find…", Some((cmd, Code::KeyF))),
                ],
            )
            .ok()?;
            let theme_dark = CheckMenuItem::with_id("theme-dark", "Dark", true, true, None);
            let theme_light = CheckMenuItem::with_id("theme-light", "Light", true, false, None);
            let theme = Submenu::with_items("Theme", true, &[&theme_dark, &theme_light]).ok()?;
            let view = Submenu::with_items(
                "View",
                true,
                &[
                    &item("home", "Home", Some((cmd_shift, Code::KeyH))),
                    &PredefinedMenuItem::separator(),
                    &item("workbench", "Workbench", Some((cmd, Code::Digit1))),
                    &item("firmware", "Firmware", Some((cmd, Code::Digit2))),
                    &item("pro", "Pro · Analysis", Some((cmd, Code::Digit3))),
                    &item("plugins", "Plugins", Some((cmd, Code::Digit4))),
                    &item("lessons", "Lessons", Some((cmd, Code::Digit5))),
                    &PredefinedMenuItem::separator(),
                    &theme,
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::fullscreen(None),
                ],
            )
            .ok()?;
            let sim = Submenu::with_items(
                "Simulate",
                true,
                &[
                    &item("run", "Run / Pause", Some((cmd, Code::KeyR))),
                    &item("step-quantum", "Step One Quantum", None),
                    &item("step-ms", "Step 1 ms", None),
                    &item("reset", "Reset Bench", None),
                    &item("reset-mcu", "Reset MCU Only", None),
                    &PredefinedMenuItem::separator(),
                    &item("verify", "Verify Sketch", Some((cmd, Code::KeyB))),
                    &item("upload", "Upload to Sim", Some((cmd, Code::KeyU))),
                    &PredefinedMenuItem::separator(),
                    &item("analyses", "Run Analyses", None),
                ],
            )
            .ok()?;
            let window = Submenu::with_items(
                "Window",
                true,
                &[
                    &PredefinedMenuItem::minimize(None),
                    &PredefinedMenuItem::maximize(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::bring_all_to_front(None),
                ],
            )
            .ok()?;
            let help = Submenu::with_items(
                "Help",
                true,
                &[&item("docs", "Breadbox Documentation", None)],
            )
            .ok()?;
            for m in [&app, &file, &edit, &view, &sim, &window, &help] {
                menu.append(m).ok()?;
            }
            menu.init_for_nsapp();
            window.set_as_windows_menu_for_nsapp();
            help.set_as_help_menu_for_nsapp();

            let (tx, rx): (Sender<Command>, Receiver<Command>) = channel();
            let tx = Mutex::new(tx);
            let ectx = ctx.clone();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                if let Some(c) = command_for(e.id().as_ref())
                    && let Ok(tx) = tx.lock()
                {
                    let _ = tx.send(c);
                }
                ectx.request_repaint();
            }));
            Some(NativeMenu {
                _menu: menu,
                rx,
                theme_dark,
                theme_light,
            })
        }

        pub fn take_commands(&self) -> Vec<Command> {
            self.rx.try_iter().collect()
        }

        pub fn sync(&self, app: &AppCtx) {
            self.theme_dark.set_checked(app.theme == ThemeChoice::Dark);
            self.theme_light
                .set_checked(app.theme == ThemeChoice::Light);
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::NativeMenu;

/// No native menu bar outside macOS (the shortcuts still work).
#[cfg(not(target_os = "macos"))]
pub struct NativeMenu;

#[cfg(not(target_os = "macos"))]
impl NativeMenu {
    pub fn install(_ctx: &eframe::egui::Context) -> Option<NativeMenu> {
        None
    }
    pub fn take_commands(&self) -> Vec<Command> {
        Vec::new()
    }
    pub fn sync(&self, _app: &AppCtx) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_commands() {
        assert_eq!(command_for("save"), Some(Command::Save));
        assert_eq!(
            command_for("theme-light"),
            Some(Command::SetTheme(ThemeChoice::Light))
        );
        assert_eq!(
            command_for("pro"),
            Some(Command::Navigate(Route::Pro(ProTab::Analysis)))
        );
        assert_eq!(
            command_for("step-ms"),
            Some(Command::StepSim(crate::router::StepSize::Millisecond))
        );
        assert_eq!(command_for("reset-mcu"), Some(Command::ResetMcu));
        assert_eq!(command_for("import-cir"), Some(Command::ImportCir));
        assert_eq!(command_for("nope"), None);
    }
}
