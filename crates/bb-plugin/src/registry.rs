//! The plugin registry / catalog (PLUG-1, PLUG-4): first-party plugins bundled with the app,
//! plugins the user installed (`~/Library/Application Support/Breadbox/plugins/`), and local
//! development folders ("Load local plugin…", hot-reloaded on file change). Install from a
//! `.bbplugin` package or the catalog requires granting the declared permissions; state
//! (installed / enabled / granted) persists in `state.json` in the user folder.

use crate::manifest::{Category, Kind, Manifest, PermSet, Permission};
use crate::sandbox::Compiled;
use crate::{PluginError, io, package};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// Where a plugin comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Shipped with the app (`Breadbox.app/Contents/Resources/plugins`, `plugins/` in the repo).
    Bundled,
    /// Installed into the user plugin folder.
    User,
    /// A development folder loaded in place (hot-reloaded).
    Local,
}

/// Size + mtime of every file of a plugin folder (hot reload).
#[derive(Debug, Clone, Default, PartialEq)]
struct Stamp(Vec<(PathBuf, u64, Option<SystemTime>)>);

fn stamp(root: &Path) -> Stamp {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, u64, Option<SystemTime>)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut items: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        items.sort_by_key(|e| e.file_name());
        for e in items {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "target" {
                continue;
            }
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if let Ok(md) = e.metadata() {
                out.push((p, md.len(), md.modified().ok()));
            }
        }
    }
    let mut v = Vec::new();
    walk(root, &mut v);
    Stamp(v)
}

/// One known plugin.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub manifest: Arc<Manifest>,
    pub root: PathBuf,
    pub source: Source,
    /// Installed (user/local plugins always are; bundled ones once installed from the catalog).
    pub installed: bool,
    pub enabled: bool,
    pub granted: PermSet,
    /// Why it cannot run (bad module, incompatible…).
    pub error: Option<String>,
    pub warnings: Vec<String>,
    compiled: Option<Arc<Compiled>>,
    stamp: Stamp,
}

impl Plugin {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }
    /// Declared permissions not granted yet.
    pub fn missing_permissions(&self) -> Vec<Permission> {
        self.manifest
            .permissions
            .iter()
            .filter(|p| !self.granted.contains(p))
            .copied()
            .collect()
    }
    /// Installed, enabled, approved and loadable.
    pub fn active(&self) -> bool {
        self.installed
            && self.enabled
            && self.error.is_none()
            && self.missing_permissions().is_empty()
    }
    /// The read-only `plugin.json` pane.
    pub fn manifest_text(&self) -> String {
        std::fs::read_to_string(self.root.join("plugin.json"))
            .unwrap_or_else(|_| serde_json::to_string_pretty(&*self.manifest).unwrap_or_default())
    }
    /// Absolute path of a file inside the plugin (`None` if it escapes the folder).
    pub fn asset(&self, rel: &str) -> Option<PathBuf> {
        crate::manifest::safe_rel_path(rel).then(|| self.root.join(crate::manifest::clean(rel)))
    }
}

/// Catalog card status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum CatalogStatus {
    Installed,
    /// Can be installed (Install button).
    Available,
    /// Installed, a newer version ships with the app.
    UpdateAvailable {
        version: String,
    },
    /// Installed but switched off.
    Disabled,
    /// Installed, waiting for permission approval.
    NeedsApproval {
        missing: Vec<Permission>,
    },
    /// Shown in "More to add" but not available in this version.
    Planned {
        note: String,
    },
    Error {
        message: String,
    },
}

/// One catalog card (installed list + "More to add").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub kinds: Vec<Kind>,
    /// `INSTRUMENT`, `WORLD MODEL`, …
    pub chip: String,
    pub category: Category,
    pub summary: String,
    pub source: Option<Source>,
    pub status: CatalogStatus,
    pub permissions: Vec<Permission>,
}

/// `catalog.json` next to the bundled plugins.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CatalogMeta {
    /// Card order.
    #[serde(default)]
    order: Vec<String>,
    /// Installed on first run (granted their permissions).
    #[serde(default)]
    preinstalled: Vec<String>,
    #[serde(default)]
    planned: Vec<PlannedEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlannedEntry {
    id: String,
    name: String,
    kind: Kind,
    summary: String,
    #[serde(default)]
    note: String,
}

/// Persisted user choices.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    installed: BTreeSet<String>,
    #[serde(default)]
    uninstalled: BTreeSet<String>,
    #[serde(default)]
    disabled: BTreeSet<String>,
    #[serde(default)]
    granted: BTreeMap<String, Vec<Permission>>,
    #[serde(default)]
    local: Vec<PathBuf>,
}

/// What an install did.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallReport {
    pub id: String,
    pub version: String,
    /// Version it replaced.
    pub replaced: Option<String>,
    pub warnings: Vec<String>,
}

/// The registry.
#[derive(Debug)]
pub struct Registry {
    bundled_dir: Option<PathBuf>,
    user_dir: PathBuf,
    meta: CatalogMeta,
    state: State,
    /// Bundled plugins by id (even when a user/local copy wins).
    bundled: BTreeMap<String, Plugin>,
    /// The copy that runs, by id.
    plugins: BTreeMap<String, Plugin>,
    /// Folders that failed to load: (folder, error).
    pub load_errors: Vec<(PathBuf, String)>,
}

impl Registry {
    /// `~/Library/Application Support/Breadbox/plugins` (or `$BREADBOX_PLUGIN_DIR`).
    pub fn default_user_dir() -> PathBuf {
        if let Ok(d) = std::env::var("BREADBOX_PLUGIN_DIR") {
            return PathBuf::from(d);
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join("Library/Application Support/Breadbox/plugins")
    }

    /// First-party plugins: `$BREADBOX_BUNDLED_PLUGINS`, `<app>/Contents/Resources/plugins`, or
    /// the repository's `plugins/` folder.
    pub fn default_bundled_dir() -> Option<PathBuf> {
        if let Ok(d) = std::env::var("BREADBOX_BUNDLED_PLUGINS") {
            return Some(PathBuf::from(d));
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(macos) = exe.parent()
        {
            let res = macos.join("../Resources/plugins");
            if res.join("catalog.json").exists() {
                return Some(res);
            }
        }
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        repo.join("catalog.json").exists().then_some(repo)
    }

    /// Opens the registry: scans bundled, user and local plugins and applies saved state.
    pub fn open(bundled_dir: Option<&Path>, user_dir: &Path) -> Registry {
        let mut r = Registry {
            bundled_dir: bundled_dir.map(Path::to_path_buf),
            user_dir: user_dir.to_path_buf(),
            meta: CatalogMeta::default(),
            state: State::default(),
            bundled: BTreeMap::new(),
            plugins: BTreeMap::new(),
            load_errors: Vec::new(),
        };
        if let Some(b) = &r.bundled_dir
            && let Ok(text) = std::fs::read_to_string(b.join("catalog.json"))
        {
            match serde_json::from_str(&text) {
                Ok(m) => r.meta = m,
                Err(e) => r.load_errors.push((b.join("catalog.json"), e.to_string())),
            }
        }
        let first_run = !user_dir.join("state.json").exists();
        if let Ok(text) = std::fs::read_to_string(user_dir.join("state.json"))
            && let Ok(s) = serde_json::from_str(&text)
        {
            r.state = s;
        }
        if first_run {
            for id in r.meta.preinstalled.clone() {
                r.state.installed.insert(id);
            }
        }
        r.rescan();
        if first_run {
            // first-party preinstalled plugins are approved
            for id in r.meta.preinstalled.clone() {
                if let Some(p) = r.bundled.get(&id) {
                    r.state
                        .granted
                        .insert(id.clone(), p.manifest.permissions.clone());
                }
            }
            r.rescan();
            let _ = r.save();
        }
        r
    }

    /// The default registry for the app.
    pub fn open_default() -> Registry {
        Registry::open(
            Registry::default_bundled_dir().as_deref(),
            &Registry::default_user_dir(),
        )
    }

    pub fn user_dir(&self) -> &Path {
        &self.user_dir
    }

    fn load_dir(&mut self, dir: &Path, source: Source) -> Option<Plugin> {
        match Manifest::load(dir) {
            Ok(m) => {
                let warnings = m.validate(Some(dir)).unwrap_or_default();
                Some(Plugin {
                    manifest: Arc::new(m),
                    root: dir.to_path_buf(),
                    source,
                    installed: true,
                    enabled: true,
                    granted: PermSet::new(),
                    error: None,
                    warnings: warnings.into_iter().map(|w| w.0).collect(),
                    compiled: None,
                    stamp: stamp(dir),
                })
            }
            Err(e) => {
                self.load_errors.push((dir.to_path_buf(), e.to_string()));
                None
            }
        }
    }

    fn subdirs(dir: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.join("plugin.json").exists())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Re-reads every plugin folder.
    pub fn rescan(&mut self) {
        self.load_errors.clear();
        self.bundled.clear();
        self.plugins.clear();
        if let Some(b) = self.bundled_dir.clone() {
            for d in Registry::subdirs(&b) {
                if let Some(p) = self.load_dir(&d, Source::Bundled) {
                    self.bundled.insert(p.manifest.id.clone(), p);
                }
            }
        }
        let mut chosen: BTreeMap<String, Plugin> = BTreeMap::new();
        for (id, p) in &self.bundled {
            chosen.insert(id.clone(), p.clone());
        }
        for d in Registry::subdirs(&self.user_dir.clone()) {
            if let Some(p) = self.load_dir(&d, Source::User) {
                let id = p.manifest.id.clone();
                let newer = match chosen.get(&id) {
                    Some(b) => p.manifest.semver().ok() >= b.manifest.semver().ok(),
                    None => true,
                };
                if newer {
                    chosen.insert(id, p);
                }
            }
        }
        for d in self.state.local.clone() {
            if let Some(p) = self.load_dir(&d, Source::Local) {
                chosen.insert(p.manifest.id.clone(), p);
            }
        }
        for (id, p) in chosen.iter_mut() {
            p.installed = match p.source {
                Source::Bundled => {
                    self.state.installed.contains(id) && !self.state.uninstalled.contains(id)
                }
                _ => true,
            };
            p.enabled = !self.state.disabled.contains(id);
            p.granted = self
                .state
                .granted
                .get(id)
                .map(|v| v.iter().copied().collect())
                .unwrap_or_default();
        }
        self.plugins = chosen;
    }

    fn save(&self) -> Result<(), PluginError> {
        std::fs::create_dir_all(&self.user_dir).map_err(|e| io(&self.user_dir, e))?;
        let p = self.user_dir.join("state.json");
        let text = serde_json::to_string_pretty(&self.state).unwrap_or_default();
        let tmp = self.user_dir.join("state.json.tmp");
        std::fs::write(&tmp, text).map_err(|e| io(&tmp, e))?;
        std::fs::rename(&tmp, &p).map_err(|e| io(&p, e))
    }

    pub fn plugins(&self) -> impl Iterator<Item = &Plugin> {
        self.plugins.values()
    }

    pub fn get(&self, id: &str) -> Option<&Plugin> {
        self.plugins.get(id)
    }

    /// Installed + enabled + approved plugins (what the bench loads).
    pub fn active(&self) -> Vec<&Plugin> {
        self.plugins.values().filter(|p| p.active()).collect()
    }

    /// Compiles (once) the plugin's wasm module.
    pub fn compiled(&mut self, id: &str) -> Result<Option<Arc<Compiled>>, PluginError> {
        let p = self
            .plugins
            .get_mut(id)
            .ok_or_else(|| PluginError::NotFound(id.into()))?;
        if p.compiled.is_some() {
            return Ok(p.compiled.clone());
        }
        let Some(model) = p.manifest.model_path().map(String::from) else {
            return Ok(None);
        };
        let path = p.root.join(crate::manifest::clean(&model));
        let bytes = std::fs::read(&path).map_err(|e| io(&path, e))?;
        match Compiled::new(p.manifest.clone(), p.root.clone(), &bytes) {
            Ok(c) => {
                let c = Arc::new(c);
                p.compiled = Some(c.clone());
                Ok(Some(c))
            }
            Err(e) => {
                p.error = Some(e.to_string());
                Err(e)
            }
        }
    }

    /// The catalog: installed plugins first, then "More to add" in `catalog.json` order.
    pub fn catalog(&self) -> Vec<CatalogEntry> {
        let entry = |p: &Plugin| {
            let m = &p.manifest;
            let status = if let Some(e) = &p.error {
                CatalogStatus::Error { message: e.clone() }
            } else if !p.installed {
                CatalogStatus::Available
            } else if !p.missing_permissions().is_empty() {
                CatalogStatus::NeedsApproval {
                    missing: p.missing_permissions(),
                }
            } else if !p.enabled {
                CatalogStatus::Disabled
            } else {
                match self.bundled.get(m.id.as_str()) {
                    Some(b)
                        if p.source == Source::User
                            && b.manifest.semver().ok() > m.semver().ok() =>
                    {
                        CatalogStatus::UpdateAvailable {
                            version: b.manifest.version.clone(),
                        }
                    }
                    _ => CatalogStatus::Installed,
                }
            };
            CatalogEntry {
                id: m.id.clone(),
                name: m.display_name(),
                version: m.version.clone(),
                kinds: m.kind.clone(),
                chip: m.kind.first().map(|k| k.chip()).unwrap_or("PLUGIN").into(),
                category: m.category(),
                summary: m.summary.clone(),
                source: Some(p.source),
                status,
                permissions: m.permissions.clone(),
            }
        };
        let rank = |id: &str| {
            self.meta
                .order
                .iter()
                .position(|x| x == id)
                .unwrap_or(usize::MAX)
        };
        let mut v: Vec<CatalogEntry> = self.plugins.values().map(entry).collect();
        for pl in &self.meta.planned {
            if !self.plugins.contains_key(&pl.id) {
                v.push(CatalogEntry {
                    id: pl.id.clone(),
                    name: pl.name.clone(),
                    version: String::new(),
                    kinds: vec![pl.kind],
                    chip: pl.kind.chip().into(),
                    category: pl.kind.category(),
                    summary: pl.summary.clone(),
                    source: None,
                    status: CatalogStatus::Planned {
                        note: if pl.note.is_empty() {
                            "Not available in this version".into()
                        } else {
                            pl.note.clone()
                        },
                    },
                    permissions: vec![],
                });
            }
        }
        v.sort_by(|a, b| {
            let inst = |e: &CatalogEntry| {
                !matches!(
                    e.status,
                    CatalogStatus::Installed
                        | CatalogStatus::UpdateAvailable { .. }
                        | CatalogStatus::Disabled
                        | CatalogStatus::NeedsApproval { .. }
                )
            };
            (inst(a), rank(&a.id), a.name.clone()).cmp(&(inst(b), rank(&b.id), b.name.clone()))
        });
        v
    }

    fn check_grant(m: &Manifest, granted: &[Permission]) -> Result<(), PluginError> {
        let missing: Vec<Permission> = m
            .permissions
            .iter()
            .filter(|p| !granted.contains(p))
            .copied()
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(PluginError::NotGranted {
                plugin: m.id.clone(),
                missing,
            })
        }
    }

    /// Installs a `.bbplugin` package into the user folder. `granted` must cover every declared
    /// permission (the install prompt); an older version than the installed one is refused
    /// unless `allow_downgrade`.
    pub fn install_package(
        &mut self,
        bytes: &[u8],
        granted: &[Permission],
        allow_downgrade: bool,
    ) -> Result<InstallReport, PluginError> {
        let m = package::inspect(bytes)?;
        Registry::check_grant(&m, granted)?;
        let new_v = m.semver()?;
        let existing = self
            .plugins
            .get(&m.id)
            .filter(|p| p.source == Source::User)
            .map(|p| p.manifest.version.clone());
        if let Some(old) = &existing
            && let Ok(old_v) = semver::Version::parse(old)
            && old_v > new_v
            && !allow_downgrade
        {
            return Err(PluginError::Downgrade {
                plugin: m.id.clone(),
                have: old.clone(),
                offered: m.version.clone(),
            });
        }
        std::fs::create_dir_all(&self.user_dir).map_err(|e| io(&self.user_dir, e))?;
        let dest = self.user_dir.join(&m.id);
        let tmp = self.user_dir.join(format!(".{}.installing", m.id));
        let _ = std::fs::remove_dir_all(&tmp);
        let unpacked = package::unpack(bytes, &tmp);
        let man = match unpacked {
            Ok(x) => x,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&tmp);
                return Err(e);
            }
        };
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&tmp, &dest).map_err(|e| io(&dest, e))?;
        self.state.uninstalled.remove(&man.id);
        self.state.granted.insert(man.id.clone(), granted.to_vec());
        self.save()?;
        self.rescan();
        Ok(InstallReport {
            id: man.id.clone(),
            version: man.version.clone(),
            replaced: existing,
            warnings: self
                .plugins
                .get(&man.id)
                .map(|p| p.warnings.clone())
                .unwrap_or_default(),
        })
    }

    /// Installs a `.bbplugin` file.
    pub fn install_file(
        &mut self,
        path: &Path,
        granted: &[Permission],
    ) -> Result<InstallReport, PluginError> {
        let bytes = std::fs::read(path).map_err(|e| io(path, e))?;
        self.install_package(&bytes, granted, false)
    }

    /// "Install" on a first-party catalog card.
    pub fn install_bundled(
        &mut self,
        id: &str,
        granted: &[Permission],
    ) -> Result<InstallReport, PluginError> {
        let p = self
            .bundled
            .get(id)
            .ok_or_else(|| PluginError::NotFound(id.into()))?;
        Registry::check_grant(&p.manifest, granted)?;
        let version = p.manifest.version.clone();
        self.state.installed.insert(id.into());
        self.state.uninstalled.remove(id);
        self.state.granted.insert(id.into(), granted.to_vec());
        self.save()?;
        self.rescan();
        Ok(InstallReport {
            id: id.into(),
            version,
            replaced: None,
            warnings: vec![],
        })
    }

    /// "Load local plugin…": runs a development folder in place (hot-reloaded).
    pub fn load_local(
        &mut self,
        dir: &Path,
        granted: &[Permission],
    ) -> Result<InstallReport, PluginError> {
        let m = Manifest::load(dir)?;
        Registry::check_grant(&m, granted)?;
        let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        if !self.state.local.contains(&dir) {
            self.state.local.push(dir);
        }
        self.state.granted.insert(m.id.clone(), granted.to_vec());
        self.state.disabled.remove(&m.id);
        self.save()?;
        self.rescan();
        Ok(InstallReport {
            id: m.id.clone(),
            version: m.version.clone(),
            replaced: None,
            warnings: vec![],
        })
    }

    /// Removes a plugin (user copy deleted, bundled one marked not installed, local forgotten).
    pub fn uninstall(&mut self, id: &str) -> Result<(), PluginError> {
        let p = self
            .plugins
            .get(id)
            .cloned()
            .ok_or_else(|| PluginError::NotFound(id.into()))?;
        match p.source {
            Source::User => {
                std::fs::remove_dir_all(&p.root).map_err(|e| io(&p.root, e))?;
                if self.bundled.contains_key(id) {
                    self.state.uninstalled.insert(id.into());
                }
            }
            Source::Bundled => {
                self.state.installed.remove(id);
                self.state.uninstalled.insert(id.into());
            }
            Source::Local => {
                self.state.local.retain(|d| *d != p.root);
            }
        }
        self.state.granted.remove(id);
        self.save()?;
        self.rescan();
        Ok(())
    }

    pub fn set_enabled(&mut self, id: &str, on: bool) -> Result<(), PluginError> {
        if !self.plugins.contains_key(id) {
            return Err(PluginError::NotFound(id.into()));
        }
        if on {
            self.state.disabled.remove(id);
        } else {
            self.state.disabled.insert(id.into());
        }
        self.save()?;
        if let Some(p) = self.plugins.get_mut(id) {
            p.enabled = on;
        }
        Ok(())
    }

    /// Approves permissions (e.g. after an update asked for more).
    pub fn grant(&mut self, id: &str, perms: &[Permission]) -> Result<(), PluginError> {
        let entry = self.state.granted.entry(id.into()).or_default();
        for p in perms {
            if !entry.contains(p) {
                entry.push(*p);
            }
        }
        self.save()?;
        self.rescan();
        Ok(())
    }

    /// Hot reload: re-reads local plugins whose files changed; returns their ids. The caller
    /// rebuilds its [`crate::PluginHost`] and the bench.
    pub fn check_changes(&mut self) -> Vec<String> {
        let mut changed = Vec::new();
        let locals: Vec<(String, PathBuf, Stamp)> = self
            .plugins
            .values()
            .filter(|p| p.source == Source::Local)
            .map(|p| (p.manifest.id.clone(), p.root.clone(), p.stamp.clone()))
            .collect();
        for (id, root, old) in locals {
            if stamp(&root) != old {
                changed.push(id);
            }
        }
        if !changed.is_empty() {
            self.rescan();
        }
        changed
    }

    /// Bundled plugin by id (catalog details for not-installed ones).
    pub fn bundled(&self, id: &str) -> Option<&Plugin> {
        self.bundled.get(id)
    }
}
