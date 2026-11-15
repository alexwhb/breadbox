//! World-model plugins (WORLD-5, PLUG-7): the plugin's `bb_world_step(dt_world_s)` runs after
//! the built-in world models every world step, reading and writing world variables through
//! `bb_world_get` / `bb_world_set` / `bb_world_water` (permissions `world:read` /
//! `world:write`).

use crate::sandbox::{CallCtx, Compiled, Sandbox};
use bb_core::WorldMut;
use bb_world::WorldModel;
use std::sync::Arc;

/// A wasm world model.
pub struct WasmWorldModel {
    sb: Option<Sandbox>,
    name: String,
    replaces: Vec<String>,
    pots: Vec<String>,
    first: bool,
    /// Why it stopped (reported once in the log).
    pub error: Option<String>,
}

impl WasmWorldModel {
    pub fn new(c: &Arc<Compiled>, project: &bb_circuit::Project) -> WasmWorldModel {
        let m = &c.manifest;
        let info = m.provides.world_model.clone().unwrap_or_default();
        let config = serde_json::json!({
            "plugin": m.id,
            "version": m.version,
            "role": "world-model",
            "specs": m.specs,
            "world": project.world,
            "permissions": m.permissions,
        });
        let sb = Sandbox::new(c, project.settings.seed ^ 0x5011, None).and_then(|mut sb| {
            sb.init(&config)?;
            Ok(sb)
        });
        let (sb, error) = match sb {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e.to_string())),
        };
        if let Some(e) = &error {
            log::error!("world model {}: {e}", m.id);
        }
        WasmWorldModel {
            sb,
            name: if info.name.is_empty() {
                m.display_name()
            } else {
                info.name
            },
            replaces: info.replaces,
            pots: project.world.pots.iter().map(|p| p.id.clone()).collect(),
            first: true,
            error,
        }
    }
}

impl WorldModel for WasmWorldModel {
    fn name(&self) -> &str {
        &self.name
    }

    fn step(&mut self, world: &mut dyn WorldMut, dt_world_s: f64) {
        if self.error.is_some() {
            return;
        }
        if self.first {
            self.first = false;
            // the pack takes over built-in behaviours
            if self.replaces.iter().any(|r| r == "evaporation") {
                for p in &self.pots {
                    world.set_var(&format!("{p}.evaporation_scale"), 0.0);
                }
            }
        }
        let Some(sb) = self.sb.as_mut() else { return };
        let ptr: *mut (dyn WorldMut + '_) = world;
        // SAFETY: valid for the synchronous call below; cleared by `Sandbox` afterwards.
        let ctx = CallCtx::World(unsafe {
            std::mem::transmute::<*mut (dyn WorldMut + '_), *mut (dyn WorldMut + 'static)>(ptr)
        });
        let r = sb.call::<f64, ()>(ctx, "bb_world_step", dt_world_s);
        for (lvl, text) in sb.out().logs.drain(..) {
            log::log!(
                match lvl {
                    bb_core::LogLevel::Error => log::Level::Error,
                    bb_core::LogLevel::Warn => log::Level::Warn,
                    _ => log::Level::Info,
                },
                "{}: {text}",
                self.name
            );
        }
        if let Err(e) = r {
            log::error!("world model {} stopped: {e}", self.name);
            self.error = Some(e.to_string());
        }
    }
}
