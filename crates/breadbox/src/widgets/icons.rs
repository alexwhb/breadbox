//! Vector icon set: stroke icons drawn from SVG path data (the exact paths of the design's
//! inline SVGs), rendered with egui shapes. No raster assets, crisp at any scale.
//!
//! Never add sparkle / magic-wand / four-point-star glyphs here (`PLAT-5`).

use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke, pos2};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Which colour a layer of an icon uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    /// The caller's colour (`currentColor`).
    Base,
    /// The amber accent (`#F2A33A`) — the "active part" of part icons.
    Accent,
    /// Cyan info colour.
    Info,
}

/// One path of an icon.
#[derive(Debug, Clone, Copy)]
pub struct Layer {
    pub d: &'static str,
    pub ink: Ink,
    /// Stroke width in view-box units; `0.0` = filled only.
    pub stroke: f32,
    pub fill: bool,
}

const fn s(d: &'static str, w: f32) -> Layer {
    Layer {
        d,
        ink: Ink::Base,
        stroke: w,
        fill: false,
    }
}
const fn a(d: &'static str, w: f32) -> Layer {
    Layer {
        d,
        ink: Ink::Accent,
        stroke: w,
        fill: false,
    }
}
const fn f(d: &'static str) -> Layer {
    Layer {
        d,
        ink: Ink::Base,
        stroke: 0.0,
        fill: true,
    }
}
const fn fi(d: &'static str, ink: Ink) -> Layer {
    Layer {
        d,
        ink,
        stroke: 0.0,
        fill: true,
    }
}
const fn si(d: &'static str, w: f32, ink: Ink) -> Layer {
    Layer {
        d,
        ink,
        stroke: w,
        fill: false,
    }
}

macro_rules! icons {
    ($( $(#[$m:meta])* $name:ident = $view:expr => [$($layer:expr),* $(,)?] ),* $(,)?) => {
        /// All icons. `Icon::paint` draws one into a rect.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Icon { $( $(#[$m])* $name ),* }

        impl Icon {
            pub const ALL: &'static [Icon] = &[$(Icon::$name),*];

            /// View-box size and layers.
            pub fn def(self) -> (f32, &'static [Layer]) {
                match self {
                    $( Icon::$name => { const L: &[Layer] = &[$($layer),*]; ($view, L) } ),*
                }
            }

            pub fn name(self) -> &'static str {
                match self { $( Icon::$name => stringify!($name) ),* }
            }
        }
    };
}

// Part icons: 24×24, base stroke 1.4 + accent stroke 1.6 (`.tile` svg in the design).
const PB: f32 = 1.4;
const PA: f32 = 1.6;
// Tool icons: 16×16, stroke 1.2 (right-rail `.rb` svg).
const TB: f32 = 1.2;
// UI glyphs: 16×16, stroke 1.5.
const UB: f32 = 1.5;

icons! {
    // ---------------------------------------------------------------- parts (design `I` table)
    Uno = 24.0 => [s("M3 6.5h18v11H3z M1.8 9.2H3 M1.8 12.2H3 M7 8.6h10.5 M9.5 15.4h8", PB), a("M9.5 11h5.5v2.6H9.5z", PA)],
    Esp32 = 24.0 => [s("M6 2.5h12v19H6z M6 9H4 M6 12.5H4 M6 16H4 M18 9h2 M18 12.5h2 M18 16h2 M9 11h6v6H9z", PB), a("M8.5 4.5h7 M8.5 4.5v2.5h2.3V4.5 M13 4.5V7h2.5", PA)],
    Pico = 24.0 => [s("M7.5 2.5h9v19h-9z M7.5 6H5.5 M7.5 9.5H5.5 M7.5 13H5.5 M7.5 16.5H5.5 M16.5 6h2 M16.5 9.5h2 M16.5 13h2 M16.5 16.5h2", PB), a("M10.2 10.5h3.6v3.6h-3.6z M10.5 3.6h3v1.8h-3z", PA)],
    Breadboard = 24.0 => [s("M2.5 5h19v14h-19z M2.5 12h19", PB), a("M6 8.2v.1 M9 8.2v.1 M12 8.2v.1 M15 8.2v.1 M18 8.2v.1 M6 15.7v.1 M9 15.7v.1 M12 15.7v.1 M15 15.7v.1 M18 15.7v.1", PA)],
    Wire = 24.0 => [s("M6 17.5C6 8 18 16 18 6.5", PB), a("M4.5 17.5h3v3h-3z M16.5 3.5h3v3h-3z", PA)],
    Resistor = 24.0 => [s("M2 12h4 M18 12h4 M6 9.4h12v5.2H6z", PB), a("M9 9.4v5.2 M11.8 9.4v5.2 M14.6 9.4v5.2", PA)],
    Capacitor = 24.0 => [s("M2 12h8 M14 12h8 M10 6v12", PB), a("M14 6v12", PA)],
    Button = 24.0 => [s("M4 15h16v5H4z M7 15v-2.6h10V15 M2 20h2 M20 20h2", PB), a("M12 10.5V4.5 M9.6 6.9 12 4.5l2.4 2.4", PA)],
    Pot = 24.0 => [s("M6 13a6 6 0 1 0 12 0a6 6 0 1 0-12 0 M8 21.5v-3 M12 21.5v-2.5 M16 21.5v-3", PB), a("M12 13l3.2-4.2", PA)],
    Battery = 24.0 => [s("M3.5 7h14v10h-14z M17.5 10h2.5v4h-2.5", PB), a("M7 12h3 M8.5 10.5v3 M12.5 12h2.5", PA)],
    Usb = 24.0 => [s("M8 2.5h8v6H8z M6.5 8.5h11v4.5a5.5 5.5 0 0 1-11 0z M12 18.5v3", PB), a("M10.2 5h.8 M13 5h.8", PA)],
    Led = 24.0 => [s("M9 21v-6.5 M15 21v-4.5 M7.5 14.5h9 M9 14.5V9a3 3 0 0 1 6 0v5.5", PB), a("M18.2 5.8l1.9-1.9 M19 9.5h2.6 M16 3.2l.6-1.7", PA)],
    RgbLed = 24.0 => [s("M8 21v-6.5 M11 21v-6.5 M14 21v-6.5 M17 21v-6.5 M6.5 14.5h12 M8 14.5V9a4.5 4.5 0 0 1 9 0v5.5", PB), a("M10.3 10v.1 M12.5 8.2v.1 M14.7 10v.1", PA)],
    Buzzer = 24.0 => [s("M5 12a7 7 0 1 0 14 0a7 7 0 1 0-14 0 M10.5 12a1.5 1.5 0 1 0 3 0a1.5 1.5 0 1 0-3 0", PB), a("M21 8a9 9 0 0 1 0 8 M3 8a9 9 0 0 0 0 8", PA)],
    Ldr = 24.0 => [s("M7 14a5 5 0 1 0 10 0a5 5 0 1 0-10 0 M9.4 12.6h1.8v2.8h1.8v-2.8h1.6 M10 19v2.5 M14 19v2.5", PB), a("M3 4l3.6 3.6 M6.8 2.2l2.4 4 M6.6 7.6l-.3-2 M9.2 6.2l-1.7-1", PA)],
    Soil = 24.0 => [s("M8 2.5h8v5.5H8z M9.5 8v10.5L11 21 M14.5 8v10.5L13 21", PB), a("M12 10.5c-1.4 1.7-2.1 2.8-2.1 3.7a2.1 2.1 0 0 0 4.2 0c0-.9-.7-2-2.1-3.7z", PA)],
    Dht = 24.0 => [s("M6 2.5h12v15H6z M9 17.5v4 M12 17.5v4 M15 17.5v4", PB), a("M9 6h6 M9 9h6 M9 12h6 M9 15h6", PA)],
    Sonar = 24.0 => [s("M2.5 8h19v10h-19z M4.8 13a2.7 2.7 0 1 0 5.4 0a2.7 2.7 0 1 0-5.4 0 M13.8 13a2.7 2.7 0 1 0 5.4 0a2.7 2.7 0 1 0-5.4 0", PB), a("M8.5 4.8a5 5 0 0 1 7 0 M10.4 6.4a2.4 2.4 0 0 1 3.2 0", PA)],
    Pir = 24.0 => [s("M5 14h14v6.5H5z M7 14a5 5 0 0 1 10 0", PB), a("M9.6 10.8l1.1 1 M14.4 10.8l-1.1 1 M12 9.2v1.4", PA)],
    Servo = 24.0 => [s("M3.5 9.5h13v10h-13z M1.5 12.5h2 M1.5 16.5h2", PB), a("M12 6.5h9 M11.8 6.5a2 2 0 1 0 4 0a2 2 0 1 0-4 0", PA)],
    Motor = 24.0 => [s("M3 7h13v10H3z M16 10h2v4h-2 M18 12h4", PB), a("M6 14V10l2.5 3 2.5-3v4", PA)],
    Relay = 24.0 => [s("M4 4.5h16v15H4z", PB), a("M7.5 15h3l4.5-5 M15 15h1.5 M7.5 9h1.5", PA)],
    Pump = 24.0 => [s("M5.5 7h9.5v10H5.5z M15 10h3v4h-3 M9 7V4h3.5 M3 17h12.5", PB), a("M20.2 11.6c-1 1.3-1.5 2.1-1.5 2.8a1.5 1.5 0 0 0 3 0c0-.7-.5-1.5-1.5-2.8z", PA)],
    Oled = 24.0 => [s("M3 4.5h18v12H3z M8 21h8 M12 16.5V21", PB), a("M6 8.5h7 M6 11.5h10 M6 14h4", PA)],
    Lcd = 24.0 => [s("M2 6h20v12H2z M4 8h16v8H4z", PB), a("M6 10.5h12 M6 13.5h8", PA)],
    Dip = 24.0 => [s("M6.5 3.5h11v17h-11z M6.5 6.5H4 M6.5 10H4 M6.5 13.5H4 M6.5 17H4 M17.5 6.5H20 M17.5 10H20 M17.5 13.5H20 M17.5 17H20", PB), a("M10.5 3.5a1.5 1.5 0 0 0 3 0", PA)],
    Diode = 24.0 => [s("M2 12h6 M16 12h6 M8 7l8 5-8 5z", PB), a("M16 7v10", PA)],
    Bjt = 24.0 => [s("M4 12a8 8 0 1 0 16 0a8 8 0 1 0-16 0 M2 12h7 M9 8v8 M9 10l5-3.6V3 M9 14l5 3.6V21", PB), a("M11.6 17.8l2.4-.2-.9-2.2", PA)],
    Mosfet = 24.0 => [s("M2 16.5h6 M8 7.5v9 M10.5 6v3 M10.5 10.5v3 M10.5 15v3 M10.5 7.5H16V3 M10.5 16.5H16V21 M10.5 12H16v4.5", PB), a("M12.2 12l2-1.3v2.6z", PA)],
    OpAmp = 24.0 => [s("M6 4v16l14-8z M2 9h4 M2 15h4 M20 12h2", PB), a("M8 9h2.2 M9.1 7.9v2.2 M8 15h2.2", PA)],
    Inductor = 24.0 => [s("M2 14h3 M19 14h3", PB), a("M5 14a1.75 1.75 0 0 1 3.5 0a1.75 1.75 0 0 1 3.5 0a1.75 1.75 0 0 1 3.5 0a1.75 1.75 0 0 1 3.5 0", PA)],
    Regulator = 24.0 => [s("M5 9h14v10.5H5z M8 19.5v2.5 M12 19.5v2.5 M16 19.5v2.5 M7 9V4h10v5", PB), a("M10.5 6.5h3", PA)],
    Source = 24.0 => [s("M4 12a8 8 0 1 0 16 0a8 8 0 1 0-16 0 M12 1v3 M12 20v3", PB), a("M10 8.4h4 M12 6.4v4 M10 15.8h4", PA)],
    Radio = 24.0 => [s("M5 9.5h14v11H5z M8 9.5V3", PB), a("M14.5 5.5a3.5 3.5 0 0 1 3.5 3.5 M14.5 2.5A6.5 6.5 0 0 1 21 9", PA)],
    Strip = 24.0 => [s("M2 9h20v6H2z", PB), a("M5.5 12a1 1 0 1 0 2 0a1 1 0 1 0-2 0 M11 12a1 1 0 1 0 2 0a1 1 0 1 0-2 0 M16.5 12a1 1 0 1 0 2 0a1 1 0 1 0-2 0", PA)],
    Stepper = 24.0 => [s("M5 12a7 7 0 1 0 14 0a7 7 0 1 0-14 0 M12 2v3", PB), a("M9.5 12a2.5 2.5 0 1 0 5 0a2.5 2.5 0 1 0-5 0", PA)],
    Imu = 24.0 => [s("M6 6h12v12H6z M9 6V3.5 M12 6V3.5 M15 6V3.5 M9 18v2.5 M12 18v2.5 M15 18v2.5", PB), a("M9.5 14.5h5 M9.5 14.5v-5", PA)],
    Crystal = 24.0 => [s("M2 12h5 M17 12h5 M7 7v10 M17 7v10", PB), a("M9.5 8h5v8h-5z", PA)],

    // ---------------------------------------------------------------- tools (right rail, 16×16)
    Multimeter = 16.0 => [s("M5 1.5h6a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-9a2 2 0 0 1 2-2z M5.5 3.5h5v3h-5z M6 10.5a2 2 0 1 0 4 0a2 2 0 1 0-4 0", TB)],
    Oscilloscope = 16.0 => [s("M3 3h10a1.5 1.5 0 0 1 1.5 1.5v7A1.5 1.5 0 0 1 13 13H3a1.5 1.5 0 0 1-1.5-1.5v-7A1.5 1.5 0 0 1 3 3z M3.5 10h2V6H8v4h2.5V6H12", TB)],
    LogicAnalyzer = 16.0 => [s("M1.5 6h3V3h3v3h3V3h4M1.5 13.5h2V10.5h4v3h3v-3h4", TB)],
    PowerSupply = 16.0 => [s("M3 3h10a1.5 1.5 0 0 1 1.5 1.5v7A1.5 1.5 0 0 1 13 13H3a1.5 1.5 0 0 1-1.5-1.5v-7A1.5 1.5 0 0 1 3 3z M4 10a1 1 0 1 0 2 0a1 1 0 1 0-2 0 M10 10a1 1 0 1 0 2 0a1 1 0 1 0-2 0 M4 6h3M5.5 4.5v3M9.5 6H12", TB)],
    FunctionGenerator = 16.0 => [s("M1.5 8c1.5-4 3-4 4.3 0s2.9 4 4.3 0 2.8-4 4.4 0", TB)],
    /// World panel: a potted plant (soil + sun).
    World = 16.0 => [s("M4 9.5h8l-1 5H5z M8 9.5V6 M8 6c0-2 1.5-3 3.5-3 0 2-1.5 3-3.5 3z M8 7.5C8 6 7 5 5 5c0 1.5 1 2.5 3 2.5z", TB)],
    /// Probes: a test probe with a tip.
    Probe = 16.0 => [s("M10.5 1.5l4 4-6 6-4-4z M4.5 7.5l-3 7 7-3 M12.5 3.5l-2 2", TB)],
    /// Inspector: magnifier over a part.
    Inspector = 16.0 => [s("M2 6.5a4.5 4.5 0 1 0 9 0a4.5 4.5 0 1 0-9 0 M9.8 9.8l4.7 4.7 M4.5 6.5h4", TB)],
    /// Waveforms panel.
    Waveform = 16.0 => [s("M1.5 8h2l1.5-4 2 8 2-8 2 8 1.5-4h2", TB)],

    // ---------------------------------------------------------------- UI glyphs (16×16)
    ExpandLeft = 16.0 => [s("M9.5 3.5L5 8l4.5 4.5M13 3.5v9", UB)],
    CollapseRight = 16.0 => [s("M6.5 3.5L11 8l-4.5 4.5M3 3.5v9", UB)],
    ChevronRight = 16.0 => [s("M6.5 3.5L11 8l-4.5 4.5", UB)],
    ChevronLeft = 16.0 => [s("M9.5 3.5L5 8l4.5 4.5", UB)],
    ChevronDown = 16.0 => [s("M3.5 6.5L8 11l4.5-4.5", UB)],
    Close = 16.0 => [s("M4 4l8 8M12 4l-8 8", UB)],
    Plus = 16.0 => [s("M8 3v10M3 8h10", UB)],
    Minus = 16.0 => [s("M3 8h10", UB)],
    Check = 16.0 => [s("M3 8.5l3.2 3L13 4.5", UB)],
    Search = 16.0 => [s("M2.5 7a4.5 4.5 0 1 0 9 0a4.5 4.5 0 1 0-9 0 M10.3 10.3l3.2 3.2", UB)],
    Folder = 16.0 => [s("M1.5 4.5a1 1 0 0 1 1-1h3.5l1.5 1.5h6a1 1 0 0 1 1 1v6.5a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1z", 1.3)],
    File = 16.0 => [s("M4 1.5h5.5l3 3v10H4z M9.5 1.5v3h3", 1.3)],
    Home = 16.0 => [s("M2 7.5L8 2.5l6 5 M3.5 6.5v7h9v-7 M6.5 13.5v-4h3v4", 1.3)],
    Gear = 16.0 => [s("M5.5 8a2.5 2.5 0 1 0 5 0a2.5 2.5 0 1 0-5 0 M8 1.5v2 M8 12.5v2 M1.5 8h2 M12.5 8h2 M3.4 3.4l1.4 1.4 M11.2 11.2l1.4 1.4 M3.4 12.6l1.4-1.4 M11.2 4.8l1.4-1.4", 1.3)],
    Download = 16.0 => [s("M8 2v8 M4.8 7L8 10.2 11.2 7 M2.5 12.5v1.5h11v-1.5", UB)],
    Upload = 16.0 => [s("M8 10.5v-8 M4.8 5.5L8 2.3l3.2 3.2 M2.5 12.5v1.5h11v-1.5", UB)],
    Refresh = 16.0 => [s("M13 8a5 5 0 1 1-1.5-3.6 M13 2.5v3h-3", UB)],
    Book = 16.0 => [s("M2 3c2-1 4-1 6 .5V14c-2-1.5-4-1.5-6-.5z M14 3c-2-1-4-1-6 .5V14c2-1.5 4-1.5 6-.5z", 1.3)],
    /// Plugins: a plug.
    Plug = 16.0 => [s("M6 1.5v3 M10 1.5v3 M4 4.5h8v3a4 4 0 0 1-8 0z M8 11.5v3", 1.3)],
    Chip = 16.0 => [s("M4 4h8v8H4z M6 4V2 M10 4V2 M6 14v-2 M10 14v-2 M4 6H2 M4 10H2 M14 6h-2 M14 10h-2", 1.3)],
    /// Pro / analysis: a Bode-like curve over axes.
    Analysis = 16.0 => [s("M2 2v12h12 M3.5 5h4c2 0 3 2 4 4.5s1.5 3 2.5 3", 1.3)],
    Terminal = 16.0 => [s("M2 3h12v10H2z M4.5 6.5l2 1.5-2 1.5 M8 10h3.5", 1.3)],
    Library = 16.0 => [s("M2.5 2.5h2.5v11H2.5z M6.5 2.5h2.5v11H6.5z M10.5 3.2l2.3-.6 2 10.6-2.3.6z", 1.3)],
    Wrench = 16.0 => [s("M10.5 2a3.5 3.5 0 0 0-3.3 4.6L2.5 11.3a1.4 1.4 0 0 0 2 2l4.7-4.7A3.5 3.5 0 0 0 13.9 5.5l-2 2-1.9-.5-.5-1.9 2-2A3.5 3.5 0 0 0 10.5 2z", 1.2)],
    Warning = 16.0 => [s("M8 2l6.5 11.5h-13z M8 6.5v3.5 M8 12v.2", 1.3)],
    Info = 16.0 => [s("M1.5 8a6.5 6.5 0 1 0 13 0a6.5 6.5 0 1 0-13 0 M8 7.2v4 M8 4.8v.2", 1.3)],
    /// Help: a question mark in a circle (the top bar's "?").
    Help = 16.0 => [s("M1.5 8a6.5 6.5 0 1 0 13 0a6.5 6.5 0 1 0-13 0 M6.1 6.3a1.9 1.9 0 1 1 2.65 1.75c-.5.22-.75.6-.75 1.1v.45 M8 11.5v.2", 1.3)],
    Lightbulb = 16.0 => [s("M5.5 11.5h5 M6 14h4 M5.6 10a4.5 4.5 0 1 1 4.8 0 V11.5H5.6z", 1.3)],
    Speech = 16.0 => [s("M2 3.5h12v7.5H7l-3 2.5v-2.5H2z", 1.3)],
    Bug = 16.0 => [s("M5 6.5a3 3 0 0 1 6 0v3a3 3 0 0 1-6 0z M8 6.5v6 M2.5 8H5 M11 8h2.5 M3 4.5l2 1.5 M13 4.5l-2 1.5 M3 12.5l2-1 M13 12.5l-2-1 M6.5 3.5L5.5 2 M9.5 3.5l1-1.5", 1.2)],
    Breakpoint = 16.0 => [fi("M3 8a5 5 0 1 0 10 0a5 5 0 1 0-10 0z", Ink::Base)],
    StepOver = 16.0 => [s("M2.5 9.5a5.5 5.5 0 0 1 10.5-2.5 M13.2 3.8v3.4H9.8", UB), f("M6.5 13a1.5 1.5 0 1 0 3 0a1.5 1.5 0 1 0-3 0z")],
    StepInto = 16.0 => [s("M8 1.5v7.5 M4.8 6L8 9.2 11.2 6", UB), f("M6.5 13a1.5 1.5 0 1 0 3 0a1.5 1.5 0 1 0-3 0z")],
    StepOut = 16.0 => [s("M8 9.5V2 M4.8 5.2L8 2l3.2 3.2", UB), f("M6.5 13a1.5 1.5 0 1 0 3 0a1.5 1.5 0 1 0-3 0z")],
    Continue = 16.0 => [s("M3 2.5v11", UB), f("M6 2.5l8 5.5-8 5.5z")],
    Stop = 16.0 => [f("M3.5 3.5h9v9h-9z")],
    Restart = 16.0 => [s("M3 8a5 5 0 1 0 1.5-3.6 M3 2.5v3h3", UB)],
    Export = 16.0 => [s("M9.5 1.5h5v5 M14.5 1.5L8 8 M12.5 9.5v4.5h-11v-11h4.5", 1.3)],
    Image = 16.0 => [s("M1.5 3h13v10h-13z M1.5 11l4-4 3 3 2-2 4 4", 1.3), f("M10 5.8a1 1 0 1 0 2 0a1 1 0 1 0-2 0z")],
    Table = 16.0 => [s("M1.5 3h13v10h-13z M1.5 6.5h13 M1.5 10h13 M6 3v10", 1.3)],
    Eye = 16.0 => [s("M1.5 8C3 5 5.3 3.5 8 3.5S13 5 14.5 8C13 11 10.7 12.5 8 12.5S3 11 1.5 8z M6 8a2 2 0 1 0 4 0a2 2 0 1 0-4 0", 1.3)],
    Clock = 16.0 => [s("M1.5 8a6.5 6.5 0 1 0 13 0a6.5 6.5 0 1 0-13 0 M8 4.5V8l2.5 1.5", 1.3)],
    Trash = 16.0 => [s("M2.5 4h11 M6 4V2.5h4V4 M3.8 4l.7 10h7l.7-10", 1.3)],
    Dots = 16.0 => [f("M2.5 8a1.2 1.2 0 1 0 2.4 0a1.2 1.2 0 1 0-2.4 0z M6.8 8a1.2 1.2 0 1 0 2.4 0a1.2 1.2 0 1 0-2.4 0z M11.1 8a1.2 1.2 0 1 0 2.4 0a1.2 1.2 0 1 0-2.4 0z")],

    // ---------------------------------------------------------------- header + cards
    /// `Breadboard` segmented icon (14×14).
    SegBreadboard = 14.0 => [s("M2.5 2h9a1.5 1.5 0 0 1 1.5 1.5v7a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 1 10.5v-7A1.5 1.5 0 0 1 2.5 2z M3.5 5h1M6.5 5h1M9.5 5h1M3.5 9h1M6.5 9h1M9.5 9h1", 1.3)],
    /// `Schematic` segmented icon (14×14).
    SegSchematic = 14.0 => [s("M1 7h3l1-3 2 6 2-6 1 3h3", 1.3)],
    /// Run (12×12, filled).
    Play = 12.0 => [f("M3 1.5l7 4.5-7 4.5z")],
    /// Pause (12×12, filled).
    Pause = 12.0 => [f("M2.6 1.5h1.4a.6.6 0 0 1 .6.6v7.8a.6.6 0 0 1-.6.6H2.6a.6.6 0 0 1-.6-.6V2.1a.6.6 0 0 1 .6-.6z M8 1.5h1.4a.6.6 0 0 1 .6.6v7.8a.6.6 0 0 1-.6.6H8a.6.6 0 0 1-.6-.6V2.1a.6.6 0 0 1 .6-.6z")],
    /// Wi-Fi radio card (18×18).
    Wifi = 18.0 => [si("M2 7a10 10 0 0 1 14 0M4.5 9.6a6.5 6.5 0 0 1 9 0M7 12.2a3 3 0 0 1 4 0", 1.6, Ink::Info), fi("M7.8 14.6a1.2 1.2 0 1 0 2.4 0a1.2 1.2 0 1 0-2.4 0z", Ink::Info)],
    /// Bluetooth card (18×18).
    Bluetooth = 18.0 => [s("M5 5l8 7-4 3V3l4 3-8 7", 1.6)],
    /// LoRa card (18×18): antenna with waves.
    Lora = 18.0 => [s("M9 16V7 M6 16h6 M5.5 4.5a5 5 0 0 0 0 5 M12.5 4.5a5 5 0 0 1 0 5 M3 2.5a8.5 8.5 0 0 0 0 9 M15 2.5a8.5 8.5 0 0 1 0 9", 1.5)],
    /// Phone (virtual BLE central).
    Phone = 16.0 => [s("M4.5 1.5h7v13h-7z M7 12.5h2", 1.3)],
}

/// Parsed, flattened subpath: points + closed flag.
type Sub = (Vec<Pos2>, bool);

type PathCache = Mutex<HashMap<&'static str, std::sync::Arc<Vec<Sub>>>>;

fn cache() -> &'static PathCache {
    static C: OnceLock<PathCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn parsed(d: &'static str) -> std::sync::Arc<Vec<Sub>> {
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    c.entry(d)
        .or_insert_with(|| std::sync::Arc::new(parse_path(d)))
        .clone()
}

impl Icon {
    /// Paints the icon into `rect` (scaled from its view box) in `color`; accent layers use
    /// `accent`.
    pub fn paint_with(self, painter: &egui::Painter, rect: Rect, color: Color32, accent: Color32) {
        let (view, layers) = self.def();
        let scale = rect.width().min(rect.height()) / view;
        let off = rect.center() - egui::vec2(view, view) * scale * 0.5;
        let info = crate::theme::DARK.info;
        for l in layers {
            let ink = match l.ink {
                Ink::Base => color,
                Ink::Accent => accent,
                Ink::Info => {
                    if color == Color32::TRANSPARENT {
                        info
                    } else {
                        info.gamma_multiply(color.a() as f32 / 255.0)
                    }
                }
            };
            for (pts, closed) in parsed(l.d).iter() {
                let pts: Vec<Pos2> = pts
                    .iter()
                    .map(|p| pos2(off.x + p.x * scale, off.y + p.y * scale))
                    .collect();
                if pts.len() < 2 {
                    continue;
                }
                if l.fill {
                    painter.add(Shape::convex_polygon(pts.clone(), ink, Stroke::NONE));
                }
                if l.stroke > 0.0 {
                    let w = (l.stroke * scale).max(1.0);
                    // Tiny "dot" segments (`v.1`) become round dots.
                    let len: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
                    if len < w * 0.6 {
                        painter.circle_filled(pts[0], w * 0.5, ink);
                    } else if *closed {
                        painter.add(Shape::closed_line(pts, Stroke::new(w, ink)));
                    } else {
                        painter.add(Shape::line(pts, Stroke::new(w, ink)));
                    }
                }
            }
        }
    }

    /// Paints with the default amber accent.
    pub fn paint(self, painter: &egui::Painter, rect: Rect, color: Color32) {
        self.paint_with(painter, rect, color, crate::theme::DARK.accent);
    }

    /// Allocates `size`×`size` and paints the icon.
    pub fn show(self, ui: &mut egui::Ui, size: f32, color: Color32) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        if ui.is_rect_visible(rect) {
            self.paint(ui.painter(), rect, color);
        }
        resp
    }
}

// --------------------------------------------------------------------------------------------
// SVG path parsing (M L H V C S Q T A Z, absolute + relative), flattened to polylines.

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl Lexer<'_> {
    fn skip(&mut self) {
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',')
        {
            self.i += 1;
        }
    }
    fn peek_cmd(&mut self) -> Option<u8> {
        self.skip();
        let c = *self.s.get(self.i)?;
        c.is_ascii_alphabetic().then_some(c)
    }
    fn at_number(&mut self) -> bool {
        self.skip();
        matches!(self.s.get(self.i), Some(c) if c.is_ascii_digit() || *c == b'-' || *c == b'+' || *c == b'.')
    }
    fn number(&mut self) -> Option<f32> {
        self.skip();
        let start = self.i;
        let mut seen_dot = false;
        let mut seen_e = false;
        if matches!(self.s.get(self.i), Some(b'-' | b'+')) {
            self.i += 1;
        }
        while let Some(&c) = self.s.get(self.i) {
            if c.is_ascii_digit() {
                self.i += 1;
            } else if c == b'.' && !seen_dot && !seen_e {
                seen_dot = true;
                self.i += 1;
            } else if (c == b'e' || c == b'E') && !seen_e {
                seen_e = true;
                self.i += 1;
                if matches!(self.s.get(self.i), Some(b'-' | b'+')) {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
        std::str::from_utf8(&self.s[start..self.i])
            .ok()?
            .parse()
            .ok()
    }
    /// Arc flags may be written without separators (`a6 6 0 1 0 12 0` or `a1 1 0 10 2 0`).
    fn flag(&mut self) -> Option<bool> {
        self.skip();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

const CURVE_SEGMENTS: usize = 12;

/// Parses SVG path data into flattened subpaths.
pub fn parse_path(d: &str) -> Vec<(Vec<Pos2>, bool)> {
    let mut out: Vec<(Vec<Pos2>, bool)> = Vec::new();
    let mut lx = Lexer {
        s: d.as_bytes(),
        i: 0,
    };
    let mut cur = pos2(0.0, 0.0);
    let mut start = cur;
    let mut sub: Vec<Pos2> = Vec::new();
    let mut last_ctrl: Option<Pos2> = None; // for S / T
    let mut last_cmd = b'M';
    let flush = |sub: &mut Vec<Pos2>, out: &mut Vec<(Vec<Pos2>, bool)>, closed: bool| {
        if sub.len() > 1 {
            out.push((std::mem::take(sub), closed));
        } else {
            sub.clear();
        }
    };
    loop {
        let cmd = match lx.peek_cmd() {
            Some(c) => {
                lx.i += 1;
                c
            }
            None => {
                if lx.at_number() {
                    // implicit repeat; after M it is L
                    match last_cmd {
                        b'M' => b'L',
                        b'm' => b'l',
                        c => c,
                    }
                } else {
                    break;
                }
            }
        };
        let rel = cmd.is_ascii_lowercase();
        let base = |cur: Pos2| if rel { cur } else { pos2(0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let (Some(x), Some(y)) = (lx.number(), lx.number()) else {
                    break;
                };
                flush(&mut sub, &mut out, false);
                let b = base(cur);
                cur = pos2(b.x + x, b.y + y);
                start = cur;
                sub.push(cur);
                last_ctrl = None;
            }
            b'L' => {
                let (Some(x), Some(y)) = (lx.number(), lx.number()) else {
                    break;
                };
                let b = base(cur);
                cur = pos2(b.x + x, b.y + y);
                if sub.is_empty() {
                    sub.push(start);
                }
                sub.push(cur);
                last_ctrl = None;
            }
            b'H' => {
                let Some(x) = lx.number() else { break };
                cur = pos2(if rel { cur.x + x } else { x }, cur.y);
                if sub.is_empty() {
                    sub.push(start);
                }
                sub.push(cur);
                last_ctrl = None;
            }
            b'V' => {
                let Some(y) = lx.number() else { break };
                cur = pos2(cur.x, if rel { cur.y + y } else { y });
                if sub.is_empty() {
                    sub.push(start);
                }
                sub.push(cur);
                last_ctrl = None;
            }
            b'C' | b'S' => {
                let b = base(cur);
                let c1 = if cmd.eq_ignore_ascii_case(&b'C') {
                    let (Some(x), Some(y)) = (lx.number(), lx.number()) else {
                        break;
                    };
                    pos2(b.x + x, b.y + y)
                } else {
                    match last_ctrl {
                        Some(lc) => pos2(2.0 * cur.x - lc.x, 2.0 * cur.y - lc.y),
                        None => cur,
                    }
                };
                let (Some(x2), Some(y2), Some(x), Some(y)) =
                    (lx.number(), lx.number(), lx.number(), lx.number())
                else {
                    break;
                };
                let c2 = pos2(b.x + x2, b.y + y2);
                let end = pos2(b.x + x, b.y + y);
                if sub.is_empty() {
                    sub.push(cur);
                }
                for k in 1..=CURVE_SEGMENTS {
                    let t = k as f32 / CURVE_SEGMENTS as f32;
                    let mt = 1.0 - t;
                    let p = cur.to_vec2() * (mt * mt * mt)
                        + c1.to_vec2() * (3.0 * mt * mt * t)
                        + c2.to_vec2() * (3.0 * mt * t * t)
                        + end.to_vec2() * (t * t * t);
                    sub.push(p.to_pos2());
                }
                last_ctrl = Some(c2);
                cur = end;
            }
            b'Q' | b'T' => {
                let b = base(cur);
                let c1 = if cmd.eq_ignore_ascii_case(&b'Q') {
                    let (Some(x), Some(y)) = (lx.number(), lx.number()) else {
                        break;
                    };
                    pos2(b.x + x, b.y + y)
                } else {
                    match last_ctrl {
                        Some(lc) => pos2(2.0 * cur.x - lc.x, 2.0 * cur.y - lc.y),
                        None => cur,
                    }
                };
                let (Some(x), Some(y)) = (lx.number(), lx.number()) else {
                    break;
                };
                let end = pos2(b.x + x, b.y + y);
                if sub.is_empty() {
                    sub.push(cur);
                }
                for k in 1..=CURVE_SEGMENTS {
                    let t = k as f32 / CURVE_SEGMENTS as f32;
                    let mt = 1.0 - t;
                    let p = cur.to_vec2() * (mt * mt)
                        + c1.to_vec2() * (2.0 * mt * t)
                        + end.to_vec2() * (t * t);
                    sub.push(p.to_pos2());
                }
                last_ctrl = Some(c1);
                cur = end;
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(rot)) = (lx.number(), lx.number(), lx.number())
                else {
                    break;
                };
                let (Some(large), Some(sweep)) = (lx.flag(), lx.flag()) else {
                    break;
                };
                let (Some(x), Some(y)) = (lx.number(), lx.number()) else {
                    break;
                };
                let b = base(cur);
                let end = pos2(b.x + x, b.y + y);
                if sub.is_empty() {
                    sub.push(cur);
                }
                arc_points(cur, end, rx, ry, rot, large, sweep, &mut sub);
                cur = end;
                last_ctrl = None;
            }
            b'Z' => {
                if !sub.is_empty() {
                    sub.push(start);
                }
                flush(&mut sub, &mut out, true);
                cur = start;
                last_ctrl = None;
            }
            _ => break,
        }
        last_cmd = cmd;
    }
    flush(&mut sub, &mut out, false);
    out
}

/// Endpoint → centre parameterisation (SVG 1.1 implementation notes F.6.5).
#[allow(clippy::too_many_arguments)]
fn arc_points(
    p0: Pos2,
    p1: Pos2,
    rx: f32,
    ry: f32,
    rot_deg: f32,
    large: bool,
    sweep: bool,
    out: &mut Vec<Pos2>,
) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-6 || ry < 1e-6 || p0.distance(p1) < 1e-6 {
        out.push(p1);
        return;
    }
    let phi = rot_deg.to_radians();
    let (sin, cos) = phi.sin_cos();
    let dx = (p0.x - p1.x) / 2.0;
    let dy = (p0.y - p1.y) / 2.0;
    let x1 = cos * dx + sin * dy;
    let y1 = -sin * dx + cos * dy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = (num / den).max(0.0).sqrt();
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1 / ry;
    let cyp = -coef * ry * x1 / rx;
    let cx = cos * cxp - sin * cyp + (p0.x + p1.x) / 2.0;
    let cy = sin * cxp + cos * cyp + (p0.y + p1.y) / 2.0;
    let ang = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 { -a } else { a }
    };
    let th1 = ang(1.0, 0.0, (x1 - cxp) / rx, (y1 - cyp) / ry);
    let mut dth = ang(
        (x1 - cxp) / rx,
        (y1 - cyp) / ry,
        (-x1 - cxp) / rx,
        (-y1 - cyp) / ry,
    );
    let tau = std::f32::consts::TAU;
    if !sweep && dth > 0.0 {
        dth -= tau;
    } else if sweep && dth < 0.0 {
        dth += tau;
    }
    let n = ((dth.abs() / tau) * 32.0).ceil().max(4.0) as usize;
    for k in 1..=n {
        let t = th1 + dth * (k as f32 / n as f32);
        let (st, ct) = t.sin_cos();
        let x = cos * rx * ct - sin * ry * st + cx;
        let y = sin * rx * ct + cos * ry * st + cy;
        out.push(pos2(x, y));
    }
    if let Some(last) = out.last_mut() {
        *last = p1;
    }
}

/// The 26×26 logo mark from the design header (rounded square, three amber dots, cyan wire).
pub fn paint_logo(painter: &egui::Painter, rect: Rect) {
    let p = &crate::theme::DARK;
    let k = rect.width() / 26.0;
    let at = |x: f32, y: f32| pos2(rect.left() + x * k, rect.top() + y * k);
    let sq = Rect::from_min_max(at(1.0, 1.0), at(25.0, 25.0));
    painter.rect(
        sq,
        6.0 * k,
        p.bg_raised,
        Stroke::new(1.5 * k, p.accent),
        egui::StrokeKind::Middle,
    );
    for x in [8.0, 13.0, 18.0] {
        painter.circle_filled(at(x, 9.0), 1.6 * k, p.accent);
    }
    // M8 9 C8 18 18 12 18 18
    let mut pts = Vec::new();
    let (p0, c1, c2, p3) = (at(8.0, 9.0), at(8.0, 18.0), at(18.0, 12.0), at(18.0, 18.0));
    for i in 0..=20 {
        let t = i as f32 / 20.0;
        let mt = 1.0 - t;
        let v = p0.to_vec2() * (mt * mt * mt)
            + c1.to_vec2() * (3.0 * mt * mt * t)
            + c2.to_vec2() * (3.0 * mt * t * t)
            + p3.to_vec2() * (t * t * t);
        pts.push(v.to_pos2());
    }
    painter.add(Shape::line(pts, Stroke::new(1.8 * k, p.info)));
    painter.circle_filled(at(18.0, 18.0), 1.6 * k, p.info);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rect_path_and_arcs() {
        let r = parse_path("M3 6.5h18v11H3z");
        assert_eq!(r.len(), 1);
        assert!(r[0].1);
        assert_eq!(r[0].0.first(), Some(&pos2(3.0, 6.5)));
        assert_eq!(r[0].0[1], pos2(21.0, 6.5));
        // full circle drawn as two arcs returns to its start
        let c = parse_path("M6 13a6 6 0 1 0 12 0a6 6 0 1 0-12 0");
        let pts = &c[0].0;
        assert!((pts.last().unwrap().x - 6.0).abs() < 1e-3);
        let max_y = pts.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        assert!((max_y - 19.0).abs() < 0.1, "{max_y}");
    }

    #[test]
    fn compact_arc_flags_and_implicit_commands() {
        // "a1 1 0 1 0 2 0a1 1 0 1 0-2 0" and implicit lineto after M
        let c = parse_path("M5.5 12a1 1 0 1 0 2 0a1 1 0 1 0-2 0 M1 1 2 2 3 3");
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].0.len(), 3);
    }

    #[test]
    fn every_icon_parses_to_something() {
        for icon in Icon::ALL {
            let (_, layers) = icon.def();
            for l in layers {
                assert!(
                    !parse_path(l.d).is_empty(),
                    "{} has an empty layer",
                    icon.name()
                );
            }
        }
    }

    #[test]
    fn no_sparkle_icons() {
        for icon in Icon::ALL {
            let n = icon.name().to_ascii_lowercase();
            assert!(!n.contains("sparkle") && !n.contains("magic") && !n.contains("wand"));
        }
    }
}
