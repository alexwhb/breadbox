//! Declarative AVR board variants (`kind: board`, PLUG-7): an Arduino Pro Mini is an
//! ATmega328P with a different header and no USB. A [`BoardVariant`] names a built-in base
//! board (its core and pin map), lists its header pins and says how it is powered; this module
//! turns it into a part definition in the bb-circuit schema (ARCHITECTURE §3b).

use crate::PluginError;
use crate::manifest::{BoardVariant, Manifest};

/// Pin role from its name.
fn role(name: &str) -> &'static str {
    let n = name.to_ascii_uppercase();
    let n = n.split('.').next().unwrap_or("");
    match n {
        "GND" | "AGND" => "ground",
        "VCC" | "5V" | "3V3" => "power_out",
        "RAW" | "VIN" => "power_in",
        "RST" | "RESET" | "AREF" => "input",
        _ => "io",
    }
}

/// The part definition JSON for a variant.
pub fn part_json(
    m: &Manifest,
    b: &BoardVariant,
    art_url: Option<String>,
) -> Result<serde_json::Value, PluginError> {
    let err = |msg: String| PluginError::Content {
        plugin: m.id.clone(),
        msg: format!("board `{}`: {msg}", b.id),
    };
    if b.pins.is_empty() {
        return Err(err("lists no pins".into()));
    }
    let core_names = match bb_firmware::Board::by_id(&b.base) {
        Some(board) => board
            .pins
            .iter()
            .map(|p| p.name.to_string())
            .collect::<Vec<_>>(),
        None => return Err(err(format!("unknown base board `{}`", b.base))),
    };
    // I/O pins must exist on the base core
    let mut seen = std::collections::BTreeSet::new();
    let mut pins = Vec::new();
    for p in &b.pins {
        let (id, name) = match p.split_once('.') {
            Some((n, _)) => (p.clone(), n.to_string()),
            None => (p.clone(), p.clone()),
        };
        if !seen.insert(id.clone()) {
            return Err(err(format!("pin `{id}` listed twice")));
        }
        let r = role(&id);
        if r == "io" && !core_names.iter().any(|c| c.eq_ignore_ascii_case(&name)) {
            return Err(err(format!(
                "pin `{name}` is not an I/O pin of the {} core",
                b.base
            )));
        }
        pins.push(if id.contains('.') {
            format!("{id}:{r}:{name}")
        } else {
            format!("{id}:{r}")
        });
    }
    let has = |n: &str| b.pins.iter().any(|p| p == n);
    if !has(&b.vcc_pin) {
        return Err(err(format!(
            "supply pin `{}` is not in its pin list",
            b.vcc_pin
        )));
    }
    if !has("GND") {
        return Err(err("needs a GND pin".into()));
    }
    let vcc = &b.vcc_pin;
    let mut spice = vec![format!("$(el:R:load) $({vcc}) $(GND) 100k")];
    if has("RAW") {
        // on-board regulator RAW → VCC (5 V, 0.25 V dropout)
        spice.push("$(el:B:reg) $(n:r) $(GND) V=min(5.0,max(v($(RAW),$(GND))-0.25,0))".to_string());
        spice.push(format!("$(el:R:reg) $(n:r) $({vcc}) 0.5"));
        spice.push("$(el:R:raw) $(RAW) $(GND) 1Meg".into());
    }
    for rst in ["RST", "RESET"] {
        if has(rst) {
            spice.push(format!("$(el:R:rst) $({vcc}) $({rst}) 10k"));
        }
    }
    if has("AREF") {
        spice.push("$(el:R:aref) $(AREF) $(GND) 1G".into());
    }
    let rows = if b.rows.is_empty() {
        let half = b.pins.len().div_ceil(2);
        vec![b.pins[..half].to_vec(), b.pins[half..].to_vec()]
    } else {
        b.rows.clone()
    };
    let label = b
        .label
        .clone()
        .unwrap_or_else(|| "ATmega328P @ 16 MHz".into());
    Ok(serde_json::json!({
        "id": b.id,
        "name": b.name,
        "category": "boards",
        "icon": "dip",
        "ref_prefix": "U",
        "description": if b.description.is_empty() { format!("{} (from {})", b.name, m.display_name()) } else { b.description.clone() },
        "part_number": label,
        "pins": pins,
        "model": {
            "mcu": {"board": b.base, "core": "avr", "vcc_pin": vcc, "label": label},
            "spice": spice,
        },
        "inspector": [
            {"label": "MCU", "value": format!("{label} · cycle-accurate AVR core")},
            {"label": "Power", "value": if has("RAW") { "RAW 5–12 V (regulated) or VCC" } else { "VCC" }},
        ],
        "bench": {
            "art": art_url.unwrap_or_else(|| b.base.clone()),
            "layout": {"rows": {"spacing": 6, "rows": rows}},
        },
        "schematic": {"symbol": "box"},
        "tags": ["firmware", "plugin"],
        "source": m.id,
    }))
}
