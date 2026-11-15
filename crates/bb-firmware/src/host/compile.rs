//! Builds a sketch for an API-level board: Arduino-style preprocessing of the `.ino` tabs
//! (concatenation + generated prototypes + `#line` mapping), the embedded host core
//! (`hostcore/`, compiled once per board kind and cached), and the system C++ compiler
//! (`clang++`, or `$BREADBOX_CXX`).

use super::catalog::HostKind;
use crate::FirmwareError;
use crate::compile::{Diagnostic, Severity, build_cache_root, parse_gcc_diagnostics};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The host core sources, embedded so the app needs no source tree at run time.
pub const HOSTCORE: &[(&str, &str)] = &[
    ("Arduino.h", include_str!("../../hostcore/Arduino.h")),
    ("BLE.cpp", include_str!("../../hostcore/BLE.cpp")),
    ("BLE2902.h", include_str!("../../hostcore/BLE2902.h")),
    ("BLEAdvertising.h", include_str!("../../hostcore/BLEAdvertising.h")),
    ("BLECharacteristic.h", include_str!("../../hostcore/BLECharacteristic.h")),
    ("BLEDescriptor.h", include_str!("../../hostcore/BLEDescriptor.h")),
    ("BLEDevice.h", include_str!("../../hostcore/BLEDevice.h")),
    ("BLEServer.h", include_str!("../../hostcore/BLEServer.h")),
    ("BLEService.h", include_str!("../../hostcore/BLEService.h")),
    ("BLEUUID.h", include_str!("../../hostcore/BLEUUID.h")),
    ("BLEUtils.h", include_str!("../../hostcore/BLEUtils.h")),
    ("Client.h", include_str!("../../hostcore/Client.h")),
    ("DHT.cpp", include_str!("../../hostcore/DHT.cpp")),
    ("DHT.h", include_str!("../../hostcore/DHT.h")),
    ("ESP8266WiFi.h", include_str!("../../hostcore/ESP8266WiFi.h")),
    ("HTTPClient.cpp", include_str!("../../hostcore/HTTPClient.cpp")),
    ("HTTPClient.h", include_str!("../../hostcore/HTTPClient.h")),
    ("HardwareSerial.cpp", include_str!("../../hostcore/HardwareSerial.cpp")),
    ("HardwareSerial.h", include_str!("../../hostcore/HardwareSerial.h")),
    ("IPAddress.cpp", include_str!("../../hostcore/IPAddress.cpp")),
    ("IPAddress.h", include_str!("../../hostcore/IPAddress.h")),
    ("Preferences.cpp", include_str!("../../hostcore/Preferences.cpp")),
    ("Preferences.h", include_str!("../../hostcore/Preferences.h")),
    ("Print.cpp", include_str!("../../hostcore/Print.cpp")),
    ("Print.h", include_str!("../../hostcore/Print.h")),
    ("Printable.h", include_str!("../../hostcore/Printable.h")),
    ("PubSubClient.cpp", include_str!("../../hostcore/PubSubClient.cpp")),
    ("PubSubClient.h", include_str!("../../hostcore/PubSubClient.h")),
    ("SPI.cpp", include_str!("../../hostcore/SPI.cpp")),
    ("SPI.h", include_str!("../../hostcore/SPI.h")),
    ("Server.h", include_str!("../../hostcore/Server.h")),
    ("Stream.cpp", include_str!("../../hostcore/Stream.cpp")),
    ("Stream.h", include_str!("../../hostcore/Stream.h")),
    ("Udp.h", include_str!("../../hostcore/Udp.h")),
    ("WString.h", include_str!("../../hostcore/WString.h")),
    ("WiFi.cpp", include_str!("../../hostcore/WiFi.cpp")),
    ("WiFi.h", include_str!("../../hostcore/WiFi.h")),
    ("WiFiClient.h", include_str!("../../hostcore/WiFiClient.h")),
    ("WiFiServer.h", include_str!("../../hostcore/WiFiServer.h")),
    ("WiFiUdp.h", include_str!("../../hostcore/WiFiUdp.h")),
    ("Wire.cpp", include_str!("../../hostcore/Wire.cpp")),
    ("Wire.h", include_str!("../../hostcore/Wire.h")),
    ("arduino_core.cpp", include_str!("../../hostcore/arduino_core.cpp")),
    ("bb_ble.h", include_str!("../../hostcore/bb_ble.h")),
    ("bb_host.cpp", include_str!("../../hostcore/bb_host.cpp")),
    ("bb_host.h", include_str!("../../hostcore/bb_host.h")),
];

/// Result of a host build.
#[derive(Debug, Clone, Serialize)]
pub struct HostBuild {
    pub success: bool,
    /// The executable to hand to `HostBoard::spawn`.
    pub exe: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
    /// Compiler output.
    pub log: String,
    pub duration: Duration,
    /// True when the cached executable was reused.
    pub cached: bool,
}

fn fnv(data: &[u8], mut h: u64) -> u64 {
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn hostcore_hash() -> u64 {
    HOSTCORE.iter().fold(0xcbf2_9ce4_8422_2325, |h, (n, s)| fnv(s.as_bytes(), fnv(n.as_bytes(), h)))
}

/// The C++ compiler: `$BREADBOX_CXX`, else `clang++`, else `c++`.
pub fn cxx() -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = std::env::var_os("BREADBOX_CXX").map(PathBuf::from).into_iter().chain(["clang++", "c++", "g++"].iter().map(PathBuf::from)).collect();
    candidates.into_iter().find(|c| Command::new(c).arg("--version").output().is_ok_and(|o| o.status.success()))
}

/// Writes the embedded host core into the cache (once per content hash) and returns its dir.
pub fn hostcore_dir() -> Result<PathBuf, FirmwareError> {
    let dir = build_cache_root().join(format!("hostcore-{:016x}", hostcore_hash()));
    let stamp = dir.join(".complete");
    if !stamp.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| FirmwareError::Io(e.to_string()))?;
        for (name, text) in HOSTCORE {
            std::fs::write(dir.join(name), text).map_err(|e| FirmwareError::Io(e.to_string()))?;
        }
        std::fs::write(&stamp, b"").map_err(|e| FirmwareError::Io(e.to_string()))?;
    }
    Ok(dir)
}

fn base_flags(kind: HostKind, core: &Path) -> Vec<String> {
    let mut v: Vec<String> = ["-std=gnu++17", "-O1", "-g", "-fno-omit-frame-pointer", "-Wno-unused-command-line-argument"].iter().map(|s| s.to_string()).collect();
    v.extend(kind.defines().iter().map(|s| s.to_string()));
    v.push(format!("-I{}", core.display()));
    v
}

/// Compiles the host core for `kind` (cached); returns the object files.
fn hostcore_objects(cxx: &Path, kind: HostKind, log: &mut String) -> Result<Vec<PathBuf>, FirmwareError> {
    let core = hostcore_dir()?;
    let obj_dir = core.join(format!("obj-{}", kind.id()));
    let stamp = obj_dir.join(".complete");
    let sources: Vec<&str> = HOSTCORE.iter().map(|(n, _)| *n).filter(|n| n.ends_with(".cpp")).collect();
    let objs: Vec<PathBuf> = sources.iter().map(|s| obj_dir.join(s.replace(".cpp", ".o"))).collect();
    if stamp.exists() {
        return Ok(objs);
    }
    std::fs::create_dir_all(&obj_dir).map_err(|e| FirmwareError::Io(e.to_string()))?;
    let flags = base_flags(kind, &core);
    let mut children = vec![];
    for (src, obj) in sources.iter().zip(&objs) {
        let child = Command::new(cxx).args(&flags).arg("-c").arg(core.join(src)).arg("-o").arg(obj).output();
        children.push(child);
    }
    let mut ok = true;
    for c in children {
        let o = c.map_err(|e| FirmwareError::Io(e.to_string()))?;
        log.push_str(&String::from_utf8_lossy(&o.stderr));
        ok &= o.status.success();
    }
    if !ok {
        return Err(FirmwareError::Cli(format!("the host core failed to compile:\n{log}")));
    }
    std::fs::write(&stamp, b"").map_err(|e| FirmwareError::Io(e.to_string()))?;
    Ok(objs)
}

/// Words that start a top-level construct that is not a function definition.
const NOT_FUNCS: &[&str] = &["if", "while", "for", "switch", "struct", "class", "namespace", "enum", "union", "typedef", "template", "else", "return", "do", "extern"];

/// Blanks comments, string and char literals (keeping newlines) so a scanner can see structure.
fn strip(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                out.push(b' ');
                i += 1;
            }
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                out.push(if b[i] == b'\n' { b'\n' } else { b' ' });
                i += 1;
            }
            out.extend_from_slice(b"  ");
            i += 2;
        } else if b[i] == b'"' || b[i] == b'\'' {
            let q = b[i];
            out.push(b' ');
            i += 1;
            while i < b.len() && b[i] != q {
                if b[i] == b'\\' {
                    out.push(b' ');
                    i += 1;
                }
                if i < b.len() {
                    out.push(if b[i] == b'\n' { b'\n' } else { b' ' });
                }
                i += 1;
            }
            out.push(b' ');
            i += 1;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out.truncate(src.len());
    String::from_utf8_lossy(&out).into_owned()
}

/// Top-level function definitions: (prototype text, line of the definition, 1-based).
pub fn find_prototypes(src: &str) -> Vec<(String, u32)> {
    let s = strip(src);
    let b = s.as_bytes();
    let mut out = vec![];
    let mut depth = 0i32;
    let mut stmt_start = 0usize;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if depth == 0 && c == b'#' {
            // Preprocessor line (with continuations).
            while i < b.len() && !(b[i] == b'\n' && (i == 0 || b[i - 1] != b'\\')) {
                i += 1;
            }
            stmt_start = i + 1;
        } else if c == b'{' {
            if depth == 0 {
                let head = s[stmt_start..i].trim();
                if let Some(p) = prototype(head) {
                    let raw = &s[stmt_start..i];
                    let head_start = stmt_start + (raw.len() - raw.trim_start().len());
                    let def_line = s[..head_start].matches('\n').count() as u32 + 1;
                    out.push((p, def_line));
                }
            }
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                stmt_start = i + 1;
            }
        } else if depth == 0 && c == b';' {
            stmt_start = i + 1;
        }
        i += 1;
    }
    out
}

fn prototype(head: &str) -> Option<String> {
    let head = head.split_whitespace().collect::<Vec<_>>().join(" ");
    let open = head.find('(')?;
    let close = head.rfind(')')?;
    if close < open {
        return None;
    }
    let before = head[..open].trim();
    let first = before.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap_or("");
    if before.is_empty() || NOT_FUNCS.contains(&first) || before.contains('=') || before.contains("::") || !before.contains(' ') && !before.contains('*') && !before.contains('&') {
        return None;
    }
    let name = before.rsplit(|c: char| c == ' ' || c == '*' || c == '&').next().unwrap_or("");
    if name.is_empty() || name == "operator" || !name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') {
        return None;
    }
    // Default arguments must not be repeated; keep the head up to the closing paren + qualifiers.
    let tail = head[close + 1..].trim();
    if !(tail.is_empty() || tail == "const" || tail.starts_with("noexcept")) {
        return None;
    }
    // Default arguments stay on the definition only.
    let params = &head[open + 1..close];
    let mut cleaned = vec![];
    let mut depth = 0;
    let mut cur = String::new();
    for c in params.chars() {
        match c {
            '(' | '<' | '[' | '{' => depth += 1,
            ')' | '>' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if c == ',' && depth == 0 {
            cleaned.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    cleaned.push(cur);
    let params: Vec<String> = cleaned.iter().map(|p| p.split('=').next().unwrap_or("").trim().to_string()).collect();
    Some(format!("{}({}){};", &head[..open], params.join(", "), if tail.is_empty() { String::new() } else { format!(" {tail}") }))
}

/// Generates `<sketch>.ino.cpp`: every `.ino` tab (main first, others alphabetically), with
/// prototypes inserted before the first function definition and `#line` directives so errors
/// point at the user's files.
pub fn preprocess(sketch_dir: &Path) -> Result<String, FirmwareError> {
    let name = sketch_dir.file_name().and_then(|n| n.to_str()).ok_or_else(|| FirmwareError::Io("bad sketch path".into()))?;
    let main = sketch_dir.join(format!("{name}.ino"));
    if !main.exists() {
        return Err(FirmwareError::Io(format!("{} not found", main.display())));
    }
    let mut tabs: Vec<PathBuf> = std::fs::read_dir(sketch_dir)
        .map_err(|e| FirmwareError::Io(e.to_string()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ino") && *p != main)
        .collect();
    tabs.sort();
    tabs.insert(0, main);
    let mut texts = vec![];
    for t in &tabs {
        texts.push((t.clone(), std::fs::read_to_string(t).map_err(|e| FirmwareError::Io(e.to_string()))?));
    }
    let mut protos = vec![];
    for (_, text) in &texts {
        protos.extend(find_prototypes(text).into_iter().map(|p| p.0));
    }
    let mut out = String::from("#include <Arduino.h>\n");
    let mut inserted = false;
    for (path, text) in &texts {
        let first_def = if inserted { None } else { find_prototypes(text).first().map(|p| p.1) };
        out.push_str(&format!("#line 1 \"{}\"\n", path.display()));
        for (i, line) in text.lines().enumerate() {
            let ln = i as u32 + 1;
            if Some(ln) == first_def && !inserted {
                for p in &protos {
                    out.push_str(p);
                    out.push('\n');
                }
                out.push_str(&format!("#line {ln} \"{}\"\n", path.display()));
                inserted = true;
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok(out)
}

/// Compiles `sketch_dir` for `kind`. `build_dir` defaults to the per-sketch cache directory.
pub fn compile_host(sketch_dir: &Path, kind: HostKind, build_dir: Option<&Path>) -> Result<HostBuild, FirmwareError> {
    let t0 = Instant::now();
    let cxx = cxx().ok_or_else(|| FirmwareError::Cli("no C++ compiler found (install the Xcode command line tools: xcode-select --install)".into()))?;
    let sketch_dir = sketch_dir.canonicalize().map_err(|e| FirmwareError::Io(e.to_string()))?;
    let name = sketch_dir.file_name().and_then(|n| n.to_str()).unwrap_or("sketch").to_string();
    let mut log = String::new();
    let cpp = preprocess(&sketch_dir)?;
    // Other tabs: .h are included from the sketch dir, .cpp/.c are compiled.
    let mut extra: Vec<PathBuf> = std::fs::read_dir(&sketch_dir)
        .map_err(|e| FirmwareError::Io(e.to_string()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "cpp" || x == "c"))
        .collect();
    extra.sort();
    let mut key = fnv(cpp.as_bytes(), hostcore_hash() ^ kind.id().len() as u64);
    key = fnv(kind.id().as_bytes(), key);
    for p in std::fs::read_dir(&sketch_dir).map_err(|e| FirmwareError::Io(e.to_string()))?.flatten() {
        let p = p.path();
        if p.extension().is_some_and(|x| x == "h" || x == "hpp" || x == "cpp" || x == "c") {
            key = fnv(&std::fs::read(&p).unwrap_or_default(), fnv(p.to_string_lossy().as_bytes(), key));
        }
    }
    let dir = match build_dir {
        Some(d) => d.to_path_buf(),
        None => build_cache_root().join(format!("host-{name}-{}", kind.id())),
    };
    std::fs::create_dir_all(&dir).map_err(|e| FirmwareError::Io(e.to_string()))?;
    let exe = dir.join(format!("{name}.{}.host", kind.id()));
    let stamp = dir.join("build.key");
    if exe.exists() && std::fs::read_to_string(&stamp).ok().as_deref() == Some(&format!("{key:016x}")) {
        return Ok(HostBuild { success: true, exe: Some(exe), diagnostics: vec![], log, duration: t0.elapsed(), cached: true });
    }
    let objs = hostcore_objects(&cxx, kind, &mut log)?;
    let core = hostcore_dir()?;
    let main_cpp = dir.join(format!("{name}.ino.cpp"));
    std::fs::write(&main_cpp, &cpp).map_err(|e| FirmwareError::Io(e.to_string()))?;
    let mut flags = base_flags(kind, &core);
    flags.push(format!("-I{}", sketch_dir.display()));
    flags.push("-Wall".into());
    flags.push("-Wno-unused-variable".into());
    flags.push("-fdiagnostics-absolute-paths".into());
    let mut sketch_objs = vec![];
    let mut ok = true;
    for (i, src) in std::iter::once(main_cpp.clone()).chain(extra).enumerate() {
        let obj = dir.join(format!("sketch{i}.o"));
        let o = Command::new(&cxx).args(&flags).arg("-c").arg(&src).arg("-o").arg(&obj).output().map_err(|e| FirmwareError::Io(e.to_string()))?;
        log.push_str(&String::from_utf8_lossy(&o.stderr));
        ok &= o.status.success();
        sketch_objs.push(obj);
    }
    if ok {
        let o = Command::new(&cxx).args(&sketch_objs).args(&objs).arg("-o").arg(&exe).output().map_err(|e| FirmwareError::Io(e.to_string()))?;
        log.push_str(&String::from_utf8_lossy(&o.stderr));
        ok = o.status.success();
    }
    let mut diagnostics = parse_gcc_diagnostics(&log);
    for d in &mut diagnostics {
        if let Some(f) = &d.file
            && let Ok(rel) = f.strip_prefix(&sketch_dir)
        {
            d.tab = Some(rel.to_string_lossy().into_owned());
        }
    }
    if !ok && !diagnostics.iter().any(|d| d.severity == Severity::Error) {
        diagnostics.push(Diagnostic { file: None, tab: None, line: None, column: None, severity: Severity::Error, message: log.lines().last().unwrap_or("build failed").to_string(), excerpt: vec![], context: vec![] });
    }
    if ok {
        let _ = std::fs::write(&stamp, format!("{key:016x}"));
    }
    Ok(HostBuild { success: ok, exe: ok.then_some(exe), diagnostics, log, duration: t0.elapsed(), cached: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_prototypes_like_arduino() {
        let src = "#include <WiFi.h>\n// void commented() {}\nconst char* s = \"void fake() {\";\nint add(int a, int b = 2) {\n  return a + b;\n}\nstatic void blink(uint8_t pin)\n{\n  if (pin) { digitalWrite(pin, HIGH); }\n}\nstruct P { int x; };\nvoid setup() { blink(add(1)); }\nvoid loop() {}\nbool Foo::bar() { return true; }\n";
        let p = find_prototypes(src);
        let names: Vec<&str> = p.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(names, vec!["int add(int a, int b);", "static void blink(uint8_t pin);", "void setup();", "void loop();"]);
        assert_eq!(p[0].1, 4);
        assert_eq!(p[1].1, 7);
    }
}
