//! Where Breadbox finds its bundled resources and keeps user data (`PLAT-1`, `PLAT-3`).
//!
//! **Resources** (example projects + prebuilt firmware, lessons, first-party plugins, docs, font
//! licences) are read-only and resolved in this order:
//!
//! 1. `$BREADBOX_RESOURCES` (tests, custom installs);
//! 2. inside an app bundle (`Breadbox.app/Contents/MacOS/breadbox`) → `Contents/Resources`.
//!    A bundled app **never** falls back to the source checkout it was built from;
//! 3. a development build → the repository root (this crate's manifest dir at compile time).
//!
//! **User data** never lives in the bundle:
//!
//! - [`app_support`] — `$BREADBOX_APP_SUPPORT` or `~/Library/Application Support/Breadbox`
//!   (installed plugins, lesson progress, the arduino-cli toolchain, app state);
//! - [`caches`] — `$BREADBOX_CACHES` or `~/Library/Caches/Breadbox` (build cache, staging).
//!
//! Other platforms use the XDG data / cache folders (`PLAT-1`: code stays portable).

use std::path::{Path, PathBuf};

/// The user's home folder (`$HOME`, else the current folder).
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `Contents/Resources` when this executable runs from inside a macOS app bundle.
pub fn bundle_resources() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    bundle_resources_for(&exe)
}

/// `…/X.app/Contents/MacOS/<exe>` → `…/X.app/Contents/Resources`.
pub fn bundle_resources_for(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    let is_bundle = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && app.extension().is_some_and(|e| e == "app");
    is_bundle.then(|| contents.join("Resources"))
}

/// True when running from inside an app bundle.
pub fn in_app_bundle() -> bool {
    bundle_resources().is_some()
}

/// This crate's folder in the source checkout, or `None` in builds made for an app bundle
/// (`BREADBOX_BUNDLE=1 cargo build`, `scripts/bundle-macos.sh`): those never contain the
/// path of the machine they were built on.
const MANIFEST_DIR: Option<&str> = match option_env!("BREADBOX_BUNDLE") {
    Some(_) => None,
    None => Some(env!("CARGO_MANIFEST_DIR")),
};

/// The source checkout this binary was built from (development builds only; `None` inside an
/// app bundle, in bundle builds, or when the checkout is gone).
pub fn dev_root() -> Option<PathBuf> {
    if in_app_bundle() {
        return None;
    }
    let root = Path::new(MANIFEST_DIR?).join("../..");
    root.join("examples")
        .is_dir()
        .then(|| root.canonicalize().unwrap_or(root))
}

/// The resources root (see the module docs for the order).
pub fn resources_root() -> Option<PathBuf> {
    if let Some(r) = std::env::var_os("BREADBOX_RESOURCES") {
        return Some(PathBuf::from(r));
    }
    if let Some(res) = bundle_resources() {
        return Some(res);
    }
    dev_root()
}

/// `<resources>/<rel>` if it exists (`resource("examples")`, `resource("docs/PLUGIN-SDK.md")`).
pub fn resource(rel: impl AsRef<Path>) -> Option<PathBuf> {
    resources_root().map(|r| r.join(rel)).filter(|p| p.exists())
}

/// `<resources>/<rel>` whether or not it exists (callers that report a missing path).
pub fn resource_path(rel: impl AsRef<Path>) -> PathBuf {
    resources_root()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(rel)
}

/// Per-user application data: `$BREADBOX_APP_SUPPORT`, else
/// `~/Library/Application Support/Breadbox` (macOS) / `$XDG_DATA_HOME/breadbox`.
pub fn app_support() -> PathBuf {
    if let Some(d) = std::env::var_os("BREADBOX_APP_SUPPORT") {
        return PathBuf::from(d);
    }
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/Breadbox")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".local/share"))
            .join("breadbox")
    }
}

/// Per-user caches: `$BREADBOX_CACHES`, else `~/Library/Caches/Breadbox` (macOS) /
/// `$XDG_CACHE_HOME/breadbox`.
pub fn caches() -> PathBuf {
    if let Some(d) = std::env::var_os("BREADBOX_CACHES") {
        return PathBuf::from(d);
    }
    if cfg!(target_os = "macos") {
        home().join("Library/Caches/Breadbox")
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".cache"))
            .join("breadbox")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_layout_is_recognised() {
        let exe = Path::new("/Applications/Breadbox.app/Contents/MacOS/breadbox");
        assert_eq!(
            bundle_resources_for(exe),
            Some(PathBuf::from(
                "/Applications/Breadbox.app/Contents/Resources"
            ))
        );
        assert_eq!(
            bundle_resources_for(Path::new("/repo/target/release/breadbox")),
            None
        );
        assert_eq!(
            bundle_resources_for(Path::new("/x/Contents/MacOS/breadbox")),
            None,
            "a Contents folder outside an .app is not a bundle"
        );
    }

    #[test]
    fn development_builds_find_the_repository() {
        // tests never run from a bundle
        assert!(!in_app_bundle());
        let root = dev_root().expect("source checkout");
        assert!(root.join("examples").is_dir());
        if std::env::var_os("BREADBOX_RESOURCES").is_none() {
            assert_eq!(resources_root(), Some(root.clone()));
            assert_eq!(resource("examples"), Some(root.join("examples")));
            assert_eq!(resource("no/such/file"), None);
        }
    }

    #[test]
    fn user_data_lives_outside_the_resources() {
        let support = app_support();
        let caches = caches();
        if std::env::var_os("BREADBOX_APP_SUPPORT").is_none() {
            assert!(support.ends_with("Breadbox") || support.ends_with("breadbox"));
        }
        if std::env::var_os("BREADBOX_CACHES").is_none() {
            assert!(caches.ends_with("Breadbox") || caches.ends_with("breadbox"));
        }
        if let Some(r) = resources_root() {
            assert!(!support.starts_with(&r));
            assert!(!caches.starts_with(&r));
        }
    }
}
