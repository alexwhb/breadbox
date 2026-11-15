//! Screens. [`default_registry`] is the single place where screens are registered into their
//! [`ScreenSlot`]s — later agents replace a placeholder here.

pub mod firmware;
pub mod home;
pub mod placeholder;
pub mod pro;

use crate::router::{ScreenRegistry, ScreenSlot};
use crate::widgets::Icon;
use placeholder::Placeholder;

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
        Box::new(Placeholder::new(
            ScreenSlot::Workbench,
            "Workbench",
            Icon::Breadboard,
            "The 2.5D breadboard bench and the schematic view of the same circuit: parts palette, jumper wires, live probes, world and inspector rail, serial monitor dock.",
            "ui-bench (phase 4)",
        )),
    );
    r.register(
        ScreenSlot::ProSchematic,
        Box::new(Placeholder::new(
            ScreenSlot::ProSchematic,
            "Schematic",
            Icon::SegSchematic,
            "The full schematic editor (IEC/ANSI symbols, net labels, auto-placement) over the shared netlist.",
            "ui-bench (phase 4)",
        )),
    );
    r.register(
        ScreenSlot::ProPcb,
        Box::new(Placeholder::new(
            ScreenSlot::ProPcb,
            "PCB",
            Icon::Chip,
            "Footprints, board outline, ratsnest, two-layer routing with vias, DRC and Gerber export.",
            "ui-plug-les (phase 4)",
        )),
    );
    r.register(
        ScreenSlot::Plugins,
        Box::new(Placeholder::new(
            ScreenSlot::Plugins,
            "Plugins",
            Icon::Plug,
            "Installed instruments, parts, radios and world models; the catalog; plugin.json and the SDK docs.",
            "ui-plug-les (phase 4)",
        )),
    );
    r.register(
        ScreenSlot::Lessons,
        Box::new(Placeholder::new(
            ScreenSlot::Lessons,
            "Lessons",
            Icon::Book,
            "Lesson tracks with a live bench, step panel, predictions and measurements, and the lesson editor.",
            "ui-plug-les (phase 4)",
        )),
    );
    r
}
