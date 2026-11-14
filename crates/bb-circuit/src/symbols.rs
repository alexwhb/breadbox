//! Built-in schematic symbol templates: pin anchors for standard symbols, and an automatic
//! box layout for ICs, modules and boards.

use crate::part::{PartDef, PinDir, PinRole, SchGeom, SymPin};

fn p(pin: &str, x: f64, y: f64, dir: PinDir) -> SymPin {
    SymPin {
        pin: pin.into(),
        x,
        y,
        dir,
    }
}

/// Symbol ids the renderer must know how to draw.
pub const SYMBOLS: [&str; 27] = [
    "resistor",
    "capacitor",
    "capacitor_polarized",
    "inductor",
    "diode",
    "led",
    "photoresistor",
    "thermistor",
    "potentiometer",
    "switch",
    "button",
    "spdt",
    "npn",
    "pnp",
    "nmos",
    "pmos",
    "opamp",
    "vsource",
    "isource",
    "battery",
    "buzzer",
    "speaker",
    "motor",
    "ground",
    "vcc",
    "box",
    "none",
];

/// Fills `def.schematic.pins`/`size` from the symbol template when the definition does not
/// list anchors itself.
pub fn finalize(def: &mut PartDef) {
    let g = &mut def.schematic;
    if g.symbol.is_empty() {
        g.symbol = "box".into();
    }
    if !g.pins.is_empty() {
        if g.size == (0.0, 0.0) {
            g.size = (6.0, 6.0);
        }
        return;
    }
    let ids: Vec<String> = def.pins.iter().map(|p| p.id.clone()).collect();
    let two = |g: &mut SchGeom, len: f64| {
        if ids.len() >= 2 {
            g.pins = vec![
                p(&ids[0], -len, 0.0, PinDir::Left),
                p(&ids[1], len, 0.0, PinDir::Right),
            ];
        }
        g.size = (2.0 * len, 2.0);
    };
    match g.symbol.as_str() {
        "resistor"
        | "capacitor"
        | "capacitor_polarized"
        | "inductor"
        | "diode"
        | "led"
        | "photoresistor"
        | "thermistor"
        | "switch"
        | "buzzer"
        | "speaker"
        | "motor" => two(g, 3.0),
        "button" => {
            // 4-pin tactile: 1/2 and 3/4 are bonded; draw 1 and 3, put 2/4 on the same anchors
            if ids.len() == 4 {
                g.pins = vec![
                    p(&ids[0], -3.0, 0.0, PinDir::Left),
                    p(&ids[1], -3.0, 0.0, PinDir::Left),
                    p(&ids[2], 3.0, 0.0, PinDir::Right),
                    p(&ids[3], 3.0, 0.0, PinDir::Right),
                ];
                g.size = (6.0, 2.0);
            } else {
                two(g, 3.0)
            }
        }
        "potentiometer" => {
            if ids.len() >= 3 {
                g.pins = vec![
                    p(&ids[0], -3.0, 0.0, PinDir::Left),
                    p(&ids[1], 0.0, 2.0, PinDir::Down),
                    p(&ids[2], 3.0, 0.0, PinDir::Right),
                ];
            }
            g.size = (6.0, 3.0);
        }
        "spdt" => {
            // a, com, b
            if ids.len() >= 3 {
                g.pins = vec![
                    p(&ids[0], 3.0, -1.0, PinDir::Right),
                    p(&ids[1], -3.0, 0.0, PinDir::Left),
                    p(&ids[2], 3.0, 1.0, PinDir::Right),
                ];
            }
            g.size = (6.0, 3.0);
        }
        "vsource" | "isource" | "battery" => {
            if ids.len() >= 2 {
                g.pins = vec![
                    p(&ids[0], 0.0, -3.0, PinDir::Up),
                    p(&ids[1], 0.0, 3.0, PinDir::Down),
                ];
            }
            g.size = (3.0, 6.0);
        }
        "ground" => {
            if let Some(id) = ids.first() {
                g.pins = vec![p(id, 0.0, -1.0, PinDir::Up)];
            }
            g.size = (2.0, 2.0);
        }
        "vcc" => {
            if let Some(id) = ids.first() {
                g.pins = vec![p(id, 0.0, 1.0, PinDir::Down)];
            }
            g.size = (2.0, 2.0);
        }
        "none" => {
            g.pins.clear();
            g.size = (0.0, 0.0);
        }
        _ => {
            g.symbol = "box".into();
            box_layout(def);
        }
    }
}

/// Box symbol: inputs/passives on the left, outputs on the right, supplies on top, grounds at
/// the bottom; 1-grid pin spacing, 2-grid stubs.
pub fn box_layout(def: &mut PartDef) {
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut top = Vec::new();
    let mut bottom = Vec::new();
    for pin in &def.pins {
        match pin.role {
            PinRole::PowerIn | PinRole::PowerOut => top.push(pin.id.clone()),
            PinRole::Ground => bottom.push(pin.id.clone()),
            PinRole::Output | PinRole::OpenDrain | PinRole::AnalogOut => right.push(pin.id.clone()),
            PinRole::Nc => {}
            _ => left.push(pin.id.clone()),
        }
    }
    // balance big pin lists (boards) between left and right
    while left.len() > right.len() + 1 && left.len() > 8 {
        let x = left.pop().unwrap_or_default();
        right.insert(0, x);
    }
    let rows = left.len().max(right.len()).max(1) as f64;
    let cols = top.len().max(bottom.len()).max(2) as f64;
    let w = (cols + 1.0).max(6.0);
    let h = rows + 1.0;
    let mut pins = Vec::new();
    for (i, id) in left.iter().enumerate() {
        pins.push(p(
            id,
            -w / 2.0 - 2.0,
            -h / 2.0 + 1.0 + i as f64,
            PinDir::Left,
        ));
    }
    for (i, id) in right.iter().enumerate() {
        pins.push(p(
            id,
            w / 2.0 + 2.0,
            -h / 2.0 + 1.0 + i as f64,
            PinDir::Right,
        ));
    }
    for (i, id) in top.iter().enumerate() {
        pins.push(p(id, -w / 2.0 + 1.0 + i as f64, -h / 2.0 - 2.0, PinDir::Up));
    }
    for (i, id) in bottom.iter().enumerate() {
        pins.push(p(
            id,
            -w / 2.0 + 1.0 + i as f64,
            h / 2.0 + 2.0,
            PinDir::Down,
        ));
    }
    // round to the grid so wires meet anchors exactly
    for q in pins.iter_mut() {
        q.x = (q.x * 2.0).round() / 2.0;
        q.y = (q.y * 2.0).round() / 2.0;
    }
    def.schematic.pins = pins;
    def.schematic.size = (w, h);
}
