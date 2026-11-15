//! Verify (compile) in the background: the editor's current buffers are staged into a cache
//! folder (so read-only examples and unsaved edits compile), then `Toolchain::compile` (AVR)
//! or `host::compile_host` (ESP32 / Pico) runs; diagnostics come back mapped to tabs.

use crate::widgets::{CodeDoc, DiagSeverity, EditorDiagnostic};
use bb_firmware::host::{HostKind, compile_host};
use bb_firmware::{BuildOutput, CompileOptions, OutputKind, Severity, Toolchain};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One line of the Output panel.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputLine {
    pub kind: LineTone,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineTone {
    /// Banner / muted.
    Muted,
    Normal,
    Note,
    Warning,
    Error,
    Success,
}

impl OutputLine {
    pub fn new(kind: LineTone, text: impl Into<String>) -> Self {
        OutputLine {
            kind,
            text: text.into(),
        }
    }
}

/// What to build.
#[derive(Clone)]
pub struct BuildRequest {
    pub sketch_name: String,
    /// `(file name, contents)`; the main `.ino` is `<sketch_name>.ino`.
    pub files: Vec<(String, String)>,
    /// Extra files copied verbatim (sketch.yaml).
    pub extra: Vec<(String, Vec<u8>)>,
    pub target: BuildTarget,
    pub debug: bool,
}

#[derive(Clone)]
pub enum BuildTarget {
    Avr {
        toolchain: Arc<Toolchain>,
        fqbn: String,
    },
    Host(HostKind),
}

/// Result of a build.
#[derive(Clone)]
pub struct BuildResult {
    pub success: bool,
    pub lines: Vec<OutputLine>,
    /// Per tab diagnostics.
    pub diagnostics: Vec<(String, EditorDiagnostic)>,
    /// AVR output (ELF/HEX for upload).
    pub avr: Option<Arc<BuildOutput>>,
    /// Host-compiled executable (ESP32 / Pico, API-level) for `FlashHost`.
    pub host_exe: Option<PathBuf>,
    pub debug: bool,
    pub duration: std::time::Duration,
}

/// Staging folder for a sketch: `~/Library/Caches/Breadbox/staging/<key>/<sketch>/`.
pub fn staging_dir(key: &str, sketch_name: &str) -> PathBuf {
    crate::platform::caches()
        .join("staging")
        .join(key)
        .join(sketch_name)
}

/// Writes the request's files into `dir` (removing stale tabs).
pub fn stage(dir: &Path, req: &BuildRequest) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for e in std::fs::read_dir(dir)?.flatten() {
        if e.path().is_file() {
            let _ = std::fs::remove_file(e.path());
        }
    }
    for (name, text) in &req.files {
        std::fs::write(dir.join(name), text)?;
    }
    for (name, bytes) in &req.extra {
        std::fs::write(dir.join(name), bytes)?;
    }
    Ok(())
}

/// Runs the build (blocking; call from a job thread).
pub fn run_build(key: &str, req: &BuildRequest) -> BuildResult {
    let t0 = std::time::Instant::now();
    let dir = staging_dir(key, &req.sketch_name);
    if let Err(e) = stage(&dir, req) {
        return failed(
            format!("could not stage the sketch in {}: {e}", dir.display()),
            req.debug,
        );
    }
    match &req.target {
        BuildTarget::Avr { toolchain, fqbn } => {
            let opts = CompileOptions::new(fqbn).debug(req.debug);
            match toolchain.compile(&dir, &opts) {
                Ok(out) => {
                    let lines = out
                        .output_lines()
                        .into_iter()
                        .enumerate()
                        .map(|(i, (k, t))| {
                            let tone = match k {
                                _ if i == 0 => LineTone::Muted,
                                OutputKind::Info => LineTone::Normal,
                                OutputKind::Warning => LineTone::Warning,
                                OutputKind::Error => LineTone::Error,
                                OutputKind::Success => LineTone::Success,
                            };
                            OutputLine::new(tone, t)
                        })
                        .collect();
                    let diagnostics = map_diags(&out.diagnostics);
                    BuildResult {
                        success: out.success,
                        lines,
                        diagnostics,
                        debug: req.debug,
                        duration: t0.elapsed(),
                        avr: Some(Arc::new(out)),
                        host_exe: None,
                    }
                }
                Err(e) => failed(e.to_string(), req.debug),
            }
        }
        BuildTarget::Host(kind) => {
            let name = match kind {
                HostKind::Esp32DevKit => "ESP32 DevKit v1",
                HostKind::Pico => "Raspberry Pi Pico",
            };
            let mut lines = vec![OutputLine::new(
                LineTone::Muted,
                format!("Compiling sketch for {name} (host C++ · API-level emulation)…"),
            )];
            match compile_host(&dir, *kind, None) {
                Ok(b) => {
                    for d in &b.diagnostics {
                        lines.push(diag_line(d));
                    }
                    if b.success {
                        lines.push(OutputLine::new(
                            LineTone::Success,
                            format!(
                                "Built {} in {:.2} s{}",
                                name,
                                b.duration.as_secs_f64(),
                                if b.cached { " (cached)" } else { "" }
                            ),
                        ));
                    } else if b.diagnostics.is_empty() {
                        for l in b.log.lines().take(30) {
                            lines.push(OutputLine::new(LineTone::Error, l));
                        }
                    }
                    BuildResult {
                        success: b.success,
                        lines,
                        diagnostics: map_diags(&b.diagnostics),
                        avr: None,
                        host_exe: b.exe.clone().filter(|_| b.success),
                        debug: req.debug,
                        duration: t0.elapsed(),
                    }
                }
                Err(e) => failed(e.to_string(), req.debug),
            }
        }
    }
}

fn failed(msg: String, debug: bool) -> BuildResult {
    BuildResult {
        success: false,
        lines: vec![OutputLine::new(LineTone::Error, msg)],
        diagnostics: Vec::new(),
        avr: None,
        host_exe: None,
        debug,
        duration: Default::default(),
    }
}

fn diag_line(d: &bb_firmware::Diagnostic) -> OutputLine {
    let loc = match (&d.tab, d.line, d.column) {
        (Some(t), Some(l), Some(c)) => format!("{t}:{l}:{c}: "),
        (Some(t), Some(l), None) => format!("{t}:{l}: "),
        _ => String::new(),
    };
    let (tone, sev) = match d.severity {
        Severity::Error => (LineTone::Error, "error"),
        Severity::Warning => (LineTone::Warning, "warning"),
        Severity::Note => (LineTone::Note, "note"),
    };
    OutputLine::new(tone, format!("{loc}{sev}: {}", d.message))
}

fn map_diags(ds: &[bb_firmware::Diagnostic]) -> Vec<(String, EditorDiagnostic)> {
    ds.iter()
        .filter_map(|d| {
            Some((
                d.tab.clone()?,
                EditorDiagnostic {
                    line: d.line?,
                    col: d.column,
                    severity: match d.severity {
                        Severity::Error => DiagSeverity::Error,
                        Severity::Warning => DiagSeverity::Warning,
                        Severity::Note => DiagSeverity::Note,
                    },
                    message: d.message.clone(),
                },
            ))
        })
        .collect()
}

/// Applies build diagnostics to the open docs (clearing old ones).
pub fn apply_diagnostics(docs: &mut [CodeDoc], diags: &[(String, EditorDiagnostic)]) {
    for d in docs.iter_mut() {
        d.diagnostics = diags
            .iter()
            .filter(|(tab, _)| tab == &d.name)
            .map(|(_, x)| x.clone())
            .collect();
    }
}

/// `#include <X.h>` names that are not sketch tabs (the Libraries list).
pub fn included_libraries(docs: &[CodeDoc]) -> Vec<String> {
    let tabs: Vec<&str> = docs.iter().map(|d| d.name.as_str()).collect();
    let mut libs = Vec::new();
    for d in docs {
        for line in d.text.lines() {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("#include") {
                let rest = rest.trim();
                let name = rest
                    .trim_start_matches(['<', '"'])
                    .split(['>', '"'])
                    .next()
                    .unwrap_or("")
                    .trim();
                if name.is_empty() || tabs.contains(&name) {
                    continue;
                }
                let lib = name.trim_end_matches(".h").to_string();
                const SYSTEM: &[&str] = &[
                    "Arduino",
                    "avr/pgmspace",
                    "avr/io",
                    "avr/interrupt",
                    "avr/eeprom",
                    "avr/sleep",
                    "avr/wdt",
                    "util/delay",
                    "stdio",
                    "stdlib",
                    "string",
                    "stdarg",
                    "stdint",
                    "stddef",
                    "stdbool",
                    "math",
                    "ctype",
                    "limits",
                    "inttypes",
                    "assert",
                ];
                if !SYSTEM.contains(&lib.as_str()) && !libs.contains(&lib) {
                    libs.push(lib);
                }
            }
        }
    }
    libs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libraries_come_from_includes_not_tabs() {
        let docs = vec![
            CodeDoc::new(
                "garden-node.ino",
                "#include <SoftwareSerial.h>\n#include \"wifi_at.h\"\n#include <ArduinoJson.h>\n",
            ),
            CodeDoc::new(
                "wifi_at.h",
                "#include <Arduino.h>\n#include <SoftwareSerial.h>\n",
            ),
        ];
        assert_eq!(
            included_libraries(&docs),
            vec!["SoftwareSerial", "ArduinoJson"]
        );
    }

    #[test]
    fn staging_replaces_stale_tabs() {
        let dir = std::env::temp_dir()
            .join(format!("bb-stage-{}", std::process::id()))
            .join("s");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.h"), "x").unwrap();
        let req = BuildRequest {
            sketch_name: "s".into(),
            files: vec![("s.ino".into(), "void setup(){}\nvoid loop(){}\n".into())],
            extra: vec![],
            target: BuildTarget::Host(HostKind::Pico),
            debug: false,
        };
        stage(&dir, &req).unwrap();
        assert!(dir.join("s.ino").exists());
        assert!(!dir.join("old.h").exists());
    }
}
