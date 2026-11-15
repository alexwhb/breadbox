//! Declarative instrument panels (PLUG-4, PLUG-6): a plugin describes its front panel once
//! (`provides.panel` → [`Panel`]) and its wasm model pushes the live [`PanelState`] with
//! `bb_set_panel`; the UI renders both and sends control changes back as
//! `UserInput::Value { key: <control id>, value }` (`Command::Input`).
//!
//! Coordinates are millimetres on the instrument face (origin top-left).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `[x, y, w, h]` in mm.
pub type Rect = [f32; 4];

/// The static panel layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    /// Face size in mm `[w, h]`.
    pub size: [f32; 2],
    /// Face colour (`#F2C14E`).
    #[serde(default)]
    pub color: Option<String>,
    /// Face art (svg/png in the plugin) drawn under the elements.
    #[serde(default)]
    pub art: Option<String>,
    pub elements: Vec<Element>,
}

/// One rotary-switch position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Position {
    /// Value sent with the control (`dc_v`).
    pub value: String,
    /// Printed label (`V⎓`).
    pub label: String,
    /// Angle on the dial (degrees, 0 = up, clockwise).
    #[serde(default)]
    pub angle: Option<f32>,
}

/// A trace channel style on a screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub label: String,
    /// `#F7C77A`.
    pub color: String,
}

/// Panel elements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Element {
    /// A numeric display (LCD / 7-segment).
    Display {
        id: String,
        rect: Rect,
        /// Digit positions (`4` for a 6000-count meter).
        digits: u32,
        /// `seven_segment`, `lcd`, `vfd`.
        #[serde(default = "default_style")]
        style: String,
        /// Units it can show (`mV`, `V`, `Ω`, `kΩ`, …).
        #[serde(default)]
        units: Vec<String>,
        /// Annunciators it can light (`AUTO`, `HOLD`, `MAX`, `AC`, `·)))`).
        #[serde(default)]
        annunciators: Vec<String>,
        /// Bar graph segments (0 = none).
        #[serde(default)]
        bar: u32,
    },
    Button {
        id: String,
        rect: Rect,
        label: String,
        /// Sends 1 on press and 0 on release (else a single 1).
        #[serde(default)]
        momentary: bool,
        #[serde(default)]
        color: Option<String>,
    },
    /// Rotary selector (a meter's dial): sends the position index.
    Rotary {
        id: String,
        rect: Rect,
        positions: Vec<Position>,
        #[serde(default)]
        default: usize,
    },
    /// Continuous knob: sends its value.
    Knob {
        id: String,
        rect: Rect,
        label: String,
        min: f64,
        max: f64,
        #[serde(default)]
        default: f64,
        #[serde(default)]
        step: Option<f64>,
        #[serde(default)]
        log: bool,
        #[serde(default)]
        unit: String,
    },
    /// Multi-position slide/toggle switch (coupling AC/DC, 1×/10×): sends the index.
    Switch {
        id: String,
        rect: Rect,
        label: String,
        options: Vec<String>,
        #[serde(default)]
        default: usize,
    },
    /// A probe jack: the part pin a lead plugs into.
    Jack {
        id: String,
        rect: Rect,
        /// Part pin id (`VO`).
        pin: String,
        label: String,
        /// Jack ring colour (`#D9453B` red, `#1C1C1C` black).
        #[serde(default)]
        color: Option<String>,
    },
    Led {
        id: String,
        rect: Rect,
        #[serde(default)]
        label: String,
        color: String,
    },
    /// A screen area the model draws traces into (scope, logic analyser, spectrum).
    Screen {
        id: String,
        rect: Rect,
        /// `scope`, `logic`, `spectrum`, `text`.
        kind: String,
        /// Graticule divisions `[x, y]`.
        #[serde(default = "default_grid")]
        grid: [u32; 2],
        #[serde(default)]
        channels: Vec<Channel>,
    },
    Label {
        rect: Rect,
        text: String,
        #[serde(default)]
        color: Option<String>,
    },
}

fn default_style() -> String {
    "lcd".into()
}
fn default_grid() -> [u32; 2] {
    [10, 8]
}

impl Element {
    /// Control id (displays/screens/LEDs/jacks have ids too; labels don't).
    pub fn id(&self) -> Option<&str> {
        match self {
            Element::Display { id, .. }
            | Element::Button { id, .. }
            | Element::Rotary { id, .. }
            | Element::Knob { id, .. }
            | Element::Switch { id, .. }
            | Element::Jack { id, .. }
            | Element::Led { id, .. }
            | Element::Screen { id, .. } => Some(id),
            Element::Label { .. } => None,
        }
    }
    /// Whether the user can operate it.
    pub fn is_control(&self) -> bool {
        matches!(
            self,
            Element::Button { .. }
                | Element::Rotary { .. }
                | Element::Knob { .. }
                | Element::Switch { .. }
        )
    }
}

impl Panel {
    /// Ids unique, rotary/switch options non-empty, jacks refer to `pins`.
    pub fn validate(&self, pins: &[String]) -> Result<(), String> {
        let mut seen = std::collections::BTreeSet::new();
        for e in &self.elements {
            if let Some(id) = e.id()
                && !seen.insert(id.to_string())
            {
                return Err(format!("panel element id `{id}` used twice"));
            }
            match e {
                Element::Rotary { id, positions, .. } if positions.is_empty() => {
                    return Err(format!("rotary `{id}` has no positions"));
                }
                Element::Switch { id, options, .. } if options.is_empty() => {
                    return Err(format!("switch `{id}` has no options"));
                }
                Element::Jack { id, pin, .. } if !pins.is_empty() && !pins.contains(pin) => {
                    return Err(format!("jack `{id}` refers to unknown pin `{pin}`"));
                }
                Element::Knob { id, min, max, .. }
                    if min.partial_cmp(max) != Some(std::cmp::Ordering::Less) =>
                {
                    return Err(format!("knob `{id}`: min must be < max"));
                }
                _ => {}
            }
        }
        Ok(())
    }
    /// The control's value as sent for `UserInput::Value` (rotary/switch → index).
    pub fn position_index(&self, control: &str, value: &str) -> Option<usize> {
        self.elements.iter().find_map(|e| match e {
            Element::Rotary { id, positions, .. } if id == control => {
                positions.iter().position(|p| p.value == value)
            }
            Element::Switch { id, options, .. } if id == control => {
                options.iter().position(|o| o == value)
            }
            _ => None,
        })
    }
}

/// Live state of a display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DisplayState {
    /// Digits as shown (`1.882`, `OL`, `-0.4`, `----`).
    pub text: String,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub annunciators: Vec<String>,
    /// Bar graph 0…1.
    #[serde(default)]
    pub bar: Option<f32>,
}

/// A trace on a screen: points in screen divisions (`x` 0…grid.x, `y` −grid.y/2…grid.y/2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Trace {
    pub channel: String,
    pub points: Vec<[f32; 2]>,
    /// Logic traces: draw as levels.
    #[serde(default)]
    pub digital: bool,
}

/// An annotation on a screen (decoded bytes, cursor labels, trigger marker).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Marker {
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    pub text: String,
    #[serde(default)]
    pub channel: String,
}

/// Live state of a screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ScreenState {
    #[serde(default)]
    pub traces: Vec<Trace>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    /// Status text along the top (`CH1 2 V/div · 1 ms/div · Trig'd`).
    #[serde(default)]
    pub header: String,
    /// Vertical cursors (divisions).
    #[serde(default)]
    pub cursors: Vec<f32>,
}

/// What a model pushes with `bb_set_panel`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PanelState {
    #[serde(default)]
    pub displays: BTreeMap<String, DisplayState>,
    /// LED id → brightness 0…1.
    #[serde(default)]
    pub leds: BTreeMap<String, f32>,
    #[serde(default)]
    pub screens: BTreeMap<String, ScreenState>,
    /// Current control values (rotary/switch index, knob value, latched button).
    #[serde(default)]
    pub controls: BTreeMap<String, f64>,
    /// Measurement rows (`Freq` / `490.2 Hz`).
    #[serde(default)]
    pub readouts: Vec<(String, String)>,
}

impl PanelState {
    /// Parses a state pushed by a model; `None` if it is not a panel state.
    pub fn from_json(v: &serde_json::Value) -> Option<PanelState> {
        serde_json::from_value(v.clone()).ok()
    }
    /// Whether drawing on screens is involved (needs `ui:panel`).
    pub fn draws(&self) -> bool {
        self.screens
            .values()
            .any(|s| !s.traces.is_empty() || !s.markers.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_round_trip_and_validation() {
        let p: Panel = serde_json::from_value(serde_json::json!({
            "size": [80, 160],
            "elements": [
                {"type": "display", "id": "lcd", "rect": [8, 8, 64, 30], "digits": 4,
                 "units": ["V", "mV"], "annunciators": ["AUTO", "HOLD"]},
                {"type": "button", "id": "hold", "rect": [8, 44, 18, 8], "label": "HOLD"},
                {"type": "rotary", "id": "dial", "rect": [20, 60, 40, 40],
                 "positions": [{"value": "off", "label": "OFF"}, {"value": "dc_v", "label": "V⎓"}]},
                {"type": "jack", "id": "j1", "rect": [10, 140, 8, 8], "pin": "VO", "label": "VΩ"}
            ]
        }))
        .unwrap();
        assert!(p.validate(&["VO".into(), "COM".into()]).is_ok());
        assert_eq!(p.position_index("dial", "dc_v"), Some(1));
        assert!(p.validate(&["X".into()]).is_err());
        let s: PanelState = serde_json::from_value(serde_json::json!({
            "displays": {"lcd": {"text": "1.882", "unit": "V", "annunciators": ["AUTO"]}},
            "controls": {"dial": 1}
        }))
        .unwrap();
        assert_eq!(s.displays["lcd"].text, "1.882");
        assert!(!s.draws());
    }
}
