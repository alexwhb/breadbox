//! WS2812B ("NeoPixel") strip: bit-accurate 800 kHz decoding from high-pulse widths (T0H ≈
//! 0.4 µs, T1H ≈ 0.8 µs), GRB order, latch after > 50 µs low, current draw from the colours.

use super::{DeviceRegistry, ok, pin_of, prop_f64};
use bb_circuit::netlist::DeviceSpec;
use bb_core::{Device, DeviceCtx, Look, PinDrive, Rgb, SimTime, VisualState};

pub(super) fn register(r: &mut DeviceRegistry) {
    r.register_fn("ws2812", |s| Box::new(Ws2812::new(s)));
}

struct Ws2812 {
    n: usize,
    leds: Vec<[u8; 3]>,
    bits: Vec<u8>,
    count: usize,
    rise: Option<SimTime>,
    din: usize,
    vdd: usize,
    frames: u64,
}

const T_LATCH: u64 = 1;

impl Ws2812 {
    fn new(s: &DeviceSpec) -> Ws2812 {
        let n = prop_f64(s, "count", 8.0).clamp(1.0, 1000.0) as usize;
        Ws2812 {
            n,
            leds: vec![[0; 3]; n],
            bits: vec![0; n * 3],
            count: 0,
            rise: None,
            din: pin_of(s, "DIN"),
            vdd: pin_of(s, "VDD"),
            frames: 0,
        }
    }
    fn latch(&mut self, ctx: &mut dyn DeviceCtx) {
        if self.count == 0 {
            return;
        }
        let full = (self.count / 24).min(self.n);
        for i in 0..full {
            let g = self.bits[i * 3];
            let r = self.bits[i * 3 + 1];
            let b = self.bits[i * 3 + 2];
            self.leds[i] = [r, g, b];
        }
        self.count = 0;
        self.frames += 1;
        if ok(self.vdd) {
            let i: f64 = self
                .leds
                .iter()
                .map(|c| 0.001 + 0.02 * (c[0] as f64 + c[1] as f64 + c[2] as f64) / 255.0)
                .sum();
            ctx.set_drive(self.vdd, PinDrive::Current { i: -i });
        }
    }
}

impl Device for Ws2812 {
    fn kind(&self) -> &str {
        "ws2812"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        if ok(self.din) {
            ctx.watch(self.din, 1.5, 3.0);
        }
        if ok(self.vdd) {
            ctx.set_drive(self.vdd, PinDrive::Current { i: -0.001 * self.n as f64 });
        }
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, _pin: usize, high: bool) {
        let now = ctx.now();
        if high {
            self.rise = Some(now);
            ctx.cancel_wake(T_LATCH);
            return;
        }
        let Some(r) = self.rise.take() else { return };
        let w = (now - r).as_secs_f64();
        if w > 5e-6 {
            return; // not a data bit
        }
        let bit = (w > 0.6e-6) as u8;
        if self.count < self.n * 24 {
            let byte = self.count / 8;
            let k = 7 - self.count % 8;
            self.bits[byte] = (self.bits[byte] & !(1 << k)) | (bit << k);
        }
        self.count += 1;
        ctx.wake_at(now + SimTime::from_us(50), T_LATCH);
    }
    fn on_wake(&mut self, ctx: &mut dyn DeviceCtx, _token: u64) {
        self.latch(ctx);
    }
    fn visual(&self) -> VisualState {
        Look::LedArray {
            leds: self.leds.iter().map(|c| Rgb::new(c[0], c[1], c[2])).collect(),
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("frames".into(), self.frames as f64)]
    }
}

impl Ws2812 {
    #[allow(dead_code)]
    pub fn colors(&self) -> &[[u8; 3]] {
        &self.leds
    }
}
