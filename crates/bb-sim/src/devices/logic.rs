//! Digital ICs (74HC595, 74HC00/04/08/32), drivers (ULN2003, L293D) and the 555 timer as
//! event-driven devices: Schmitt watches on inputs, Thevenin outputs from their own supply pin.

use super::{DeviceRegistry, logic, ok, pin_of, prop_text};
use bb_circuit::netlist::DeviceSpec;
use bb_core::{Device, DeviceCtx, Look, PinDrive, SimTime, VisualState};

pub(super) fn register(r: &mut DeviceRegistry) {
    r.register_fn("hc595", |s| Box::new(Hc595::new(s)));
    r.register_fn("logic", |s| Box::new(Gates::new(s)));
    r.register_fn("uln2003", |s| Box::new(Uln2003::new(s)));
    r.register_fn("l293d", |s| Box::new(L293d::new(s)));
    r.register_fn("ne555", |s| Box::new(Ne555::new(s)));
}

fn supply(ctx: &dyn DeviceCtx, pin: usize) -> f64 {
    if ok(pin) { ctx.pin_voltage(pin) } else { 5.0 }
}

/// HC CMOS output: ~50 Ω to the rails.
fn out_drive(level: bool, vcc: f64) -> PinDrive {
    PinDrive::Thevenin {
        v: if level { vcc } else { 0.0 },
        r: 50.0,
    }
}

// ------------------------------------------------------------------------------------ 74HC595

struct Hc595 {
    q: [usize; 8],
    q7s: usize,
    mr: usize,
    shcp: usize,
    stcp: usize,
    oe: usize,
    ds: usize,
    vcc: usize,
    shift: u8,
    latch: u8,
    oe_low: bool,
}

impl Hc595 {
    fn new(s: &DeviceSpec) -> Hc595 {
        let q = std::array::from_fn(|i| pin_of(s, &format!("Q{i}")));
        Hc595 {
            q,
            q7s: pin_of(s, "Q7S"),
            mr: pin_of(s, "MR"),
            shcp: pin_of(s, "SHCP"),
            stcp: pin_of(s, "STCP"),
            oe: pin_of(s, "OE"),
            ds: pin_of(s, "DS"),
            vcc: pin_of(s, "VCC"),
            shift: 0,
            latch: 0,
            oe_low: true,
        }
    }
    fn outputs(&self, ctx: &mut dyn DeviceCtx) {
        let vcc = supply(ctx, self.vcc);
        for (i, &p) in self.q.iter().enumerate() {
            if ok(p) {
                let d = if self.oe_low {
                    out_drive(self.latch >> i & 1 == 1, vcc)
                } else {
                    PinDrive::HighZ
                };
                ctx.set_drive(p, d);
            }
        }
        if ok(self.q7s) {
            ctx.set_drive(self.q7s, out_drive(self.shift >> 7 & 1 == 1, vcc));
        }
    }
}

impl Device for Hc595 {
    fn kind(&self) -> &str {
        "hc595"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        let vcc = supply(ctx, self.vcc);
        for p in [self.mr, self.shcp, self.stcp, self.oe] {
            if ok(p) {
                ctx.watch(p, 0.3 * vcc, 0.7 * vcc);
            }
        }
        self.oe_low = !logic(ctx, self.oe, vcc);
        self.outputs(ctx);
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, high: bool) {
        let vcc = supply(ctx, self.vcc);
        let mr_low = ok(self.mr) && !logic(ctx, self.mr, vcc);
        if pin == self.mr && !high {
            self.shift = 0;
        } else if pin == self.shcp && high && !mr_low {
            let bit = logic(ctx, self.ds, vcc) as u8;
            self.shift = (self.shift << 1) | bit;
        } else if pin == self.stcp && high {
            self.latch = self.shift;
        } else if pin == self.oe {
            self.oe_low = !high;
        }
        self.outputs(ctx);
    }
    fn visual(&self) -> VisualState {
        Look::Readout {
            text: format!("{:08b}", self.latch),
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![
            ("latch".into(), self.latch as f64),
            ("shift".into(), self.shift as f64),
        ]
    }
}

// ------------------------------------------------------------------------------------ gates

struct Gates {
    op: String,
    /// (inputs, output) per gate.
    gates: Vec<(Vec<usize>, usize)>,
    vcc: usize,
}

impl Gates {
    fn new(s: &DeviceSpec) -> Gates {
        let op = prop_text(s, "gate", "nand");
        let n = if op == "not" { 6 } else { 4 };
        let gates = (1..=n)
            .map(|k| {
                let ins = if op == "not" {
                    vec![pin_of(s, &format!("{k}A"))]
                } else {
                    vec![pin_of(s, &format!("{k}A")), pin_of(s, &format!("{k}B"))]
                };
                (ins, pin_of(s, &format!("{k}Y")))
            })
            .collect();
        Gates {
            op,
            gates,
            vcc: pin_of(s, "VCC"),
        }
    }
    fn eval(&self, ctx: &mut dyn DeviceCtx) {
        let vcc = supply(ctx, self.vcc);
        for (ins, out) in &self.gates {
            if !ok(*out) {
                continue;
            }
            let v: Vec<bool> = ins.iter().map(|&p| logic(ctx, p, vcc)).collect();
            let y = match self.op.as_str() {
                "and" => v.iter().all(|x| *x),
                "or" => v.iter().any(|x| *x),
                "not" => !v[0],
                "nor" => !v.iter().any(|x| *x),
                "xor" => v.iter().filter(|x| **x).count() % 2 == 1,
                _ => !v.iter().all(|x| *x),
            };
            ctx.set_drive(*out, out_drive(y, vcc));
        }
    }
}

impl Device for Gates {
    fn kind(&self) -> &str {
        "logic"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        let vcc = supply(ctx, self.vcc);
        for (ins, _) in &self.gates {
            for &p in ins {
                if ok(p) {
                    ctx.set_drive(p, PinDrive::Pull { v: 0.0, r: 1e9 });
                    ctx.watch(p, 0.3 * vcc, 0.7 * vcc);
                }
            }
        }
        self.eval(ctx);
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, _pin: usize, _high: bool) {
        // propagation delay ≈ 8 ns
        ctx.wake_at(ctx.now() + SimTime::from_ns(8), 1);
    }
    fn on_wake(&mut self, ctx: &mut dyn DeviceCtx, _token: u64) {
        self.eval(ctx);
    }
    fn visual(&self) -> VisualState {
        VisualState::new(Look::None)
    }
}

// ------------------------------------------------------------------------------------ ULN2003

struct Uln2003 {
    pairs: Vec<(usize, usize)>,
    on: Vec<bool>,
}

impl Uln2003 {
    fn new(s: &DeviceSpec) -> Uln2003 {
        let pairs = (1..=7)
            .map(|k| (pin_of(s, &format!("{k}B")), pin_of(s, &format!("{k}C"))))
            .collect();
        Uln2003 {
            pairs,
            on: vec![false; 7],
        }
    }
}

impl Device for Uln2003 {
    fn kind(&self) -> &str {
        "uln2003"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        for &(b, _) in &self.pairs {
            if ok(b) {
                // 2.7 kΩ base resistor + Darlington input
                ctx.set_drive(b, PinDrive::Pull { v: 1.4, r: 2.7e3 });
                ctx.watch(b, 1.6, 1.9);
            }
        }
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, high: bool) {
        if let Some(k) = self.pairs.iter().position(|(b, _)| *b == pin) {
            self.on[k] = high;
            let c = self.pairs[k].1;
            if ok(c) {
                let d = if high {
                    PinDrive::Thevenin { v: 0.85, r: 1.5 }
                } else {
                    PinDrive::HighZ
                };
                ctx.set_drive(c, d);
            }
        }
    }
    fn visual(&self) -> VisualState {
        Look::Switches {
            on: self.on.clone(),
        }
        .into()
    }
}

// ------------------------------------------------------------------------------------ L293D

struct L293d {
    en: [usize; 2],
    a: [usize; 4],
    y: [usize; 4],
}

impl L293d {
    fn new(s: &DeviceSpec) -> L293d {
        L293d {
            en: [pin_of(s, "EN12"), pin_of(s, "EN34")],
            a: [
                pin_of(s, "1A"),
                pin_of(s, "2A"),
                pin_of(s, "3A"),
                pin_of(s, "4A"),
            ],
            y: [
                pin_of(s, "1Y"),
                pin_of(s, "2Y"),
                pin_of(s, "3Y"),
                pin_of(s, "4Y"),
            ],
        }
    }
    fn eval(&self, ctx: &mut dyn DeviceCtx) {
        // VCC2 is a SPICE pin of the part: read it through any output's supply reference
        let vmotor = 5.0f64.max(0.0);
        for k in 0..4 {
            if !ok(self.y[k]) {
                continue;
            }
            let en = logic(ctx, self.en[k / 2], 5.0) || !ok(self.en[k / 2]);
            let d = if en {
                if logic(ctx, self.a[k], 5.0) {
                    PinDrive::Thevenin {
                        v: vmotor - 1.4,
                        r: 1.5,
                    }
                } else {
                    PinDrive::Thevenin { v: 0.2, r: 1.5 }
                }
            } else {
                PinDrive::HighZ
            };
            ctx.set_drive(self.y[k], d);
        }
    }
}

impl Device for L293d {
    fn kind(&self) -> &str {
        "l293d"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        for &p in self.en.iter().chain(self.a.iter()) {
            if ok(p) {
                ctx.set_drive(p, PinDrive::Pull { v: 0.0, r: 1e6 });
                ctx.watch(p, 1.5, 2.3);
            }
        }
        self.eval(ctx);
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, _pin: usize, _high: bool) {
        self.eval(ctx);
    }
    fn visual(&self) -> VisualState {
        VisualState::new(Look::None)
    }
}

// ------------------------------------------------------------------------------------ 555

/// NE555: comparators at ⅔·Vcc (THR, set by CTRL) and ⅓·Vcc (TRIG), an SR latch, the output
/// stage and the discharge transistor — event driven, exact on the timing capacitor's
/// threshold crossings.
struct Ne555 {
    trig: usize,
    thr: usize,
    out: usize,
    dis: usize,
    reset: usize,
    ctrl: usize,
    vcc: usize,
    q: bool,
    thr_high: bool,
    trig_low: bool,
    reset_low: bool,
    cycles: u64,
    watched_hi: f64,
}

impl Ne555 {
    fn new(s: &DeviceSpec) -> Ne555 {
        Ne555 {
            trig: pin_of(s, "TRIG"),
            thr: pin_of(s, "THR"),
            out: pin_of(s, "OUT"),
            dis: pin_of(s, "DIS"),
            reset: pin_of(s, "RESET"),
            ctrl: pin_of(s, "CTRL"),
            vcc: pin_of(s, "VCC"),
            q: false,
            thr_high: false,
            trig_low: false,
            reset_low: false,
            cycles: 0,
            watched_hi: 0.0,
        }
    }
    fn levels(&self, ctx: &dyn DeviceCtx) -> (f64, f64, f64) {
        let vcc = supply(ctx, self.vcc);
        let hi = if ok(self.ctrl) {
            ctx.pin_voltage(self.ctrl)
        } else {
            vcc * 2.0 / 3.0
        };
        (vcc, hi, hi / 2.0)
    }
    fn rewatch(&mut self, ctx: &mut dyn DeviceCtx) {
        let (vcc, hi, lo) = self.levels(ctx);
        let eps = 1e-3 * vcc.max(1.0);
        if ok(self.thr) {
            ctx.watch(self.thr, hi - eps, hi);
        }
        if ok(self.trig) {
            ctx.watch(self.trig, lo, lo + eps);
        }
        self.watched_hi = hi;
    }
    fn update(&mut self, ctx: &mut dyn DeviceCtx) {
        let before = self.q;
        if self.reset_low {
            self.q = false;
        } else if self.trig_low {
            self.q = true;
        } else if self.thr_high {
            self.q = false;
        }
        if self.q && !before {
            self.cycles += 1;
        }
        let (vcc, _, _) = self.levels(ctx);
        if ok(self.out) {
            let d = if self.q {
                PinDrive::Thevenin {
                    v: (vcc - 1.7).max(0.0),
                    r: 10.0,
                }
            } else {
                PinDrive::Thevenin { v: 0.1, r: 10.0 }
            };
            ctx.set_drive(self.out, d);
        }
        if ok(self.dis) {
            let d = if self.q {
                PinDrive::HighZ
            } else {
                PinDrive::Thevenin { v: 0.05, r: 5.0 }
            };
            ctx.set_drive(self.dis, d);
        }
    }
}

impl Device for Ne555 {
    fn kind(&self) -> &str {
        "ne555"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        self.rewatch(ctx);
        if ok(self.thr) {
            self.thr_high = ctx.pin_voltage(self.thr) > self.watched_hi;
        }
        if ok(self.trig) {
            self.trig_low = ctx.pin_voltage(self.trig) < self.watched_hi / 2.0;
        }
        ctx.tick_every(SimTime::from_ms(20));
        if ok(self.reset) {
            ctx.watch(self.reset, 0.4, 1.0);
            self.reset_low = ctx.pin_voltage(self.reset) < 0.7;
        }
        self.update(ctx);
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        // follow the supply (battery sag, CTRL modulation)
        let (_, hi, _) = self.levels(ctx);
        if (hi - self.watched_hi).abs() > 0.005 * self.watched_hi.abs().max(0.1) {
            self.rewatch(ctx);
        }
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, high: bool) {
        if pin == self.thr {
            self.thr_high = high;
        } else if pin == self.trig {
            self.trig_low = !high;
        } else if pin == self.reset {
            self.reset_low = !high;
        }
        self.update(ctx);
    }
    fn visual(&self) -> VisualState {
        Look::Switch { on: self.q }.into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![
            ("output".into(), self.q as u8 as f64),
            ("cycles".into(), self.cycles as f64),
        ]
    }
}
