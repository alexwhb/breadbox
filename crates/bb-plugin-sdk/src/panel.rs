//! Live panel state (`bb_set_panel`), mirroring `bb_plugin::panel::PanelState`.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct DisplayState {
    pub text: String,
    pub unit: String,
    pub annunciators: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bar: Option<f32>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Trace {
    pub channel: String,
    /// Points in divisions: x 0…grid.x, y −grid.y/2…grid.y/2.
    pub points: Vec<[f32; 2]>,
    pub digital: bool,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Marker {
    pub x: f32,
    pub y: f32,
    pub text: String,
    pub channel: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct ScreenState {
    pub traces: Vec<Trace>,
    pub markers: Vec<Marker>,
    pub header: String,
    pub cursors: Vec<f32>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct PanelState {
    pub displays: BTreeMap<String, DisplayState>,
    pub leds: BTreeMap<String, f32>,
    pub screens: BTreeMap<String, ScreenState>,
    pub controls: BTreeMap<String, f64>,
    pub readouts: Vec<(String, String)>,
}
