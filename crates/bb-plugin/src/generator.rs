//! Generator plugins (PLUG-7): a custom source waveform `bb_generator_eval(t) -> volts`. The
//! host samples one period when the plugin loads and builds a source part whose behavioural
//! SPICE source plays it back exactly in the analog solver (periodic `table()` over the phase),
//! scaled live by its `gain` / `offset` / `frequency` properties.

use crate::PluginError;
use crate::sandbox::{CallCtx, Compiled, Sandbox};
use std::sync::Arc;

/// Samples `(phase 0…1, volts)` of one period.
pub fn sample(c: &Arc<Compiled>) -> Result<Vec<(f64, f64)>, PluginError> {
    let m = &c.manifest;
    let g = m
        .provides
        .generator
        .as_ref()
        .ok_or_else(|| PluginError::Content {
            plugin: m.id.clone(),
            msg: "no `provides.generator`".into(),
        })?;
    let mut sb = Sandbox::new(c, 7, None)?;
    sb.init(&serde_json::json!({
        "plugin": m.id, "version": m.version, "role": "generator", "specs": m.specs,
        "permissions": m.permissions,
    }))?;
    let f = sb
        .func::<f64, f64>("bb_generator_eval")
        .ok_or_else(|| PluginError::Abi {
            plugin: m.id.clone(),
            msg: "does not export bb_generator_eval(t: f64) -> f64".into(),
        })?;
    let n = g.points;
    let mut out = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let phase = i as f64 / n as f64;
        let v = sb.call_typed(CallCtx::None, &f, phase * g.period)?;
        if !v.is_finite() {
            return Err(PluginError::Content {
                plugin: m.id.clone(),
                msg: format!("bb_generator_eval({}) is not finite", phase * g.period),
            });
        }
        out.push((phase, v.clamp(-1e3, 1e3)));
    }
    Ok(out)
}

/// The source part for a generator plugin.
pub fn part_json(c: &Arc<Compiled>) -> Result<serde_json::Value, PluginError> {
    let m = &c.manifest;
    let g = m
        .provides
        .generator
        .as_ref()
        .ok_or_else(|| PluginError::Content {
            plugin: m.id.clone(),
            msg: "no `provides.generator`".into(),
        })?;
    let pts = sample(c)?;
    let table: Vec<String> = pts
        .iter()
        .map(|(x, y)| {
            format!(
                "{},{}",
                bb_spice::expr::fmt_g(*x),
                bb_spice::expr::fmt_g(*y)
            )
        })
        .collect();
    let id = g
        .part
        .clone()
        .unwrap_or_else(|| format!("{}_source", m.id.replace(['.', '-'], "_")));
    Ok(serde_json::json!({
        "id": id,
        "name": g.name,
        "category": "power",
        "icon": "wave",
        "ref_prefix": "V",
        "description": format!("{} — waveform from the {} plugin", g.name, m.display_name()),
        "props": [
            {"key": "gain", "label": "Gain", "unit": "×", "type": "number", "default": 1.0, "min": 0, "max": 100},
            {"key": "offset", "label": "Offset", "unit": "V", "type": "number", "default": 0.0, "min": -50, "max": 50},
            {"key": "frequency", "label": "Frequency", "unit": "Hz", "type": "number",
             "default": 1.0 / g.period, "min": 1e-3, "max": 1e7},
        ],
        "pins": ["OUT:output", "COM:passive"],
        "model": {
            "spice": [
                format!("$(el:B:gen) $(n:g) $(COM) V={{$(ext:offset)+$(ext:gain)*table(fmod(time*$(ext:freq),1),{})}}", table.join(",")),
                format!("$(el:R:out) $(n:g) $(OUT) {}", bb_spice::expr::fmt_g(g.r_out)),
            ],
            "ext": [
                {"name": "gain", "default": "$(gain)"},
                {"name": "offset", "default": "$(offset)"},
                {"name": "freq", "default": "$(frequency)"},
            ],
        },
        "inspector": [
            {"label": "Waveform", "value": g.name},
            {"label": "Output impedance", "value": format!("{} Ω", g.r_out)},
            {"label": "Frequency", "value": "$(frequency|Hz)"},
        ],
        "bench": {"art": "function_generator", "layout": "row", "off_board": true, "size": [10, 6]},
        "schematic": {"symbol": "box"},
        "tags": ["spice", "plugin"],
        "source": m.id,
    }))
}
