//! Plugin template: a "blinker" part — an LED driver chip with its own oscillator. Copy this
//! folder, rename the id in plugin.json, and build with `scripts/build-plugins.sh`.
//!
//! It shows the essentials: reading a property, driving a pin (`drive:pins`), reading the
//! supply (`read:nets`), a periodic tick, a live property change, readings for the
//! inspector, and the bench look.

use bb_plugin_sdk::prelude::*;

struct Blinker {
    out: usize,
    vcc: usize,
    rate_hz: f64,
    on: bool,
    toggles: u64,
}

impl Part for Blinker {
    fn init(cfg: &Config) -> Result<Self, String> {
        let part = cfg.part.as_ref().ok_or("not a part")?;
        Ok(Blinker {
            out: part.pin("OUT").ok_or("no OUT pin")?,
            vcc: part.pin("VCC").ok_or("no VCC pin")?,
            rate_hz: part.prop("rate").unwrap_or(2.0),
            on: false,
            toggles: 0,
        })
    }

    fn on_start(&mut self, ctx: &Ctx) {
        ctx.tick_every(0.5 / self.rate_hz);
    }

    fn on_tick(&mut self, ctx: &Ctx) {
        self.on = !self.on;
        self.toggles += 1;
        let vcc = ctx.pin_voltage(self.vcc);
        let v = if self.on && vcc > 2.0 { vcc } else { 0.0 };
        ctx.set_drive(self.out, Drive::Thevenin { v, r: 30.0 });
        ctx.set_readout(if self.on { "on" } else { "off" });
        ctx.set_readings(&[("toggles", self.toggles as f64), ("rate_hz", self.rate_hz)]);
    }

    fn set_param(&mut self, ctx: &Ctx, key: &str, value: f64) -> bool {
        if key == "rate" && value > 0.0 {
            self.rate_hz = value;
            ctx.tick_every(0.5 / value);
            return true;
        }
        false
    }
}

export_part!(Blinker);
