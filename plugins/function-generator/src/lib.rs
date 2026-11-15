//! Function generator (PLUG-5): sine, square, triangle, pulse, noise and an arbitrary
//! 16-point waveform behind a 50 Ω output.
//!
//! The waveform is a behavioural SPICE source in the part template, driven live through ext
//! variables (`shape`, `freq`, `amp`, `offset`, `duty`, `en`, `a0…a15`), so the analog solver
//! plays it back exactly — no per-sample plugin calls. Noise is the only shape the model feeds
//! sample by sample (band-limited to 20 kHz).

use bb_plugin_sdk::prelude::*;

const SHAPES: [&str; 6] = ["sine", "square", "triangle", "pulse", "noise", "arb"];
const NOISE_DT: f64 = 25e-6;

struct Gen {
    shape: usize,
    freq: f64,
    vpp: f64,
    offset: f64,
    duty: f64,
    on: bool,
    arb: [f64; 16],
}

impl Gen {
    fn apply(&self, ctx: &Ctx) {
        ctx.set_ext("shape", self.shape as f64);
        ctx.set_ext("freq", self.freq);
        ctx.set_ext("amp", self.vpp / 2.0);
        ctx.set_ext("offset", self.offset);
        ctx.set_ext("duty", self.duty);
        ctx.set_ext("en", if self.on { 1.0 } else { 0.0 });
        for (i, a) in self.arb.iter().enumerate() {
            ctx.set_ext(&format!("a{i}"), *a);
        }
        ctx.tick_every(if self.shape == 4 && self.on {
            NOISE_DT
        } else {
            0.0
        });
    }

    fn publish(&self, ctx: &Ctx) {
        let mut p = PanelState::default();
        let f = if self.freq >= 1e6 {
            format!("{:.4} MHz", self.freq / 1e6)
        } else if self.freq >= 1e3 {
            format!("{:.4} kHz", self.freq / 1e3)
        } else {
            format!("{:.3} Hz", self.freq)
        };
        p.displays.insert(
            "lcd".into(),
            DisplayState {
                text: f.clone(),
                unit: String::new(),
                annunciators: vec![SHAPES[self.shape].to_uppercase()],
                bar: None,
            },
        );
        p.leds.insert("on".into(), self.on as u8 as f32);
        p.controls.insert("shape".into(), self.shape as f64);
        p.controls.insert("frequency".into(), self.freq);
        p.controls.insert("amplitude".into(), self.vpp);
        p.controls.insert("offset".into(), self.offset);
        p.controls.insert("duty".into(), self.duty);
        p.readouts = vec![
            ("Amplitude".into(), format!("{:.3} Vpp", self.vpp)),
            ("Offset".into(), format!("{:+.3} V", self.offset)),
            ("Duty".into(), format!("{:.0} %", self.duty * 100.0)),
            ("Output".into(), "50 Ω".into()),
        ];
        ctx.set_panel(&p);
        ctx.set_readout(&format!("{} {} {:.2} Vpp", SHAPES[self.shape], f, self.vpp));
        ctx.set_readings(&[
            ("shape", self.shape as f64),
            ("frequency", self.freq),
            ("vpp", self.vpp),
            ("offset", self.offset),
            ("duty", self.duty),
            ("output", self.on as u8 as f64),
        ]);
    }

    fn control(&mut self, key: &str, v: f64) -> bool {
        match key {
            "shape" => self.shape = (v.max(0.0) as usize).min(SHAPES.len() - 1),
            "frequency" => self.freq = v.clamp(1e-3, 10e6),
            "amplitude" => self.vpp = v.clamp(0.0, 20.0),
            "offset" => self.offset = v.clamp(-10.0, 10.0),
            "duty" => self.duty = v.clamp(0.01, 0.99),
            "output" => self.on = v > 0.5,
            k if k.starts_with('a') => {
                let Some(i) = k[1..].parse::<usize>().ok().filter(|i| *i < 16) else {
                    return false;
                };
                self.arb[i] = v.clamp(-1.0, 1.0);
            }
            _ => return false,
        }
        true
    }
}

impl Part for Gen {
    fn init(cfg: &Config) -> Result<Self, String> {
        let part = cfg.part.clone().unwrap_or_default();
        let shape = part
            .props
            .get("shape")
            .and_then(|v| {
                v.as_str()
                    .map(String::from)
                    .or_else(|| v.get("Text").and_then(|t| t.as_str()).map(String::from))
            })
            .and_then(|s| SHAPES.iter().position(|x| *x == s))
            .unwrap_or(0);
        // default arbitrary waveform: a ramp with a dip (a "heartbeat")
        let arb = [
            0.0, 0.1, 0.2, 0.3, 1.0, -0.6, 0.2, 0.1, 0.0, 0.0, 0.25, 0.3, 0.2, 0.0, 0.0, 0.0,
        ];
        cfg.pin("OUT").ok_or("no OUT pin")?;
        Ok(Gen {
            shape,
            freq: part.prop("frequency").unwrap_or(1000.0),
            vpp: part.prop("amplitude").unwrap_or(2.0),
            offset: part.prop("offset").unwrap_or(0.0),
            duty: part.prop("duty").unwrap_or(0.5),
            on: part.prop("output").unwrap_or(1.0) > 0.5,
            arb,
        })
    }
    fn on_start(&mut self, ctx: &Ctx) {
        self.apply(ctx);
        self.publish(ctx);
    }
    fn on_tick(&mut self, ctx: &Ctx) {
        // noise: Gaussian, ±1 ≈ 1 σ of the amplitude
        ctx.set_ext("noise", ctx.gauss().clamp(-3.0, 3.0) / 3.0);
    }
    fn on_input(&mut self, ctx: &Ctx, input: &Input) {
        if let Input::Value { key, value } = input
            && self.control(key, *value)
        {
            self.apply(ctx);
            self.publish(ctx);
        }
    }
    fn set_param(&mut self, ctx: &Ctx, key: &str, value: f64) -> bool {
        let key = match key {
            "amplitude" | "frequency" | "offset" | "duty" => key,
            _ => return false,
        };
        self.control(key, value);
        self.apply(ctx);
        self.publish(ctx);
        true
    }
}

export_part!(Gen);
