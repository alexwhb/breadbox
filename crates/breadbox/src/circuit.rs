//! The open project's circuit as one shared, undoable document: `bb_circuit::edit::Editor`
//! plus the parts library and cached derived data (connectivity, ERC/DRC, Pro netlist).
//!
//! The Workbench (breadboard + schematic), Pro → Schematic and Pro → Analysis all edit and
//! read this one model, so an edit in any view shows up everywhere ("same netlist"). The app
//! pushes electrical changes to the running bench (`SimBench::update_project`) and autosaves
//! user projects (examples stay read-only in memory).

use anyhow::{Context as _, Result};
use bb_circuit::edit::Editor;
use bb_circuit::erc::{self, ErcItem};
use bb_circuit::{Clipboard, Connectivity, Library, Project};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Debounce for autosave after the last edit (`APP-5`).
pub const AUTOSAVE_AFTER: Duration = Duration::from_millis(1200);

/// ERC + DRC results of one circuit revision.
#[derive(Debug, Clone, Default)]
pub struct Checks {
    pub erc: Vec<ErcItem>,
    pub drc: Vec<ErcItem>,
}

impl Checks {
    /// `ERC 1 warning · DRC clean`.
    pub fn summary(&self) -> (String, String) {
        (
            erc::summary("ERC", &self.erc),
            erc::summary("DRC", &self.drc),
        )
    }
}

pub struct CircuitDoc {
    pub editor: Editor,
    pub lib: Arc<Library>,
    pub dir: PathBuf,
    pub read_only: bool,
    conn: Option<(u64, Arc<Connectivity>)>,
    checks: Option<(u64, Arc<Checks>)>,
    saved_generation: u64,
    last_edit: Option<Instant>,
    /// Generation last seen by `changed_since`.
    pub clipboard: Clipboard,
    /// The Pro netlist was edited by hand and could not be mapped back (`PRO-2` "detached").
    pub detached_netlist: Option<String>,
}

impl CircuitDoc {
    pub fn load(dir: &Path, read_only: bool) -> Result<CircuitDoc> {
        let project = Project::load(dir).with_context(|| format!("{}", dir.display()))?;
        Ok(CircuitDoc::new(project, dir, read_only))
    }

    pub fn new(mut project: Project, dir: &Path, read_only: bool) -> CircuitDoc {
        let lib = Library::builtin();
        // parts never placed in the schematic get a place (WB-7 auto-placement)
        project.auto_place_schematic(&lib);
        let detached = project.spice.detached.clone();
        CircuitDoc {
            editor: Editor::new(project),
            lib,
            dir: dir.to_path_buf(),
            read_only,
            conn: None,
            checks: None,
            saved_generation: 0,
            last_edit: None,
            clipboard: Clipboard::default(),
            detached_netlist: detached,
        }
    }

    /// Replaces the parts library (plugin parts); caches are recomputed.
    pub fn set_library(&mut self, lib: Arc<Library>) {
        self.lib = lib;
        self.conn = None;
        self.checks = None;
    }

    pub fn project(&self) -> &Project {
        &self.editor.project
    }

    pub fn generation(&self) -> u64 {
        self.editor.generation
    }

    pub fn electrical_generation(&self) -> u64 {
        self.editor.electrical_generation
    }

    /// Call after every edit through `editor` (autosave timer). Parts added on the bench get
    /// a place in the schematic right away (WB-7 auto-placement).
    pub fn touched(&mut self) {
        self.last_edit = Some(Instant::now());
        let lib = self.lib.clone();
        let unplaced = self.editor.project.parts.iter().any(|p| {
            p.schematic.is_none()
                && lib
                    .get(&p.part)
                    .is_some_and(|d| d.schematic.symbol != "none")
        });
        if unplaced && self.editor.project.auto_place_schematic(&lib) > 0 {
            self.editor.generation += 1;
            self.editor.electrical_generation += 1;
        }
    }

    /// Connectivity of the current revision (cached per electrical generation).
    pub fn connectivity(&mut self) -> Arc<Connectivity> {
        let g = self.editor.electrical_generation;
        if let Some((cg, c)) = &self.conn
            && *cg == g
        {
            return c.clone();
        }
        let c = Arc::new(Connectivity::compute(&self.editor.project, &self.lib));
        self.conn = Some((g, c.clone()));
        c
    }

    /// ERC/DRC of the current revision (cached).
    pub fn checks(&mut self) -> Arc<Checks> {
        let g = self.editor.electrical_generation;
        if let Some((cg, c)) = &self.checks
            && *cg == g
        {
            return c.clone();
        }
        let conn = self.connectivity();
        let c = Arc::new(Checks {
            erc: erc::erc(&self.editor.project, &self.lib, &conn),
            drc: erc::drc(&self.editor.project, &self.lib, &conn),
        });
        self.checks = Some((g, c.clone()));
        c
    }

    pub fn dirty(&self) -> bool {
        self.editor.generation != self.saved_generation
    }

    /// Writes `project.json` (atomic).
    pub fn save(&mut self) -> Result<()> {
        let mut p = self.editor.project.clone();
        p.spice.detached = self.detached_netlist.clone();
        p.save(&self.dir)?;
        self.saved_generation = self.editor.generation;
        self.last_edit = None;
        Ok(())
    }

    /// Saves user projects 1.2 s after the last edit. Returns `Some` when it saved.
    pub fn autosave(&mut self) -> Option<Result<()>> {
        if self.read_only || !self.dirty() {
            return None;
        }
        let due = self.last_edit.is_none_or(|t| t.elapsed() >= AUTOSAVE_AFTER);
        due.then(|| self.save())
    }

    /// Time until the pending autosave is due (for a repaint request).
    pub fn autosave_due_in(&self) -> Option<Duration> {
        if self.read_only || !self.dirty() {
            return None;
        }
        Some(
            self.last_edit
                .map(|t| AUTOSAVE_AFTER.saturating_sub(t.elapsed()))
                .unwrap_or_default(),
        )
    }

    /// The Pro netlist text synced from the schematic (`PRO-2`).
    pub fn pro_netlist_text(&self) -> Result<String> {
        bb_circuit::spice_io::pro_display_text(&self.editor.project, &self.lib)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    pub fn undo(&mut self) -> bool {
        let r = self.editor.undo();
        if r {
            self.touched();
        }
        r
    }

    pub fn redo(&mut self) -> bool {
        let r = self.editor.redo();
        if r {
            self.touched();
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples")
            .join(name)
    }

    #[test]
    fn garden_node_loads_with_cached_checks_and_undo() {
        let mut d = CircuitDoc::load(&example("garden-node"), true).unwrap();
        let c1 = d.connectivity();
        let c2 = d.connectivity();
        assert!(Arc::ptr_eq(&c1, &c2), "cached per generation");
        assert!(c1.net_by_name("A0").is_some());
        let (erc, drc) = d.checks().summary();
        assert!(erc.starts_with("ERC"), "{erc}");
        assert_eq!(drc, "DRC clean");
        let lib = d.lib.clone();
        d.editor.add_part(&lib, "resistor", 40.0, 30.0).unwrap();
        d.touched();
        assert!(d.dirty());
        assert!(!Arc::ptr_eq(&c1, &d.connectivity()));
        assert!(d.undo());
        assert!(d.redo());
        assert!(d.autosave().is_none(), "examples never autosave");
    }

    #[test]
    fn user_projects_autosave_after_the_debounce() {
        let tmp = std::env::temp_dir().join(format!("bb-doc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let p = Project::load(example("blink")).unwrap();
        p.save(&tmp).unwrap();
        let mut d = CircuitDoc::load(&tmp, false).unwrap();
        let lib = d.lib.clone();
        d.editor.add_part(&lib, "led", 50.0, 10.0).unwrap();
        d.touched();
        assert!(d.autosave().is_none(), "debounced");
        std::thread::sleep(AUTOSAVE_AFTER + Duration::from_millis(50));
        assert!(d.autosave().unwrap().is_ok());
        let back = Project::load(&tmp).unwrap();
        assert!(back.parts.iter().any(|p| p.part == "led"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
