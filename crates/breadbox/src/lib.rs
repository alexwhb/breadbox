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
pub mod theme;
pub mod toolchain;
pub mod widgets;

pub use app::{AppOptions, BreadboxApp};

/// Native window options: 1440×900 default, 1280×800 minimum, persisted geometry.
pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Breadbox")
            .with_app_id("dev.breadbox.Breadbox")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1280.0, 800.0]),
        persist_window: true,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    }
}

/// Process start (for the start-up time log, `PLAT-4`).
pub static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Runs the app.
pub fn run() -> eframe::Result<()> {
    STARTED.get_or_init(std::time::Instant::now);
    eframe::run_native(
        "Breadbox",
        native_options(),
        Box::new(|cc| Ok(Box::new(BreadboxApp::new(cc, AppOptions::default())))),
    )
}
