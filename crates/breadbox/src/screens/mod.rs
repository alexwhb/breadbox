//! Screens. [`default_registry`] is the single place where screens are registered into their
//! [`ScreenSlot`]s — later agents replace a placeholder here.

pub mod firmware;
pub mod home;
pub mod lessons;
pub mod placeholder;
pub mod plugins;
pub mod pro;
pub mod workbench;

use crate::router::{ScreenRegistry, ScreenSlot};

/// All screens of the app.
pub fn default_registry() -> ScreenRegistry {
    let mut r = ScreenRegistry::default();
    r.register(ScreenSlot::Home, Box::new(home::HomeScreen::default()));
    r.register(
        ScreenSlot::Firmware,
        Box::new(firmware::FirmwareScreen::default()),
    );
    r.register(
        ScreenSlot::ProAnalysis,
        Box::new(pro::AnalysisScreen::default()),
    );
    // ---- slots filled by later phases (replace the placeholder with the real screen) ----
    r.register(
        ScreenSlot::Workbench,
        Box::new(workbench::WorkbenchScreen::default()),
    );
    r.register(
        ScreenSlot::ProSchematic,
        Box::new(pro::schematic::ProSchematicScreen::default()),
    );
    r.register(ScreenSlot::ProPcb, Box::new(pro::pcb::PcbScreen::default()));
    r.register(
        ScreenSlot::Plugins,
        Box::new(plugins::PluginsScreen::default()),
    );
    r.register(
        ScreenSlot::Lessons,
        Box::new(lessons::LessonsScreen::default()),
    );
    r
}
