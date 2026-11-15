//! Procedural 2.5D "hobbyist" art for every bench art id in the parts library
//! (`PartDef::bench.art`). Parts are drawn in their local coordinates (leads at their layout
//! positions, 0.1" units) through a [`Pen`]; parts that stand up off the board (LEDs, sensors,
//! capacitors, TO-92s) draw their body above their leads in screen space, whatever their
//! rotation, which is what gives the bench its depth.
//!
//! Live state comes from the simulator's `VisualState` (LED bloom ∝ current, displays, motor
//! and servo angles, knob positions, damage + magic smoke).

mod boards;
mod breadboard;
mod modules;
mod passives;

pub use breadboard::breadboard_holes_visible;
pub use passives::ohms_label;
pub use passives::pot_knob as passives_pot_knob;

use super::pen::{Pen, WRect, darken, lighten, rgb, with_alpha};
use crate::theme::font;
use bb_circuit::project::{PartInstance, Project};
use bb_circuit::{PartDef, PropValue};
use bb_core::{Health, Look, VisualState};
use eframe::egui::{Color32, FontId};

/// Everything an art function may use.
pub struct ArtCtx<'a> {
    pub def: &'a PartDef,
    pub part: &'a PartInstance,
    /// World positions of the leads (`pin`, x, y).
    pub leads: &'a [(String, f64, f64)],
    pub vis: Option<&'a VisualState>,
    pub readings: &'a [(String, f64)],
    /// Wall-clock seconds (animations).
    pub t: f64,
    /// Extra text for instruments (`DC 1.882 V`, scope label) and their traces.
    pub instrument: Option<&'a InstrumentView>,
    /// Soil moisture of an attached pot (% VWC) for pot art.
    pub soil_vwc: Option<f64>,
    /// Ambient light 0…1 (pot plant colour, LDR face).
    pub ambient: f32,
    /// Pins of this part with a wire or probe attached (header labels).
    pub wired: &'a [String],
    /// A lead sits in a breadboard hole (labels then print dark on the cream board).
    pub on_board: bool,
}

/// Live data an instrument art shows (mini scope trace, meter display).
#[derive(Debug, Clone, Default)]
pub struct InstrumentView {
    /// Display text (`1.882`) and unit (`V`).
    pub display: String,
    pub unit: String,
    pub mode: String,
    /// Scope traces: per channel (t from left edge 0…1, volts / div units).
    pub traces: Vec<Vec<(f32, f32)>>,
    pub caption: String,
    pub triggered: bool,
    /// A plugin instrument's live front panel (`bb_plugin::panel::PanelState` JSON).
    pub panel: Option<serde_json::Value>,
}

impl ArtCtx<'_> {
    pub fn prop_text(&self, key: &str) -> String {
        Project::prop(self.def, self.part, key)
            .map(|v| v.as_text())
            .unwrap_or_default()
    }
    pub fn prop_f64(&self, key: &str) -> Option<f64> {
        Project::prop_f64(self.def, self.part, key)
    }
    pub fn prop_bool(&self, key: &str) -> bool {
        matches!(
            Project::prop(self.def, self.part, key),
            Some(PropValue::Bool(true))
        ) || self.prop_f64(key).is_some_and(|v| v != 0.0)
    }
    pub fn look(&self) -> Option<&Look> {
        self.vis.map(|v| &v.look)
    }
    pub fn damaged(&self) -> bool {
        self.vis.is_some_and(|v| v.health.is_damaged())
    }
    pub fn stressed(&self) -> bool {
        self.vis
            .is_some_and(|v| matches!(v.health, Health::Stressed { .. }))
    }
    /// World position of a lead.
    pub fn lead(&self, pin: &str) -> Option<(f64, f64)> {
        self.leads
            .iter()
            .find(|(p, _, _)| p == pin)
            .map(|(_, x, y)| (*x, *y))
    }
    /// World positions of the first and last lead.
    pub fn ends(&self) -> Option<((f64, f64), (f64, f64))> {
        let a = self.leads.first()?;
        let b = self.leads.last()?;
        Some(((a.1, a.2), (b.1, b.2)))
    }
    /// World centre of all leads.
    pub fn lead_center(&self) -> (f64, f64) {
        if self.leads.is_empty() {
            return (0.0, 0.0);
        }
        let n = self.leads.len() as f64;
        let (sx, sy) = self
            .leads
            .iter()
            .fold((0.0, 0.0), |(a, b), (_, x, y)| (a + x, b + y));
        (sx / n, sy / n)
    }
    pub fn reading(&self, key: &str) -> Option<f64> {
        self.readings
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| *v)
    }
}

/// Draw passes: world objects' backs, parts, world objects' fronts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    Back,
    Body,
    Front,
}

/// Which pass a part draws in (pots draw a back and a front around the parts in them).
pub fn passes(art: &str) -> &'static [Pass] {
    match art {
        "pot_soil" => &[Pass::Back, Pass::Front],
        _ => &[Pass::Body],
    }
}

/// Stacking layer: breadboards, boards, parts on the board, off-board things, instruments.
pub fn layer(def: &PartDef) -> u8 {
    if def.is_breadboard() {
        0
    } else if def.is_mcu() {
        1
    } else if def.category == bb_circuit::Category::Instruments {
        4
    } else if def.bench.off_board {
        3
    } else {
        2
    }
}

/// Visual bounds in local coordinates (hit testing + selection) — the body, not just the
/// leads. Standing parts extend above their leads.
pub fn local_bounds(def: &PartDef) -> WRect {
    let (w, h) = def.bench.size;
    let leads = &def.bench.leads;
    let (mut lx0, mut ly0, mut lx1, mut ly1) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for l in leads {
        lx0 = lx0.min(l.x);
        ly0 = ly0.min(l.y);
        lx1 = lx1.max(l.x);
        ly1 = ly1.max(l.y);
    }
    match def.bench.art.as_str() {
        "breadboard" => WRect::new(0.0, 0.0, w, h),
        "uno_r3" => WRect::new(-1.6, 0.0, 27.0, 21.0),
        "mega_2560" => WRect::new(-1.6, 0.0, 40.0, 21.0),
        "nano" => WRect::new(-2.0, -0.8, 15.0, 6.8),
        "esp32_devkit" => WRect::new(-1.0, -1.0, 18.8, 11.0),
        "pico" => WRect::new(-1.8, -0.7, 19.7, 7.7),
        "esp01" => WRect::new(-0.8, -0.8, 9.6, 2.0 + 2.0),
        "hm10" | "hc05" => WRect::new(-0.7, -h + 0.4, lx1 + 0.7, 0.6),
        "resistor_axial" | "diode_axial" | "inductor_radial" | "soil_fork" | "speaker" => {
            WRect::new(-0.5, -0.9, lx1 + 0.5, 0.9)
        }
        "led_5mm" => WRect::new(-0.7, -4.6, lx1 + 0.7, 0.5),
        "rgb_led" => WRect::new(-0.5, -4.8, lx1 + 0.5, 0.5),
        "cap_ceramic" => WRect::new(-0.5, -3.2, lx1 + 0.5, 0.5),
        "cap_electrolytic" => WRect::new(-0.9, -4.6, lx1 + 0.9, 0.5),
        "ldr" | "thermistor" => WRect::new(-0.6, -3.6, lx1 + 0.6, 0.5),
        "to92" => WRect::new(-0.5, -2.6, lx1 + 0.5, 0.5),
        "to220" => WRect::new(-0.7, -5.2, lx1 + 0.7, 0.5),
        "sot223_module" | "opamp_module" => WRect::new(-0.6, -2.6, lx1 + 0.6, 0.5),
        "dip" | "dip_switch" | "led_bar" => WRect::new(-0.6, ly0 - 0.4, lx1 + 0.6, ly1 + 0.4),
        "tactile_button" => WRect::new(-0.4, -0.4, 3.4, 2.4),
        "potentiometer" | "trimmer" => WRect::new(-0.6, -3.4, lx1 + 0.6, 0.5),
        "slide_switch" => WRect::new(-0.7, -1.6, lx1 + 0.7, 0.5),
        "tilt_switch" => WRect::new(-0.5, -2.8, lx1 + 0.5, 0.5),
        "buzzer" => WRect::new(-0.6, -3.4, lx1 + 0.6, 0.5),
        "seven_segment" => WRect::new(-0.5, -0.5, 4.5, 6.5),
        "ws2812_strip" => WRect::new(-0.5, -2.2, 12.0, 0.5),
        "capsoil_v12" => WRect::new(-0.6, -0.6, 2.6, 10.2),
        "dht22" | "dht11" => WRect::new(-0.5, -h - 0.2, lx1 + 0.5, 0.5),
        "breakout_4pin" | "breakout_3pin" | "pir" | "hc_sr04" | "oled_096" | "relay_module"
        | "lcd_1602" | "lcd_1602_i2c" => WRect::new(-0.6, -h + 0.3, w.max(lx1 + 1.2) - 0.4, 0.6),
        "sg90" => WRect::new(-0.6, -0.6, 11.5, 5.6),
        "stepper_28byj48" => WRect::new(-0.6, -0.6, 11.5, 11.5),
        "dc_motor" | "vibration_motor" | "water_pump" => {
            WRect::new(-0.6, -h + 0.4, w.max(lx1 + 1.0) - 0.4, 0.6)
        }
        "battery_pack" => WRect::new(-0.6, -h + 0.4, w - 0.6, 0.6),
        "usb_supply" | "barrel_jack" => WRect::new(-0.6, -h + 0.4, w - 0.4, 0.6),
        "source" | "spice_box" => WRect::new(-0.6, -2.6, lx1 + 0.6, 0.5),
        "pot_soil" => WRect::new(0.0, -6.0, w, h),
        "desk_lamp" | "hand" => WRect::new(0.0, 0.0, w, h),
        "mini_scope" => WRect::new(-1.0, -1.0, 19.0, 10.0),
        "dmm" => WRect::new(-1.0, -1.0, 12.0, 15.6),
        a if a.starts_with("plugin://") && (w > 0.0 || h > 0.0) => WRect::new(0.0, 0.0, w, h),
        _ => {
            if w > 0.0 || h > 0.0 {
                WRect::new(-0.5, -0.5, w.max(lx1 + 1.0) - 0.5, h.max(ly1 + 1.0) - 0.5)
            } else {
                WRect::new(lx0 - 0.5, ly0 - 0.5, lx1 + 0.5, ly1 + 0.5)
            }
        }
    }
}

/// Parts whose bodies stand up above their leads (drawn in screen orientation).
pub fn standing(art: &str) -> bool {
    matches!(
        art,
        "led_5mm"
            | "rgb_led"
            | "cap_ceramic"
            | "cap_electrolytic"
            | "ldr"
            | "thermistor"
            | "to92"
            | "to220"
            | "sot223_module"
            | "opamp_module"
            | "potentiometer"
            | "trimmer"
            | "slide_switch"
            | "tilt_switch"
            | "buzzer"
            | "dht22"
            | "dht11"
            | "source"
            | "spice_box"
    )
}

/// Draws one part. `pen` is in world coordinates; the part's placement is applied inside.
pub fn draw(pen: &Pen, cx: &ArtCtx, pass: Pass) {
    let at = cx.part.bench.as_ref().map(|b| b.at).unwrap_or_default();
    let lp = pen.at(at);
    let art = cx.def.bench.art.as_str();
    match art {
        "breadboard" => breadboard::breadboard(&lp, cx),
        "uno_r3" => boards::uno(&lp, cx, false),
        "mega_2560" => boards::uno(&lp, cx, true),
        "nano" => boards::nano(&lp, cx),
        "esp32_devkit" => boards::esp32(&lp, cx),
        "pico" => boards::pico(&lp, cx),
        "esp01" => boards::esp01(&lp, cx),
        "hm10" | "hc05" => boards::bt_module(&lp, cx),
        "resistor_axial" => passives::resistor(pen, cx),
        "diode_axial" => passives::diode(pen, cx),
        "inductor_radial" => passives::inductor(pen, cx),
        "soil_fork" => passives::soil_fork(pen, cx),
        "speaker" => passives::speaker(pen, cx),
        "led_5mm" => passives::led(pen, cx),
        "rgb_led" => passives::rgb_led(pen, cx),
        "cap_ceramic" => passives::cap_ceramic(pen, cx),
        "cap_electrolytic" => passives::cap_electrolytic(pen, cx),
        "ldr" => passives::ldr(pen, cx),
        "thermistor" => passives::thermistor(pen, cx),
        "to92" => passives::to92(pen, cx),
        "to220" => passives::to220(pen, cx),
        "sot223_module" | "opamp_module" => passives::small_module(pen, cx),
        "potentiometer" | "trimmer" => passives::pot(pen, cx),
        "slide_switch" => passives::slide_switch(pen, cx),
        "tilt_switch" => passives::tilt_switch(pen, cx),
        "buzzer" => passives::buzzer(pen, cx),
        "tactile_button" => passives::button(&lp, cx),
        "dip" => passives::dip(&lp, cx),
        "dip_switch" => passives::dip_switch(&lp, cx),
        "led_bar" => passives::led_bar(&lp, cx),
        "seven_segment" => passives::seven_segment(&lp, cx),
        "ws2812_strip" => passives::ws2812(pen, cx),
        "source" | "spice_box" => passives::source(pen, cx),
        "capsoil_v12" => modules::capsoil(&lp, cx),
        "dht22" | "dht11" => modules::dht(pen, cx),
        "breakout_4pin" | "breakout_3pin" => modules::breakout(&lp, cx),
        "pir" => modules::pir(&lp, cx),
        "hc_sr04" => modules::hc_sr04(&lp, cx),
        "oled_096" => modules::oled(&lp, cx),
        "lcd_1602" | "lcd_1602_i2c" => modules::lcd(&lp, cx),
        "relay_module" => modules::relay(&lp, cx),
        "sg90" => modules::servo(&lp, cx),
        "stepper_28byj48" => modules::stepper(&lp, cx),
        "dc_motor" | "vibration_motor" | "water_pump" => modules::motor(&lp, cx),
        "battery_pack" => modules::battery(&lp, cx),
        "usb_supply" | "barrel_jack" => modules::usb_supply(&lp, cx),
        "pot_soil" => modules::pot_soil(&lp, cx, pass),
        "desk_lamp" => modules::lamp(&lp, cx),
        "hand" => modules::hand(&lp, cx),
        "mini_scope" => modules::mini_scope(&lp, cx),
        "dmm" => modules::dmm(&lp, cx),
        _ => {
            // plugin parts: instrument front panels / SVG art (canvas::plugin_parts)
            if !super::plugin_parts::draw(&lp, cx) {
                generic(&lp, cx)
            }
        }
    }
    if pass != Pass::Back {
        overlay_health(pen, cx);
    }
}

/// Anything without dedicated art: a labelled module board with its pins.
fn generic(p: &Pen, cx: &ArtCtx) {
    let b = local_bounds(cx.def);
    p.shadow(b, 0.4, 6.0, 4.0, 0.45);
    p.grad(b, 0.4, rgb(0x2B6E4F), rgb(0x1E5139));
    for l in &cx.def.bench.leads {
        p.rect(
            WRect::xywh(l.x - 0.3, l.y - 0.3, 0.6, 0.6),
            0.1,
            rgb(0xC9A54A),
        );
    }
    let (cx0, cy0) = b.center();
    p.text(
        cx0,
        cy0,
        &cx.part.reference,
        0.9,
        font::mono_medium,
        rgb(0xEAF3F7),
    );
}

/// Damage: charred tint, a crack and rising magic smoke.
fn overlay_health(pen: &Pen, cx: &ArtCtx) {
    let Some(v) = cx.vis else { return };
    let at = cx.part.bench.as_ref().map(|b| b.at).unwrap_or_default();
    let lb = local_bounds(cx.def);
    // world-space box of the body
    let a = at.apply(lb.x0, lb.y0);
    let b = at.apply(lb.x1, lb.y1);
    let wb = WRect::new(a.0, a.1, b.0, b.1);
    if v.health.is_damaged() {
        pen.rect(wb, 0.3, with_alpha(rgb(0x140C08), 0.45));
        let (mx, my) = wb.center();
        let s = (wb.w().min(wb.h()) * 0.3).clamp(0.4, 2.0);
        pen.polyline(
            &[
                (mx - s, my - s * 0.6),
                (mx - s * 0.2, my - s * 0.1),
                (mx + s * 0.1, my - s * 0.5),
                (mx + s * 0.8, my + s * 0.4),
            ],
            pen.px(0.12, 1.0),
            rgb(0x1A1410),
        );
    } else if matches!(v.health, Health::Stressed { .. }) {
        let pulse = 0.35 + 0.25 * ((cx.t * 6.0).sin() as f32 + 1.0) / 2.0;
        pen.rect_stroke(
            wb.expand(0.25),
            0.4,
            pen.px(0.15, 1.5),
            with_alpha(rgb(0xF2A33A), pulse),
        );
    }
}

/// Magic smoke: soft grey puffs rising and widening above the part.
pub fn smoke(pen: &Pen, wb: WRect, amount: f32, t: f64, seed: f64) {
    let (mx, _) = wb.center();
    let top = wb.y0;
    let n = 14;
    for i in 0..n {
        let phase = ((t * 0.45 + i as f64 / n as f64 + seed * 0.13) % 1.0 + 1.0) % 1.0;
        let rise = phase * 9.0;
        let drift = ((i as f64 * 2.3 + t * 0.8 + seed).sin()) * (0.5 + phase * 2.2);
        let r = 0.7 + phase * 3.2;
        let a = (amount.max(0.35) * (1.0 - phase as f32).powf(1.2) * 0.62).min(0.7);
        let g = 0x9C + ((1.0 - phase) * 50.0) as u32;
        let c = rgb(g << 16 | g << 8 | g);
        pen.circle(mx + drift, top - 0.2 - rise, r, with_alpha(c, a));
        pen.circle(
            mx + drift - r * 0.3,
            top - 0.3 - rise - r * 0.25,
            r * 0.55,
            with_alpha(lighten(c, 0.3), a * 0.6),
        );
    }
}

/// Metal lead from a hole up to a body point (2.5D legs).
pub fn leg(pen: &Pen, from: (f64, f64), to: (f64, f64)) {
    pen.wline(from, to, 0.24, rgb(0x8E949B));
    pen.wline(
        (from.0 - 0.05, from.1),
        (to.0 - 0.05, to.1),
        0.08,
        rgb(0xD5DADF),
    );
}

/// Small gold/black header pin seen from above.
pub fn header_pin(p: &Pen, x: f64, y: f64) {
    p.rect(
        WRect::xywh(x - 0.42, y - 0.42, 0.84, 0.84),
        0.08,
        rgb(0x141414),
    );
    p.rect(WRect::xywh(x - 0.2, y - 0.2, 0.4, 0.4), 0.04, rgb(0xC9A54A));
}

/// A female socket (Arduino headers): black block with a dark hole.
pub fn socket(p: &Pen, x: f64, y: f64) {
    p.rect(
        WRect::xywh(x - 0.24, y - 0.24, 0.48, 0.48),
        0.05,
        rgb(0x050505),
    );
    p.rect(
        WRect::xywh(x - 0.12, y - 0.2, 0.24, 0.1),
        0.0,
        rgb(0x2C2C2C),
    );
}

/// The standard text colour on PCBs.
pub fn silk() -> Color32 {
    rgb(0xEAF3F7)
}

pub fn mono(px: f32) -> FontId {
    font::mono(px)
}

/// Body colour lightened when lit (LED domes etc.).
pub fn lit(base: Color32, glow: Color32, b: f32) -> Color32 {
    crate::theme::mix(base, lighten(glow, 0.35), b.clamp(0.0, 1.0))
}

/// Darker edge of a body colour.
pub fn edge(c: Color32) -> Color32 {
    darken(c, 0.3)
}

/// Resistor colour-band colours (digit 0–9, gold, silver).
pub fn band_color(d: i32) -> Color32 {
    match d {
        0 => rgb(0x111111),
        1 => rgb(0x7A4A21),
        2 => rgb(0xC8352B),
        3 => rgb(0xE07A1F),
        4 => rgb(0xE7C531),
        5 => rgb(0x3E9E4F),
        6 => rgb(0x3567C6),
        7 => rgb(0x7E4FC4),
        8 => rgb(0x8A8A8A),
        9 => rgb(0xE8E8E8),
        -1 => rgb(0xC9A54A),
        _ => rgb(0xB8BDC4),
    }
}

/// Colour bands for a resistance: 4 bands (5 %: two digits, multiplier, gold) or 5 bands
/// (1 %: three digits, multiplier, brown). Multiplier digits −1/−2 are gold/silver.
pub fn resistor_bands(ohms: f64, tolerance_pct: f64) -> Vec<i32> {
    if !(ohms.is_finite() && ohms > 0.0) {
        return vec![0, 0, 0, -1];
    }
    let digits = if tolerance_pct <= 2.0 { 3 } else { 2 };
    let mut exp = ohms.log10().floor() as i32 - (digits - 1);
    let mut mant = (ohms / 10f64.powi(exp)).round() as i64;
    if mant >= 10i64.pow(digits as u32) {
        mant /= 10;
        exp += 1;
    }
    let mut v = Vec::new();
    let s = format!("{:0width$}", mant, width = digits as usize);
    for c in s.chars() {
        v.push(c.to_digit(10).unwrap_or(0) as i32);
    }
    v.push(exp.clamp(-2, 9));
    v.push(if digits == 3 { 1 } else { -1 });
    v
}

/// LED colour for a `color` prop value.
pub fn led_color(name: &str) -> Color32 {
    match name.to_ascii_lowercase().as_str() {
        "red" => rgb(0xFF4A3A),
        "amber" | "orange" => rgb(0xFFB547),
        "yellow" => rgb(0xFFE14A),
        "green" => rgb(0x4BE36A),
        "blue" => rgb(0x4A8BFF),
        "white" => rgb(0xF4F7FF),
        _ => rgb(0xFF6A5A),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resistor_bands_match_the_colour_code() {
        assert_eq!(resistor_bands(10_000.0, 5.0), vec![1, 0, 3, -1]); // brown black orange gold
        assert_eq!(resistor_bands(220.0, 5.0), vec![2, 2, 1, -1]); // red red brown
        assert_eq!(resistor_bands(330.0, 5.0), vec![3, 3, 1, -1]);
        assert_eq!(resistor_bands(4_700.0, 5.0), vec![4, 7, 2, -1]);
        assert_eq!(resistor_bands(1.0, 5.0), vec![1, 0, -1, -1]); // gold multiplier
        assert_eq!(resistor_bands(12_100.0, 1.0), vec![1, 2, 1, 2, 1]);
        assert_eq!(
            resistor_bands(999.6, 5.0),
            vec![1, 0, 2, -1],
            "rounding carries"
        );
    }

    #[test]
    fn every_library_art_has_bounds_containing_its_leads() {
        let lib = bb_circuit::Library::builtin();
        for d in lib.parts() {
            if d.bench.hidden || d.is_breadboard() {
                continue;
            }
            let b = local_bounds(d);
            for l in &d.bench.leads {
                assert!(
                    b.expand(0.01).contains(l.x, l.y),
                    "{} ({}) lead {} at {},{} outside {:?}",
                    d.id,
                    d.bench.art,
                    l.pin,
                    l.x,
                    l.y,
                    b
                );
            }
        }
    }
}
