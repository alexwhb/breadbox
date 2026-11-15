//! Example part plugin: an LM317 adjustable regulator whose analog model is a SPICE
//! subcircuit shipped in `parts.json` (`model.library` → `.subckt LM317BB`), so it works the
//! same on the bench (co-simulation) and in Pro analyses / `.cir` exports.
//!
//! The wasm side only watches the part (`read:nets`): the reference it holds (OUT − ADJ), the
//! headroom (IN − OUT), whether it regulates, and a warning when it drops out.

use bb_plugin_sdk::prelude::*;

/// Below this headroom (V) the LM317 no longer regulates.
const DROPOUT_V: f64 = 2.0;

struct Lm317 {
    vin: usize,
    adj: usize,
    out: usize,
    dropout: bool,
}

impl Part for Lm317 {
    fn init(cfg: &Config) -> Result<Self, String> {
        let part = cfg.part.as_ref().ok_or("not a part")?;
        Ok(Lm317 {
            vin: part.pin("IN").ok_or("no IN pin")?,
            adj: part.pin("ADJ").ok_or("no ADJ pin")?,
            out: part.pin("OUT").ok_or("no OUT pin")?,
            dropout: false,
        })
    }

    fn on_start(&mut self, ctx: &Ctx) {
        ctx.tick_every(5e-3);
        self.on_tick(ctx);
    }

    fn on_tick(&mut self, ctx: &Ctx) {
        let (vin, adj, out) = (
            ctx.pin_voltage(self.vin),
            ctx.pin_voltage(self.adj),
            ctx.pin_voltage(self.out),
        );
        let vref = out - adj;
        let headroom = vin - out;
        let powered = vin > 3.0;
        let regulating = powered && (vref - 1.25).abs() < 0.03 && headroom >= DROPOUT_V;
        let dropout = powered && headroom < DROPOUT_V;
        if dropout && !self.dropout {
            ctx.warn(&format!(
                "LM317 in dropout: {headroom:.2} V between IN and OUT, it needs about {DROPOUT_V:.0} V"
            ));
        }
        self.dropout = dropout;
        ctx.set_readout(&if regulating {
            format!("{out:.2} V out · {headroom:.1} V headroom")
        } else if dropout {
            format!("dropout · {out:.2} V out")
        } else {
            format!("{out:.2} V out")
        });
        ctx.set_readings(&[
            ("vout", out),
            ("vref", vref),
            ("headroom", headroom),
            ("regulating", regulating as u8 as f64),
            ("dropout", dropout as u8 as f64),
        ]);
    }
}

export_part!(Lm317);
