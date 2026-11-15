//! "Place on bench" (`PLUG-4`, `PLUG-6`): puts a plugin instrument or part on the open
//! circuit as one undoable edit.
//!
//! An instrument comes with its probe leads (wires from its jacks). If the bench already has
//! the built-in DMM placeholder (`dmm`), a plugin meter with `VO` / `COM` jacks takes its place
//! and its leads clip onto the same two nodes (garden-node: A0 = J1 out and the GND rail). A
//! scope's leads copy the mini scope's channels. Otherwise the leads dangle next to the
//! instrument, ready to be dragged onto any pin.

use crate::circuit::CircuitDoc;
use bb_circuit::edit::{Op, Transaction};
use bb_circuit::project::{Endpoint, PartInstance, Wire};

/// What `place` did.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaceReport {
    pub part: String,
    pub reference: String,
    /// Jack → where its lead clipped on (`VO` → `j1.AOUT`); dangling leads are not listed.
    pub clipped: Vec<(String, String)>,
    /// The built-in instrument it replaced.
    pub replaced: Option<String>,
}

/// The jacks that get a lead: the first (signal) pin and the return (`COM` / `GND` / `NEG`).
fn lead_pins(pins: &[String]) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(first) = pins.first() {
        v.push(first.clone());
    }
    if let Some(ret) = pins
        .iter()
        .find(|p| matches!(p.as_str(), "COM" | "GND" | "NEG"))
        && !v.contains(ret)
    {
        v.push(ret.clone());
    }
    v
}

/// Built-in probe keys a jack takes over (`VO` ← `plus`).
fn keys_for(jack: &str) -> &'static [&'static str] {
    match jack {
        "VO" | "POS" | "OUT" => &["plus", "v", "vo"],
        "COM" | "NEG" => &["com", "gnd"],
        "GND" => &["gnd", "com"],
        "CH1" | "D0" => &["ch1"],
        "CH2" | "D1" => &["ch2"],
        _ => &[],
    }
}

/// Places a part definition (plugin instrument or part) on the bench of `doc`.
pub fn place(doc: &mut CircuitDoc, def_id: &str) -> Result<PlaceReport, String> {
    let lib = doc.lib.clone();
    let def = lib
        .get(def_id)
        .ok_or_else(|| format!("`{def_id}` is not in the parts library — is its plugin active?"))?
        .clone();
    let project = doc.project().clone();
    let pins: Vec<String> = def.pins.iter().map(|p| p.id.clone()).collect();
    let is_instrument = def.category == bb_circuit::Category::Instruments;
    let has_meter_jacks = pins.iter().any(|p| p == "VO") && pins.iter().any(|p| p == "COM");
    // a built-in instrument to take over (the DMM placeholder) or to copy probes from
    let builtin_dmm = project
        .instruments
        .iter()
        .find(|i| i.kind == "dmm" && i.part.is_some())
        .filter(|_| has_meter_jacks)
        .cloned();
    let probe_source = builtin_dmm.clone().or_else(|| {
        project
            .instruments
            .iter()
            .find(|i| i.kind == "mini_scope" && pins.iter().any(|p| p == "CH1"))
            .cloned()
    });
    // where: the replaced part's spot, else right of everything
    let replaced_at = builtin_dmm
        .as_ref()
        .and_then(|i| i.part.as_ref())
        .and_then(|pid| project.part(pid))
        .and_then(|p| p.bench.as_ref())
        .map(|b| (b.at.x, b.at.y));
    let (x, y) = replaced_at.unwrap_or_else(|| {
        let mut max_x = 0.0f64;
        let mut min_y = f64::INFINITY;
        for p in &project.parts {
            let (Some(b), Some(d)) = (&p.bench, lib.get(&p.part)) else {
                continue;
            };
            max_x = max_x.max(b.at.x + d.bench.size.0);
            min_y = min_y.min(b.at.y);
        }
        (
            (max_x + 4.0).round(),
            if min_y.is_finite() {
                min_y.round()
            } else {
                4.0
            },
        )
    });
    // a replaced placeholder hands over its designator + id (DMM1 stays DMM1)
    let old_part = builtin_dmm
        .as_ref()
        .and_then(|i| i.part.as_ref())
        .and_then(|pid| project.part(pid))
        .filter(|p| p.reference.starts_with(&def.ref_prefix));
    let (reference, id) = match old_part {
        Some(p) => (p.reference.clone(), p.id.clone()),
        None => {
            let reference = project.next_ref(&def.ref_prefix);
            let mut id = reference.to_ascii_lowercase();
            if project.part(&id).is_some() || project.wire(&id).is_some() {
                id = project.next_id(&format!("{id}_"));
            }
            (reference, id)
        }
    };
    let mut tx = Transaction::new(&format!("Place {}", def.name));
    let mut replaced = None;
    if let Some(ins) = &builtin_dmm {
        tx.ops.push(Op::Instrument {
            before: Some(ins.clone()),
            after: None,
            index: None,
        });
        if let Some(pid) = &ins.part
            && let Some(old) = project.part(pid)
        {
            // wires on the old meter go too
            for w in &project.wires {
                if w.a.part() == Some(pid.as_str()) || w.b.part() == Some(pid.as_str()) {
                    tx.ops.push(Op::Wire {
                        before: Some(w.clone()),
                        after: None,
                        index: None,
                    });
                }
            }
            tx.ops.push(Op::Part {
                before: Some(old.clone()),
                after: None,
                index: None,
            });
            replaced = Some(old.reference.clone());
        }
    }
    let inst = PartInstance::new(&id, &reference, def_id).at(x, y);
    let leads_at = bb_circuit::Project::bench_leads(&def, &inst).unwrap_or_default();
    tx.ops.push(Op::Part {
        before: None,
        after: Some(inst),
        index: None,
    });
    let mut clipped = Vec::new();
    if is_instrument {
        let mut next_wire = 0usize;
        let mut wire_id = || loop {
            next_wire += 1;
            let w = format!("{id}_lead{next_wire}");
            if project.wire(&w).is_none() {
                return w;
            }
        };
        for (k, jack) in lead_pins(&pins).iter().enumerate() {
            let target = probe_source.as_ref().and_then(|src| {
                keys_for(jack)
                    .iter()
                    .find_map(|key| src.probes.get(*key).cloned())
            });
            let (lx, ly) = leads_at
                .iter()
                .find(|(p, _, _)| p == jack)
                .map(|(_, x, y)| (*x, *y))
                .unwrap_or((x, y));
            let tip = match &target {
                Some(t) => {
                    if let Endpoint::Pin { part, pin } = t {
                        clipped.push((jack.clone(), format!("{part}.{pin}")));
                    }
                    t.clone()
                }
                // dangling below the instrument, a little apart
                None => Endpoint::Free {
                    x: lx - 1.5 + k as f64 * 3.0,
                    y: ly + 5.0,
                },
            };
            let color = if matches!(jack.as_str(), "COM" | "GND" | "NEG") {
                "black"
            } else {
                "red"
            };
            tx.ops.push(Op::Wire {
                before: None,
                after: Some(Wire {
                    id: wire_id(),
                    a: Endpoint::pin(&id, jack),
                    b: tip,
                    color: color.into(),
                    bends: Vec::new(),
                }),
                index: None,
            });
        }
    }
    doc.editor.commit(tx);
    doc.touched();
    Ok(PlaceReport {
        part: id,
        reference,
        clipped,
        replaced,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn plugins_lib() -> std::sync::Arc<bb_circuit::Library> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        let h = bb_plugin::PluginHost::from_dirs(
            &[root.join("dmm-handheld"), root.join("scope-4ch")],
            None,
        );
        assert!(h.errors.is_empty(), "{:?}", h.errors);
        h.library()
    }

    fn garden() -> CircuitDoc {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/garden-node");
        let mut d = CircuitDoc::load(&dir, true).unwrap();
        d.set_library(plugins_lib());
        d
    }

    #[test]
    fn the_plugin_dmm_takes_over_the_placeholder_on_a0() {
        let mut d = garden();
        let r = place(&mut d, "dmm_handheld").unwrap();
        assert_eq!(r.replaced.as_deref(), Some("DMM1"));
        assert_eq!(
            r.clipped,
            vec![
                ("VO".to_string(), "j1.AOUT".to_string()),
                ("COM".to_string(), "bb1.t-1".to_string())
            ]
        );
        let p = d.project();
        assert!(p.instruments.iter().all(|i| i.kind != "dmm"));
        assert!(p.parts.iter().any(|x| x.part == "dmm_handheld"));
        let conn = d.connectivity();
        let a0 = conn.net_of("j1", "AOUT").unwrap();
        assert_eq!(conn.net_of(&r.part, "VO"), Some(a0));
        // one undo step
        assert!(d.undo());
        assert!(d.project().parts.iter().any(|x| x.part == "dmm"));
    }

    #[test]
    fn a_scope_copies_the_mini_scope_channel_and_an_unclipped_meter_dangles() {
        let mut d = garden();
        let r = place(&mut d, "scope_4ch").unwrap();
        assert_eq!(r.replaced, None);
        assert_eq!(r.clipped, vec![("CH1".to_string(), "u1.D9".to_string())]);
        // a second meter: no placeholder left → dangling leads
        place(&mut d, "dmm_handheld").unwrap();
        let r2 = place(&mut d, "dmm_handheld").unwrap();
        assert!(r2.clipped.is_empty());
        let free = d
            .project()
            .wires
            .iter()
            .filter(|w| w.a.part() == Some(r2.part.as_str()))
            .filter(|w| matches!(w.b, Endpoint::Free { .. }))
            .count();
        assert_eq!(free, 2);
    }
}
