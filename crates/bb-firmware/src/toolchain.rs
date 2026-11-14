//! arduino-cli discovery, installation and library management.
//!
//! Search order (`MCU-4`): `$BREADBOX_ARDUINO_CLI`, `PATH`, Homebrew (`/opt/homebrew/bin`,
//! `/usr/local/bin`), then Breadbox's own install in
//! `~/Library/Application Support/Breadbox/toolchain/`. The app-local install uses its own
//! `arduino-cli.yaml` so it never touches the user's Arduino15 folder.
//!
//! **Native overrides.** On Apple Silicon Macs without Rosetta, the x86_64 avr-gcc 7.3.0 and
//! ctags shipped in the arduino:avr core cannot run. Detection checks that the bundled tools
//! execute; if not, it looks for a native avr-gcc (Homebrew `osx-cross/avr/avr-gcc@N` +
//! `avr-binutils`, or `toolchain/native/avr-gcc/bin`) and a native Arduino ctags
//! (`toolchain/native/ctags/ctags`) and passes them to arduino-cli as build properties.

use crate::FirmwareError;
use serde::Serialize;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the arduino-cli binary was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ToolchainSource {
    Explicit,
    Path,
    Homebrew,
    AppSupport,
}

/// Replacement tools passed to arduino-cli when the bundled ones cannot run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct NativeOverrides {
    /// Directory holding `avr-gcc`, `avr-g++`, `avr-objcopy`, `avr-size`, … (`compiler.path`).
    pub compiler_bin: Option<PathBuf>,
    /// Directory holding a native `ctags` (`runtime.tools.ctags.path`).
    pub ctags_dir: Option<PathBuf>,
}

/// A usable arduino-cli installation.
#[derive(Debug, Clone, Serialize)]
pub struct Toolchain {
    pub arduino_cli: PathBuf,
    pub cli_version: String,
    pub source: ToolchainSource,
    /// `--config-file` for app-local installs.
    pub config_file: Option<PathBuf>,
    pub data_dir: PathBuf,
    /// Installed arduino:avr core version, if any.
    pub avr_core_version: Option<String>,
    /// avr-gcc version that will actually compile ("7.3.0", or the native override's).
    pub avr_gcc_version: Option<String>,
    pub overrides: NativeOverrides,
}

/// Detection knobs (tests point these at temporary folders).
#[derive(Debug, Clone)]
pub struct DetectOptions {
    pub explicit: Option<PathBuf>,
    pub search_path: bool,
    pub search_homebrew: bool,
    pub app_dir: PathBuf,
}

impl Default for DetectOptions {
    fn default() -> Self {
        DetectOptions {
            explicit: std::env::var_os("BREADBOX_ARDUINO_CLI").map(PathBuf::from),
            search_path: true,
            search_homebrew: true,
            app_dir: Toolchain::app_dir(),
        }
    }
}

/// Install progress events.
#[derive(Debug, Clone, PartialEq)]
pub enum InstallProgress {
    /// Downloading arduino-cli: bytes so far / total (if known).
    Download {
        bytes: u64,
        total: Option<u64>,
    },
    /// A step started ("Extracting arduino-cli", "Installing arduino:avr", …).
    Step(String),
    /// A line of output from arduino-cli.
    Log(String),
    Done,
}

/// A library from the Library Manager index.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LibraryInfo {
    pub name: String,
    pub version: String,
    pub author: String,
    pub sentence: String,
    pub versions: Vec<String>,
}

/// An installed library.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InstalledLibrary {
    pub name: String,
    pub version: String,
    pub location: String,
    pub install_dir: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Runs a binary with `--version`-like args and returns stdout if it executes.
fn runs(bin: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn gcc_version(bin_dir: &Path) -> Option<String> {
    let out = runs(&bin_dir.join("avr-gcc"), &["--version"])?;
    out.lines()
        .next()?
        .split_whitespace()
        .last()
        .map(|s| s.to_string())
}

impl Toolchain {
    /// `~/Library/Application Support/Breadbox/toolchain` (or the XDG data dir elsewhere).
    pub fn app_dir() -> PathBuf {
        if let Some(d) = std::env::var_os("BREADBOX_TOOLCHAIN_DIR") {
            return PathBuf::from(d);
        }
        if cfg!(target_os = "macos") {
            home().join("Library/Application Support/Breadbox/toolchain")
        } else {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".local/share"))
                .join("breadbox/toolchain")
        }
    }

    /// Finds a working arduino-cli with default options.
    pub fn detect() -> Result<Toolchain, FirmwareError> {
        Toolchain::detect_with(&DetectOptions::default())
    }

    pub fn detect_with(opts: &DetectOptions) -> Result<Toolchain, FirmwareError> {
        let mut candidates: Vec<(PathBuf, ToolchainSource)> = Vec::new();
        if let Some(e) = &opts.explicit {
            candidates.push((e.clone(), ToolchainSource::Explicit));
        }
        if opts.search_path
            && let Some(path) = std::env::var_os("PATH")
        {
            for dir in std::env::split_paths(&path) {
                candidates.push((dir.join("arduino-cli"), ToolchainSource::Path));
            }
        }
        if opts.search_homebrew {
            for d in ["/opt/homebrew/bin", "/usr/local/bin"] {
                candidates.push((
                    PathBuf::from(d).join("arduino-cli"),
                    ToolchainSource::Homebrew,
                ));
            }
        }
        candidates.push((
            opts.app_dir.join("bin/arduino-cli"),
            ToolchainSource::AppSupport,
        ));
        for (bin, source) in candidates {
            if !bin.is_file() {
                continue;
            }
            let config_file = if source == ToolchainSource::AppSupport {
                Some(opts.app_dir.join("arduino-cli.yaml"))
            } else {
                None
            };
            if let Ok(tc) = Toolchain::probe(bin, source, config_file, &opts.app_dir) {
                return Ok(tc);
            }
        }
        Err(FirmwareError::ToolchainMissing)
    }

    fn probe(
        bin: PathBuf,
        source: ToolchainSource,
        config_file: Option<PathBuf>,
        app_dir: &Path,
    ) -> Result<Toolchain, FirmwareError> {
        let mut tc = Toolchain {
            arduino_cli: bin,
            cli_version: String::new(),
            source,
            config_file,
            data_dir: PathBuf::new(),
            avr_core_version: None,
            avr_gcc_version: None,
            overrides: NativeOverrides::default(),
        };
        let v = tc.run_json(&["version"])?;
        tc.cli_version = v["VersionString"].as_str().unwrap_or("?").to_string();
        let data = tc.run_text(&["config", "get", "directories.data"])?;
        tc.data_dir = PathBuf::from(data.trim());
        tc.refresh_core();
        tc.detect_overrides(app_dir);
        Ok(tc)
    }

    /// Re-reads the installed arduino:avr version.
    pub fn refresh_core(&mut self) {
        self.avr_core_version = self.run_json(&["core", "list"]).ok().and_then(|v| {
            v["platforms"]
                .as_array()?
                .iter()
                .find(|p| p["id"] == "arduino:avr")
                .and_then(|p| p["installed_version"].as_str().map(|s| s.to_string()))
        });
    }

    fn bundled_gcc_bin(&self) -> Option<PathBuf> {
        let dir = self.data_dir.join("packages/arduino/tools/avr-gcc");
        let mut versions: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .collect();
        versions.sort();
        versions.pop().map(|v| v.join("bin"))
    }

    fn bundled_ctags(&self) -> Option<PathBuf> {
        let dir = self.data_dir.join("packages/builtin/tools/ctags");
        let mut versions: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .collect();
        versions.sort();
        versions.pop().map(|v| v.join("ctags"))
    }

    /// Decides whether native replacements are needed and where they are.
    fn detect_overrides(&mut self, app_dir: &Path) {
        let native = app_dir.join("native");
        let bundled = self.bundled_gcc_bin();
        match bundled.as_ref().and_then(|b| gcc_version(b)) {
            Some(v) => self.avr_gcc_version = Some(v),
            None if self.avr_core_version.is_some() => {
                if let Some(bin) = find_native_gcc(&native) {
                    self.avr_gcc_version = gcc_version(&bin);
                    self.overrides.compiler_bin = Some(bin);
                }
            }
            None => {}
        }
        let ctags_ok = self
            .bundled_ctags()
            .is_some_and(|c| runs(&c, &["--version"]).is_some());
        if !ctags_ok && self.avr_core_version.is_some() {
            let dir = std::env::var_os("BREADBOX_CTAGS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| native.join("ctags"));
            if runs(&dir.join("ctags"), &["--version"]).is_some() {
                self.overrides.ctags_dir = Some(dir);
            }
        }
    }

    /// True if sketches can be compiled (core installed and a runnable compiler + ctags).
    pub fn ready(&self) -> bool {
        self.avr_core_version.is_some() && self.avr_gcc_version.is_some() && self.ctags_usable()
    }

    fn ctags_usable(&self) -> bool {
        self.overrides.ctags_dir.is_some()
            || self
                .bundled_ctags()
                .is_some_and(|c| runs(&c, &["--version"]).is_some())
    }

    /// Text for the firmware screen's toolchain box, e.g. `"avr-gcc 7.3.0"`.
    pub fn compiler_label(&self) -> String {
        match &self.avr_gcc_version {
            Some(v) => format!("avr-gcc {v}"),
            None => "avr-gcc (not installed)".to_string(),
        }
    }

    /// `--build-property` arguments for the native overrides.
    pub fn build_property_args(&self) -> Vec<String> {
        let mut a = Vec::new();
        if let Some(b) = &self.overrides.compiler_bin {
            a.push("--build-property".to_string());
            a.push(format!("compiler.path={}/", b.display()));
        }
        if let Some(c) = &self.overrides.ctags_dir {
            a.push("--build-property".to_string());
            a.push(format!("runtime.tools.ctags.path={}", c.display()));
        }
        a
    }

    /// A `Command` for arduino-cli with this installation's config.
    pub fn command(&self) -> Command {
        let mut c = Command::new(&self.arduino_cli);
        if let Some(cfg) = &self.config_file {
            c.arg("--config-file").arg(cfg);
        }
        c.stdin(Stdio::null());
        c
    }

    fn run_text(&self, args: &[&str]) -> Result<String, FirmwareError> {
        let out = self
            .command()
            .args(args)
            .output()
            .map_err(|e| FirmwareError::Io(e.to_string()))?;
        if !out.status.success() {
            return Err(FirmwareError::Cli(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub(crate) fn run_json(&self, args: &[&str]) -> Result<Value, FirmwareError> {
        let mut full: Vec<&str> = args.to_vec();
        full.extend(["--format", "json"]);
        let text = self.run_text(&full)?;
        serde_json::from_str(&text)
            .map_err(|e| FirmwareError::Cli(format!("bad JSON from arduino-cli: {e}")))
    }

    /// Runs arduino-cli streaming each output line to `progress`.
    fn run_streaming(
        &self,
        args: &[&str],
        progress: &mut dyn FnMut(InstallProgress),
    ) -> Result<(), FirmwareError> {
        let mut child = self
            .command()
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| FirmwareError::Io(e.to_string()))?;
        let mut err_text = String::new();
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                progress(InstallProgress::Log(line));
            }
        }
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err_text);
        }
        let status = child.wait().map_err(|e| FirmwareError::Io(e.to_string()))?;
        if !status.success() {
            return Err(FirmwareError::Cli(err_text.trim().to_string()));
        }
        Ok(())
    }

    /// Installs (or updates) the arduino:avr core.
    pub fn install_core(
        &mut self,
        progress: &mut dyn FnMut(InstallProgress),
    ) -> Result<(), FirmwareError> {
        progress(InstallProgress::Step("Updating the board index".into()));
        self.run_streaming(&["core", "update-index"], progress)?;
        progress(InstallProgress::Step(
            "Installing arduino:avr (avr-gcc 7.3.0)".into(),
        ));
        self.run_streaming(&["core", "install", "arduino:avr"], progress)?;
        self.refresh_core();
        let app = self
            .config_file
            .as_ref()
            .and_then(|c| c.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(Toolchain::app_dir);
        self.detect_overrides(&app);
        Ok(())
    }

    /// Downloads arduino-cli for this Mac into `dir` (normally [`Toolchain::app_dir`]), writes a
    /// private config, installs the arduino:avr core and returns the ready toolchain.
    pub fn install(
        dir: &Path,
        progress: &mut dyn FnMut(InstallProgress),
    ) -> Result<Toolchain, FirmwareError> {
        Toolchain::install_from(dir, &download_url(), progress)
    }

    /// Like [`Toolchain::install`] with an explicit archive URL (`http(s)://` or `file://`).
    pub fn install_from(
        dir: &Path,
        url: &str,
        progress: &mut dyn FnMut(InstallProgress),
    ) -> Result<Toolchain, FirmwareError> {
        let mut tc = Toolchain::install_cli(dir, url, progress)?;
        tc.install_core(progress)?;
        progress(InstallProgress::Done);
        Ok(tc)
    }

    /// First half of the install: fetch + extract arduino-cli into `dir/bin` and write its
    /// private `arduino-cli.yaml` (data in `dir/data`). No core yet.
    pub fn install_cli(
        dir: &Path,
        url: &str,
        progress: &mut dyn FnMut(InstallProgress),
    ) -> Result<Toolchain, FirmwareError> {
        std::fs::create_dir_all(dir.join("bin")).map_err(|e| FirmwareError::Io(e.to_string()))?;
        progress(InstallProgress::Step("Downloading arduino-cli".into()));
        let archive = fetch(url, progress)?;
        progress(InstallProgress::Step("Extracting arduino-cli".into()));
        extract_cli(&archive, &dir.join("bin"))?;
        let cfg = dir.join("arduino-cli.yaml");
        let yaml = format!(
            "directories:\n  data: \"{}\"\n  downloads: \"{}\"\n  user: \"{}\"\n",
            dir.join("data").display(),
            dir.join("staging").display(),
            dir.join("sketchbook").display()
        );
        std::fs::write(&cfg, yaml).map_err(|e| FirmwareError::Io(e.to_string()))?;
        let opts = DetectOptions {
            explicit: None,
            search_path: false,
            search_homebrew: false,
            app_dir: dir.to_path_buf(),
        };
        Toolchain::detect_with(&opts)
    }

    // ------------------------------------------------------------------ libraries

    /// Library Manager search (e.g. "ArduinoJson").
    pub fn lib_search(&self, query: &str) -> Result<Vec<LibraryInfo>, FirmwareError> {
        let v = self.run_json(&["lib", "search", query])?;
        let libs = v["libraries"].as_array().cloned().unwrap_or_default();
        Ok(libs
            .iter()
            .map(|l| LibraryInfo {
                name: l["name"].as_str().unwrap_or("").to_string(),
                version: l["latest"]["version"].as_str().unwrap_or("").to_string(),
                author: l["latest"]["author"].as_str().unwrap_or("").to_string(),
                sentence: l["latest"]["sentence"].as_str().unwrap_or("").to_string(),
                versions: l["available_versions"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect())
    }

    /// Installs a library (optionally a specific version).
    pub fn lib_install(
        &self,
        name: &str,
        version: Option<&str>,
        progress: &mut dyn FnMut(InstallProgress),
    ) -> Result<(), FirmwareError> {
        let spec = match version {
            Some(v) => format!("{name}@{v}"),
            None => name.to_string(),
        };
        progress(InstallProgress::Step(format!("Installing {spec}")));
        self.run_streaming(&["lib", "install", &spec], progress)
    }

    /// Installed libraries (user and platform-bundled).
    pub fn lib_list(&self) -> Result<Vec<InstalledLibrary>, FirmwareError> {
        let v = self.run_json(&["lib", "list", "--all"])?;
        let libs = v["installed_libraries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(libs
            .iter()
            .map(|l| {
                let l = &l["library"];
                InstalledLibrary {
                    name: l["name"].as_str().unwrap_or("").to_string(),
                    version: l["version"].as_str().unwrap_or("").to_string(),
                    location: l["location"].as_str().unwrap_or("").to_string(),
                    install_dir: PathBuf::from(l["install_dir"].as_str().unwrap_or("")),
                }
            })
            .collect())
    }
}

/// Native avr-gcc: env override, Breadbox's merged bin dir, or Homebrew osx-cross kegs (a
/// merged bin dir with gcc + binutils is created under `native/avr-gcc/bin`).
fn find_native_gcc(native: &Path) -> Option<PathBuf> {
    if let Some(b) = std::env::var_os("BREADBOX_AVR_GCC_BIN").map(PathBuf::from)
        && gcc_version(&b).is_some()
    {
        return Some(b);
    }
    let merged = native.join("avr-gcc/bin");
    if gcc_version(&merged).is_some() && merged.join("avr-objcopy").exists() {
        return Some(merged);
    }
    let binutils = [
        "/opt/homebrew/opt/avr-binutils/bin",
        "/usr/local/opt/avr-binutils/bin",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.join("avr-objcopy").exists())?;
    for prefix in ["/opt/homebrew/opt", "/usr/local/opt"] {
        for v in [8, 9, 10, 11, 12, 13, 14, 15, 16] {
            let gcc = PathBuf::from(prefix).join(format!("avr-gcc@{v}/bin"));
            if gcc_version(&gcc).is_some() {
                if std::fs::create_dir_all(&merged).is_err() {
                    return None;
                }
                for d in [&gcc, &binutils] {
                    for e in std::fs::read_dir(d).ok()?.flatten() {
                        let target = merged.join(e.file_name());
                        let _ = std::fs::remove_file(&target);
                        #[cfg(unix)]
                        let _ = std::os::unix::fs::symlink(e.path(), &target);
                    }
                }
                return gcc_version(&merged).map(|_| merged);
            }
        }
    }
    None
}

/// arduino-cli release archive for this machine.
pub fn download_url() -> String {
    let asset = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "macOS_ARM64.tar.gz",
        ("macos", _) => "macOS_64bit.tar.gz",
        ("linux", "aarch64") => "Linux_ARM64.tar.gz",
        _ => "Linux_64bit.tar.gz",
    };
    format!("https://downloads.arduino.cc/arduino-cli/arduino-cli_latest_{asset}")
}

fn fetch(url: &str, progress: &mut dyn FnMut(InstallProgress)) -> Result<Vec<u8>, FirmwareError> {
    if let Some(path) = url.strip_prefix("file://") {
        let data = std::fs::read(path).map_err(|e| FirmwareError::Io(e.to_string()))?;
        progress(InstallProgress::Download {
            bytes: data.len() as u64,
            total: Some(data.len() as u64),
        });
        return Ok(data);
    }
    let resp = ureq::get(url)
        .call()
        .map_err(|e| FirmwareError::Download(e.to_string()))?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());
    let mut reader = resp.into_body().into_reader();
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut chunk)
            .map_err(|e| FirmwareError::Download(e.to_string()))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        progress(InstallProgress::Download {
            bytes: buf.len() as u64,
            total,
        });
    }
    Ok(buf)
}

/// Extracts the `arduino-cli` binary from a release `.tar.gz` into `bin_dir`.
pub fn extract_cli(archive: &[u8], bin_dir: &Path) -> Result<PathBuf, FirmwareError> {
    let gz = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(gz);
    let entries = tar
        .entries()
        .map_err(|e| FirmwareError::Io(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| FirmwareError::Io(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| FirmwareError::Io(e.to_string()))?
            .into_owned();
        if path.file_name().is_some_and(|n| n == "arduino-cli") {
            let dest = bin_dir.join("arduino-cli");
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|e| FirmwareError::Io(e.to_string()))?;
            let mut f =
                std::fs::File::create(&dest).map_err(|e| FirmwareError::Io(e.to_string()))?;
            f.write_all(&data)
                .map_err(|e| FirmwareError::Io(e.to_string()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
            }
            return Ok(dest);
        }
    }
    Err(FirmwareError::Io(
        "arduino-cli not found in the archive".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_url_matches_platform() {
        let u = download_url();
        assert!(u.starts_with("https://downloads.arduino.cc/arduino-cli/arduino-cli_latest_"));
        assert!(u.ends_with(".tar.gz"));
    }

    #[test]
    fn extracts_binary_from_tarball() {
        let tmp = std::env::temp_dir().join(format!("bbfw-extract-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let data = b"#!/bin/sh\necho hi\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "LICENSE.txt", &b"x"[..])
            .unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "arduino-cli", &data[..])
            .unwrap();
        let gz = builder.into_inner().unwrap().finish().unwrap();
        let p = extract_cli(&gz, &tmp).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), data);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
