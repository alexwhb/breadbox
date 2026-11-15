//! Motors, servos, steppers, relays and sound makers.

use super::{DeviceRegistry, ok, pin_of, prop_bool, prop_f64};
use bb_circuit::netlist::DeviceSpec;
use bb_core::{
    AudioChunk, Device, DeviceCtx, DeviceEvent, Look, PinDrive, SimTime, Sound, VisualState,
};
use std::collections::VecDeque;

pub(super) fn register(r: &mut DeviceRegistry) {
    r.register_fn("dc_motor", |s| Box::new(DcMotor::new(s)));
    r.register_fn("servo", |s| Box::new(Servo::new(s)));
    r.register_fn("stepper", |s| Box::new(Stepper::new(s)));
    r.register_fn("relay", |s| Box::new(Relay::new(s)));
    r.register_fn("buzzer", |s| Box::new(Buzzer::new(s, false)));
    r.register_fn("buzzer_active", |s| Box::new(Buzzer::new(s, true)));
}

// ------------------------------------------------------------------------------------ DC motor

/// Brushed DC motor: armature resistance + back-EMF source in SPICE; the device integrates the
/// mechanics (J dω/dt = Kt·I − b·ω − load) and updates the back-EMF (Ke·ω).
struct DcMotor {
    ke: f64,
    j: f64,
    b: f64,
    load: f64,
    omega: f64,
    angle: f64,
    sent: f64,
    last_t: SimTime,
}

impl DcMotor {
    fn new(s: &DeviceSpec) -> DcMotor {
        let kv = prop_f64(s, "kv", 1200.0).max(1.0);
        let ke = 60.0 / (2.0 * std::f64::consts::PI * kv);
        let small = s.def == "vibration_motor";
        DcMotor {
            ke,
            j: if small { 2e-8 } else { 1.5e-6 },
            b: if small { 5e-9 } else { 4e-7 },
            load: prop_f64(s, "load", 0.2) * 1e-3,
            omega: 0.0,
            angle: 0.0,
            sent: 0.0,
            last_t: SimTime::ZERO,
        }
    }
}

impl Device for DcMotor {
    fn kind(&self) -> &str {
        "dc_motor"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        ctx.set_ext("emf", 0.0);
        self.last_t = ctx.now();
        ctx.tick_every(SimTime::from_us(500));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let dt = (ctx.now() - self.last_t).as_secs_f64();
        self.last_t = ctx.now();
        if dt <= 0.0 {
            return;
        }
        // current through the back-EMF source (SPICE sign: into its + node = motoring)
        let i = ctx.element_current("V:emf").unwrap_or(0.0);
        let torque = self.ke * i;
        let friction = self.load * self.omega.signum();
        // integrate in small substeps for stability
        let n = 4;
        let h = dt / n as f64;
        for _ in 0..n {
            let net = torque - self.b * self.omega - friction;
            let w = self.omega + net / self.j * h;
            // static friction holds a stalled rotor
            self.omega = if self.omega.abs() < 1e-3 && torque.abs() <= self.load {
                0.0
            } else {
                w
            };
        }
        self.angle = (self.angle + self.omega.to_degrees() * dt).rem_euclid(360.0);
        let emf = self.ke * self.omega;
        if (emf - self.sent).abs() > 1e-3 {
            ctx.set_ext("emf", emf);
            self.sent = emf;
        }
    }
    fn set_param(&mut self, _ctx: &mut dyn DeviceCtx, key: &str, value: f64) -> bool {
        if key == "load" {
            self.load = value * 1e-3;
            true
        } else {
            false
        }
    }
    fn visual(&self) -> VisualState {
        Look::Motor {
            rpm: self.omega * 60.0 / (2.0 * std::f64::consts::PI),
            angle_deg: self.angle,
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![(
            "rpm".into(),
            self.omega * 60.0 / (2.0 * std::f64::consts::PI),
        )]
    }
}

// ------------------------------------------------------------------------------------ servo

/// SG90: pulse width (544–2400 µs) sets the target angle; the horn slews at 600°/s; supply
/// current 10 mA idle, ~200 mA while moving.
struct Servo {
    sig: usize,
    vcc: usize,
    rise: Option<SimTime>,
    target: f64,
    angle: f64,
    last_t: SimTime,
    moving: bool,
    last_pulse_us: f64,
    pulses: u64,
}

impl Servo {
    fn new(s: &DeviceSpec) -> Servo {
        Servo {
            sig: pin_of(s, "SIG"),
            vcc: pin_of(s, "VCC"),
            rise: None,
            target: 90.0,
            angle: 90.0,
            last_t: SimTime::ZERO,
            moving: false,
            last_pulse_us: 0.0,
            pulses: 0,
        }
    }
    fn supply(&self, ctx: &mut dyn DeviceCtx) {
        if ok(self.vcc) {
            let i = if self.moving { 0.2 } else { 0.01 };
            ctx.set_drive(self.vcc, PinDrive::Current { i: -i });
        }
    }
}

impl Device for Servo {
    fn kind(&self) -> &str {
        "servo"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        if ok(self.sig) {
            ctx.set_drive(self.sig, PinDrive::Pull { v: 0.0, r: 20e3 });
            ctx.watch(self.sig, 0.8, 2.0);
        }
        self.supply(ctx);
        self.last_t = ctx.now();
        ctx.tick_every(SimTime::from_ms(5));
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, high: bool) {
        if pin != self.sig {
            return;
        }
        if high {
            self.rise = Some(ctx.now());
        } else if let Some(r) = self.rise.take() {
            let us = (ctx.now() - r).as_secs_f64() * 1e6;
            if (400.0..=2700.0).contains(&us) {
                self.last_pulse_us = us;
                self.pulses += 1;
                self.target = ((us - 544.0) / (2400.0 - 544.0) * 180.0).clamp(0.0, 180.0);
            }
        }
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let dt = (ctx.now() - self.last_t).as_secs_f64();
        self.last_t = ctx.now();
        let supply_ok = !ok(self.vcc) || ctx.pin_voltage(self.vcc) > 3.5;
        let err = self.target - self.angle;
        let step = 600.0 * dt;
        let moving = supply_ok && err.abs() > 0.5;
        if supply_ok {
            if err.abs() <= step {
                self.angle = self.target;
            } else {
                self.angle += step * err.signum();
            }
        }
        if moving != self.moving {
            self.moving = moving;
            self.supply(ctx);
        }
    }
    fn visual(&self) -> VisualState {
        Look::Servo {
            angle_deg: self.angle,
            target_deg: self.target,
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![
            ("angle_deg".into(), self.angle),
            ("target_deg".into(), self.target),
            ("pulse_us".into(), self.last_pulse_us),
            ("pulses".into(), self.pulses as f64),
        ]
    }
}

// ------------------------------------------------------------------------------------ stepper

/// 28BYJ-48: energized coil pattern → half-step phase; 4096 half-steps per output revolution.
struct Stepper {
    last_q: [f64; 4],
    last_t: SimTime,
    phase: Option<i32>,
    steps: i64,
}

impl Stepper {
    fn new(_s: &DeviceSpec) -> Stepper {
        Stepper {
            last_q: [0.0; 4],
            last_t: SimTime::ZERO,
            phase: None,
            steps: 0,
        }
    }
}

const COILS: [&str; 4] = ["R:a", "R:b", "R:c", "R:d"];

/// Half-step phase (0..8) from energized coils A B C D (orange, yellow, pink, blue).
fn phase_of(on: [bool; 4]) -> Option<i32> {
    const TABLE: [[bool; 4]; 8] = [
        [true, false, false, false],
        [true, true, false, false],
        [false, true, false, false],
        [false, true, true, false],
        [false, false, true, false],
        [false, false, true, true],
        [false, false, false, true],
        [true, false, false, true],
    ];
    TABLE.iter().position(|p| *p == on).map(|i| i as i32)
}

impl Device for Stepper {
    fn kind(&self) -> &str {
        "stepper"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        for c in COILS {
            ctx.track_element(c);
        }
        self.last_t = ctx.now();
        ctx.tick_every(SimTime::from_us(500));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let dt = (ctx.now() - self.last_t).as_secs_f64();
        if dt <= 0.0 {
            return;
        }
        self.last_t = ctx.now();
        let mut on = [false; 4];
        for (k, c) in COILS.iter().enumerate() {
            let q = ctx.element_charge(c).unwrap_or(self.last_q[k]);
            let i = ((q - self.last_q[k]) / dt).abs();
            self.last_q[k] = q;
            on[k] = i > 0.03;
        }
        if let Some(p) = phase_of(on) {
            if let Some(prev) = self.phase {
                let d = (p - prev).rem_euclid(8);
                match d {
                    1..=3 => self.steps += d as i64,
                    5..=7 => self.steps -= (8 - d) as i64,
                    _ => {}
                }
            }
            self.phase = Some(p);
        }
    }
    fn visual(&self) -> VisualState {
        Look::Stepper {
            steps: self.steps,
            angle_deg: (self.steps as f64 * 360.0 / 4096.0).rem_euclid(360.0),
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("half_steps".into(), self.steps as f64)]
    }
}

// ------------------------------------------------------------------------------------ relay

/// Relay module: IN (active high or low) energizes the coil after 5 ms, releases 3 ms after;
/// contacts are ext-controlled; it clicks.
struct Relay {
    input: usize,
    vcc: usize,
    active_low: bool,
    want: bool,
    on: bool,
}

const TOKEN_SWITCH: u64 = 1;

impl Relay {
    fn new(s: &DeviceSpec) -> Relay {
        Relay {
            input: pin_of(s, "IN"),
            vcc: pin_of(s, "VCC"),
            active_low: prop_bool(s, "active_low", false),
            want: false,
            on: false,
        }
    }
    fn supply(&self, ctx: &mut dyn DeviceCtx) {
        if ok(self.vcc) {
            let i = if self.on { 0.072 } else { 0.002 };
            ctx.set_drive(self.vcc, PinDrive::Current { i: -i });
        }
    }
}

impl Device for Relay {
    fn kind(&self) -> &str {
        "relay"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        if ok(self.input) {
            // the module's input draws a little current through its LED/transistor
            ctx.set_drive(self.input, PinDrive::Pull { v: 0.0, r: 10e3 });
            ctx.watch(self.input, 1.2, 2.2);
        }
        ctx.set_ext("on", 0.0);
        self.supply(ctx);
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, _pin: usize, high: bool) {
        let want = high != self.active_low;
        if want != self.want {
            self.want = want;
            ctx.cancel_wake(TOKEN_SWITCH);
            let delay = if want { 5 } else { 3 };
            ctx.wake_at(ctx.now() + SimTime::from_ms(delay), TOKEN_SWITCH);
        }
    }
    fn on_wake(&mut self, ctx: &mut dyn DeviceCtx, _token: u64) {
        let supply_ok = !ok(self.vcc) || ctx.pin_voltage(self.vcc) > 3.5;
        let on = self.want && supply_ok;
        if on != self.on {
            self.on = on;
            ctx.set_ext("on", if on { 1.0 } else { 0.0 });
            ctx.emit(DeviceEvent::Sound(Sound::Click));
            self.supply(ctx);
        }
    }
    fn visual(&self) -> VisualState {
        Look::Relay { energized: self.on }.into()
    }
}

// ------------------------------------------------------------------------------------ buzzer

/// Passive buzzer / speaker: the audio is the drive waveform itself (edge times → PCM at
/// 22.05 kHz); the dominant frequency is measured from the edges. Active buzzer: a 2.3 kHz tone
/// whenever powered.
struct Buzzer {
    active: bool,
    plus: usize,
    minus: usize,
    edges: VecDeque<(SimTime, bool)>,
    level: bool,
    freq: f64,
    loud: f32,
    chunk_start: SimTime,
    powered: bool,
    last_edge: SimTime,
}

const RATE: u32 = 22_050;
const CHUNK: SimTime = SimTime::from_ms(20);

impl Buzzer {
    fn new(s: &DeviceSpec, active: bool) -> Buzzer {
        Buzzer {
            active,
            plus: pin_of(s, "plus"),
            minus: pin_of(s, "minus"),
            edges: VecDeque::new(),
            level: false,
            freq: 0.0,
            loud: 0.0,
            chunk_start: SimTime::ZERO,
            powered: false,
            last_edge: SimTime::ZERO,
        }
    }
}

impl Device for Buzzer {
    fn kind(&self) -> &str {
        "buzzer"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        if ok(self.plus) {
            ctx.watch(self.plus, 1.0, 2.0);
        }
        self.chunk_start = ctx.now();
        ctx.tick_every(CHUNK);
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, _pin: usize, high: bool) {
        let now = ctx.now();
        if high && self.last_edge > SimTime::ZERO {
            let period = (now - self.last_edge).as_secs_f64();
            if period > 0.0 {
                let f = 1.0 / period;
                self.freq = if self.freq == 0.0 {
                    f
                } else {
                    0.7 * self.freq + 0.3 * f
                };
            }
        }
        if high {
            self.last_edge = now;
        }
        self.level = high;
        self.edges.push_back((now, high));
        if self.edges.len() > 100_000 {
            self.edges.pop_front();
        }
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let now = ctx.now();
        let start = self.chunk_start;
        self.chunk_start = now;
        let n = ((now - start).as_secs_f64() * RATE as f64).round() as usize;
        let v = |c: &dyn DeviceCtx, p: usize| if ok(p) { c.pin_voltage(p) } else { 0.0 };
        let across = v(ctx, self.plus) - v(ctx, self.minus);
        let mut samples = Vec::with_capacity(n);
        if self.active {
            self.powered = across > 2.5;
            self.freq = if self.powered { 2300.0 } else { 0.0 };
            for k in 0..n {
                let t = start.as_secs_f64() + k as f64 / RATE as f64;
                let s = if self.powered {
                    if (t * 2300.0).fract() < 0.5 {
                        0.6
                    } else {
                        -0.6
                    }
                } else {
                    0.0
                };
                samples.push(s);
            }
        } else {
            // replay the level changes inside the chunk
            let mut level = self
                .edges
                .iter()
                .rev()
                .find(|(t, _)| *t <= start)
                .map(|e| e.1)
                .unwrap_or(self.level);
            let mut it = self.edges.iter().filter(|(t, _)| *t > start).peekable();
            for k in 0..n {
                let t = start + SimTime::from_secs_f64(k as f64 / RATE as f64);
                while let Some(&&(te, lv)) = it.peek() {
                    if te <= t {
                        level = lv;
                        it.next();
                    } else {
                        break;
                    }
                }
                samples.push(if level { 0.5 } else { -0.5 });
            }
            while self.edges.len() > 2 && self.edges[1].0 <= start {
                self.edges.pop_front();
            }
            if (now - self.last_edge).as_secs_f64() > 0.05 {
                self.freq = 0.0;
            }
        }
        // remove DC so silence is silent
        let mean = samples.iter().sum::<f32>() / samples.len().max(1) as f32;
        for s in samples.iter_mut() {
            *s -= mean;
        }
        self.loud = samples.iter().map(|s| s.abs()).fold(0.0, f32::max);
        if self.loud > 1e-3 {
            ctx.emit(DeviceEvent::Audio(AudioChunk {
                start,
                sample_rate: RATE,
                samples,
            }));
        }
    }
    fn visual(&self) -> VisualState {
        Look::Buzzer {
            freq_hz: self.freq,
            level: self.loud,
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("freq_hz".into(), self.freq)]
    }
}
