//! # bb-plugin-sdk — write Breadbox plugins in Rust
//!
//! Compile to `wasm32-unknown-unknown` as a `cdylib`. The crate is `#![no_std]` (it needs
//! `alloc`); `std` plugins work unchanged, `no_std` plugins enable the `global-allocator`
//! feature. The ABI underneath is documented in `docs/PLUGIN-SDK.md` for other languages.
//!
//! ```ignore
//! use bb_plugin_sdk::prelude::*;
//!
//! struct Blinker { led: usize, on: bool }
//!
//! impl Part for Blinker {
//!     fn init(cfg: &Config) -> Result<Self, String> {
//!         Ok(Blinker { led: cfg.pin("OUT").ok_or("no OUT pin")?, on: false })
//!     }
//!     fn on_start(&mut self, ctx: &Ctx) { ctx.tick_every(0.5); }
//!     fn on_tick(&mut self, ctx: &Ctx) {
//!         self.on = !self.on;
//!         ctx.set_drive(self.led, Drive::Thevenin { v: if self.on { 5.0 } else { 0.0 }, r: 25.0 });
//!     }
//! }
//!
//! export_part!(Blinker);
//! ```

#![no_std]

extern crate alloc;

#[cfg(feature = "global-allocator")]
#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

pub mod panel;
pub mod sys;

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

pub use serde_json;
pub use serde_json::{Value, json};

/// The ABI version these exports implement.
pub const ABI_VERSION: i32 = 1;

/// Picoseconds per second (host times are integer picoseconds).
pub const PS: f64 = 1e12;

/// Prelude for plugin crates.
pub mod prelude {
    pub use crate::panel::{DisplayState, Marker, PanelState, ScreenState, Trace};
    pub use crate::{
        Analysis, AnalysisInput, Config, Ctx, Drive, Generator, Input, Level, Part, PartInfo,
        RadioExtension, WorldModel, export_analysis, export_generator, export_part, export_radio,
        export_world_model, json,
    };
    pub use alloc::format;
    pub use alloc::string::{String, ToString};
    pub use alloc::vec;
    pub use alloc::vec::Vec;
}

/// The part a device model runs for (`Config::part`).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PartInfo {
    pub id: String,
    pub reference: String,
    pub def: String,
    #[serde(default)]
    pub behaviour: String,
    /// Pin ids in index order (the `pin` arguments of every call).
    pub pins: Vec<String>,
    /// Effective properties.
    #[serde(default)]
    pub props: Value,
}

impl PartInfo {
    /// Index of a pin by id.
    pub fn pin(&self, id: &str) -> Option<usize> {
        self.pins.iter().position(|p| p == id)
    }
    /// A numeric property.
    pub fn prop(&self, key: &str) -> Option<f64> {
        let v = self.props.get(key)?;
        v.as_f64()
            .or_else(|| v.get("Num").and_then(|x| x.as_f64()))
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    }
}

/// What `bb_init` receives.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Config {
    pub plugin: String,
    pub version: String,
    /// `device`, `world-model`, `analysis`, `generator`, `radio`.
    pub role: String,
    #[serde(default)]
    pub part: Option<PartInfo>,
    /// The manifest's `specs`.
    #[serde(default)]
    pub specs: Value,
    /// The project's world settings (world models).
    #[serde(default)]
    pub world: Value,
    #[serde(default)]
    pub permissions: Vec<String>,
}

impl Config {
    /// Pin index of the part this instance runs for.
    pub fn pin(&self, id: &str) -> Option<usize> {
        self.part.as_ref()?.pin(id)
    }
    /// A numeric spec (`specs.counts`).
    pub fn spec_f64(&self, key: &str) -> Option<f64> {
        self.specs.get(key)?.as_f64()
    }
}

/// A user interaction (`bb_on_input`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Input {
    Press {
        on: bool,
    },
    Toggle {
        index: usize,
    },
    Switch {
        index: usize,
        on: bool,
    },
    Position {
        value: f64,
    },
    /// Panel controls: rotary/switch index, knob value, button press (1/0).
    Value {
        key: String,
        value: f64,
    },
}

/// How a pin is driven.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Drive {
    HighZ,
    /// Push-pull: `v` volts behind `r` ohms.
    Thevenin {
        v: f64,
        r: f64,
    },
    /// Weak pull to `v` through `r`.
    Pull {
        v: f64,
        r: f64,
    },
    /// Current into the node (A).
    Current {
        i: f64,
    },
}

/// Log levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

/// Input to an analysis (`bb_analysis_run`).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AnalysisInput {
    pub netlist: String,
    #[serde(default)]
    pub options: Value,
}

/// Handle to the host for the current call. All methods are cheap; reads fail (NaN / `None`)
/// rather than panicking, and calls the plugin lacks the permission for stop it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ctx;

fn ps(seconds: f64) -> i64 {
    (seconds.max(0.0) * PS) as i64
}

fn finite(x: f64) -> Option<f64> {
    if x.is_nan() { None } else { Some(x) }
}

impl Ctx {
    // ---- time
    /// Simulated time (s).
    pub fn now(&self) -> f64 {
        unsafe { sys::bb_now_s() }
    }
    /// Simulated time (ps).
    pub fn now_ps(&self) -> i64 {
        unsafe { sys::bb_now() }
    }
    /// `on_wake(token)` at absolute time `t` (s).
    pub fn wake_at(&self, t: f64, token: u64) {
        unsafe { sys::bb_wake_at(ps(t), token as i64) }
    }
    /// `on_wake(token)` after `dt` seconds.
    pub fn wake_in(&self, dt: f64, token: u64) {
        let t = self.now_ps() + ps(dt);
        unsafe { sys::bb_wake_at(t, token as i64) }
    }
    pub fn cancel_wake(&self, token: u64) {
        unsafe { sys::bb_cancel_wake(token as i64) }
    }
    /// `on_tick` every `period` seconds (0 stops).
    pub fn tick_every(&self, period: f64) {
        unsafe { sys::bb_tick_every(ps(period)) }
    }
    /// Deterministic random number in [0, 1).
    pub fn rng(&self) -> f64 {
        unsafe { sys::bb_rng() }
    }
    /// Standard normal random number.
    pub fn gauss(&self) -> f64 {
        let u1 = self.rng().max(1e-12);
        let u2 = self.rng();
        libm_sqrt(-2.0 * libm_ln(u1)) * libm_cos(2.0 * core::f64::consts::PI * u2)
    }

    // ---- circuit (read:nets)
    pub fn pin_voltage(&self, pin: usize) -> f64 {
        unsafe { sys::bb_pin_voltage(pin as i32) }
    }
    pub fn pin_current(&self, pin: usize) -> f64 {
        unsafe { sys::bb_pin_current(pin as i32) }
    }
    /// Current through one of the part's SPICE elements (local name, e.g. `"R:in"`).
    pub fn element_current(&self, name: &str) -> Option<f64> {
        finite(unsafe { sys::bb_element_current(name.as_ptr() as i32, name.len() as i32) })
    }
    /// ∫ i dt of a tracked element (C).
    pub fn element_charge(&self, name: &str) -> Option<f64> {
        finite(unsafe { sys::bb_element_charge(name.as_ptr() as i32, name.len() as i32) })
    }
    /// ∫ v·i dt of a tracked element (J).
    pub fn element_energy(&self, name: &str) -> Option<f64> {
        finite(unsafe { sys::bb_element_energy(name.as_ptr() as i32, name.len() as i32) })
    }
    /// Starts integrating an element's charge and energy.
    pub fn track_element(&self, name: &str) {
        unsafe { sys::bb_track_element(name.as_ptr() as i32, name.len() as i32) }
    }
    /// Schmitt edges on `pin` → `on_edge`.
    pub fn watch(&self, pin: usize, lo: f64, hi: f64) {
        unsafe { sys::bb_watch(pin as i32, lo, hi) }
    }
    /// Records `pin` at least every `period` seconds (0 stops).
    pub fn capture(&self, pin: usize, period: f64) {
        unsafe { sys::bb_capture(pin as i32, ps(period)) }
    }
    /// Captured `(t, volts)` samples between `from` and `to` (s), at most `max` (newest kept).
    pub fn samples(&self, pin: usize, from: f64, to: f64, max: usize) -> Vec<(f64, f64)> {
        let mut buf = vec![0f64; max * 2];
        let n = unsafe {
            sys::bb_samples(
                pin as i32,
                ps(from),
                ps(to),
                buf.as_mut_ptr() as i32,
                max as i32,
            )
        };
        (0..n.max(0) as usize)
            .map(|i| (buf[2 * i], buf[2 * i + 1]))
            .collect()
    }

    // ---- driving (drive:pins / inject:load)
    pub fn set_drive(&self, pin: usize, d: Drive) {
        let (k, a, b) = match d {
            Drive::HighZ => (0, 0.0, 0.0),
            Drive::Thevenin { v, r } => (1, v, r),
            Drive::Pull { v, r } => (2, v, r),
            Drive::Current { i } => (3, i, 0.0),
        };
        unsafe { sys::bb_set_drive(pin as i32, k, a, b) }
    }
    /// Sets an external variable of the part's own SPICE template (`$(ext:name)`).
    pub fn set_ext(&self, name: &str, value: f64) {
        unsafe { sys::bb_set_ext(name.as_ptr() as i32, name.len() as i32, value) }
    }

    // ---- world (world:read / world:write)
    pub fn world_get(&self, key: &str) -> Option<f64> {
        finite(unsafe { sys::bb_world_get(key.as_ptr() as i32, key.len() as i32) })
    }
    pub fn world_set(&self, key: &str, value: f64) {
        unsafe { sys::bb_world_set(key.as_ptr() as i32, key.len() as i32, value) }
    }
    pub fn water(&self, pot: &str, ml: f64) {
        unsafe { sys::bb_world_water(pot.as_ptr() as i32, pot.len() as i32, ml) }
    }
    /// World clock (s since the world started, time-warped).
    pub fn world_secs(&self) -> f64 {
        unsafe { sys::bb_world_secs() }
    }

    // ---- reporting
    pub fn log(&self, level: Level, msg: &str) {
        unsafe { sys::bb_log(level as i32, msg.as_ptr() as i32, msg.len() as i32) }
    }
    pub fn info(&self, msg: &str) {
        self.log(Level::Info, msg)
    }
    pub fn warn(&self, msg: &str) {
        self.log(Level::Warn, msg)
    }
    /// Error message for a failing `bb_init` / `bb_analysis_run`.
    pub fn error(&self, msg: &str) {
        unsafe { sys::bb_error(msg.as_ptr() as i32, msg.len() as i32) }
    }
    /// Emits a `bb_core::DeviceEvent` (JSON).
    pub fn emit(&self, event: &Value) {
        let s = event.to_string();
        unsafe { sys::bb_emit(s.as_ptr() as i32, s.len() as i32) }
    }
    /// A beep (continuity tester).
    pub fn beep(&self, freq_hz: f64, secs: f64) {
        self.emit(&json!({"Sound": {"Beep": {"freq_hz": freq_hz, "secs": secs}}}));
    }
    /// A serial-monitor annotation (`channel: text`).
    pub fn annotate(&self, channel: &str, text: &str) {
        self.emit(&json!({"Annotation": {"channel": channel, "text": text}}));
    }
    /// Marks the part damaged.
    pub fn damage(&self, reason: &str) {
        self.emit(&json!({"Damage": {"reason": reason}}));
    }
    /// The part's bench look (a `bb_core::Look` / `VisualState` JSON).
    pub fn set_visual(&self, v: &Value) {
        let s = v.to_string();
        unsafe { sys::bb_set_visual(s.as_ptr() as i32, s.len() as i32) }
    }
    /// A text readout look.
    pub fn set_readout(&self, text: &str) {
        self.set_visual(&json!({"Readout": {"text": text}}));
    }
    /// Live values for the inspector / lessons.
    pub fn set_readings(&self, r: &[(&str, f64)]) {
        let v: Vec<Value> = r.iter().map(|(k, x)| json!([k, x])).collect();
        let s = Value::Array(v).to_string();
        unsafe { sys::bb_set_readings(s.as_ptr() as i32, s.len() as i32) }
    }
    /// The live front panel (screens with traces need `ui:panel`).
    pub fn set_panel(&self, p: &panel::PanelState) {
        let s = serde_json::to_string(p).unwrap_or_default();
        unsafe { sys::bb_set_panel(s.as_ptr() as i32, s.len() as i32) }
    }
    /// Return value of the current export (`bb_result`).
    pub fn result(&self, bytes: &[u8]) {
        unsafe { sys::bb_result(bytes.as_ptr() as i32, bytes.len() as i32) }
    }

    fn inbox(&self, n: i32) -> Vec<u8> {
        let n = n.unsigned_abs() as usize;
        let mut buf = vec![0u8; n];
        let got = unsafe { sys::bb_inbox_read(buf.as_mut_ptr() as i32, n as i32) };
        buf.truncate(got.max(0) as usize);
        buf
    }

    // ---- solver
    /// Runs a SPICE netlist with the bench's solver; JSON results (`plots`, `measures`).
    pub fn spice_run(&self, netlist: &str) -> Result<Value, String> {
        let n = unsafe { sys::bb_spice_run(netlist.as_ptr() as i32, netlist.len() as i32) };
        let v: Value = serde_json::from_slice(&self.inbox(n)).unwrap_or(Value::Null);
        if n < 0 {
            Err(v
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("solver error")
                .to_string())
        } else {
            Ok(v)
        }
    }

    // ---- radio space (radio:space)
    pub fn mqtt_subscribe(&self, filter: &str) -> bool {
        unsafe { sys::bb_mqtt_subscribe(filter.as_ptr() as i32, filter.len() as i32) == 0 }
    }
    pub fn mqtt_publish(&self, topic: &str, payload: &[u8], qos: u8, retain: bool) -> bool {
        unsafe {
            sys::bb_mqtt_publish(
                topic.as_ptr() as i32,
                topic.len() as i32,
                payload.as_ptr() as i32,
                payload.len() as i32,
                qos as i32,
                retain as i32,
            ) == 0
        }
    }
    /// Next message for the plugin's subscriptions: (topic, payload).
    pub fn mqtt_poll(&self) -> Option<(String, String)> {
        let n = unsafe { sys::bb_mqtt_poll() };
        if n <= 0 {
            return None;
        }
        let v: Value = serde_json::from_slice(&self.inbox(n)).ok()?;
        Some((
            v.get("topic")?.as_str()?.to_string(),
            v.get("payload")?.as_str()?.to_string(),
        ))
    }
}

// Tiny no_std math helpers (libm-free: wasm has sqrt; ln/cos by series are enough for noise).
fn libm_sqrt(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut g = if x > 1.0 { x / 2.0 } else { 1.0 };
    for _ in 0..40 {
        g = 0.5 * (g + x / g);
    }
    g
}
fn libm_ln(x: f64) -> f64 {
    // ln x = 2 atanh((x-1)/(x+1)), after scaling into [0.5, 2)
    if x <= 0.0 {
        return -745.0;
    }
    let mut k = 0i32;
    let mut m = x;
    while m > 2.0 {
        m /= 2.0;
        k += 1;
    }
    while m < 0.5 {
        m *= 2.0;
        k -= 1;
    }
    let y = (m - 1.0) / (m + 1.0);
    let y2 = y * y;
    let mut term = y;
    let mut sum = 0.0;
    let mut n = 1.0;
    for _ in 0..30 {
        sum += term / n;
        term *= y2;
        n += 2.0;
    }
    2.0 * sum + k as f64 * core::f64::consts::LN_2
}
fn libm_cos(x: f64) -> f64 {
    let two_pi = 2.0 * core::f64::consts::PI;
    let mut a = x % two_pi;
    if a > core::f64::consts::PI {
        a -= two_pi;
    }
    if a < -core::f64::consts::PI {
        a += two_pi;
    }
    let a2 = a * a;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..20 {
        term *= -a2 / ((2 * k - 1) as f64 * (2 * k) as f64);
        sum += term;
    }
    sum
}

/// A part / instrument behaviour (`kind: part` / `instrument`).
pub trait Part: Sized {
    fn init(cfg: &Config) -> Result<Self, String>;
    fn on_start(&mut self, _ctx: &Ctx) {}
    fn on_wake(&mut self, _ctx: &Ctx, _token: u64) {}
    fn on_edge(&mut self, _ctx: &Ctx, _pin: usize, _high: bool) {}
    fn on_tick(&mut self, _ctx: &Ctx) {}
    fn on_input(&mut self, _ctx: &Ctx, _input: &Input) {}
    /// A property changed live; return `true` if applied (else the bench rebuilds).
    fn set_param(&mut self, _ctx: &Ctx, _key: &str, _value: f64) -> bool {
        false
    }
    /// Transaction-level I²C from an API-level board: `None` = NACK.
    fn i2c_transfer(
        &mut self,
        _ctx: &Ctx,
        _addr: u8,
        _write: &[u8],
        _read: usize,
    ) -> Option<Vec<u8>> {
        None
    }
}

/// A world model (`kind: world-model`).
pub trait WorldModel: Sized {
    fn init(cfg: &Config) -> Result<Self, String>;
    /// Advance the model by `dt_world_s` world seconds.
    fn step(&mut self, ctx: &Ctx, dt_world_s: f64);
}

/// A custom source waveform (`kind: generator`).
pub trait Generator: Sized {
    fn init(cfg: &Config) -> Result<Self, String>;
    /// Volts at `t` seconds into the period.
    fn eval(&mut self, t: f64) -> f64;
}

/// A Pro-mode analysis (`kind: analysis`).
pub trait Analysis: Sized {
    fn init(cfg: &Config) -> Result<Self, String>;
    /// Returns the `AnalysisOutput` JSON (`summary`, `plots`, `table`, `text`).
    fn run(&mut self, ctx: &Ctx, input: &AnalysisInput) -> Result<Value, String>;
}

/// A radio-space extension (`kind: radio`, `provides.radio.extension`).
pub trait RadioExtension: Sized {
    fn init(cfg: &Config) -> Result<Self, String>;
    /// Called after radio activity and at the requested time; returns the next wake (s).
    fn step(&mut self, ctx: &Ctx, now_s: f64) -> Option<f64>;
}

/// Plumbing the export macros use.
#[doc(hidden)]
pub mod rt {
    use super::*;
    use core::cell::UnsafeCell;

    /// Single-threaded global slot (wasm32-unknown-unknown has one thread).
    pub struct Slot<T>(UnsafeCell<Option<T>>);
    // SAFETY: plugins run on one thread; the host never re-enters an export.
    unsafe impl<T> Sync for Slot<T> {}
    impl<T> Slot<T> {
        pub const fn new() -> Self {
            Slot(UnsafeCell::new(None))
        }
        #[allow(clippy::mut_from_ref)]
        pub fn get(&self) -> &mut Option<T> {
            // SAFETY: see `Sync` above.
            unsafe { &mut *self.0.get() }
        }
    }

    pub fn alloc(len: i32) -> i32 {
        if len <= 0 {
            return 8;
        }
        let layout = core::alloc::Layout::from_size_align(len as usize, 8).expect("layout");
        // SAFETY: non-zero size.
        unsafe { alloc::alloc::alloc(layout) as i32 }
    }

    /// Takes ownership of a buffer the host wrote after `bb_alloc`.
    pub fn take(ptr: i32, len: i32) -> Vec<u8> {
        if len <= 0 {
            return Vec::new();
        }
        // SAFETY: the host wrote `len` bytes at `ptr`, allocated by `alloc(len)`.
        unsafe {
            let s = core::slice::from_raw_parts(ptr as *const u8, len as usize);
            let v = s.to_vec();
            alloc::alloc::dealloc(
                ptr as *mut u8,
                core::alloc::Layout::from_size_align(len as usize, 8).expect("layout"),
            );
            v
        }
    }

    pub fn config(ptr: i32, len: i32) -> Result<Config, String> {
        serde_json::from_slice(&take(ptr, len)).map_err(|e| e.to_string())
    }

    pub fn fail(msg: &str) -> i32 {
        Ctx.error(msg);
        1
    }

    pub fn input(ptr: i32, len: i32) -> Option<Input> {
        serde_json::from_slice(&take(ptr, len)).ok()
    }

    pub fn text(ptr: i32, len: i32) -> String {
        String::from_utf8(take(ptr, len)).unwrap_or_default()
    }

    pub fn analysis_input(ptr: i32, len: i32) -> Option<AnalysisInput> {
        serde_json::from_slice(&take(ptr, len)).ok()
    }
}

/// Exports a [`Part`] (also used for instruments).
#[macro_export]
macro_rules! export_part {
    ($t:ty) => {
        static __BB: $crate::rt::Slot<$t> = $crate::rt::Slot::new();
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_abi_version() -> i32 {
            $crate::ABI_VERSION
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_alloc(len: i32) -> i32 {
            $crate::rt::alloc(len)
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_init(ptr: i32, len: i32) -> i32 {
            match $crate::rt::config(ptr, len).and_then(|c| <$t as $crate::Part>::init(&c)) {
                Ok(v) => {
                    *__BB.get() = Some(v);
                    0
                }
                Err(e) => $crate::rt::fail(&e),
            }
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_on_start() {
            if let Some(s) = __BB.get().as_mut() {
                $crate::Part::on_start(s, &$crate::Ctx);
            }
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_on_wake(token: i64) {
            if let Some(s) = __BB.get().as_mut() {
                $crate::Part::on_wake(s, &$crate::Ctx, token as u64);
            }
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_on_edge(pin: i32, high: i32) {
            if let Some(s) = __BB.get().as_mut() {
                $crate::Part::on_edge(s, &$crate::Ctx, pin as usize, high != 0);
            }
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_on_tick() {
            if let Some(s) = __BB.get().as_mut() {
                $crate::Part::on_tick(s, &$crate::Ctx);
            }
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_on_input(ptr: i32, len: i32) {
            let i = $crate::rt::input(ptr, len);
            if let (Some(s), Some(i)) = (__BB.get().as_mut(), i) {
                $crate::Part::on_input(s, &$crate::Ctx, &i);
            }
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_set_param(ptr: i32, len: i32, value: f64) -> i32 {
            let k = $crate::rt::text(ptr, len);
            match __BB.get().as_mut() {
                Some(s) => $crate::Part::set_param(s, &$crate::Ctx, &k, value) as i32,
                None => 0,
            }
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_i2c_transfer(addr: i32, ptr: i32, len: i32, read: i32) -> i32 {
            let w = $crate::rt::take(ptr, len);
            let Some(s) = __BB.get().as_mut() else {
                return -1;
            };
            match $crate::Part::i2c_transfer(s, &$crate::Ctx, addr as u8, &w, read.max(0) as usize)
            {
                Some(bytes) => {
                    $crate::Ctx.result(&bytes);
                    0
                }
                None => -1,
            }
        }
    };
}

/// Exports a [`WorldModel`].
#[macro_export]
macro_rules! export_world_model {
    ($t:ty) => {
        static __BB: $crate::rt::Slot<$t> = $crate::rt::Slot::new();
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_abi_version() -> i32 {
            $crate::ABI_VERSION
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_alloc(len: i32) -> i32 {
            $crate::rt::alloc(len)
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_init(ptr: i32, len: i32) -> i32 {
            match $crate::rt::config(ptr, len).and_then(|c| <$t as $crate::WorldModel>::init(&c)) {
                Ok(v) => {
                    *__BB.get() = Some(v);
                    0
                }
                Err(e) => $crate::rt::fail(&e),
            }
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_world_step(dt: f64) {
            if let Some(s) = __BB.get().as_mut() {
                $crate::WorldModel::step(s, &$crate::Ctx, dt);
            }
        }
    };
}

/// Exports a [`Generator`].
#[macro_export]
macro_rules! export_generator {
    ($t:ty) => {
        static __BB: $crate::rt::Slot<$t> = $crate::rt::Slot::new();
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_abi_version() -> i32 {
            $crate::ABI_VERSION
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_alloc(len: i32) -> i32 {
            $crate::rt::alloc(len)
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_init(ptr: i32, len: i32) -> i32 {
            match $crate::rt::config(ptr, len).and_then(|c| <$t as $crate::Generator>::init(&c)) {
                Ok(v) => {
                    *__BB.get() = Some(v);
                    0
                }
                Err(e) => $crate::rt::fail(&e),
            }
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_generator_eval(t: f64) -> f64 {
            match __BB.get().as_mut() {
                Some(s) => $crate::Generator::eval(s, t),
                None => 0.0,
            }
        }
    };
}

/// Exports an [`Analysis`].
#[macro_export]
macro_rules! export_analysis {
    ($t:ty) => {
        static __BB: $crate::rt::Slot<$t> = $crate::rt::Slot::new();
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_abi_version() -> i32 {
            $crate::ABI_VERSION
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_alloc(len: i32) -> i32 {
            $crate::rt::alloc(len)
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_init(ptr: i32, len: i32) -> i32 {
            match $crate::rt::config(ptr, len).and_then(|c| <$t as $crate::Analysis>::init(&c)) {
                Ok(v) => {
                    *__BB.get() = Some(v);
                    0
                }
                Err(e) => $crate::rt::fail(&e),
            }
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_analysis_run(ptr: i32, len: i32) -> i32 {
            let Some(input) = $crate::rt::analysis_input(ptr, len) else {
                return $crate::rt::fail("bad analysis input");
            };
            let Some(s) = __BB.get().as_mut() else {
                return 1;
            };
            match $crate::Analysis::run(s, &$crate::Ctx, &input) {
                Ok(v) => {
                    $crate::Ctx.result(v.to_string().as_bytes());
                    0
                }
                Err(e) => $crate::rt::fail(&e),
            }
        }
    };
}

/// Exports a [`RadioExtension`].
#[macro_export]
macro_rules! export_radio {
    ($t:ty) => {
        static __BB: $crate::rt::Slot<$t> = $crate::rt::Slot::new();
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_abi_version() -> i32 {
            $crate::ABI_VERSION
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_alloc(len: i32) -> i32 {
            $crate::rt::alloc(len)
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_init(ptr: i32, len: i32) -> i32 {
            match $crate::rt::config(ptr, len)
                .and_then(|c| <$t as $crate::RadioExtension>::init(&c))
            {
                Ok(v) => {
                    *__BB.get() = Some(v);
                    0
                }
                Err(e) => $crate::rt::fail(&e),
            }
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn bb_radio_step(now: f64) -> f64 {
            match __BB.get().as_mut() {
                Some(s) => $crate::RadioExtension::step(s, &$crate::Ctx, now).unwrap_or(-1.0),
                None => -1.0,
            }
        }
    };
}

/// Formats like a `ToString` for no_std users.
pub fn to_string<T: ToString>(t: T) -> String {
    t.to_string()
}

#[doc(hidden)]
pub fn _serde_check<T: Serialize>(_: &T) {}
