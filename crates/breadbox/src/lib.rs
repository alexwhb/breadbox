//! Breadbox — the eframe/egui app: shell + router, theme, widget kit, screens, and the
//! [`bench::BenchLink`] boundary to the simulator. Module map: `docs/ARCHITECTURE.md` §8.

pub mod app;
pub mod bench;
pub mod canvas;
pub mod circuit;
pub mod export;
pub mod jobs;
pub mod platform;
pub mod plugins;
pub mod project;
pub mod router;
pub mod screens;
pub mod smoke;
pub mod theme;
pub mod toolchain;
pub mod widgets;

pub use app::{AppOptions, BreadboxApp};

/// The bundle identifier (`Info.plist` `CFBundleIdentifier`, `scripts/bundle-macos.sh`).
pub const APP_ID: &str = "app.breadbox.Breadbox";

/// Native window options: 1440×900 default, 1280×800 minimum, persisted geometry. The saved
/// state lives in `~/Library/Application Support/Breadbox/app-state.ron`
/// ([`platform::app_state_file`]).
pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Breadbox")
            .with_app_id(APP_ID)
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1280.0, 800.0]),
        persist_window: true,
        persistence_path: Some(platform::app_state_file()),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    }
}

/// Earlier builds kept their state in eframe's default folder for the old app id; copy it to
/// [`platform::app_state_file`] once so an update keeps the window, recents and last project.
fn migrate_app_state() {
    let new = platform::app_state_file();
    if new.exists() {
        return;
    }
    let old =
        bb_core::paths::home().join("Library/Application Support/dev.breadbox.Breadbox/app.ron");
    if old.is_file()
        && let Some(dir) = new.parent()
        && std::fs::create_dir_all(dir).is_ok()
        && std::fs::copy(&old, &new).is_ok()
    {
        log::info!("moved the saved app state to {}", new.display());
    }
}

/// Process start (for the start-up time log, `PLAT-4`).
pub static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Runs the app.
pub fn run() -> eframe::Result<()> {
    STARTED.get_or_init(std::time::Instant::now);
    migrate_app_state();
    eframe::run_native(
        "Breadbox",
        native_options(),
        Box::new(|cc| Ok(Box::new(BreadboxApp::new(cc, AppOptions::default())))),
    )
}
