//! Sketch compilation through arduino-cli: build cache, diagnostics mapped back to the user's
//! tabs (including `.ino` line remapping through the generated `.ino.cpp`'s `#line`
//! directives) and the size report.

use crate::FirmwareError;
use crate::boards::Board;
use crate::process::{self, CancelToken};
use crate::toolchain::Toolchain;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub enum Warnings {
    None,
    #[default]
    Default,
    More,
    All,
}

impl Warnings {
    fn arg(self) -> &'static str {
        match self {
            Warnings::None => "none",
            Warnings::Default => "default",
            Warnings::More => "more",
            Warnings::All => "all",
        }
    }
}

/// Default budget for one `arduino-cli compile` (a cold build of a big sketch with libraries
/// takes well under a minute).
pub const COMPILE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompileOptions {
    pub fqbn: String,
    /// Build for source-level debugging: `-Og -fno-lto` (locals stay visible, lines map 1:1).
    pub debug: bool,
    /// Build directory; `None` = a per-sketch cache dir (fast re-verify).
    pub build_dir: Option<PathBuf>,
    pub warnings: Warnings,
    /// Extra `--build-property` values.
    pub properties: Vec<String>,
    /// Time budget ([`COMPILE_TIMEOUT`]); over it the compile is stopped and
    /// [`FirmwareError::Timeout`] returned.
    pub timeout: Duration,
    /// Stops the compile (and every compiler process it started) when cancelled:
    /// [`FirmwareError::Cancelled`].
    #[serde(skip)]
    pub cancel: Option<CancelToken>,
}

impl CompileOptions {
    pub fn new(fqbn: &str) -> Self {
        CompileOptions {
            fqbn: fqbn.to_string(),
            debug: false,
            build_dir: None,
            warnings: Warnings::Default,
            properties: Vec::new(),
            timeout: COMPILE_TIMEOUT,
            cancel: None,
        }
    }
    pub fn debug(mut self, on: bool) -> Self {
        self.debug = on;
        self
    }
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }
    pub fn cancel(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

/// A compiler/linker message mapped to the user's sources.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diagnostic {
    /// Absolute path of the source file, if known.
    pub file: Option<PathBuf>,
    /// File name relative to the sketch folder when the file is one of the sketch's tabs.
    pub tab: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub severity: Severity,
    /// First line of the message.
    pub message: String,
    /// Remaining lines (source excerpt, caret).
    pub excerpt: Vec<String>,
    /// "In function 'void loop()':" style context.
    pub context: Vec<String>,
}

/// Flash / SRAM usage, as arduino-cli computes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SizeReport {
    /// .text + .data (+ .bootloader) bytes.
    pub program: usize,
    pub program_max: usize,
    /// .data + .bss + .noinit bytes.
    pub data: usize,
    pub data_max: usize,
    pub eeprom: usize,
}

/// Groups digits like the design does: 32256 → "32,256".
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl SizeReport {
    /// Section sizes from an AVR ELF (like `avr-size -A`) with the board's limits.
    pub fn from_elf(
        elf: &[u8],
        program_max: usize,
        data_max: usize,
    ) -> Result<SizeReport, FirmwareError> {
        use object::{Object, ObjectSection};
        let file = object::File::parse(elf).map_err(|e| FirmwareError::Elf(e.to_string()))?;
        let size = |names: &[&str]| -> usize {
            file.sections()
                .filter(|s| s.name().is_ok_and(|n| names.contains(&n)))
                .map(|s| s.size() as usize)
                .sum()
        };
        Ok(SizeReport {
            program: size(&[".text", ".data", ".bootloader"]),
            program_max,
            data: size(&[".data", ".bss", ".noinit"]),
            data_max,
            eeprom: size(&[".eeprom"]),
        })
    }

    /// Integer percentage, truncated like arduino-cli.
    pub fn program_percent(&self) -> usize {
        (self.program * 100)
            .checked_div(self.program_max)
            .unwrap_or(0)
    }
    pub fn data_percent(&self) -> usize {
        (self.data * 100).checked_div(self.data_max).unwrap_or(0)
    }

    /// Exactly what arduino-cli prints.
    pub fn arduino_lines(&self) -> Vec<String> {
        vec![
            format!(
                "Sketch uses {} bytes ({}%) of program storage space. Maximum is {} bytes.",
                self.program,
                self.program_percent(),
                self.program_max
            ),
            format!(
                "Global variables use {} bytes ({}%) of dynamic memory, leaving {} bytes for local variables. Maximum is {} bytes.",
                self.data,
                self.data_percent(),
                self.data_max as i64 - self.data as i64,
                self.data_max
            ),
        ]
    }

    /// The firmware screen's variant with thousands separators
    /// ("Sketch uses 9,842 bytes (30%) of program storage space. Maximum is 32,256 bytes.").
    pub fn display_lines(&self) -> Vec<String> {
        let leaving = self.data_max.saturating_sub(self.data);
        vec![
            format!(
                "Sketch uses {} bytes ({}%) of program storage space. Maximum is {} bytes.",
                thousands(self.program),
                self.program_percent(),
                thousands(self.program_max)
            ),
            format!(
                "Global variables use {} bytes ({}%) of dynamic memory, leaving {} bytes for local variables. Maximum is {} bytes.",
                thousands(self.data),
                self.data_percent(),
                thousands(leaving),
                thousands(self.data_max)
            ),
        ]
    }

    /// arduino-cli's warnings/errors about the sizes.
    pub fn problems(&self) -> Vec<(Severity, String)> {
        let mut v = Vec::new();
        if self.program > self.program_max {
            v.push((
                Severity::Error,
                "Sketch too big; see https://support.arduino.cc/hc/en-us/articles/360013825179 for tips on reducing it."
                    .to_string(),
            ));
        }
        if self.data > self.data_max {
            v.push((
                Severity::Error,
                "Not enough memory; see https://support.arduino.cc/hc/en-us/articles/360013825179 for tips on reducing your footprint."
                    .to_string(),
            ));
        } else if self.data * 100 > self.data_max * 75 {
            v.push((
                Severity::Warning,
                "Low memory available, stability problems may occur.".to_string(),
            ));
        }
        v
    }
}

/// Kind of a line in the firmware screen's output panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OutputKind {
    Info,
    Warning,
    Error,
    Success,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BuildOutput {
    pub success: bool,
    pub fqbn: String,
    pub elf: Option<Vec<u8>>,
    pub hex: Option<String>,
    pub size: Option<SizeReport>,
    pub diagnostics: Vec<Diagnostic>,
    /// Raw compiler stdout + stderr.
    pub log: String,
    pub build_dir: PathBuf,
    #[serde(skip)]
    pub duration: Duration,
    /// "Compiling sketch for UNO R3 (avr-gcc 7.3.0 -Os -mmcu=atmega328p)…"
    pub banner: String,
}

impl BuildOutput {
    /// Parses the ELF into a flashable image.
    pub fn image(&self) -> Result<bb_avr::FirmwareImage, FirmwareError> {
        let elf = self.elf.as_ref().ok_or(FirmwareError::NoOutput)?;
        bb_avr::FirmwareImage::from_elf(elf).map_err(|e| FirmwareError::Elf(e.to_string()))
    }

    /// Lines for the output panel: banner, errors/warnings, size report.
    pub fn output_lines(&self) -> Vec<(OutputKind, String)> {
        let mut v = vec![(OutputKind::Info, self.banner.clone())];
        for d in &self.diagnostics {
            let loc = match (&d.tab, d.line, d.column) {
                (Some(t), Some(l), Some(c)) => format!("{t}:{l}:{c}: "),
                (Some(t), Some(l), None) => format!("{t}:{l}: "),
                _ => String::new(),
            };
            let kind = match d.severity {
                Severity::Error => OutputKind::Error,
                Severity::Warning => OutputKind::Warning,
                Severity::Note => OutputKind::Info,
            };
            let sev = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Note => "note",
            };
            v.push((kind, format!("{loc}{sev}: {}", d.message)));
        }
        if let Some(s) = &self.size {
            for l in s.display_lines() {
                v.push((OutputKind::Info, l));
            }
            for (sev, msg) in s.problems() {
                v.push((
                    if sev == Severity::Error {
                        OutputKind::Error
                    } else {
                        OutputKind::Warning
                    },
                    msg,
                ));
            }
        }
        v
    }
}

/// Cache directory for builds: `$BREADBOX_BUILD_CACHE`, else
/// `~/Library/Caches/Breadbox/builds` ([`bb_core::paths::caches`]).
pub fn build_cache_root() -> PathBuf {
    if let Some(d) = std::env::var_os("BREADBOX_BUILD_CACHE") {
        return PathBuf::from(d);
    }
    bb_core::paths::caches().join("builds")
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Debug-build properties: optimise for debugging and keep DWARF un-LTO'd.
pub fn debug_properties() -> Vec<String> {
    [
        "compiler.c.extra_flags",
        "compiler.cpp.extra_flags",
        "compiler.c.elf.extra_flags",
    ]
    .iter()
    .map(|k| format!("{k}=-Og -fno-lto"))
    .collect()
}

/// The output panel's first line.
pub fn banner(board_name: &str, gcc: Option<&str>, mcu: &str, debug: bool) -> String {
    let opt = if debug { "-Og" } else { "-Os" };
    match gcc {
        Some(v) => format!("Compiling sketch for {board_name} (avr-gcc {v} {opt} -mmcu={mcu})…"),
        None => format!("Compiling sketch for {board_name} ({opt} -mmcu={mcu})…"),
    }
}

impl Toolchain {
    /// Compiles the sketch folder (whose main file is `<folder>/<folder>.ino`).
    pub fn compile(
        &self,
        sketch_dir: &Path,
        opts: &CompileOptions,
    ) -> Result<BuildOutput, FirmwareError> {
        let start = Instant::now();
        let sketch_dir = sketch_dir
            .canonicalize()
            .map_err(|e| FirmwareError::Io(format!("{}: {e}", sketch_dir.display())))?;
        let board = Board::by_fqbn(&opts.fqbn);
        let build_dir = opts.build_dir.clone().unwrap_or_else(|| {
            let key = format!("{}|{}|{}", sketch_dir.display(), opts.fqbn, opts.debug);
            let name = sketch_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            build_cache_root().join(format!("{name}-{:016x}", fnv(&key)))
        });
        std::fs::create_dir_all(&build_dir).map_err(|e| FirmwareError::Io(e.to_string()))?;
        let mut cmd = self.command();
        cmd.arg("compile")
            .arg("--fqbn")
            .arg(&opts.fqbn)
            .arg("--build-path")
            .arg(&build_dir)
            .arg("--warnings")
            .arg(opts.warnings.arg())
            .args(["--format", "json"])
            .args(self.build_property_args());
        let mut props = opts.properties.clone();
        if opts.debug {
            props.extend(debug_properties());
        }
        for p in &props {
            cmd.arg("--build-property").arg(p);
        }
        cmd.arg(&sketch_dir);
        let out = process::run(cmd, opts.timeout, opts.cancel.as_ref(), &mut |_| {})?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let json: Value = serde_json::from_str(&stdout).unwrap_or(Value::Null);
        let compiler_out = json["compiler_out"].as_str().unwrap_or("").to_string();
        let compiler_err = json["compiler_err"].as_str().unwrap_or("").to_string();
        let mut log = compiler_out;
        log.push_str(&compiler_err);
        if json.is_null() {
            log.push_str(&stdout);
            log.push_str(&String::from_utf8_lossy(&out.stderr));
        }
        let success = json["success"].as_bool().unwrap_or(out.status.success());
        let remap = InoMap::load(&build_dir, &sketch_dir);
        let mut diagnostics = match json["builder_result"]["diagnostics"].as_array() {
            Some(a) if !a.is_empty() => a.iter().filter_map(diag_from_json).collect(),
            _ => parse_gcc_diagnostics(&compiler_err),
        };
        if !success && diagnostics.iter().all(|d| d.severity != Severity::Error) {
            let msg = json["error"]
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    if e.is_empty() {
                        "build failed".to_string()
                    } else {
                        e
                    }
                });
            diagnostics.push(Diagnostic {
                file: None,
                tab: None,
                line: None,
                column: None,
                severity: Severity::Error,
                message: msg,
                excerpt: vec![],
                context: vec![],
            });
        }
        for d in &mut diagnostics {
            remap.apply(d);
        }
        let name = sketch_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (elf, hex) = if success {
            (
                std::fs::read(build_dir.join(format!("{name}.ino.elf"))).ok(),
                std::fs::read_to_string(build_dir.join(format!("{name}.ino.hex"))).ok(),
            )
        } else {
            (None, None)
        };
        let (pmax, dmax) = sizes_from_json(&json).unwrap_or_else(|| {
            board
                .as_ref()
                .map(|b| (b.max_program, b.max_data))
                .unwrap_or((0, 0))
        });
        let size = elf
            .as_ref()
            .and_then(|e| SizeReport::from_elf(e, pmax, dmax).ok());
        let (bname, mcu) = board
            .as_ref()
            .map(|b| (b.name.to_string(), b.mcu.gcc_name().to_string()))
            .unwrap_or_else(|| (opts.fqbn.clone(), "?".to_string()));
        Ok(BuildOutput {
            success: success && elf.is_some(),
            fqbn: opts.fqbn.clone(),
            elf,
            hex,
            size,
            diagnostics,
            log,
            build_dir,
            duration: start.elapsed(),
            banner: banner(&bname, self.avr_gcc_version.as_deref(), &mcu, opts.debug),
        })
    }
}

fn sizes_from_json(v: &Value) -> Option<(usize, usize)> {
    let a = v["builder_result"]["executable_sections_size"].as_array()?;
    let get = |n: &str| {
        a.iter()
            .find(|s| s["name"] == n)
            .and_then(|s| s["max_size"].as_u64())
            .map(|x| x as usize)
    };
    Some((get("text")?, get("data")?))
}

fn severity(s: &str) -> Severity {
    match s.to_ascii_lowercase().as_str() {
        "error" | "fatal error" => Severity::Error,
        "warning" => Severity::Warning,
        _ => Severity::Note,
    }
}

fn diag_from_json(d: &Value) -> Option<Diagnostic> {
    let msg = d["message"].as_str()?;
    let mut lines = msg.lines();
    let first = lines.next().unwrap_or("").to_string();
    Some(Diagnostic {
        file: d["file"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from),
        tab: None,
        line: d["line"].as_u64().filter(|&l| l > 0).map(|l| l as u32),
        column: d["column"].as_u64().filter(|&c| c > 0).map(|c| c as u32),
        severity: severity(d["severity"].as_str().unwrap_or("error")),
        message: first,
        excerpt: lines.map(|s| s.to_string()).collect(),
        context: d["context"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|c| c["message"].as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Parses GCC-style `file:line:col: severity: message` output (plus linker
/// `file:line: undefined reference …`).
pub fn parse_gcc_diagnostics(text: &str) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = Vec::new();
    let mut context: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(d) = parse_gcc_line(line) {
            let mut d = d;
            d.context = std::mem::take(&mut context);
            out.push(d);
        } else if line.ends_with(':')
            && (line.contains("In function")
                || line.contains("In member function")
                || line.contains("At global scope")
                || line.contains("In file included from"))
        {
            context.push(
                line.rsplit_once(": ")
                    .map(|x| x.1)
                    .unwrap_or(line)
                    .to_string(),
            );
        } else if let Some(last) = out.last_mut()
            && (line.starts_with(' ') || line.trim_start().starts_with('^'))
        {
            last.excerpt.push(line.to_string());
        }
    }
    out
}

fn parse_gcc_line(line: &str) -> Option<Diagnostic> {
    // Split off "file:line[:col]: rest" scanning for ":<digits>:".
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            let rest = &line[i + 1..];
            let n_end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            if n_end > 0 && rest[n_end..].starts_with(':') {
                let file = &line[..i];
                let ln: u32 = rest[..n_end].parse().ok()?;
                let mut rest2 = &rest[n_end + 1..];
                let mut col = None;
                let c_end = rest2.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
                if c_end > 0 && rest2[c_end..].starts_with(':') {
                    col = rest2[..c_end].parse().ok();
                    rest2 = &rest2[c_end + 1..];
                }
                let rest2 = rest2.trim_start();
                for (prefix, sev) in [
                    ("fatal error: ", Severity::Error),
                    ("error: ", Severity::Error),
                    ("warning: ", Severity::Warning),
                    ("note: ", Severity::Note),
                ] {
                    if let Some(m) = rest2.strip_prefix(prefix) {
                        return Some(Diagnostic {
                            file: Some(PathBuf::from(file)),
                            tab: None,
                            line: Some(ln),
                            column: col,
                            severity: sev,
                            message: m.to_string(),
                            excerpt: vec![],
                            context: vec![],
                        });
                    }
                }
                if rest2.starts_with("undefined reference")
                    || rest2.starts_with("multiple definition")
                {
                    return Some(Diagnostic {
                        file: Some(PathBuf::from(file)),
                        tab: None,
                        line: Some(ln),
                        column: col,
                        severity: Severity::Error,
                        message: rest2.to_string(),
                        excerpt: vec![],
                        context: vec![],
                    });
                }
                return None;
            }
        }
        i += 1;
    }
    None
}

/// Line map of the preprocessed `<sketch>.ino.cpp` back to the user's `.ino` tabs.
#[derive(Debug, Default, Clone)]
pub struct InoMap {
    /// Path of the generated .ino.cpp.
    cpp: Option<PathBuf>,
    /// cpp line → (original file, line) at each `#line` directive; lines in between count up.
    marks: BTreeMap<u32, (PathBuf, u32)>,
    sketch_dir: PathBuf,
}

impl InoMap {
    pub fn load(build_dir: &Path, sketch_dir: &Path) -> InoMap {
        let name = sketch_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let cpp = build_dir.join("sketch").join(format!("{name}.ino.cpp"));
        let text = std::fs::read_to_string(&cpp).unwrap_or_default();
        let mut m = InoMap::from_text(&text, sketch_dir);
        m.cpp = Some(cpp);
        m
    }

    /// Builds the map from the text of a generated `.ino.cpp`.
    pub fn from_text(text: &str, sketch_dir: &Path) -> InoMap {
        let mut marks = BTreeMap::new();
        for (i, l) in text.lines().enumerate() {
            let t = l.trim_start();
            if let Some(rest) = t.strip_prefix("#line ") {
                let mut parts = rest.splitn(2, ' ');
                let n: u32 = match parts.next().and_then(|n| n.parse().ok()) {
                    Some(n) => n,
                    None => continue,
                };
                let file = parts
                    .next()
                    .map(|f| f.trim().trim_matches('"').replace("\\\\", "\\"))
                    .unwrap_or_default();
                // The directive applies to the *next* physical line.
                marks.insert(i as u32 + 2, (PathBuf::from(file), n));
            }
        }
        InoMap {
            cpp: None,
            marks,
            sketch_dir: sketch_dir.to_path_buf(),
        }
    }

    /// Maps a line of the generated .ino.cpp to (file, line).
    pub fn map_line(&self, cpp_line: u32) -> Option<(PathBuf, u32)> {
        let (&mark, (file, base)) = self.marks.range(..=cpp_line).next_back()?;
        Some((file.clone(), base + (cpp_line - mark)))
    }

    fn apply(&self, d: &mut Diagnostic) {
        if let (Some(f), Some(l)) = (&d.file, d.line) {
            let is_cpp = self.cpp.as_ref().is_some_and(|c| same_file(c, f))
                || f.to_string_lossy().ends_with(".ino.cpp");
            if is_cpp && let Some((file, line)) = self.map_line(l) {
                d.file = Some(file);
                d.line = Some(line);
            }
        }
        if let Some(f) = &d.file {
            let canon = f.canonicalize().unwrap_or_else(|_| f.clone());
            if let Ok(rel) = canon.strip_prefix(&self.sketch_dir) {
                d.tab = Some(rel.to_string_lossy().into_owned());
            } else if let Ok(rel) = f.strip_prefix(&self.sketch_dir) {
                d.tab = Some(rel.to_string_lossy().into_owned());
            }
        }
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || a.canonicalize()
            .ok()
            .zip(b.canonicalize().ok())
            .is_some_and(|(x, y)| x == y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_report_text() {
        let s = SizeReport {
            program: 9_842,
            program_max: 32_256,
            data: 1_104,
            data_max: 2_048,
            eeprom: 0,
        };
        assert_eq!(
            s.arduino_lines()[0],
            "Sketch uses 9842 bytes (30%) of program storage space. Maximum is 32256 bytes."
        );
        assert_eq!(
            s.display_lines(),
            vec![
                "Sketch uses 9,842 bytes (30%) of program storage space. Maximum is 32,256 bytes.".to_string(),
                "Global variables use 1,104 bytes (53%) of dynamic memory, leaving 944 bytes for local variables. Maximum is 2,048 bytes."
                    .to_string(),
            ]
        );
        assert!(s.problems().is_empty());
        let big = SizeReport {
            program: 40_000,
            data: 1_700,
            ..s
        };
        let p = big.problems();
        assert_eq!(p[0].0, Severity::Error);
        assert!(p[1].1.starts_with("Low memory available"));
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(999), "999");
    }

    #[test]
    fn parses_gcc_text() {
        let text = "/s/a/a.ino: In function 'void loop()':\n/s/a/a.ino:7:3: error: 'foo' was not declared in this scope\n   foo();\n   ^~~\n/s/a/util.h:2:9: warning: unused variable 'x' [-Wunused-variable]\n/tmp/x/a.ino.cpp.o: In function `loop':\n/s/a/a.ino:9: undefined reference to `bar()'\n";
        let d = parse_gcc_diagnostics(text);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].line, Some(7));
        assert_eq!(d[0].column, Some(3));
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].context, vec!["In function 'void loop()':"]);
        assert_eq!(d[0].excerpt.len(), 2);
        assert_eq!(d[1].severity, Severity::Warning);
        assert_eq!(d[2].message, "undefined reference to `bar()'");
        assert_eq!(d[2].column, None);
    }

    #[test]
    fn ino_cpp_line_remapping() {
        let cpp = "#include <Arduino.h>\n#line 1 \"/s/a/a.ino\"\nint x;\n#line 3 \"/s/a/a.ino\"\nvoid setup();\n#line 3 \"/s/a/a.ino\"\nvoid setup() {\n}\n#line 1 \"/s/a/b.ino\"\nvoid helper() {\n  oops;\n}\n";
        let m = InoMap::from_text(cpp, Path::new("/s/a"));
        assert_eq!(m.map_line(3), Some((PathBuf::from("/s/a/a.ino"), 1)));
        assert_eq!(m.map_line(7), Some((PathBuf::from("/s/a/a.ino"), 3)));
        assert_eq!(m.map_line(8), Some((PathBuf::from("/s/a/a.ino"), 4)));
        assert_eq!(m.map_line(11), Some((PathBuf::from("/s/a/b.ino"), 2)));
        let mut d = Diagnostic {
            file: Some(PathBuf::from("/tmp/build/sketch/a.ino.cpp")),
            tab: None,
            line: Some(11),
            column: Some(3),
            severity: Severity::Error,
            message: "x".into(),
            excerpt: vec![],
            context: vec![],
        };
        m.apply(&mut d);
        assert_eq!(d.file, Some(PathBuf::from("/s/a/b.ino")));
        assert_eq!(d.line, Some(2));
        assert_eq!(d.tab.as_deref(), Some("b.ino"));
    }

    #[test]
    fn banner_text() {
        assert_eq!(
            banner("UNO R3", Some("7.3.0"), "atmega328p", false),
            "Compiling sketch for UNO R3 (avr-gcc 7.3.0 -Os -mmcu=atmega328p)…"
        );
    }
}
