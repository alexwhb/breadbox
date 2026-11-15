//! Everything operating-system specific in `bb-firmware` (`PLAT-1`): where package managers
//! put arduino-cli and the AVR cross compilers, Arduino's default data folder, which folders
//! macOS guards with privacy prompts, the arduino-cli release asset for this machine, file
//! permissions / symlinks, and process-group handling for subprocess timeouts.
//!
//! The rest of the crate is portable and asks this module. A grep test
//! (`tests/platform.rs`) keeps `target_os` / `cfg(unix)` / Homebrew paths out of every other
//! module.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

/// Package-manager prefixes (Homebrew on Apple Silicon and Intel Macs, Linuxbrew).
fn package_prefixes() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["/opt/homebrew", "/usr/local"]
    } else if cfg!(target_os = "linux") {
        &["/home/linuxbrew/.linuxbrew", "/usr/local"]
    } else {
        &[]
    }
}

/// `bin` folders of the package managers, searched for `arduino-cli` after `PATH` (a Mac app
/// started from Finder gets a minimal `PATH` without Homebrew).
pub fn package_bin_dirs() -> Vec<PathBuf> {
    package_prefixes()
        .iter()
        .map(|p| PathBuf::from(p).join("bin"))
        .collect()
}

/// Prefixes searched for Homebrew `osx-cross` kegs (`opt/avr-gcc@N`, `opt/avr-binutils`):
/// `$BREADBOX_BREW_PREFIX` first.
pub fn brew_prefixes() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::env::var_os("BREADBOX_BREW_PREFIX")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    v.extend(package_prefixes().iter().map(PathBuf::from));
    v
}

/// The `brew` executable: `$BREADBOX_BREW`, the standard prefixes, then `PATH`.
pub fn find_brew() -> Option<PathBuf> {
    std::env::var_os("BREADBOX_BREW")
        .map(PathBuf::from)
        .into_iter()
        .chain(package_bin_dirs().into_iter().map(|d| d.join("brew")))
        .chain(
            std::env::var_os("PATH")
                .map(|p| {
                    std::env::split_paths(&p)
                        .map(|d| d.join("brew"))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        )
        .find(|p| p.is_file())
}

/// File name of the arduino-cli executable.
pub fn arduino_cli_exe() -> &'static str {
    if cfg!(target_os = "windows") {
        "arduino-cli.exe"
    } else {
        "arduino-cli"
    }
}

/// The arduino-cli release archive suffix for this machine
/// (`arduino-cli_latest_<asset>` on downloads.arduino.cc).
pub fn arduino_cli_asset() -> &'static str {
    let arm = cfg!(target_arch = "aarch64");
    if cfg!(target_os = "macos") {
        if arm {
            "macOS_ARM64.tar.gz"
        } else {
            "macOS_64bit.tar.gz"
        }
    } else if arm {
        "Linux_ARM64.tar.gz"
    } else {
        "Linux_64bit.tar.gz"
    }
}

/// Arduino's default data folder (cores, tools, package indexes) when no config moves it:
/// `~/Library/Arduino15` (macOS), `%LOCALAPPDATA%\Arduino15` (Windows), `~/.arduino15`.
pub fn default_arduino_data_dir() -> PathBuf {
    let home = bb_core::paths::home();
    if cfg!(target_os = "macos") {
        home.join("Library/Arduino15")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Local"))
            .join("Arduino15")
    } else {
        home.join(".arduino15")
    }
}

/// True for folders macOS protects with a privacy (TCC) prompt — Documents, Desktop,
/// Downloads, iCloud Drive, removable and network volumes. A subprocess touching one of them
/// for an app launched from Finder blocks in `open()` until someone answers the prompt, so
/// arduino-cli must never be pointed there (its default sketchbook is `~/Documents/Arduino`).
pub fn privacy_protected(path: &Path) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let home = bb_core::paths::home();
    let guarded = [
        home.join("Documents"),
        home.join("Desktop"),
        home.join("Downloads"),
        home.join("Library/Mobile Documents"),
        PathBuf::from("/Volumes"),
    ];
    guarded.iter().any(|g| path.starts_with(g))
}

/// Marks a file executable (`chmod 755`); a no-op where files carry no such bit.
pub fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Creates a symbolic link `link` → `target` (merged native tool folders).
pub fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(not(unix))]
    {
        std::fs::copy(target, link).map(|_| ())
    }
}

/// Starts the command in its own process group, so a timeout or cancel can stop it together
/// with everything it started (arduino-cli → avr-gcc, `sh` → `sleep`).
pub fn isolate(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = cmd;
}

/// Kills a child started with [`isolate`] and its whole process group.
pub fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: plain syscall on a process group we created; failure is harmless
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
}

/// Kills what is left of a finished child's process group (a grandchild holding its pipes).
pub fn kill_group(pid: u32) {
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(pid) {
            // SAFETY: as above; the group may already be gone
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_and_exe_name_match_this_machine() {
        assert!(arduino_cli_asset().ends_with(".tar.gz"));
        assert!(arduino_cli_exe().starts_with("arduino-cli"));
    }

    #[test]
    fn documents_is_protected_on_macos_and_app_support_is_not() {
        let home = bb_core::paths::home();
        assert_eq!(
            privacy_protected(&home.join("Documents/Arduino")),
            cfg!(target_os = "macos")
        );
        assert!(!privacy_protected(&home.join(
            "Library/Application Support/Breadbox/toolchain/sketchbook"
        )));
        assert!(!privacy_protected(&default_arduino_data_dir()));
    }
}
