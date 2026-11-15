//! OS-specific glue (`PLAT-1`): where resources and user data live, the native menu bar
//! (`APP-4`) and file dialogs. Everything platform-dependent lives here; other modules stay
//! portable.

use crate::app::AppCtx;
use crate::project::{ProjectMode, Resources};
use crate::router::{Command, ProTab, Route};
use crate::theme::ThemeChoice;
use std::path::PathBuf;

/// Bundled read-only resources (examples, lessons, first-party plugins, docs, licences):
/// `$BREADBOX_RESOURCES`, `Breadbox.app/Contents/Resources`, or the repository in development
/// builds. The one resolver every crate uses is [`bb_core::paths`].
pub fn resources() -> Resources {
    Resources::locate()
}

/// Per-user data (plugins, lesson progress, toolchain, app state):
/// `~/Library/Application Support/Breadbox` (or `$BREADBOX_APP_SUPPORT`).
pub fn app_support() -> PathBuf {
    bb_core::paths::app_support()
}

/// Per-user caches (build cache, staging): `~/Library/Caches/Breadbox` (or `$BREADBOX_CACHES`).
pub fn caches() -> PathBuf {
    bb_core::paths::caches()
}

/// The saved window + screen state (`APP-5`): `<app support>/app-state.ron`.
pub fn app_state_file() -> PathBuf {
    app_support().join("app-state.ron")
}

/// Projects / bundles the system asked Breadbox to open (Finder "Open With", a drop on the
/// Dock icon, `open -a Breadbox <folder>`, `breadbox <path>`), until the app takes them.
static OPENED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
/// The app's egui context, to wake it when a document arrives while it is idle.
static WAKE: std::sync::Mutex<Option<eframe::egui::Context>> = std::sync::Mutex::new(None);

/// Queues documents to open and wakes the app.
pub fn queue_open(paths: impl IntoIterator<Item = PathBuf>) {
    if let Ok(mut q) = OPENED.lock() {
        q.extend(paths);
    }
    if let Ok(w) = WAKE.lock()
        && let Some(ctx) = w.as_ref()
    {
        ctx.request_repaint();
    }
}

/// Documents waiting to be opened (drained; the app calls this every frame).
pub fn take_opened() -> Vec<PathBuf> {
    OPENED
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

/// Lets [`queue_open`] wake this context.
pub fn set_wake(ctx: &eframe::egui::Context) {
    if let Ok(mut w) = WAKE.lock() {
        *w = Some(ctx.clone());
    }
}

/// Before the event loop starts: listen for the system's open-documents requests (macOS
/// Apple Event `odoc`; the documents of the launch itself arrive this way too) and queue the
/// paths given on the command line.
pub fn install_open_documents() {
    let args: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .filter(|a| !a.to_string_lossy().starts_with('-'))
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect();
    queue_open(args);
    #[cfg(target_os = "macos")]
    open_documents::install();
}

#[cfg(target_os = "macos")]
mod open_documents {
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{AllocAnyThread, define_class, msg_send, sel};
    use objc2_foundation::{
        NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter, NSString,
    };
    use std::ptr::NonNull;

    const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
    const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
    const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "BreadboxOpenDocumentsHandler"]
        struct Handler;

        impl Handler {
            #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
            fn handle(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
                // SAFETY: `paramDescriptorForKeyword:` takes an AEKeyword (u32)
                let list: Option<Retained<NSAppleEventDescriptor>> =
                    unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
                let Some(list) = list else { return };
                let paths: Vec<std::path::PathBuf> = (1..=list.numberOfItems())
                    .filter_map(|i| list.descriptorAtIndex(i))
                    .filter_map(|d| d.fileURLValue())
                    .filter_map(|u| u.to_file_path())
                    .collect();
                for p in &paths {
                    log::info!("asked to open {}", p.display());
                }
                super::queue_open(paths);
            }
        }
    );

    /// NSApplication installs its own `odoc` handler while it finishes launching; ours goes in
    /// right after (`NSApplicationWillFinishLaunchingNotification`), before the launch's own
    /// documents are delivered.
    pub fn install() {
        let block = RcBlock::new(|_n: NonNull<NSNotification>| {
            let handler: Retained<Handler> = unsafe { msg_send![Handler::alloc(), init] };
            let manager = NSAppleEventManager::sharedAppleEventManager();
            // SAFETY: a live handler object with the selector's signature; event class and id
            // are FourCharCodes
            unsafe {
                let _: () = msg_send![
                    &*manager,
                    setEventHandler: &*handler,
                    andSelector: sel!(handleOpenDocuments:withReplyEvent:),
                    forEventClass: CORE_EVENT_CLASS,
                    andEventID: OPEN_DOCUMENTS
                ];
            }
            // the event manager does not retain its handlers
            std::mem::forget(handler);
        });
        let name = NSString::from_str("NSApplicationWillFinishLaunchingNotification");
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: the block only touches the main-thread event manager, posted on the main thread
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(&name), None, None, &block)
        };
        std::mem::forget(observer);
    }
}

/// GUI tests answer file dialogs through `$BREADBOX_TEST_PICK` (the path to return).
fn test_pick() -> Option<PathBuf> {
    std::env::var_os("BREADBOX_TEST_PICK").map(PathBuf::from)
}

/// Folder picker (Open project).
pub fn pick_folder(title: &str) -> Option<PathBuf> {
    if let Some(p) = test_pick() {
        return Some(p);
    }
    rfd::FileDialog::new().set_title(title).pick_folder()
}

/// "Save as" for a project folder: returns the destination folder path.
pub fn save_folder(title: &str, suggested: &str) -> Option<PathBuf> {
    if let Some(p) = test_pick() {
        return Some(p);
    }
    rfd::FileDialog::new()
        .set_title(title)
        .set_file_name(suggested)
        .save_file()
}

/// Save dialog for a file with one extension.
pub fn save_file(title: &str, suggested: &str, ext_name: &str, ext: &str) -> Option<PathBuf> {
    if let Some(p) = test_pick() {
        return Some(p);
    }
    rfd::FileDialog::new()
        .set_title(title)
        .set_file_name(suggested)
        .add_filter(ext_name, &[ext])
        .save_file()
}

/// Open dialog for one file type.
pub fn open_file(title: &str, ext_name: &str, exts: &[&str]) -> Option<PathBuf> {
    if let Some(p) = test_pick() {
        return Some(p);
    }
    rfd::FileDialog::new()
        .set_title(title)
        .add_filter(ext_name, exts)
        .pick_file()
}

/// A key equivalent: ⌘ (+ ⇧) and a key (`'N'`, `'1'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub shift: bool,
    pub key: char,
}

/// One of Breadbox's own menu items (the native menu adds the standard macOS items — About,
/// Services, Hide, Quit, Minimize, Zoom, Full Screen — around them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub shortcut: Option<Shortcut>,
}

const fn cmd(key: char) -> Option<Shortcut> {
    Some(Shortcut { shift: false, key })
}
const fn cmd_shift(key: char) -> Option<Shortcut> {
    Some(Shortcut { shift: true, key })
}
const fn m(id: &'static str, title: &'static str, shortcut: Option<Shortcut>) -> MenuSpec {
    MenuSpec {
        id,
        title,
        shortcut,
    }
}

/// Every Breadbox menu item, menu by menu (`APP-4`); [`command_for`] maps each id.
pub const MENUS: &[(&str, &[MenuSpec])] = &[
    ("Breadbox", &[m("about", "About Breadbox", None)]),
    (
        "File",
        &[
            m("new-playground", "New Playground Project…", cmd('N')),
            m("new-pro", "New Pro Project…", None),
            m("open", "Open Project…", cmd('O')),
            m(
                "import-bundle",
                "Import Project Bundle (.breadbox.zip)…",
                None,
            ),
            m("import-cir", "Import .cir Netlist…", None),
            m("save", "Save", cmd('S')),
            m("save-as", "Save As…", cmd_shift('S')),
            m("export-cir", "Export .cir…", None),
            m("close", "Close", cmd('W')),
        ],
    ),
    (
        "Edit",
        &[
            m("undo", "Undo", cmd('Z')),
            m("redo", "Redo", cmd_shift('Z')),
            m("cut", "Cut", cmd('X')),
            m("copy", "Copy", cmd('C')),
            m("paste", "Paste", cmd('V')),
            m("select-all", "Select All", cmd('A')),
            m("find", "Find…", cmd('F')),
        ],
    ),
    (
        "View",
        &[
            m("home", "Home", cmd_shift('H')),
            m("workbench", "Workbench", cmd('1')),
            m("firmware", "Firmware", cmd('2')),
            m("pro", "Pro · Analysis", cmd('3')),
            m("plugins", "Plugins", cmd('4')),
            m("lessons", "Lessons", cmd('5')),
            m("theme-dark", "Dark", None),
            m("theme-light", "Light", None),
        ],
    ),
    (
        "Simulate",
        &[
            m("run", "Run / Pause", cmd('R')),
            m("step-quantum", "Step One Quantum", None),
            m("step-ms", "Step 1 ms", None),
            m("reset", "Reset Bench", None),
            m("reset-mcu", "Reset MCU Only", None),
            m("verify", "Verify Sketch", cmd('B')),
            m("upload", "Upload to Sim", cmd('U')),
            m("analyses", "Run Analyses", None),
        ],
    ),
    (
        "Help",
        &[
            m("docs", "Breadbox Help", None),
            m("help-sdk", "Plugin SDK", None),
            m("help-lessons", "Lesson Format", None),
        ],
    ),
];

/// The spec of a menu item by id.
pub fn menu_spec(id: &str) -> Option<&'static MenuSpec> {
    MENUS
        .iter()
        .flat_map(|(_, items)| items.iter())
        .find(|s| s.id == id)
}

/// Menu ids → commands.
pub fn command_for(id: &str) -> Option<Command> {
    use crate::widgets::help::HelpTopic;
    Some(match id {
        "new-playground" => Command::NewProject(ProjectMode::Playground),
        "new-pro" => Command::NewProject(ProjectMode::Pro),
        "open" => Command::OpenProject,
        "import-bundle" => Command::ImportBundle,
        "save" => Command::Save,
        "save-as" => Command::SaveAs,
        "close" => Command::Close,
        "undo" => Command::Undo,
        "redo" => Command::Redo,
        "cut" => Command::Cut,
        "copy" => Command::Copy,
        "paste" => Command::Paste,
        "select-all" => Command::SelectAll,
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
        "help-sdk" => Command::Help(HelpTopic::PluginSdk),
        "help-lessons" => Command::Help(HelpTopic::LessonFormat),
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use muda::accelerator::{Accelerator, Code, Modifiers};
    use muda::{
        AboutMetadata, CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem,
        Submenu,
    };
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, Sender, channel};

    pub struct NativeMenu {
        _menu: Menu,
        rx: Receiver<Command>,
        theme_dark: CheckMenuItem,
        theme_light: CheckMenuItem,
    }

    fn code(key: char) -> Option<Code> {
        Some(match key {
            'A' => Code::KeyA,
            'B' => Code::KeyB,
            'C' => Code::KeyC,
            'F' => Code::KeyF,
            'H' => Code::KeyH,
            'N' => Code::KeyN,
            'O' => Code::KeyO,
            'R' => Code::KeyR,
            'S' => Code::KeyS,
            'U' => Code::KeyU,
            'V' => Code::KeyV,
            'W' => Code::KeyW,
            'X' => Code::KeyX,
            'Z' => Code::KeyZ,
            '1' => Code::Digit1,
            '2' => Code::Digit2,
            '3' => Code::Digit3,
            '4' => Code::Digit4,
            '5' => Code::Digit5,
            _ => return None,
        })
    }

    /// A Breadbox item from [`MENUS`].
    fn item(id: &str) -> MenuItem {
        let spec = menu_spec(id).expect("menu id in MENUS");
        let accel = spec.shortcut.and_then(|s| {
            let mods = if s.shift {
                Modifiers::META | Modifiers::SHIFT
            } else {
                Modifiers::META
            };
            code(s.key).map(|c| Accelerator::new(mods, c))
        });
        MenuItem::with_id(id, spec.title, true, accel)
    }

    fn sep() -> PredefinedMenuItem {
        PredefinedMenuItem::separator()
    }

    impl NativeMenu {
        pub fn install(ctx: &eframe::egui::Context) -> Option<NativeMenu> {
            let menu = Menu::new();
            // the standard About panel (name, version, the bundle's icon)
            let about = PredefinedMenuItem::about(
                Some("About Breadbox"),
                Some(AboutMetadata {
                    name: Some("Breadbox".into()),
                    version: Some(env!("CARGO_PKG_VERSION").into()),
                    short_version: Some(env!("CARGO_PKG_VERSION").into()),
                    comments: Some("A circuit playground and SPICE workbench.".into()),
                    ..Default::default()
                }),
            );
            let app = Submenu::with_items(
                "Breadbox",
                true,
                &[
                    &about as &dyn IsMenuItem,
                    &sep(),
                    &PredefinedMenuItem::services(None),
                    &sep(),
                    &PredefinedMenuItem::hide(None),
                    &PredefinedMenuItem::hide_others(None),
                    &PredefinedMenuItem::show_all(None),
                    &sep(),
                    &PredefinedMenuItem::quit(None),
                ],
            )
            .ok()?;
            let file = Submenu::with_items(
                "File",
                true,
                &[
                    &item("new-playground") as &dyn IsMenuItem,
                    &item("new-pro"),
                    &item("open"),
                    &item("import-bundle"),
                    &item("import-cir"),
                    &sep(),
                    &item("save"),
                    &item("save-as"),
                    &sep(),
                    &item("export-cir"),
                    &sep(),
                    &item("close"),
                ],
            )
            .ok()?;
            let edit = Submenu::with_items(
                "Edit",
                true,
                &[
                    &item("undo") as &dyn IsMenuItem,
                    &item("redo"),
                    &sep(),
                    &item("cut"),
                    &item("copy"),
                    &item("paste"),
                    &item("select-all"),
                    &sep(),
                    &item("find"),
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
                    &item("home") as &dyn IsMenuItem,
                    &sep(),
                    &item("workbench"),
                    &item("firmware"),
                    &item("pro"),
                    &item("plugins"),
                    &item("lessons"),
                    &sep(),
                    &theme,
                    &sep(),
                    &PredefinedMenuItem::fullscreen(None),
                ],
            )
            .ok()?;
            let sim = Submenu::with_items(
                "Simulate",
                true,
                &[
                    &item("run") as &dyn IsMenuItem,
                    &item("step-quantum"),
                    &item("step-ms"),
                    &item("reset"),
                    &item("reset-mcu"),
                    &sep(),
                    &item("verify"),
                    &item("upload"),
                    &sep(),
                    &item("analyses"),
                ],
            )
            .ok()?;
            let window = Submenu::with_items(
                "Window",
                true,
                &[
                    &PredefinedMenuItem::minimize(None) as &dyn IsMenuItem,
                    &PredefinedMenuItem::maximize(None),
                    &sep(),
                    &PredefinedMenuItem::bring_all_to_front(None),
                ],
            )
            .ok()?;
            let help = Submenu::with_items(
                "Help",
                true,
                &[
                    &item("docs") as &dyn IsMenuItem,
                    &item("help-sdk"),
                    &item("help-lessons"),
                ],
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
    fn every_menu_item_has_a_command_and_unique_shortcut() {
        let mut ids = std::collections::BTreeSet::new();
        let mut keys = std::collections::BTreeSet::new();
        for (menu, items) in MENUS {
            for s in *items {
                assert!(ids.insert(s.id), "duplicate id {}", s.id);
                assert!(
                    command_for(s.id).is_some(),
                    "{menu} → {} has no command",
                    s.id
                );
                if let Some(k) = s.shortcut {
                    assert!(keys.insert((k.shift, k.key)), "{} reuses ⌘{k:?}", s.id);
                }
            }
        }
        // the menus the spec names, in order
        let names: Vec<&str> = MENUS.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            ["Breadbox", "File", "Edit", "View", "Simulate", "Help"]
        );
        assert_eq!(menu_spec("verify").unwrap().shortcut, cmd('B'));
    }

    #[test]
    fn opened_documents_queue_until_taken() {
        let a = PathBuf::from("/tmp/a.breadbox");
        queue_open([a.clone()]);
        assert_eq!(take_opened(), vec![a]);
        assert!(take_opened().is_empty());
    }

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
