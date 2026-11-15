//! Generator example: a sawtooth with a soft reset, as a source part whose waveform the
//! analog solver plays back (`kind: generator`).

use bb_plugin_sdk::prelude::*;

struct Saw {
    period: f64,
}

impl Generator for Saw {
    fn init(_cfg: &Config) -> Result<Self, String> {
        Ok(Saw { period: 1e-3 })
    }
    fn eval(&mut self, t: f64) -> f64 {
        let ph = (t / self.period).fract();
        // ramp 0 → 1 over 90 % of the period, fall back over the last 10 %
        if ph < 0.9 { ph / 0.9 } else { (1.0 - ph) / 0.1 }
    }
}

export_generator!(Saw);
