//! Bench power supply (PLUG-5): CV/CC with a current limit, foldback on shorts, a sag curve
//! and output enable.
//!
//! The output stage is a SPICE source behind a small output resistance (part template, ext
//! `vset` / `rout`); the model runs the regulation loop every 100 µs like a linear supply's
//! control amplifier: in CV it holds the set voltage minus the transformer sag (50 mV/A);
//! when the load would draw more than the limit it drops into CC and drives the load at the
//! limit; with foldback on, a dead short (< 10 % of the set voltage) folds the limit back to
//! 10 % so the pass transistor survives.

use bb_plugin_sdk::prelude::*;

const LOOP: f64 = 100e-6;
const R_OUT: f64 = 0.02;
const SAG: f64 = 0.05;

struct Psu {
    pos: usize,
    neg: usize,
    v_set: f64,
    i_lim: f64,
    foldback: bool,
    enabled: bool,
    cc: bool,
    folded: bool,
    v_drive: f64,
    v_out: f64,
    i_out: f64,
    ticks: u64,
}

impl Psu {
    fn apply(&self, ctx: &Ctx) {
        if self.enabled {
            ctx.set_ext("vset", self.v_drive.max(0.0));
            ctx.set_ext("rout", R_OUT);
        } else {
            ctx.set_ext("vset", 0.0);
            ctx.set_ext("rout", 1e9);
        }
    }

    fn regulate(&mut self, ctx: &Ctx) {
        self.v_out = ctx.pin_voltage(self.pos) - ctx.pin_voltage(self.neg);
        self.i_out = ctx.element_current("R:out").unwrap_or(0.0);
        if !self.enabled {
            self.cc = false;
            self.folded = false;
            self.apply(ctx);
            return;
        }
        let cv_target = (self.v_set - SAG * self.i_out.max(0.0)).max(0.0);
        // the effective limit (foldback on a short)
        if !self.foldback {
            self.folded = false;
        } else if self.cc && self.v_out < 0.1 * self.v_set {
            self.folded = true;
        }
        let limit = if self.folded {
            0.1 * self.i_lim
        } else {
            self.i_lim
        };
        // load seen by the output: R_L = V/I (or open)
        let r_load = if self.i_out > 1e-9 {
            (self.v_out / self.i_out).max(0.0)
        } else {
            f64::INFINITY
        };
        let i_cv = if r_load.is_finite() {
            cv_target / (r_load + R_OUT)
        } else {
            0.0
        };
        if i_cv > limit {
            // constant current: the drive that pushes exactly `limit` through the load
            self.cc = true;
            self.v_drive = (limit * (r_load + R_OUT)).min(cv_target);
        } else {
            self.cc = false;
            self.v_drive = cv_target + R_OUT * self.i_out.max(0.0);
            if self.folded && r_load * self.i_lim > 0.5 * self.v_set {
                self.folded = false; // the short went away
            }
        }
        self.apply(ctx);
    }

    fn publish(&self, ctx: &Ctx) {
        let mut p = PanelState::default();
        p.displays.insert(
            "volts".into(),
            DisplayState {
                text: format!("{:05.2}", self.v_out.max(0.0)),
                unit: "V".into(),
                annunciators: vec![],
                bar: None,
            },
        );
        p.displays.insert(
            "amps".into(),
            DisplayState {
                text: format!("{:.3}", self.i_out.max(0.0)),
                unit: "A".into(),
                annunciators: if self.folded {
                    vec!["FOLD".into()]
                } else {
                    vec![]
                },
                bar: None,
            },
        );
        p.leds
            .insert("cv".into(), (self.enabled && !self.cc) as u8 as f32);
        p.leds
            .insert("cc".into(), (self.enabled && self.cc) as u8 as f32);
        p.leds.insert("out".into(), self.enabled as u8 as f32);
        p.controls.insert("voltage".into(), self.v_set);
        p.controls.insert("current".into(), self.i_lim);
        p.controls
            .insert("foldback".into(), self.foldback as u8 as f64);
        p.readouts.push((
            "Set".into(),
            format!("{:.2} V · {:.3} A", self.v_set, self.i_lim),
        ));
        p.readouts.push((
            "Power".into(),
            format!("{:.2} W", (self.v_out * self.i_out).max(0.0)),
        ));
        ctx.set_panel(&p);
        ctx.set_readout(&format!(
            "{:.2} V {:.3} A {}",
            self.v_out,
            self.i_out,
            if !self.enabled {
                "OFF"
            } else if self.cc {
                "CC"
            } else {
                "CV"
            }
        ));
        ctx.set_readings(&[
            ("voltage", self.v_out),
            ("current", self.i_out),
            ("cc", self.cc as u8 as f64),
            ("folded", self.folded as u8 as f64),
            ("enabled", self.enabled as u8 as f64),
            ("v_set", self.v_set),
            ("i_limit", self.i_lim),
        ]);
    }
}

impl Part for Psu {
    fn init(cfg: &Config) -> Result<Self, String> {
        let part = cfg.part.clone().unwrap_or_default();
        Ok(Psu {
            pos: cfg.pin("POS").ok_or("no POS pin")?,
            neg: cfg.pin("NEG").ok_or("no NEG pin")?,
            v_set: part.prop("voltage").unwrap_or(5.0).clamp(0.0, 30.0),
            i_lim: part.prop("current_limit").unwrap_or(1.0).clamp(0.001, 3.0),
            foldback: part.prop("foldback").unwrap_or(1.0) > 0.5,
            enabled: part.prop("output").unwrap_or(1.0) > 0.5,
            cc: false,
            folded: false,
            v_drive: 0.0,
            v_out: 0.0,
            i_out: 0.0,
            ticks: 0,
        })
    }
    fn on_start(&mut self, ctx: &Ctx) {
        self.v_drive = self.v_set;
        self.apply(ctx);
        ctx.track_element("R:out");
        ctx.tick_every(LOOP);
    }
    fn on_tick(&mut self, ctx: &Ctx) {
        self.regulate(ctx);
        // refresh the panel ~30× per second
        self.ticks += 1;
        if self.ticks % 333 == 1 {
            self.publish(ctx);
        }
    }
    fn on_input(&mut self, ctx: &Ctx, input: &Input) {
        if let Input::Value { key, value } = input {
            match key.as_str() {
                "voltage" => self.v_set = value.clamp(0.0, 30.0),
                "current" => self.i_lim = value.clamp(0.001, 3.0),
                "foldback" => self.foldback = *value > 0.5,
                "output" if *value > 0.5 => {
                    self.enabled = !self.enabled;
                    self.folded = false;
                }
                _ => return,
            }
            self.regulate(ctx);
            self.publish(ctx);
        }
    }
    fn set_param(&mut self, ctx: &Ctx, key: &str, value: f64) -> bool {
        let k = match key {
            "voltage" => "voltage",
            "current_limit" => "current",
            "foldback" => "foldback",
            _ => return false,
        };
        self.on_input(
            ctx,
            &Input::Value {
                key: k.into(),
                value,
            },
        );
        true
    }
}

export_part!(Psu);
