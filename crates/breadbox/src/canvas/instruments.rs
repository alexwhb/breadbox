//! The bench instrument hook (`PLUG-6`): how instruments are drawn on the bench and where
//! their probe leads clip on.
//!
//! An instrument is a part on the bench (`InstrumentDef::part`, art from the parts library)
//! plus probe leads (`InstrumentDef::probes`: `"ch1"`, `"plus"`, … → a pin or hole). The
//! canvas asks the [`InstrumentRegistry`] for the [`BenchInstrument`] of an instrument's
//! `kind` and uses it to
//!
//! * turn the live telemetry (scope frames, meter readings, a plugin's front-panel JSON in
//!   `PartTelemetry::panel`) into the [`InstrumentView`] the part art shows,
//! * optionally draw the whole instrument itself (`draw` → `true` replaces the part art), and
//! * route each probe lead: which jack it leaves from and its colour.
//!
//! The built-in 2-channel mini scope and the DMM placeholder are registered here through
//! exactly this path. Plugin instruments (declarative panels from `bb_plugin::panel`) register
//! an implementation keyed by their instrument id with
//! `InstrumentRegistry::register(Arc<dyn BenchInstrument>)` — the canvas needs no change.

use super::art::{ArtCtx, InstrumentView};
use super::pen::{Pen, rgb};
use bb_circuit::project::InstrumentDef;
use bb_sim::Telemetry;
use eframe::egui::Color32;
use std::collections::HashMap;
use std::sync::Arc;

/// One instrument kind on the bench.
pub trait BenchInstrument: Send + Sync {
    /// `InstrumentDef::kind` this handles (`"mini_scope"`, `"dmm"`, a plugin instrument id).
    fn kind(&self) -> &str;

    /// Live data for the art (display text, traces, caption).
    fn view(&self, def: &InstrumentDef, tel: &Telemetry) -> InstrumentView;

    /// Draws the instrument body itself (plugin front panels). Return `false` to let the
    /// part's library art draw it with [`BenchInstrument::view`].
    fn draw(
        &self,
        _pen: &Pen,
        _cx: &ArtCtx,
        _def: &InstrumentDef,
        _tel: Option<&Telemetry>,
    ) -> bool {
        false
    }

    /// The instrument part's lead a probe key leaves from (`"ch1"` → `"CH1"`).
    fn lead_pin(&self, key: &str) -> String {
        key.to_ascii_uppercase()
    }

    /// Lead colour for a probe key.
    fn lead_color(&self, key: &str) -> Color32 {
        match key {
            "ch1" => rgb(0xF2D03B),
            "ch2" => rgb(0x4CC3F0),
            "plus" | "v" | "vo" => rgb(0xC8352B),
            _ => rgb(0x2B2B2B),
        }
    }

    /// Text next to the probe tip (`COM → GND`), if any.
    fn tip_label(&self, _key: &str, _target_net: &str) -> Option<String> {
        None
    }
}

/// Instrument kind → implementation.
#[derive(Clone)]
pub struct InstrumentRegistry {
    map: HashMap<String, Arc<dyn BenchInstrument>>,
}

impl Default for InstrumentRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

impl InstrumentRegistry {
    /// The built-ins: mini scope and DMM.
    pub fn builtin() -> InstrumentRegistry {
        let mut r = InstrumentRegistry {
            map: HashMap::new(),
        };
        r.register(Arc::new(MiniScope));
        r.register(Arc::new(Dmm));
        r
    }

    /// Adds or replaces an implementation (plugin instruments).
    pub fn register(&mut self, i: Arc<dyn BenchInstrument>) {
        self.map.insert(i.kind().to_string(), i);
    }

    pub fn get(&self, kind: &str) -> Option<&Arc<dyn BenchInstrument>> {
        self.map.get(kind)
    }

    pub fn kinds(&self) -> Vec<String> {
        let mut v: Vec<String> = self.map.keys().cloned().collect();
        v.sort();
        v
    }
}

/// The built-in 2-channel mini scope.
pub struct MiniScope;

impl BenchInstrument for MiniScope {
    fn kind(&self) -> &str {
        "mini_scope"
    }

    fn view(&self, def: &InstrumentDef, tel: &Telemetry) -> InstrumentView {
        let Some(f) = tel.scopes.iter().find(|s| s.id == def.id) else {
            return InstrumentView::default();
        };
        let span = (f.timebase * 10.0).max(1e-9);
        let traces = f
            .traces
            .iter()
            .enumerate()
            .map(|(ch, tr)| {
                let vdiv = f.vdiv.get(ch).copied().unwrap_or(1.0).max(1e-9);
                // a step plot (the frame holds value changes); 0 V at −2 divisions
                let mut pts: Vec<(f32, f32)> = Vec::with_capacity(tr.len() * 2);
                for (i, (t, v)) in tr.iter().enumerate() {
                    let x = (t / span) as f32;
                    let y = (v / vdiv - 2.0) as f32;
                    if i > 0
                        && let Some(&(_, py)) = pts.last()
                    {
                        pts.push((x, py));
                    }
                    pts.push((x, y));
                }
                pts
            })
            .collect();
        let duty = if f.frequency > 0.0 {
            format!(" · {:.0}%", f.duty * 100.0)
        } else {
            String::new()
        };
        InstrumentView {
            traces,
            caption: format!("{}{duty}", f.label),
            triggered: f.triggered,
            ..Default::default()
        }
    }

    fn tip_label(&self, key: &str, net: &str) -> Option<String> {
        (key == "gnd").then(|| format!("GND → {net}"))
    }
}

/// The built-in DMM placeholder (a plugin DMM with a real front panel replaces it).
pub struct Dmm;

impl BenchInstrument for Dmm {
    fn kind(&self) -> &str {
        "dmm"
    }

    fn view(&self, def: &InstrumentDef, tel: &Telemetry) -> InstrumentView {
        let Some(m) = tel.meters.iter().find(|m| m.id == def.id) else {
            return InstrumentView::default();
        };
        let mode = match m.mode.as_str() {
            "dc_v" => "DC  AUTO",
            "ac_v" => "AC  AUTO",
            "ohm" | "ohms" => "Ω   AUTO",
            "dc_a" => "DC  A",
            other => other,
        };
        InstrumentView {
            display: m.display.clone(),
            unit: m.unit.clone(),
            mode: mode.into(),
            ..Default::default()
        }
    }

    fn lead_pin(&self, key: &str) -> String {
        match key {
            "plus" | "v" => "VO".into(),
            "com" => "COM".into(),
            k => k.to_ascii_uppercase(),
        }
    }

    fn tip_label(&self, key: &str, net: &str) -> Option<String> {
        (key == "com").then(|| format!("COM → {net}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_registered_and_route_leads() {
        let r = InstrumentRegistry::builtin();
        assert_eq!(r.kinds(), vec!["dmm".to_string(), "mini_scope".to_string()]);
        assert_eq!(r.get("dmm").unwrap().lead_pin("plus"), "VO");
        assert_eq!(r.get("mini_scope").unwrap().lead_pin("ch1"), "CH1");
        struct Fake;
        impl BenchInstrument for Fake {
            fn kind(&self) -> &str {
                "dmm"
            }
            fn view(&self, _: &InstrumentDef, _: &Telemetry) -> InstrumentView {
                InstrumentView {
                    display: "4.200".into(),
                    ..Default::default()
                }
            }
        }
        let mut r = r;
        r.register(Arc::new(Fake));
        let def = InstrumentDef {
            id: "x".into(),
            kind: "dmm".into(),
            part: None,
            probes: Default::default(),
            settings: serde_json::Value::Null,
        };
        assert_eq!(
            r.get("dmm")
                .unwrap()
                .view(&def, &Telemetry::default())
                .display,
            "4.200",
            "plugins replace built-ins"
        );
    }
}
