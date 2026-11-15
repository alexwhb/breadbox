//! Lights, passive sensors, switches, knobs, supplies and other simple devices.

use super::{DeviceRegistry, NullDevice, ok, pin_of, prop_bool, prop_f64, prop_text};
use bb_circuit::netlist::DeviceSpec;
use bb_core::{
    Device, DeviceCtx, DeviceEvent, LogLevel, Look, PinDrive, Pos, Rgb, SimTime, UserInput,
    VisualState,
};

pub(super) fn register(r: &mut DeviceRegistry) {
    r.register_fn("led", |s| Box::new(Led::new(s)));
    r.register_fn("rgb_led", |s| Box::new(MultiLed::new(s, MultiKind::Rgb)));
    r.register_fn("seven_seg", |s| {
        Box::new(MultiLed::new(s, MultiKind::SevenSeg))
    });
    r.register_fn("led_bar", |s| Box::new(MultiLed::new(s, MultiKind::Bar)));
    r.register_fn("thermal", |s| Box::new(Thermal::new(s)));
    r.register_fn("ldr", |s| Box::new(Ldr::new(s)));
    r.register_fn("capsoil", |s| Box::new(CapSoil::new(s)));
    r.register_fn("soil_resistive", |s| Box::new(SoilResistive::new(s)));
    r.register_fn("tmp36", |s| Box::new(Tmp36::new(s)));
    r.register_fn("thermistor", |s| Box::new(Thermistor::new(s)));
    r.register_fn("switch", |s| Box::new(Switch::new(s)));
    r.register_fn("pot", |s| Box::new(Pot::new(s)));
    r.register_fn("battery", |s| Box::new(Battery::new(s)));
    r.register_fn("board_power", |s| Box::new(Polyfuse::new(s)));
    r.register_fn("lamp", |s| Box::new(Lamp::new(s)));
    r.register_fn("pir", |s| Box::new(Pir::new(s)));
    r.register_fn("hall", |s| Box::new(Hall::new(s)));
    r.register_fn("microphone", |s| Box::new(Microphone::new(s)));
    r.register_fn("pump", |s| Box::new(Pump::new(s)));
    r.register_fn("instrument", |_| Box::new(NullDevice));
}

fn pos_of(spec: &DeviceSpec) -> Pos {
    spec.pos.unwrap_or_default()
}

fn lag(v: &mut f64, target: f64, dt: f64, tau: f64) {
    if tau <= 0.0 {
        *v = target;
    } else {
        *v += (target - *v) * (1.0 - (-dt / tau).exp());
    }
}

fn led_color(name: &str) -> Rgb {
    match name {
        "amber" => Rgb::new(0xFF, 0xB0, 0x20),
        "yellow" => Rgb::new(0xFF, 0xE1, 0x35),
        "green" => Rgb::new(0x34, 0xC7, 0x59),
        "blue" => Rgb::new(0x2F, 0x7D, 0xFF),
        "white" => Rgb::new(0xF5, 0xF5, 0xFF),
        _ => Rgb::new(0xFF, 0x3B, 0x30),
    }
}

// ------------------------------------------------------------------------------------ LED

/// Luminous intensity per amp of forward current (≈ 50 mcd at 20 mA, averaged over the beam).
const CD_PER_A: f64 = 2.5;

struct Led {
    id: String,
    pos: Pos,
    color: Rgb,
    rated: f64,
    last_q: f64,
    last_t: SimTime,
    avg: f64,
}

impl Led {
    fn new(s: &DeviceSpec) -> Led {
        Led {
            id: s.part.clone(),
            pos: pos_of(s),
            color: led_color(&prop_text(s, "color", "red")),
            rated: prop_f64(s, "if_rated", 0.02),
            last_q: 0.0,
            last_t: SimTime::ZERO,
            avg: 0.0,
        }
    }
}

impl Device for Led {
    fn kind(&self) -> &str {
        "led"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        ctx.track_element("D");
        self.last_t = ctx.now();
        self.last_q = ctx.element_charge("D").unwrap_or(0.0);
        ctx.tick_every(SimTime::from_ms(10));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let q = ctx.element_charge("D").unwrap_or(0.0);
        let dt = (ctx.now() - self.last_t).as_secs_f64();
        if dt > 0.0 {
            self.avg = ((q - self.last_q) / dt).max(0.0);
        }
        self.last_q = q;
        self.last_t = ctx.now();
        let (id, pos, cd) = (self.id.clone(), self.pos, CD_PER_A * self.avg);
        if let Some(w) = ctx.world_mut() {
            w.set_light_source(&id, pos, cd);
        }
    }
    fn visual(&self) -> VisualState {
        Look::Led {
            color: self.color,
            brightness: (self.avg / self.rated).clamp(0.0, 1.5) as f32,
            current: self.avg,
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("current_a".into(), self.avg)]
    }
}

#[derive(Clone, Copy, PartialEq)]
enum MultiKind {
    Rgb,
    SevenSeg,
    Bar,
}

/// RGB LEDs, 7-segment digits and bar graphs: several diodes with their own average currents.
struct MultiLed {
    kind: MultiKind,
    elements: Vec<String>,
    last_q: Vec<f64>,
    avg: Vec<f64>,
    last_t: SimTime,
}

impl MultiLed {
    fn new(s: &DeviceSpec, kind: MultiKind) -> MultiLed {
        let elements: Vec<String> = match kind {
            MultiKind::Rgb => vec!["D:r".into(), "D:g".into(), "D:b".into()],
            MultiKind::SevenSeg => ["a", "b", "c", "d", "e", "f", "g", "dp"]
                .iter()
                .map(|x| format!("D:{x}"))
                .collect(),
            MultiKind::Bar => (1..=10).map(|i| format!("D:{i}")).collect(),
        };
        let _ = s;
        let n = elements.len();
        MultiLed {
            kind,
            elements,
            last_q: vec![0.0; n],
            avg: vec![0.0; n],
            last_t: SimTime::ZERO,
        }
    }
}

impl Device for MultiLed {
    fn kind(&self) -> &str {
        "multi_led"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        for e in &self.elements {
            ctx.track_element(e);
        }
        self.last_t = ctx.now();
        ctx.tick_every(SimTime::from_ms(10));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let dt = (ctx.now() - self.last_t).as_secs_f64();
        for (i, e) in self.elements.iter().enumerate() {
            let q = ctx.element_charge(e).unwrap_or(0.0);
            if dt > 0.0 {
                self.avg[i] = ((q - self.last_q[i]) / dt).max(0.0);
            }
            self.last_q[i] = q;
        }
        self.last_t = ctx.now();
    }
    fn visual(&self) -> VisualState {
        let b = |i: usize| (self.avg.get(i).copied().unwrap_or(0.0) / 0.02).clamp(0.0, 1.0) as f32;
        match self.kind {
            MultiKind::Rgb => Look::RgbLed {
                r: b(0),
                g: b(1),
                b: b(2),
            }
            .into(),
            MultiKind::SevenSeg => {
                let mut bits = 0u8;
                for i in 0..8 {
                    if b(i) > 0.05 {
                        bits |= 1 << i;
                    }
                }
                let lit: Vec<f32> = (0..8).map(b).filter(|x| *x > 0.05).collect();
                let brightness = if lit.is_empty() {
                    0.0
                } else {
                    lit.iter().sum::<f32>() / lit.len() as f32
                };
                Look::SevenSeg {
                    digits: vec![bits],
                    brightness,
                }
                .into()
            }
            MultiKind::Bar => Look::LedArray {
                leds: (0..10)
                    .map(|i| {
                        let k = b(i);
                        Rgb::new((255.0 * k) as u8, (40.0 * k) as u8, (30.0 * k) as u8)
                    })
                    .collect(),
            }
            .into(),
        }
    }
    fn readings(&self) -> Vec<(String, f64)> {
        self.avg
            .iter()
            .enumerate()
            .map(|(i, a)| (format!("current_{i}"), *a))
            .collect()
    }
}

// ------------------------------------------------------------------------------------ resistor self-heating

/// Resistor body temperature from its dissipation (θ ≈ 150 K/W for ¼ W parts, τ ≈ 5 s): feeds
/// the tempco expression (`ext temp`) and warms the bench around it (WORLD-3).
struct Thermal {
    id: String,
    pos: Pos,
    theta: f64,
    tau: f64,
    temp: f64,
    sent: f64,
    last_e: f64,
    last_t: SimTime,
    power: f64,
    started: bool,
}

impl Thermal {
    fn new(s: &DeviceSpec) -> Thermal {
        let rating = prop_f64(s, "power", 0.25).max(0.01);
        Thermal {
            id: s.part.clone(),
            pos: pos_of(s),
            theta: 37.5 / rating,
            tau: 5.0 * (rating / 0.25).sqrt(),
            temp: 25.0,
            sent: f64::NAN,
            last_e: 0.0,
            last_t: SimTime::ZERO,
            power: 0.0,
            started: false,
        }
    }
}

impl Device for Thermal {
    fn kind(&self) -> &str {
        "thermal"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        ctx.track_element("R");
        self.temp = ctx.world().var("air.temp_c").unwrap_or(21.0);
        self.last_t = ctx.now();
        self.last_e = ctx.element_energy("R").unwrap_or(0.0);
        ctx.set_ext("temp", self.temp);
        self.sent = self.temp;
        self.started = true;
        ctx.tick_every(SimTime::from_ms(100));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let dt = (ctx.now() - self.last_t).as_secs_f64();
        if dt <= 0.0 {
            return;
        }
        let e = ctx.element_energy("R").unwrap_or(self.last_e);
        self.power = ((e - self.last_e) / dt).max(0.0);
        self.last_e = e;
        self.last_t = ctx.now();
        // ambient = the air (its own heat must not feed back through the world)
        let amb = ctx.world().var("air.temp_c").unwrap_or(21.0);
        let target = amb + self.power * self.theta;
        lag(&mut self.temp, target, dt, self.tau);
        if (self.temp - self.sent).abs() > 0.1 {
            ctx.set_ext("temp", self.temp);
            self.sent = self.temp;
        }
        let (id, pos, p) = (self.id.clone(), self.pos, self.power);
        if let Some(w) = ctx.world_mut() {
            w.set_heat_source(&id, pos, if p > 1e-3 { p } else { 0.0 });
        }
    }
    fn set_param(&mut self, ctx: &mut dyn DeviceCtx, key: &str, value: f64) -> bool {
        match key {
            "resistance" => {
                ctx.set_ext("r", value);
                true
            }
            "power" => {
                self.theta = 37.5 / value.max(0.01);
                true
            }
            "tempco" => false,
            _ => false,
        }
    }
    fn visual(&self) -> VisualState {
        VisualState::new(Look::None)
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("temp_c".into(), self.temp), ("power_w".into(), self.power)]
    }
}

// ------------------------------------------------------------------------------------ light / soil / temperature sensors

struct Ldr {
    pos: Pos,
    lux: f64,
    sent: f64,
    last_t: SimTime,
}

impl Ldr {
    fn new(s: &DeviceSpec) -> Ldr {
        Ldr {
            pos: pos_of(s),
            lux: 0.0,
            sent: -1.0,
            last_t: SimTime::ZERO,
        }
    }
}

impl Device for Ldr {
    fn kind(&self) -> &str {
        "ldr"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        self.lux = ctx.world().lux_at(self.pos);
        ctx.set_ext("lux", self.lux);
        self.sent = self.lux;
        self.last_t = ctx.now();
        ctx.tick_every(SimTime::from_ms(10));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let dt = (ctx.now() - self.last_t).as_secs_f64();
        self.last_t = ctx.now();
        let target = ctx.world().lux_at(self.pos);
        let tau = if target > self.lux { 0.020 } else { 0.030 };
        lag(&mut self.lux, target, dt, tau);
        if (self.lux - self.sent).abs() > 0.002 * self.sent.abs().max(0.05) {
            ctx.set_ext("lux", self.lux);
            self.sent = self.lux;
        }
    }
    fn visual(&self) -> VisualState {
        Look::Readout {
            text: format!("{:.0} lx", self.lux),
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("lux".into(), self.lux)]
    }
}

/// Capacitive soil moisture sensor v1.2: VWC → output voltage (3.0 V dry → 1.4 V wet) with a
/// response time constant and ADC-visible noise.
struct CapSoil {
    pot: Option<String>,
    tau: f64,
    noise_lsb: f64,
    v_dry: f64,
    slope: f64,
    vwc: f64,
    v: f64,
    last_t: SimTime,
    vcc: usize,
    aout: usize,
}

impl CapSoil {
    fn new(s: &DeviceSpec) -> CapSoil {
        CapSoil {
            pot: s.attach.get("pot").cloned(),
            tau: prop_f64(s, "tau", 1.8),
            noise_lsb: prop_f64(s, "noise_lsb", 4.0),
            v_dry: prop_f64(s, "v_dry", 3.03),
            slope: prop_f64(s, "slope", -0.02737),
            vwc: 0.0,
            v: 0.0,
            last_t: SimTime::ZERO,
            vcc: pin_of(s, "VCC"),
            aout: pin_of(s, "AOUT"),
        }
    }

    fn soil(&self, ctx: &dyn DeviceCtx) -> f64 {
        match &self.pot {
            Some(p) => ctx.world().soil_vwc(p).unwrap_or(0.0),
            None => ctx.world().soil_vwc("pot1").unwrap_or(0.0),
        }
    }

    fn output(&mut self, ctx: &mut dyn DeviceCtx) {
        let supply = if ok(self.vcc) {
            ctx.pin_voltage(self.vcc)
        } else {
            5.0
        };
        let mut v = if supply < 3.0 {
            0.0
        } else {
            (self.v_dry + self.slope * self.vwc).clamp(0.0, supply)
        };
        if ctx.world().noise_enabled() && self.noise_lsb > 0.0 {
            let lsb = 5.0 / 1024.0;
            v += ctx.rng().gauss() * self.noise_lsb * lsb / 2.0;
        }
        self.v = v.max(0.0);
        if ok(self.aout) {
            // v1.2 board: 2.2 kΩ output resistor (a 10 MΩ meter shifts A0 by −0.4 mV)
            ctx.set_drive(
                self.aout,
                PinDrive::Thevenin {
                    v: self.v,
                    r: 2.2e3,
                },
            );
        }
    }
}

impl Device for CapSoil {
    fn kind(&self) -> &str {
        "capsoil"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        self.vwc = self.soil(ctx);
        self.last_t = ctx.now();
        if ok(self.vcc) {
            ctx.set_drive(self.vcc, PinDrive::Current { i: -0.005 });
        }
        self.output(ctx);
        ctx.tick_every(SimTime::from_ms(20));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let dt = (ctx.now() - self.last_t).as_secs_f64();
        self.last_t = ctx.now();
        let target = self.soil(ctx);
        lag(&mut self.vwc, target, dt, self.tau);
        self.output(ctx);
    }
    fn set_param(&mut self, _ctx: &mut dyn DeviceCtx, key: &str, value: f64) -> bool {
        match key {
            "tau" => self.tau = value,
            "noise_lsb" => self.noise_lsb = value,
            _ => return false,
        }
        true
    }
    fn visual(&self) -> VisualState {
        Look::Readout {
            text: format!("{:.2} V", self.v),
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("vwc".into(), self.vwc), ("vout".into(), self.v)]
    }
}

struct SoilResistive {
    pot: Option<String>,
    tau: f64,
    vwc: f64,
    sent: f64,
    last_t: SimTime,
}

impl SoilResistive {
    fn new(s: &DeviceSpec) -> SoilResistive {
        SoilResistive {
            pot: s.attach.get("pot").cloned(),
            tau: prop_f64(s, "tau", 3.0),
            vwc: 0.0,
            sent: -1.0,
            last_t: SimTime::ZERO,
        }
    }
}

impl Device for SoilResistive {
    fn kind(&self) -> &str {
        "soil_resistive"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        let pot = self.pot.clone().unwrap_or_else(|| "pot1".into());
        self.vwc = ctx.world().soil_vwc(&pot).unwrap_or(0.0);
        ctx.set_ext("vwc", self.vwc);
        self.sent = self.vwc;
        self.last_t = ctx.now();
        ctx.tick_every(SimTime::from_ms(50));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let dt = (ctx.now() - self.last_t).as_secs_f64();
        self.last_t = ctx.now();
        let pot = self.pot.clone().unwrap_or_else(|| "pot1".into());
        let target = ctx.world().soil_vwc(&pot).unwrap_or(0.0);
        lag(&mut self.vwc, target, dt, self.tau);
        if (self.vwc - self.sent).abs() > 0.02 {
            ctx.set_ext("vwc", self.vwc);
            self.sent = self.vwc;
        }
    }
    fn visual(&self) -> VisualState {
        VisualState::new(Look::None)
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("vwc".into(), self.vwc)]
    }
}

/// TMP36: 0.5 V + 10 mV/°C of its own (lagging) temperature.
struct Tmp36 {
    pos: Pos,
    tau: f64,
    temp: f64,
    v: f64,
    last_t: SimTime,
    vs: usize,
    vout: usize,
}

impl Tmp36 {
    fn new(s: &DeviceSpec) -> Tmp36 {
        Tmp36 {
            pos: pos_of(s),
            tau: prop_f64(s, "tau", 8.0),
            temp: 21.0,
            v: 0.0,
            last_t: SimTime::ZERO,
            vs: pin_of(s, "VS"),
            vout: pin_of(s, "VOUT"),
        }
    }
    fn output(&mut self, ctx: &mut dyn DeviceCtx) {
        let supply = if ok(self.vs) {
            ctx.pin_voltage(self.vs)
        } else {
            5.0
        };
        let mut v = if supply < 2.7 {
            0.0
        } else {
            0.5 + 0.01 * self.temp
        };
        if ctx.world().noise_enabled() {
            v += ctx.rng().gauss() * 0.0005;
        }
        self.v = v.clamp(0.0, supply.max(0.0));
        if ok(self.vout) {
            ctx.set_drive(self.vout, PinDrive::Thevenin { v: self.v, r: 10.0 });
        }
    }
}

impl Device for Tmp36 {
    fn kind(&self) -> &str {
        "tmp36"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        self.temp = ctx.world().temperature_at(self.pos);
        self.last_t = ctx.now();
        if ok(self.vs) {
            ctx.set_drive(self.vs, PinDrive::Current { i: -50e-6 });
        }
        self.output(ctx);
        ctx.tick_every(SimTime::from_ms(100));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let dt = (ctx.now() - self.last_t).as_secs_f64();
        self.last_t = ctx.now();
        let target = ctx.world().temperature_at(self.pos);
        lag(&mut self.temp, target, dt, self.tau);
        self.output(ctx);
    }
    fn visual(&self) -> VisualState {
        Look::Readout {
            text: format!("{:.1} °C", self.temp),
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("temp_c".into(), self.temp), ("vout".into(), self.v)]
    }
}

struct Thermistor {
    pos: Pos,
    tau: f64,
    temp: f64,
    sent: f64,
    last_t: SimTime,
}

impl Thermistor {
    fn new(s: &DeviceSpec) -> Thermistor {
        Thermistor {
            pos: pos_of(s),
            tau: prop_f64(s, "tau", 5.0),
            temp: 21.0,
            sent: f64::NAN,
            last_t: SimTime::ZERO,
        }
    }
}

impl Device for Thermistor {
    fn kind(&self) -> &str {
        "thermistor"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        self.temp = ctx.world().temperature_at(self.pos);
        ctx.set_ext("temp", self.temp);
        self.sent = self.temp;
        self.last_t = ctx.now();
        ctx.tick_every(SimTime::from_ms(100));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let dt = (ctx.now() - self.last_t).as_secs_f64();
        self.last_t = ctx.now();
        let target = ctx.world().temperature_at(self.pos);
        lag(&mut self.temp, target, dt, self.tau);
        if (self.temp - self.sent).abs() > 0.02 {
            ctx.set_ext("temp", self.temp);
            self.sent = self.temp;
        }
    }
    fn visual(&self) -> VisualState {
        VisualState::new(Look::None)
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("temp_c".into(), self.temp)]
    }
}

// ------------------------------------------------------------------------------------ switches, knobs

#[derive(Clone, Copy, PartialEq)]
enum SwitchKind {
    Button,
    Slide,
    Dip,
    Tilt,
}

/// Push buttons (with contact bounce), slide/DIP/tilt switches: contacts are ext-controlled
/// resistances in the part's SPICE template.
struct Switch {
    kind: SwitchKind,
    on: Vec<bool>,
    actual: bool,
    target: bool,
    bounce_ms: f64,
    flips: u32,
}

const TOKEN_BOUNCE: u64 = 1;
const TOKEN_SETTLE: u64 = 2;

impl Switch {
    fn new(s: &DeviceSpec) -> Switch {
        let kind = match s.def.as_str() {
            "slide_switch" => SwitchKind::Slide,
            "dip_switch" => SwitchKind::Dip,
            "tilt_switch" => SwitchKind::Tilt,
            _ => SwitchKind::Button,
        };
        let on = match kind {
            SwitchKind::Dip => vec![false; 4],
            SwitchKind::Slide => vec![prop_bool(s, "on", false)],
            SwitchKind::Tilt => vec![!prop_bool(s, "tilted", false)],
            SwitchKind::Button => vec![false],
        };
        Switch {
            kind,
            actual: on[0],
            target: on[0],
            on,
            bounce_ms: prop_f64(s, "bounce_ms", 2.0),
            flips: 0,
        }
    }
    fn ext_name(i: usize, kind: SwitchKind) -> String {
        if kind == SwitchKind::Dip {
            format!("s{i}")
        } else {
            "closed".into()
        }
    }
    fn apply(&self, ctx: &mut dyn DeviceCtx, i: usize, closed: bool) {
        ctx.set_ext(
            &Self::ext_name(i, self.kind),
            if closed { 1.0 } else { 0.0 },
        );
    }
}

impl Device for Switch {
    fn kind(&self) -> &str {
        "switch"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        for (i, on) in self.on.clone().into_iter().enumerate() {
            self.apply(ctx, i, on);
        }
    }
    fn on_input(&mut self, ctx: &mut dyn DeviceCtx, input: &UserInput) {
        match (self.kind, input) {
            (SwitchKind::Button, UserInput::Press(p)) => {
                self.target = *p;
                self.on[0] = *p;
                // contact bounce: a few random flips, then the final state
                if self.bounce_ms > 0.0 {
                    ctx.cancel_wake(TOKEN_BOUNCE);
                    ctx.cancel_wake(TOKEN_SETTLE);
                    self.actual = *p;
                    self.apply(ctx, 0, *p);
                    let n = 2 + ctx.rng().below(4) as u32;
                    let span = self.bounce_ms * 1e-3;
                    let mut ts: Vec<f64> = (0..n * 2).map(|_| ctx.rng().range(0.0, span)).collect();
                    ts.sort_by(f64::total_cmp);
                    self.flips = ts.len() as u32;
                    for t in ts {
                        ctx.wake_at(ctx.now() + SimTime::from_secs_f64(t), TOKEN_BOUNCE);
                    }
                    ctx.wake_at(
                        ctx.now() + SimTime::from_secs_f64(span * 1.05),
                        TOKEN_SETTLE,
                    );
                } else {
                    self.actual = *p;
                    self.apply(ctx, 0, *p);
                }
            }
            (_, UserInput::Toggle { index }) => {
                let i = (*index).min(self.on.len() - 1);
                self.on[i] = !self.on[i];
                self.apply(ctx, i, self.on[i]);
            }
            (_, UserInput::SetSwitch { index, on }) => {
                let i = (*index).min(self.on.len() - 1);
                self.on[i] = *on;
                self.apply(ctx, i, *on);
            }
            (SwitchKind::Button, UserInput::Value { key, value }) if key == "bounce_ms" => {
                self.bounce_ms = *value;
            }
            _ => {}
        }
    }
    fn on_wake(&mut self, ctx: &mut dyn DeviceCtx, token: u64) {
        match token {
            TOKEN_BOUNCE => {
                self.actual = !self.actual;
                self.apply(ctx, 0, self.actual);
            }
            TOKEN_SETTLE => {
                self.actual = self.target;
                self.apply(ctx, 0, self.target);
            }
            _ => {}
        }
    }
    fn set_param(&mut self, ctx: &mut dyn DeviceCtx, key: &str, value: f64) -> bool {
        match key {
            "bounce_ms" => self.bounce_ms = value,
            "on" => {
                self.on[0] = value != 0.0;
                self.apply(ctx, 0, self.on[0]);
            }
            "tilted" => {
                self.on[0] = value == 0.0;
                self.apply(ctx, 0, self.on[0]);
            }
            _ => return false,
        }
        true
    }
    fn visual(&self) -> VisualState {
        if self.kind == SwitchKind::Dip {
            Look::Switches {
                on: self.on.clone(),
            }
            .into()
        } else {
            Look::Switch { on: self.on[0] }.into()
        }
    }
}

struct Pot {
    pos: f64,
}

impl Pot {
    fn new(s: &DeviceSpec) -> Pot {
        Pot {
            pos: prop_f64(s, "position", 0.5).clamp(0.0, 1.0),
        }
    }
}

impl Device for Pot {
    fn kind(&self) -> &str {
        "pot"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        ctx.set_ext("pos", self.pos);
    }
    fn on_input(&mut self, ctx: &mut dyn DeviceCtx, input: &UserInput) {
        if let UserInput::Position(p) = input {
            self.pos = p.clamp(0.0, 1.0);
            ctx.set_ext("pos", self.pos);
        }
    }
    fn set_param(&mut self, ctx: &mut dyn DeviceCtx, key: &str, value: f64) -> bool {
        if key == "position" {
            self.pos = value.clamp(0.0, 1.0);
            ctx.set_ext("pos", self.pos);
            true
        } else {
            false
        }
    }
    fn visual(&self) -> VisualState {
        Look::Knob { position: self.pos }.into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("position".into(), self.pos)]
    }
}

// ------------------------------------------------------------------------------------ supplies

/// Battery pack with a discharge curve and rising internal resistance (SIM-6).
struct Battery {
    kind: String,
    cells: f64,
    capacity_as: f64,
    soc: f64,
    rint0: f64,
    last_q: f64,
    emf: f64,
}

impl Battery {
    fn new(s: &DeviceSpec) -> Battery {
        let kind = prop_text(s, "kind", "aa4");
        let cells = match kind.as_str() {
            "aa2" => 2.0,
            "aa4" => 4.0,
            "9v" => 6.0,
            _ => 1.0,
        };
        Battery {
            cells,
            capacity_as: prop_f64(s, "capacity_mah", 2500.0) * 3.6,
            soc: prop_f64(s, "charge", 100.0) / 100.0,
            rint0: prop_f64(s, "rint", 0.6),
            last_q: 0.0,
            emf: 0.0,
            kind,
        }
    }
    fn ocv(&self) -> f64 {
        let s = self.soc.clamp(0.0, 1.0);
        let cell = if self.kind == "lipo1s" {
            let table = [
                (0.0, 3.0),
                (0.05, 3.45),
                (0.1, 3.6),
                (0.3, 3.72),
                (0.6, 3.87),
                (0.9, 4.07),
                (1.0, 4.2),
            ];
            let mut v = table[0].1;
            for w in table.windows(2) {
                if s >= w[0].0 && s <= w[1].0 {
                    v = w[0].1 + (w[1].1 - w[0].1) * (s - w[0].0) / (w[1].0 - w[0].0);
                }
            }
            v
        } else {
            // alkaline: ~1.6 V fresh, sloping to 1.0 V
            1.0 + 0.45 * s + 0.15 * s.powi(4)
        };
        cell * self.cells
    }
    fn update(&mut self, ctx: &mut dyn DeviceCtx) {
        self.emf = self.ocv();
        ctx.set_ext("emf", self.emf);
        let r = self.rint0 * (1.0 + 2.0 * (1.0 - self.soc).powi(2));
        ctx.set_ext("rint", r);
    }
}

impl Device for Battery {
    fn kind(&self) -> &str {
        "battery"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        ctx.track_element("V");
        self.last_q = ctx.element_charge("V").unwrap_or(0.0);
        self.update(ctx);
        ctx.tick_every(SimTime::from_ms(100));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let q = ctx.element_charge("V").unwrap_or(self.last_q);
        // SPICE: current into + terminal is positive when charging
        let drawn = -(q - self.last_q);
        self.last_q = q;
        let before = self.soc;
        self.soc = (self.soc - drawn / self.capacity_as).clamp(0.0, 1.0);
        if (before - self.soc).abs() > 1e-6 || self.soc == 0.0 {
            self.update(ctx);
        }
        if before > 0.05 && self.soc <= 0.05 {
            ctx.emit(DeviceEvent::Log {
                level: LogLevel::Warn,
                text: "battery nearly flat".into(),
            });
        }
    }
    fn set_param(&mut self, ctx: &mut dyn DeviceCtx, key: &str, value: f64) -> bool {
        if key == "charge" {
            self.soc = (value / 100.0).clamp(0.0, 1.0);
            self.update(ctx);
            true
        } else {
            false
        }
    }
    fn visual(&self) -> VisualState {
        Look::Readout {
            text: format!("{:.2} V · {:.0} %", self.emf, self.soc * 100.0),
        }
        .into()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![("soc".into(), self.soc * 100.0), ("emf".into(), self.emf)]
    }
}

/// USB 500 mA polyfuse (PTC): trips on overload (I²t above the hold current), stays tripped
/// while the fault draws current, resets once it cools (SIM-6, WB-13).
struct Polyfuse {
    hold: f64,
    k: f64,
    energy: f64,
    tripped: bool,
    low_since: Option<SimTime>,
    last_q: f64,
    last_t: SimTime,
    current: f64,
}

const FUSE_OK: f64 = 0.1;
const FUSE_TRIPPED: f64 = 50.0;

impl Polyfuse {
    fn new(_s: &DeviceSpec) -> Polyfuse {
        Polyfuse {
            hold: 0.5,
            k: 0.75,
            energy: 0.0,
            tripped: false,
            low_since: None,
            last_q: 0.0,
            last_t: SimTime::ZERO,
            current: 0.0,
        }
    }
}

impl Device for Polyfuse {
    fn kind(&self) -> &str {
        "polyfuse"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        ctx.track_element("R:fuse");
        self.last_q = ctx.element_charge("R:fuse").unwrap_or(0.0);
        self.last_t = ctx.now();
        ctx.set_ext("fuse_r", FUSE_OK);
        ctx.tick_every(SimTime::from_ms(2));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let dt = (ctx.now() - self.last_t).as_secs_f64();
        if dt <= 0.0 {
            return;
        }
        let q = ctx.element_charge("R:fuse").unwrap_or(self.last_q);
        let i = ((q - self.last_q) / dt).abs();
        self.current = i;
        self.last_q = q;
        self.last_t = ctx.now();
        if !self.tripped {
            let over = i * i - self.hold * self.hold;
            if over > 0.0 {
                self.energy += over * dt;
            } else {
                self.energy *= (-dt / 2.0f64).exp();
            }
            if self.energy > self.k {
                self.tripped = true;
                ctx.set_ext("fuse_r", FUSE_TRIPPED);
                let msg = format!(
                    "USB polyfuse tripped ({}) — 5 V rail collapsed",
                    bb_core::si::format(i, "A")
                );
                ctx.emit(DeviceEvent::Stress {
                    reason: msg.clone(),
                });
                ctx.emit(DeviceEvent::Log {
                    level: LogLevel::Warn,
                    text: msg,
                });
            }
        } else if i < 0.06 {
            let since = *self.low_since.get_or_insert(ctx.now());
            if (ctx.now() - since).as_secs_f64() > 1.0 {
                self.tripped = false;
                self.energy = 0.0;
                self.low_since = None;
                ctx.set_ext("fuse_r", FUSE_OK);
                ctx.emit(DeviceEvent::Log {
                    level: LogLevel::Info,
                    text: "USB polyfuse cooled down and reset".into(),
                });
            }
        } else {
            self.low_since = None;
        }
    }
    fn visual(&self) -> VisualState {
        VisualState::new(Look::None)
    }
    fn readings(&self) -> Vec<(String, f64)> {
        vec![
            ("usb_current_a".into(), self.current),
            ("fuse_tripped".into(), self.tripped as u8 as f64),
        ]
    }
}

// ------------------------------------------------------------------------------------ world objects / misc

struct Lamp {
    id: String,
    pos: Pos,
    candela: f64,
    on: bool,
}

impl Lamp {
    fn new(s: &DeviceSpec) -> Lamp {
        Lamp {
            id: s.part.clone(),
            pos: pos_of(s),
            candela: prop_f64(s, "candela", 60.0),
            on: prop_bool(s, "on", true),
        }
    }
    fn apply(&self, ctx: &mut dyn DeviceCtx) {
        let cd = if self.on { self.candela } else { 0.0 };
        let (id, pos) = (self.id.clone(), self.pos);
        if let Some(w) = ctx.world_mut() {
            w.set_light_source(&id, pos, cd);
        }
    }
}

impl Device for Lamp {
    fn kind(&self) -> &str {
        "lamp"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        self.apply(ctx);
    }
    fn on_input(&mut self, ctx: &mut dyn DeviceCtx, input: &UserInput) {
        match input {
            UserInput::Toggle { .. } | UserInput::Press(true) => self.on = !self.on,
            UserInput::SetSwitch { on, .. } => self.on = *on,
            UserInput::Position(p) => self.candela = p * 2000.0,
            _ => {}
        }
        self.apply(ctx);
    }
    fn set_param(&mut self, ctx: &mut dyn DeviceCtx, key: &str, value: f64) -> bool {
        match key {
            "candela" => self.candela = value,
            "on" => self.on = value != 0.0,
            _ => return false,
        }
        self.apply(ctx);
        true
    }
    fn visual(&self) -> VisualState {
        Look::Switch { on: self.on }.into()
    }
}

struct Pir {
    pos: Pos,
    range: f64,
    hold: f64,
    warmup: f64,
    high_until: SimTime,
    high: bool,
    out: usize,
    vcc: usize,
}

impl Pir {
    fn new(s: &DeviceSpec) -> Pir {
        Pir {
            pos: pos_of(s),
            range: prop_f64(s, "range_m", 3.0),
            hold: prop_f64(s, "hold_s", 3.0),
            warmup: prop_f64(s, "warmup_s", 1.0),
            high_until: SimTime::ZERO,
            high: false,
            out: pin_of(s, "OUT"),
            vcc: pin_of(s, "VCC"),
        }
    }
    fn drive(&self, ctx: &mut dyn DeviceCtx) {
        let v = if self.high { 3.3 } else { 0.0 };
        if ok(self.out) {
            ctx.set_drive(self.out, PinDrive::Thevenin { v, r: 100.0 });
        }
    }
}

impl Device for Pir {
    fn kind(&self) -> &str {
        "pir"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        if ok(self.vcc) {
            ctx.set_drive(self.vcc, PinDrive::Current { i: -65e-6 });
        }
        self.drive(ctx);
        ctx.tick_every(SimTime::from_ms(50));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let now = ctx.now();
        let powered = !ok(self.vcc) || ctx.pin_voltage(self.vcc) > 4.0;
        if powered
            && now.as_secs_f64() >= self.warmup
            && ctx.world().motion(self.pos, self.range, 0.25)
        {
            self.high_until = now + SimTime::from_secs_f64(self.hold);
        }
        let high = powered && now < self.high_until;
        if high != self.high {
            self.high = high;
            self.drive(ctx);
        }
    }
    fn visual(&self) -> VisualState {
        Look::Switch { on: self.high }.into()
    }
}

struct Hall {
    pos: Pos,
    on: bool,
    out: usize,
    vcc: usize,
}

impl Hall {
    fn new(s: &DeviceSpec) -> Hall {
        Hall {
            pos: pos_of(s),
            on: false,
            out: pin_of(s, "OUT"),
            vcc: pin_of(s, "VCC"),
        }
    }
}

impl Device for Hall {
    fn kind(&self) -> &str {
        "hall"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        if ok(self.vcc) {
            ctx.set_drive(self.vcc, PinDrive::Current { i: -4e-3 });
        }
        ctx.tick_every(SimTime::from_ms(5));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let b = ctx.world().magnetic_field(self.pos);
        let on = if self.on { b > 15.0 } else { b > 25.0 };
        if on != self.on {
            self.on = on;
            if ok(self.out) {
                let d = if on {
                    PinDrive::Thevenin { v: 0.0, r: 20.0 }
                } else {
                    PinDrive::HighZ
                };
                ctx.set_drive(self.out, d);
            }
        }
    }
    fn visual(&self) -> VisualState {
        Look::Switch { on: self.on }.into()
    }
}

struct Microphone {
    gain: f64,
    phase: f64,
    out: usize,
    vcc: usize,
}

impl Microphone {
    fn new(s: &DeviceSpec) -> Microphone {
        Microphone {
            gain: prop_f64(s, "gain", 1.0),
            phase: 0.0,
            out: pin_of(s, "OUT"),
            vcc: pin_of(s, "VCC"),
        }
    }
}

impl Device for Microphone {
    fn kind(&self) -> &str {
        "microphone"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        ctx.tick_every(SimTime::from_us(250));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let vcc = if ok(self.vcc) {
            ctx.pin_voltage(self.vcc)
        } else {
            5.0
        };
        let db = ctx.world().var("sound.db").unwrap_or(40.0);
        let amp = (self.gain * 1e-3 * 10f64.powf((db - 40.0) / 20.0)).min(vcc / 2.0);
        self.phase += 2.0 * std::f64::consts::PI * 440.0 * 250e-6;
        let noise = if ctx.world().noise_enabled() {
            ctx.rng().gauss() * 0.3 * amp
        } else {
            0.0
        };
        let v = (vcc / 2.0 + amp * self.phase.sin() + noise).clamp(0.0, vcc);
        if ok(self.out) {
            ctx.set_drive(self.out, PinDrive::Thevenin { v, r: 100.0 });
        }
    }
    fn visual(&self) -> VisualState {
        VisualState::new(Look::None)
    }
}

/// A pump waters its pot while current flows (WORLD-1 "firmware-driven pump").
struct Pump {
    pot: String,
    flow: f64,
    current: f64,
    last_q: f64,
    last_t: SimTime,
    angle: f64,
}

impl Pump {
    fn new(s: &DeviceSpec) -> Pump {
        Pump {
            pot: s
                .attach
                .get("pot")
                .cloned()
                .unwrap_or_else(|| prop_text(s, "pot", "pot1")),
            flow: prop_f64(s, "flow_ml_s", 20.0),
            current: 0.0,
            last_q: 0.0,
            last_t: SimTime::ZERO,
            angle: 0.0,
        }
    }
}

impl Device for Pump {
    fn kind(&self) -> &str {
        "pump"
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        ctx.track_element("R");
        self.last_t = ctx.now();
        ctx.tick_every(SimTime::from_ms(100));
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        let dt = (ctx.now() - self.last_t).as_secs_f64();
        if dt <= 0.0 {
            return;
        }
        let q = ctx.element_charge("R").unwrap_or(self.last_q);
        self.current = ((q - self.last_q) / dt).abs();
        self.last_q = q;
        self.last_t = ctx.now();
        let volts = self.current * 25.0;
        if self.current > 0.05 {
            let ml = self.flow * (volts / 5.0).min(1.5) * dt;
            let pot = self.pot.clone();
            if let Some(w) = ctx.world_mut() {
                w.water(&pot, ml);
            }
            self.angle = (self.angle + 360.0 * 50.0 * dt) % 360.0;
        }
    }
    fn visual(&self) -> VisualState {
        Look::Motor {
            rpm: self.current / 0.2 * 3000.0,
            angle_deg: self.angle,
        }
        .into()
    }
}
