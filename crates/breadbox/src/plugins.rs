//! The app's plugin service (`PLUG-4`, `PLUG-7`): the `bb_plugin::Registry` (bundled, user
//! and local plugins with their install / enable / grant state), the active
//! [`PluginHost`] (rebuilt whenever the set of active plugins changes), the plugin-aware bench
//! factory (plugin parts + wasm behaviours + radio packs + world models on every bench), hot
//! reload of local plugins and theme plugins.
//!
//! Screens reach it as `app.plugins`. After anything that changes the active plugins call
//! [`PluginService::changed`]; the app then hands the new parts library to the open circuit
//! and respawns the bench (`AppCtx::apply_plugins`).

use crate::bench::sim::{OptionsHook, SimBench};
use crate::bench::{BenchFactory, BenchLink, BenchSpec};
use bb_circuit::Library;
use bb_plugin::{PluginHost, Registry};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// What a bench needs from the plugins: the parts library and the options hook.
#[derive(Clone)]
pub struct BenchPlugins {
    pub lib: Arc<Library>,
    pub hook: OptionsHook,
}

/// How often local plugin folders are checked for changes (hot reload).
pub const HOT_RELOAD_EVERY: Duration = Duration::from_millis(800);

pub struct PluginService {
    pub registry: Registry,
    host: Option<Arc<PluginHost>>,
    shared: Arc<RwLock<Option<BenchPlugins>>>,
    /// Bumped whenever the active plugins change (screens rebuild what they derived).
    pub generation: u64,
    /// Generation the app last applied to the circuit + bench.
    pub applied: u64,
    last_check: Instant,
    /// Hot-reload notices not shown yet (plugin names).
    pub reloaded: Vec<String>,
    /// Theme plugin whose palette is active.
    pub theme: Option<String>,
}

impl PluginService {
    /// Opens the registry in `user_dir` (default: `~/Library/Application Support/Breadbox/
    /// plugins`) with the bundled first-party plugins.
    pub fn open(user_dir: Option<&Path>) -> PluginService {
        let user = user_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(Registry::default_user_dir);
        let registry = Registry::open(Registry::default_bundled_dir().as_deref(), &user);
        PluginService {
            registry,
            host: None,
            shared: Arc::new(RwLock::new(None)),
            generation: 1,
            applied: 0,
            last_check: Instant::now(),
            reloaded: Vec::new(),
            theme: None,
        }
    }

    /// The host for the active plugins (built on first use: compiles their wasm modules).
    pub fn host(&mut self) -> Arc<PluginHost> {
        if let Some(h) = &self.host {
            return h.clone();
        }
        let t0 = Instant::now();
        let host = Arc::new(PluginHost::new(&mut self.registry, None));
        for e in &host.errors {
            log::warn!("plugin: {e}");
        }
        crate::canvas::plugin_parts::register(&host);
        log::info!(
            "plugins: {} active, host built in {} ms",
            host.ids().len(),
            t0.elapsed().as_millis()
        );
        let base = host.bench_options();
        let hook: OptionsHook = Arc::new(move |o: &mut bb_sim::BenchOptions| {
            o.registry = base.registry.clone();
            o.services = base.services.clone();
            o.world_models.extend(base.world_models.iter().cloned());
        });
        if let Ok(mut g) = self.shared.write() {
            *g = Some(BenchPlugins {
                lib: host.library(),
                hook,
            });
        }
        self.host = Some(host.clone());
        host
    }

    /// Built-in + plugin parts.
    pub fn library(&mut self) -> Arc<Library> {
        self.host().library()
    }

    /// Library + options hook for a bench (lessons build their own `SimBench`).
    pub fn bench_plugins(&mut self) -> BenchPlugins {
        self.host();
        self.shared
            .read()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or_else(|| BenchPlugins {
                lib: Library::builtin(),
                hook: Arc::new(|_| {}),
            })
    }

    /// The app's bench factory: `SimBench` with the active plugins (falls back to the
    /// built-in library before the host was built).
    pub fn bench_factory(&self) -> BenchFactory {
        let shared = self.shared.clone();
        Arc::new(move |spec: &BenchSpec| {
            let bp = shared.read().ok().and_then(|g| g.clone());
            match bp {
                Some(b) => {
                    Box::new(SimBench::with(spec, b.lib, Some(b.hook))) as Box<dyn BenchLink>
                }
                None => Box::new(SimBench::new(spec)) as Box<dyn BenchLink>,
            }
        })
    }

    /// The active plugins changed (install, uninstall, enable, grant, hot reload): the host
    /// is rebuilt on next use and the app re-applies the library + bench.
    pub fn changed(&mut self) {
        self.host = None;
        if let Ok(mut g) = self.shared.write() {
            *g = None;
        }
        self.generation += 1;
    }

    /// Hot reload: checks local plugin folders (throttled). Returns the ids that changed.
    pub fn poll_hot_reload(&mut self) -> Vec<String> {
        if self.last_check.elapsed() < HOT_RELOAD_EVERY {
            return Vec::new();
        }
        self.last_check = Instant::now();
        self.force_hot_reload_check()
    }

    /// Checks local plugin folders now.
    pub fn force_hot_reload_check(&mut self) -> Vec<String> {
        let changed = self.registry.check_changes();
        if !changed.is_empty() {
            for id in &changed {
                let name = self
                    .registry
                    .get(id)
                    .map(|p| p.manifest.display_name())
                    .unwrap_or_else(|| id.clone());
                self.reloaded.push(name);
            }
            self.changed();
        }
        changed
    }

    /// Whether any local (development) plugin is loaded — hot reload only matters then.
    pub fn has_local(&self) -> bool {
        self.registry
            .plugins()
            .any(|p| p.source == bb_plugin::Source::Local)
    }

    /// Folder of a plugin.
    pub fn root(&self, id: &str) -> Option<PathBuf> {
        self.registry.get(id).map(|p| p.root.clone())
    }

    /// Applies (or clears) a theme plugin's palette.
    pub fn apply_theme(&mut self, ctx: &eframe::egui::Context, id: Option<&str>) {
        let theme = id.and_then(|id| {
            self.host()
                .themes()
                .into_iter()
                .find(|(pid, _)| pid == id)
                .map(|(_, t)| t)
        });
        match theme {
            Some(t) => {
                let tokens: Vec<(String, [u8; 4])> = t.colors().into_iter().collect();
                let dark = t.base != "light";
                crate::theme::set_plugin_palette(
                    ctx,
                    Some(crate::theme::palette_with_tokens(dark, &tokens)),
                );
                // the theme's base decides dark / light
                ctx.set_theme(if dark {
                    eframe::egui::ThemePreference::Dark
                } else {
                    eframe::egui::ThemePreference::Light
                });
                self.theme = id.map(str::to_string);
            }
            None => {
                crate::theme::set_plugin_palette(ctx, None);
                self.theme = None;
            }
        }
    }
}

/// A fresh, empty per-process folder for tests (plugins, lesson progress).
pub fn temp_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "breadbox-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::create_dir_all(&d);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_has_the_preinstalled_dmm_and_a_bench_factory() {
        let dir = temp_dir("plugins-unit");
        let mut s = PluginService::open(Some(&dir));
        let lib = s.library();
        assert!(lib.get("dmm_handheld").is_some(), "preinstalled DMM part");
        let g = s.generation;
        s.changed();
        assert!(s.generation > g);
        assert!(s.library().get("dmm_handheld").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
