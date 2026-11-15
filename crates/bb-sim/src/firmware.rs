//! Loading firmware for a bench: pre-built images (`.hex` / `.elf`) or compiling a project's
//! sketch with `bb_firmware::Toolchain`.

use crate::SimError;
use bb_avr::FirmwareImage;
use bb_circuit::Project;
use std::collections::HashMap;
use std::path::Path;

/// Loads an Intel HEX or ELF file.
pub fn load_image(path: impl AsRef<Path>) -> Result<FirmwareImage, SimError> {
    let p = path.as_ref();
    let bytes =
        std::fs::read(p).map_err(|e| SimError::Firmware(format!("{}: {e}", p.display())))?;
    if bytes.starts_with(b"\x7fELF") {
        FirmwareImage::from_elf(&bytes).map_err(|e| SimError::Firmware(e.to_string()))
    } else {
        let text = String::from_utf8_lossy(&bytes);
        FirmwareImage::from_hex(&text).map_err(|e| SimError::Firmware(e.to_string()))
    }
}

/// Images for every MCU of a project from their `firmware.image` paths (relative to the
/// project folder). MCUs without an image are skipped.
pub fn project_images(project: &Project, dir: impl AsRef<Path>) -> HashMap<String, FirmwareImage> {
    let mut out = HashMap::new();
    for p in &project.parts {
        if let Some(fw) = &p.firmware
            && let Some(img) = &fw.image
            && let Ok(i) = load_image(dir.as_ref().join(img))
        {
            out.insert(p.id.clone(), i);
        }
    }
    out
}

/// Compiles every MCU sketch of a project with arduino-cli (slow: seconds per sketch on a cold
/// cache). Returns images by part id and the build logs.
pub fn compile_project(
    project: &Project,
    dir: impl AsRef<Path>,
    lib: &bb_circuit::Library,
) -> Result<HashMap<String, FirmwareImage>, SimError> {
    let tc = bb_firmware::Toolchain::detect();
    let tc = tc.map_err(|e| SimError::Firmware(e.to_string()))?;
    let mut out = HashMap::new();
    for p in &project.parts {
        let Some(fw) = &p.firmware else { continue };
        let Some(def) = lib.get(&p.part) else {
            continue;
        };
        let Some(mcu) = &def.model.mcu else { continue };
        if mcu.core != "avr" {
            continue;
        }
        let board = bb_firmware::Board::by_id(&mcu.board)
            .ok_or_else(|| SimError::Firmware(format!("unknown board {}", mcu.board)))?;
        let fqbn = fw.fqbn.clone().unwrap_or_else(|| board.fqbn.to_string());
        let opts = bb_firmware::CompileOptions::new(&fqbn);
        let build = tc
            .compile(&dir.as_ref().join(&fw.sketch), &opts)
            .map_err(|e| SimError::Firmware(e.to_string()))?;
        if !build.success {
            return Err(SimError::Firmware(format!(
                "{}: build failed\n{}",
                p.reference, build.log
            )));
        }
        out.insert(
            p.id.clone(),
            build
                .image()
                .map_err(|e| SimError::Firmware(e.to_string()))?,
        );
    }
    Ok(out)
}

/// Builds every API-level board's sketch (ESP32 / Pico) with the host C++ compiler (cached per
/// sketch; the first build of the host core takes a few seconds). Returns executables by part
/// id for [`crate::BenchOptions::host_firmware`].
pub fn compile_host_project(
    project: &Project,
    dir: impl AsRef<Path>,
    lib: &bb_circuit::Library,
) -> Result<HashMap<String, std::path::PathBuf>, SimError> {
    use bb_firmware::host::{HostKind, compile_host};
    let mut out = HashMap::new();
    for p in &project.parts {
        let Some(fw) = &p.firmware else { continue };
        let Some(mcu) = lib.get(&p.part).and_then(|d| d.model.mcu.as_ref()) else {
            continue;
        };
        if mcu.core != "host" {
            continue;
        }
        let kind = HostKind::by_id(fw.fqbn.as_deref().unwrap_or(&mcu.board))
            .or_else(|| HostKind::by_id(&mcu.board))
            .ok_or_else(|| SimError::Firmware(format!("unknown host board {}", mcu.board)))?;
        let build = compile_host(&dir.as_ref().join(&fw.sketch), kind, None)
            .map_err(|e| SimError::Firmware(e.to_string()))?;
        match build.exe {
            Some(exe) if build.success => {
                out.insert(p.id.clone(), exe);
            }
            _ => {
                return Err(SimError::Firmware(format!(
                    "{}: build failed\n{}",
                    p.reference, build.log
                )));
            }
        }
    }
    Ok(out)
}
