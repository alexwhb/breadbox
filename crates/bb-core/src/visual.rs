//! What the UI draws for a simulated part ([`VisualState`]): LED glow, displays, motor
//! positions, … plus health (OK / stressed / damaged) and the magic-smoke level.

use serde::{Deserialize, Serialize};

/// 8-bit sRGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }
    /// Parses `#RRGGBB`.
    pub fn hex(s: &str) -> Option<Rgb> {
        let s = s.strip_prefix('#')?;
        if s.len() != 6 {
            return None;
        }
        let p = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
        Some(Rgb::new(p(0)?, p(2)?, p(4)?))
    }
}

/// Part health shown in the inspector and on the bench.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum Health {
    #[default]
    Ok,
    /// Operating outside its ratings but not (yet) destroyed.
    Stressed { reason: String },
    /// Destroyed; stays so until replaced or "Repair all".
    Damaged { reason: String },
}

impl Health {
    pub fn is_damaged(&self) -> bool {
        matches!(self, Health::Damaged { .. })
    }
    pub fn label(&self) -> &'static str {
        match self {
            Health::Ok => "OK",
            Health::Stressed { .. } => "stressed",
            Health::Damaged { .. } => "damaged",
        }
    }
}

/// Appearance of a part (one variant per kind of visual).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum Look {
    #[default]
    None,
    /// Single LED: `brightness` 0…1 is proportional to the average forward current
    /// (1 = rated current); `current` is that average in amps.
    Led { color: Rgb, brightness: f32, current: f64 },
    /// RGB LED: per-channel brightness 0…1.
    RgbLed { r: f32, g: f32, b: f32 },
    /// Several lights in a row (LED bar graph, WS2812 strip): colour already scaled by
    /// brightness.
    LedArray { leds: Vec<Rgb> },
    /// 7-segment digits: bit 0 = segment a … bit 6 = g, bit 7 = decimal point.
    SevenSeg { digits: Vec<u8>, brightness: f32 },
    /// DC motor / vibration motor.
    Motor { rpm: f64, angle_deg: f64 },
    /// Hobby servo horn angle (0…180°) and where it is heading.
    Servo { angle_deg: f64, target_deg: f64 },
    /// Stepper rotor position.
    Stepper { steps: i64, angle_deg: f64 },
    /// Character LCD (HD44780): visible text per row plus state.
    TextLcd {
        cols: u8,
        rows: u8,
        text: Vec<String>,
        /// Raw DDRAM codes per row (custom characters are codes 0–7).
        codes: Vec<Vec<u8>>,
        /// CGRAM: 8 custom characters × 8 rows of 5 bits.
        cgram: Vec<[u8; 8]>,
        cursor: Option<(u8, u8)>,
        blink: bool,
        display_on: bool,
        backlight: bool,
    },
    /// Monochrome pixel display (SSD1306): `data` is one byte per pixel (0 or 1), row-major.
    Pixels {
        width: u16,
        height: u16,
        data: Vec<u8>,
        on: bool,
        inverted: bool,
        contrast: u8,
    },
    Relay { energized: bool },
    /// Buzzer / speaker: dominant frequency of the drive waveform and loudness 0…1.
    Buzzer { freq_hz: f64, level: f32 },
    /// Push button, slide switch, tilt switch, reed switch.
    Switch { on: bool },
    /// Several switches (DIP switch).
    Switches { on: Vec<bool> },
    /// Potentiometer / trimmer knob position 0…1.
    Knob { position: f64 },
    /// Board status LEDs (power, L on D13, TX, RX).
    Board { power: bool, l: f32, tx: bool, rx: bool },
    /// Generic numeric readout (sensors: "21.0 °C").
    Readout { text: String },
}

/// Everything the UI needs to draw one part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VisualState {
    pub look: Look,
    pub health: Health,
    /// Magic-smoke intensity 0…1 (decays after damage).
    pub smoke: f32,
}

impl VisualState {
    pub fn new(look: Look) -> VisualState {
        VisualState {
            look,
            ..Default::default()
        }
    }
}

impl From<Look> for VisualState {
    fn from(look: Look) -> Self {
        VisualState::new(look)
    }
}
