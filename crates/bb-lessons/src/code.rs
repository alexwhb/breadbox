//! Code steps (LES-3 "code"): compile the student's sketch, or the step's reference solution,
//! for the step's MCU with the Arduino toolchain (`bb_firmware`). The scripted solver uses the
//! step's prebuilt image instead, so lessons run without a toolchain; the UI compiles what the
//! student typed and flashes the result (`Command::Flash`), then calls
//! `Session::set_firmware_loaded(true)`.

use crate::LessonError;
use bb_circuit::{Library, Project};
use std::path::PathBuf;

/// The result of compiling a code step's sketch.
#[derive(Debug, Clone)]
pub struct Build {
    pub success: bool,
    pub image: Option<bb_avr::FirmwareImage>,
    /// Compiler errors and warnings (line numbers are the sketch's).
    pub diagnostics: Vec<bb_firmware::Diagnostic>,
    pub log: String,
}

/// The board the MCU part `mcu` compiles for (`arduino:avr:uno`), or `None` for parts without
/// an AVR core (API-level boards are built with `bb_firmware::host`).
pub fn fqbn_for(project: &Project, lib: &Library, mcu: &str) -> Option<String> {
    let part = project.part(mcu)?;
    let def = lib.get(&part.part)?;
    let m = def.model.mcu.as_ref()?;
    if m.core != "avr" {
        return None;
    }
    if let Some(f) = part.firmware.as_ref().and_then(|f| f.fqbn.clone()) {
        return Some(f);
    }
    bb_firmware::Board::by_id(&m.board).map(|b| b.fqbn.to_string())
}

/// Compiles one sketch's source (`.ino` text) for `fqbn`. Errors only when no toolchain is
/// installed or the build could not run; a sketch with compile errors gives `success: false`
/// and diagnostics.
pub fn compile(source: &str, fqbn: &str) -> Result<Build, LessonError> {
    use std::hash::{Hash, Hasher};
    let tc = bb_firmware::Toolchain::detect().map_err(|e| LessonError::Bench(e.to_string()))?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (source, fqbn).hash(&mut h);
    let dir: PathBuf = std::env::temp_dir()
        .join(format!("bb-lessons-{:016x}", h.finish()))
        .join("sketch");
    std::fs::create_dir_all(&dir).map_err(|e| crate::io(&dir, e))?;
    let file = dir.join("sketch.ino");
    std::fs::write(&file, source).map_err(|e| crate::io(&file, e))?;
    let out = tc
        .compile(&dir, &bb_firmware::CompileOptions::new(fqbn))
        .map_err(|e| LessonError::Bench(e.to_string()))?;
    let image = if out.success { out.image().ok() } else { None };
    Ok(Build {
        success: out.success && image.is_some(),
        image,
        diagnostics: out.diagnostics.clone(),
        log: out.log.clone(),
    })
}
