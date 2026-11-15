//! Screen routing.
//!
//! A [`Route`] says which screen is visible; each route maps to a [`ScreenSlot`] in the
//! [`ScreenRegistry`]. Every slot holds a `Box<dyn Screen>`. The slots for screens built by
//! later phases (Workbench, Pro → Schematic, Pro → PCB, Plugins, Lessons) start as
//! [`crate::screens::placeholder::Placeholder`]s; a later agent replaces one with
//! `registry.register(ScreenSlot::Workbench, Box::new(MyWorkbench::new(..)))` in
//! [`crate::screens::default_registry`] — nothing else in the shell changes.

use crate::app::AppCtx;
use crate::project::ProjectMode;
use crate::theme::ThemeChoice;
use eframe::egui::Ui;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Pro-mode tabs (`PRO-1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ProTab {
    Schematic,
    #[default]
    Analysis,
    Pcb,
}

/// Which screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Route {
    #[default]
    Home,
    Workbench,
    Firmware,
    Pro(ProTab),
    Plugins,
    Lessons,
}

impl Route {
    pub fn slot(self) -> ScreenSlot {
        match self {
            Route::Home => ScreenSlot::Home,
            Route::Workbench => ScreenSlot::Workbench,
            Route::Firmware => ScreenSlot::Firmware,
            Route::Pro(ProTab::Schematic) => ScreenSlot::ProSchematic,
            Route::Pro(ProTab::Analysis) => ScreenSlot::ProAnalysis,
            Route::Pro(ProTab::Pcb) => ScreenSlot::ProPcb,
            Route::Plugins => ScreenSlot::Plugins,
            Route::Lessons => ScreenSlot::Lessons,
        }
    }

    /// Section name for the breadcrumb menu.
    pub fn title(self) -> &'static str {
        match self {
            Route::Home => "Home",
            Route::Workbench => "Workbench",
            Route::Firmware => "Firmware",
            Route::Pro(_) => "Pro",
            Route::Plugins => "Plugins",
            Route::Lessons => "Lessons",
        }
    }

    /// Screens reachable from the breadcrumb section menu / View menu, in order.
    pub const SECTIONS: [Route; 5] = [
        Route::Workbench,
        Route::Firmware,
        Route::Pro(ProTab::Analysis),
        Route::Plugins,
        Route::Lessons,
    ];

    /// Routes that only make sense with a project open.
    pub fn needs_project(self) -> bool {
        matches!(self, Route::Workbench | Route::Firmware | Route::Pro(_))
    }
}

/// A registry slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScreenSlot {
    Home,
    Workbench,
    Firmware,
    ProSchematic,
    ProAnalysis,
    ProPcb,
    Plugins,
    Lessons,
}

impl ScreenSlot {
    pub const ALL: [ScreenSlot; 8] = [
        ScreenSlot::Home,
        ScreenSlot::Workbench,
        ScreenSlot::Firmware,
        ScreenSlot::ProSchematic,
        ScreenSlot::ProAnalysis,
        ScreenSlot::ProPcb,
        ScreenSlot::Plugins,
        ScreenSlot::Lessons,
    ];

    pub fn key(self) -> &'static str {
        match self {
            ScreenSlot::Home => "home",
            ScreenSlot::Workbench => "workbench",
            ScreenSlot::Firmware => "firmware",
            ScreenSlot::ProSchematic => "pro-schematic",
            ScreenSlot::ProAnalysis => "pro-analysis",
            ScreenSlot::ProPcb => "pro-pcb",
            ScreenSlot::Plugins => "plugins",
            ScreenSlot::Lessons => "lessons",
        }
    }
}

/// App-level commands from the menu bar, shortcuts and screens.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    NewProject(ProjectMode),
    OpenProject,
    OpenPath(PathBuf),
    Save,
    SaveAs,
    Undo,
    Redo,
    /// ⌘W: close the project (or the window on Home).
    Close,
    Find,
    Navigate(Route),
    SetTheme(ThemeChoice),
    /// Simulate → Run / Pause.
    RunPause,
    /// Simulate → Reset.
    ResetSim,
    /// Simulate → Reset MCU Only: every board restarts its firmware; the circuit, world and
    /// radios keep their state (`SIM-3`).
    ResetMcu,
    /// Simulate → Step: pause, then run exactly one sync quantum or one millisecond (`SIM-3`).
    StepSim(StepSize),
    /// File → Import .cir…: an ngspice / LTspice netlist becomes a new Pro project (`PRJ-4`).
    ImportCir,
    Verify,
    Upload,
    RunAnalyses,
    ExportCir,
    About,
    /// Help → the guide (the Help window on its first topic).
    OpenDocs,
    /// Help → a document in the Help window.
    Help(crate::widgets::help::HelpTopic),
    /// File → Import Project Bundle (`.breadbox.zip`, `PRJ-5`).
    ImportBundle,
    /// Edit → Cut / Copy / Paste / Select All (text fields, the bench's parts).
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// How far Simulate → Step advances the paused bench.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepSize {
    /// One co-simulation sync quantum (project setting, 1 µs by default).
    Quantum,
    /// One millisecond of simulated time.
    Millisecond,
}

/// A screen. All methods but `ui` are optional.
pub trait Screen {
    /// Breadcrumb text after the project name (`"Firmware · U1"`); `None` hides it.
    fn crumb(&self, _app: &AppCtx) -> Option<String> {
        None
    }
    /// Top-bar contents after the breadcrumb and mode badge. The ui is a horizontal layout
    /// spanning the rest of the 56 px bar; use a right-to-left child for right-aligned groups.
    fn header(&mut self, _ui: &mut Ui, _app: &mut AppCtx) {}
    /// The screen body below the top bar.
    fn ui(&mut self, ui: &mut Ui, app: &mut AppCtx);
    /// A menu / shortcut command; return true if the screen handled it.
    fn command(&mut self, _cmd: &Command, _app: &mut AppCtx) -> bool {
        false
    }
    /// Called after the open project changed (also on first show).
    fn project_changed(&mut self, _app: &mut AppCtx) {}
    /// Called when the screen becomes visible.
    fn on_enter(&mut self, _app: &mut AppCtx) {}
    /// Write pending edits now (the app is quitting, `APP-5`): debounced autosaves must not
    /// lose the last keystrokes.
    fn flush(&mut self, _app: &mut AppCtx) {}
    /// Called when another screen replaces this one (pause private work, e.g. a lesson bench).
    fn on_leave(&mut self, _app: &mut AppCtx) {}
    /// The concrete screen (tests and cross-screen glue downcast through this).
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }
    /// Per-screen state persisted with the app (window/screen restore, `APP-5`).
    fn save_state(&self) -> Option<serde_json::Value> {
        None
    }
    fn load_state(&mut self, _state: &serde_json::Value) {}
}

/// Slot → screen.
#[derive(Default)]
pub struct ScreenRegistry {
    screens: HashMap<ScreenSlot, Box<dyn Screen>>,
}

impl ScreenRegistry {
    pub fn register(&mut self, slot: ScreenSlot, screen: Box<dyn Screen>) {
        self.screens.insert(slot, screen);
    }

    pub fn get_mut(&mut self, slot: ScreenSlot) -> Option<&mut (dyn Screen + 'static)> {
        self.screens.get_mut(&slot).map(|b| b.as_mut())
    }

    pub fn get(&self, slot: ScreenSlot) -> Option<&(dyn Screen + 'static)> {
        self.screens.get(&slot).map(|b| b.as_ref())
    }

    pub fn slots(&self) -> impl Iterator<Item = ScreenSlot> + '_ {
        ScreenSlot::ALL
            .into_iter()
            .filter(|s| self.screens.contains_key(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_map_to_slots() {
        assert_eq!(Route::Pro(ProTab::Pcb).slot(), ScreenSlot::ProPcb);
        assert_eq!(Route::Firmware.slot(), ScreenSlot::Firmware);
        assert!(Route::Firmware.needs_project());
        assert!(!Route::Lessons.needs_project());
        let j = serde_json::to_string(&Route::Pro(ProTab::Analysis)).unwrap();
        assert_eq!(
            serde_json::from_str::<Route>(&j).unwrap(),
            Route::Pro(ProTab::Analysis)
        );
    }
}
