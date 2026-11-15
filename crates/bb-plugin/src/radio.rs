//! Radio packs (PLUG-7): a radio plugin can enable the LoRa medium + virtual gateway, apply
//! gateway settings, add module parts (RYLR896 bridged by `bb_sim`'s radio-module devices) and
//! run a wasm extension next to the modems (`bb_radio_step(now_s) -> next_s`) with MQTT access
//! to the built-in broker (`radio:space`) — e.g. an uplink decoder that turns LoRa payloads
//! into JSON topics.

use crate::sandbox::{CallCtx, Compiled, Sandbox};
use bb_core::SimTime;
use bb_radio::RadioSpace;
use bb_sim::radio::RadioExtension;
use std::sync::Arc;

/// A wasm radio extension.
pub struct WasmRadioExtension {
    sb: Option<Sandbox>,
    name: String,
    logs: Vec<String>,
    error: Option<String>,
}

impl WasmRadioExtension {
    pub fn new(c: &Arc<Compiled>, project: &bb_circuit::Project) -> WasmRadioExtension {
        let m = &c.manifest;
        let config = serde_json::json!({
            "plugin": m.id,
            "version": m.version,
            "role": "radio",
            "specs": m.specs,
            "permissions": m.permissions,
        });
        let sb = Sandbox::new(c, project.settings.seed ^ 0x7ad10, None).and_then(|mut sb| {
            sb.init(&config)?;
            Ok(sb)
        });
        let (sb, error) = match sb {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e.to_string())),
        };
        WasmRadioExtension {
            sb,
            name: m.display_name(),
            logs: error.iter().cloned().collect(),
            error,
        }
    }
}

impl RadioExtension for WasmRadioExtension {
    fn name(&self) -> &str {
        &self.name
    }
    fn step(&mut self, space: &mut RadioSpace, now: SimTime) -> Option<SimTime> {
        if self.error.is_some() {
            return None;
        }
        let sb = self.sb.as_mut()?;
        let ctx = CallCtx::Radio(space as *mut RadioSpace, now);
        let r = sb.call::<f64, f64>(ctx, "bb_radio_step", now.as_secs_f64());
        for (_, text) in sb.out().logs.drain(..) {
            self.logs.push(text);
        }
        match r {
            Ok(Some(next)) if next.is_finite() && next > now.as_secs_f64() => {
                Some(SimTime::from_secs_f64(next))
            }
            Ok(_) => None,
            Err(e) => {
                self.logs.push(format!("stopped: {e}"));
                self.error = Some(e.to_string());
                None
            }
        }
    }
    fn take_logs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.logs)
    }
}
