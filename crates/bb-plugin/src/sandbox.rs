//! The WebAssembly sandbox (PLUG-2): one wasmtime [`Engine`] for every plugin, with
//!
//! * **fuel** — every call gets a fresh instruction budget; a plugin that runs out traps,
//! * **epoch interruption** — a 1 ms ticker thread bumps the engine epoch; a call that runs
//!   longer than its wall-clock budget traps (backstop for slow host calls),
//! * **memory caps** — `StoreLimits` bound linear memory, tables and instances,
//! * **no WASI** unless the plugin declares `fs:project` (then WASI preview 1 with only the
//!   project folder preopened),
//! * **permission-checked imports** — every `bb.*` host function checks the permission it needs
//!   and traps with "permission denied" otherwise.
//!
//! A trap never reaches the simulation: the adapter marks the plugin faulted, reports why, and
//! the bench keeps running without it.
//!
//! The exact ABI is documented in `docs/PLUGIN-SDK.md`.

use crate::PluginError;
use crate::manifest::{Manifest, PermSet, Permission};
use bb_core::{DeviceCtx, DeviceEvent, LogLevel, PinDrive, Rng, SimTime, WorldMut};
use bb_radio::RadioSpace;
use bb_radio::mqtt::{ConnKey, Message};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use wasmtime::{
    Caller, Config, Engine, Extern, Instance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, Trap, TypedFunc, WasmParams, WasmResults,
};

/// Budgets per call / per instance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// Fuel (≈ wasm instructions) per call.
    pub fuel_per_call: u64,
    /// Wall-clock budget per call in epoch ticks (1 ms each).
    pub epoch_ticks: u64,
    /// Linear-memory cap (bytes).
    pub memory_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            fuel_per_call: 50_000_000,
            epoch_ticks: 250,
            memory_bytes: 32 << 20,
        }
    }
}

/// Hard caps a manifest cannot exceed.
pub const MAX_FUEL: u64 = 2_000_000_000;
pub const MAX_MEMORY: usize = 256 << 20;
/// Largest message a plugin may pass in one call.
pub const MAX_MESSAGE: usize = 16 << 20;

impl Limits {
    /// Defaults with the manifest's overrides, clamped to the caps.
    pub fn for_manifest(m: &Manifest) -> Limits {
        let mut l = Limits::default();
        if let Some(x) = &m.limits {
            if let Some(f) = x.fuel_per_call {
                l.fuel_per_call = f.clamp(1_000, MAX_FUEL);
            }
            if let Some(mb) = x.memory_mb {
                l.memory_bytes = ((mb as usize) << 20).clamp(1 << 16, MAX_MEMORY);
            }
        }
        l
    }
}

/// The engine shared by every plugin (fuel + epochs on, 1 ms epoch ticker running).
pub fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut c = Config::new();
        c.consume_fuel(true);
        c.epoch_interruption(true);
        c.max_wasm_stack(1 << 20);
        let e = Engine::new(&c).unwrap_or_else(|_| Engine::default());
        let weak = e.weak();
        let _ = std::thread::Builder::new()
            .name("bb-plugin-epoch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    match weak.upgrade() {
                        Some(e) => e.increment_epoch(),
                        None => break,
                    }
                }
            });
        e
    })
}

/// A plugin's code, compiled once and instantiated per part / model.
#[derive(Clone)]
pub struct Compiled {
    pub manifest: Arc<Manifest>,
    pub root: PathBuf,
    pub module: Module,
    pub perms: PermSet,
    pub limits: Limits,
}

impl std::fmt::Debug for Compiled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compiled")
            .field("plugin", &self.manifest.id)
            .finish_non_exhaustive()
    }
}

/// Host functions a module may import from `bb` (see `docs/PLUGIN-SDK.md`).
pub const IMPORTS: &[&str] = &[
    "bb_log",
    "bb_error",
    "bb_now",
    "bb_now_s",
    "bb_rng",
    "bb_pin_voltage",
    "bb_pin_current",
    "bb_element_current",
    "bb_element_charge",
    "bb_element_energy",
    "bb_track_element",
    "bb_watch",
    "bb_capture",
    "bb_samples",
    "bb_set_drive",
    "bb_set_ext",
    "bb_wake_at",
    "bb_cancel_wake",
    "bb_tick_every",
    "bb_world_get",
    "bb_world_set",
    "bb_world_water",
    "bb_world_secs",
    "bb_emit",
    "bb_set_visual",
    "bb_set_readings",
    "bb_set_panel",
    "bb_result",
    "bb_spice_run",
    "bb_inbox_read",
    "bb_mqtt_subscribe",
    "bb_mqtt_publish",
    "bb_mqtt_poll",
    "bb_lan_bridge",
];

impl Compiled {
    /// Compiles and checks a module against the ABI: required exports present, imports known,
    /// WASI only with `fs:project`.
    pub fn new(
        manifest: Arc<Manifest>,
        root: PathBuf,
        wasm: &[u8],
    ) -> Result<Compiled, PluginError> {
        let id = manifest.id.clone();
        let abi = |msg: String| PluginError::Abi {
            plugin: id.clone(),
            msg,
        };
        let module = Module::new(engine(), wasm)
            .map_err(|e| abi(format!("not a valid WebAssembly module: {e}")))?;
        let perms = manifest.perms();
        for imp in module.imports() {
            match imp.module() {
                "bb" => {
                    if !IMPORTS.contains(&imp.name()) {
                        return Err(abi(format!(
                            "imports unknown host function `bb.{}` (this Breadbox offers: {})",
                            imp.name(),
                            IMPORTS.join(", ")
                        )));
                    }
                }
                "wasi_snapshot_preview1" => {
                    if !perms.contains(&Permission::FsProject) {
                        return Err(PluginError::PermissionDenied {
                            plugin: id.clone(),
                            permission: Permission::FsProject,
                            action: format!("import WASI (`{}`)", imp.name()),
                        });
                    }
                }
                other => {
                    return Err(abi(format!(
                        "imports `{other}.{}`; only `bb` (and WASI with fs:project) exist",
                        imp.name()
                    )));
                }
            }
        }
        let exports: Vec<&str> = module.exports().map(|e| e.name()).collect();
        for need in ["memory", "bb_abi_version", "bb_alloc"] {
            if !exports.contains(&need) {
                return Err(abi(format!("does not export `{need}`")));
            }
        }
        Ok(Compiled {
            limits: Limits::for_manifest(&manifest),
            manifest,
            root,
            module,
            perms,
        })
    }

    pub fn exports(&self, name: &str) -> bool {
        self.module.exports().any(|e| e.name() == name)
    }
}

/// What the guest is allowed to touch during the current call.
#[derive(Clone, Copy)]
pub(crate) enum CallCtx {
    None,
    Device(*mut (dyn DeviceCtx + 'static)),
    World(*mut (dyn WorldMut + 'static)),
    Radio(*mut RadioSpace, SimTime),
}

/// Values the guest pushed with `bb_set_*` / `bb_result` during the calls so far.
#[derive(Debug, Default, Clone)]
pub struct Outputs {
    pub visual: Option<serde_json::Value>,
    pub readings: Option<Vec<(String, f64)>>,
    pub panel: Option<serde_json::Value>,
    pub result: Option<Vec<u8>>,
    pub error: Option<String>,
    /// Log lines from calls without a device context.
    pub logs: Vec<(LogLevel, String)>,
}

/// Store data.
pub struct HostState {
    pub plugin: String,
    pub perms: PermSet,
    limits: StoreLimits,
    ctx: CallCtx,
    pub out: Outputs,
    wasi: Option<wasmtime_wasi::p1::WasiP1Ctx>,
    rng: Rng,
    inbox: Vec<u8>,
    mqtt: Option<ConnKey>,
    mqtt_in: VecDeque<(String, Vec<u8>)>,
    violation: Option<String>,
}

// SAFETY: the raw context pointers are only set for the duration of one synchronous call on
// the calling thread (see `Sandbox::call`) and cleared afterwards; between calls the state
// holds no borrowed data.
unsafe impl Send for HostState {}

impl HostState {
    fn need(&mut self, p: Permission, what: &str) -> wasmtime::Result<()> {
        if self.perms.contains(&p) {
            return Ok(());
        }
        let msg = format!(
            "permission denied: plugin `{}` tried to {what} without declaring `{p}`",
            self.plugin
        );
        self.violation = Some(msg.clone());
        Err(wasmtime::Error::msg(msg))
    }

    fn device(&mut self, what: &str) -> wasmtime::Result<&mut dyn DeviceCtx> {
        match self.ctx {
            // SAFETY: set by `Sandbox::call` from a live `&mut dyn DeviceCtx` borrowed for the
            // whole call; cleared before the borrow ends.
            CallCtx::Device(p) => Ok(unsafe { &mut *p }),
            _ => Err(wasmtime::Error::msg(format!(
                "`{what}` can only be called from a part/instrument hook"
            ))),
        }
    }

    fn world(&mut self) -> Option<&mut dyn WorldMut> {
        match self.ctx {
            // SAFETY: as in `device`.
            CallCtx::World(p) => Some(unsafe { &mut *p }),
            CallCtx::Device(p) => unsafe { &mut *p }.world_mut(),
            _ => None,
        }
    }

    fn world_view(&mut self) -> Option<&dyn bb_core::WorldView> {
        match self.ctx {
            // SAFETY: as in `device`.
            CallCtx::World(p) => Some(unsafe { &*p }),
            CallCtx::Device(p) => Some(unsafe { &*p }.world()),
            _ => None,
        }
    }

    fn radio(&mut self) -> Option<(&mut RadioSpace, SimTime)> {
        match self.ctx {
            // SAFETY: as in `device`.
            CallCtx::Radio(p, t) => Some((unsafe { &mut *p }, t)),
            _ => None,
        }
    }

    fn now(&mut self) -> SimTime {
        match self.ctx {
            // SAFETY: as in `device`.
            CallCtx::Device(p) => unsafe { &*p }.now(),
            CallCtx::World(p) => unsafe { &*p }.time(),
            CallCtx::Radio(_, t) => t,
            CallCtx::None => SimTime::ZERO,
        }
    }
}

fn memory(caller: &mut Caller<'_, HostState>) -> wasmtime::Result<Memory> {
    match caller.get_export("memory") {
        Some(Extern::Memory(m)) => Ok(m),
        _ => Err(wasmtime::Error::msg("plugin exports no memory")),
    }
}

fn read(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> wasmtime::Result<Vec<u8>> {
    if ptr < 0 || len < 0 || len as usize > MAX_MESSAGE {
        return Err(wasmtime::Error::msg(format!("bad buffer ({ptr}, {len})")));
    }
    let m = memory(caller)?;
    let data = m.data(&*caller);
    let (a, b) = (ptr as usize, ptr as usize + len as usize);
    if b > data.len() {
        return Err(wasmtime::Error::msg(format!(
            "buffer ({ptr}, {len}) is outside the plugin's memory"
        )));
    }
    Ok(data[a..b].to_vec())
}

fn read_str(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> wasmtime::Result<String> {
    let b = read(caller, ptr, len)?;
    String::from_utf8(b).map_err(|_| wasmtime::Error::msg("string is not UTF-8"))
}

fn write(caller: &mut Caller<'_, HostState>, ptr: i32, bytes: &[u8]) -> wasmtime::Result<()> {
    if ptr < 0 {
        return Err(wasmtime::Error::msg("bad output pointer"));
    }
    let m = memory(caller)?;
    m.write(&mut *caller, ptr as usize, bytes)
        .map_err(|_| wasmtime::Error::msg("output buffer is outside the plugin's memory"))
}

fn json<T: serde::de::DeserializeOwned>(bytes: &[u8], what: &str) -> wasmtime::Result<T> {
    serde_json::from_slice(bytes)
        .map_err(|e| wasmtime::Error::msg(format!("{what}: bad JSON: {e}")))
}

fn ps(t: i64) -> SimTime {
    SimTime::from_ps(t.max(0) as u64)
}

fn level(l: i32) -> LogLevel {
    match l {
        0 => LogLevel::Debug,
        1 => LogLevel::Info,
        2 => LogLevel::Warn,
        _ => LogLevel::Error,
    }
}

fn add_bb(l: &mut Linker<HostState>) -> wasmtime::Result<()> {
    type C<'a> = Caller<'a, HostState>;
    l.func_wrap(
        "bb",
        "bb_log",
        |mut c: C, lv: i32, p: i32, n: i32| -> wasmtime::Result<()> {
            let text = read_str(&mut c, p, n)?;
            let s = c.data_mut();
            let lv = level(lv);
            if let CallCtx::Device(_) = s.ctx {
                s.device("bb_log")?
                    .emit(DeviceEvent::Log { level: lv, text });
            } else {
                s.out.logs.push((lv, text));
            }
            Ok(())
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_error",
        |mut c: C, p: i32, n: i32| -> wasmtime::Result<()> {
            let text = read_str(&mut c, p, n)?;
            c.data_mut().out.error = Some(text);
            Ok(())
        },
    )?;
    l.func_wrap("bb", "bb_now", |mut c: C| -> i64 {
        c.data_mut().now().0 as i64
    })?;
    l.func_wrap("bb", "bb_now_s", |mut c: C| -> f64 {
        c.data_mut().now().as_secs_f64()
    })?;
    l.func_wrap("bb", "bb_rng", |mut c: C| -> f64 {
        c.data_mut().rng.uniform()
    })?;
    // ---- reading the circuit (read:nets)
    l.func_wrap(
        "bb",
        "bb_pin_voltage",
        |mut c: C, pin: i32| -> wasmtime::Result<f64> {
            let s = c.data_mut();
            s.need(Permission::ReadNets, "read a pin voltage")?;
            Ok(s.device("bb_pin_voltage")?.pin_voltage(pin.max(0) as usize))
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_pin_current",
        |mut c: C, pin: i32| -> wasmtime::Result<f64> {
            let s = c.data_mut();
            s.need(Permission::ReadNets, "read a pin current")?;
            Ok(s.device("bb_pin_current")?.pin_current(pin.max(0) as usize))
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_element_current",
        |mut c: C, p: i32, n: i32| -> wasmtime::Result<f64> {
            let name = read_str(&mut c, p, n)?;
            let s = c.data_mut();
            s.need(Permission::ReadNets, "read an element current")?;
            Ok(s.device("bb_element_current")?
                .element_current(&name)
                .unwrap_or(f64::NAN))
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_element_charge",
        |mut c: C, p: i32, n: i32| -> wasmtime::Result<f64> {
            let name = read_str(&mut c, p, n)?;
            let s = c.data_mut();
            s.need(Permission::ReadNets, "read an element charge")?;
            Ok(s.device("bb_element_charge")?
                .element_charge(&name)
                .unwrap_or(f64::NAN))
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_element_energy",
        |mut c: C, p: i32, n: i32| -> wasmtime::Result<f64> {
            let name = read_str(&mut c, p, n)?;
            let s = c.data_mut();
            s.need(Permission::ReadNets, "read an element energy")?;
            Ok(s.device("bb_element_energy")?
                .element_energy(&name)
                .unwrap_or(f64::NAN))
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_track_element",
        |mut c: C, p: i32, n: i32| -> wasmtime::Result<()> {
            let name = read_str(&mut c, p, n)?;
            let s = c.data_mut();
            s.need(Permission::ReadNets, "track an element")?;
            s.device("bb_track_element")?.track_element(&name);
            Ok(())
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_watch",
        |mut c: C, pin: i32, lo: f64, hi: f64| -> wasmtime::Result<()> {
            let s = c.data_mut();
            s.need(Permission::ReadNets, "watch a pin")?;
            s.device("bb_watch")?.watch(pin.max(0) as usize, lo, hi);
            Ok(())
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_capture",
        |mut c: C, pin: i32, period: i64| -> wasmtime::Result<()> {
            let s = c.data_mut();
            s.need(Permission::ReadNets, "capture a waveform")?;
            s.device("bb_capture")?
                .capture(pin.max(0) as usize, ps(period));
            Ok(())
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_samples",
        |mut c: C, pin: i32, from: i64, to: i64, out: i32, max: i32| -> wasmtime::Result<i32> {
            let s = c.data_mut();
            s.need(Permission::ReadNets, "read captured samples")?;
            let v = s
                .device("bb_samples")?
                .samples(pin.max(0) as usize, ps(from), ps(to));
            let n = v.len().min(max.max(0) as usize);
            let start = v.len() - n; // the newest `n`
            let mut bytes = Vec::with_capacity(n * 16);
            for (t, x) in &v[start..] {
                bytes.extend_from_slice(&t.as_secs_f64().to_le_bytes());
                bytes.extend_from_slice(&x.to_le_bytes());
            }
            write(&mut c, out, &bytes)?;
            Ok(n as i32)
        },
    )?;
    // ---- driving (drive:pins / inject:load)
    l.func_wrap(
        "bb",
        "bb_set_drive",
        |mut c: C, pin: i32, kind: i32, v: f64, r: f64| -> wasmtime::Result<()> {
            let s = c.data_mut();
            s.need(Permission::DrivePins, "drive a pin")?;
            let d = match kind {
                1 => PinDrive::Thevenin { v, r: r.max(1e-3) },
                2 => PinDrive::Pull { v, r: r.max(1e-3) },
                3 => PinDrive::Current { i: v },
                _ => PinDrive::HighZ,
            };
            if !(v.is_finite() && r.is_finite()) {
                return Err(wasmtime::Error::msg("bb_set_drive: non-finite value"));
            }
            s.device("bb_set_drive")?.set_drive(pin.max(0) as usize, d);
            Ok(())
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_set_ext",
        |mut c: C, p: i32, n: i32, v: f64| -> wasmtime::Result<()> {
            let name = read_str(&mut c, p, n)?;
            let s = c.data_mut();
            if !s.perms.contains(&Permission::DrivePins) {
                s.need(Permission::InjectLoad, "change its circuit load")?;
            }
            if !v.is_finite() {
                return Err(wasmtime::Error::msg("bb_set_ext: non-finite value"));
            }
            s.device("bb_set_ext")?.set_ext(&name, v);
            Ok(())
        },
    )?;
    // ---- time
    l.func_wrap(
        "bb",
        "bb_wake_at",
        |mut c: C, t: i64, token: i64| -> wasmtime::Result<()> {
            c.data_mut()
                .device("bb_wake_at")?
                .wake_at(ps(t), token as u64);
            Ok(())
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_cancel_wake",
        |mut c: C, token: i64| -> wasmtime::Result<()> {
            c.data_mut()
                .device("bb_cancel_wake")?
                .cancel_wake(token as u64);
            Ok(())
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_tick_every",
        |mut c: C, period: i64| -> wasmtime::Result<()> {
            let p = ps(period);
            if p > SimTime::ZERO && p < SimTime::from_us(1) {
                return Err(wasmtime::Error::msg("bb_tick_every: period below 1 µs"));
            }
            c.data_mut().device("bb_tick_every")?.tick_every(p);
            Ok(())
        },
    )?;
    // ---- world
    l.func_wrap(
        "bb",
        "bb_world_get",
        |mut c: C, p: i32, n: i32| -> wasmtime::Result<f64> {
            let key = read_str(&mut c, p, n)?;
            let s = c.data_mut();
            s.need(Permission::WorldRead, "read the world")?;
            Ok(s.world_view().and_then(|w| w.var(&key)).unwrap_or(f64::NAN))
        },
    )?;
    l.func_wrap("bb", "bb_world_secs", |mut c: C| -> wasmtime::Result<f64> {
        let s = c.data_mut();
        s.need(Permission::WorldRead, "read the world clock")?;
        Ok(s.world_view().map(|w| w.world_secs()).unwrap_or(0.0))
    })?;
    l.func_wrap(
        "bb",
        "bb_world_set",
        |mut c: C, p: i32, n: i32, v: f64| -> wasmtime::Result<()> {
            let key = read_str(&mut c, p, n)?;
            let s = c.data_mut();
            s.need(Permission::WorldWrite, "change the world")?;
            if !v.is_finite() {
                return Err(wasmtime::Error::msg("bb_world_set: non-finite value"));
            }
            if let Some(w) = s.world() {
                w.set_var(&key, v);
            }
            Ok(())
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_world_water",
        |mut c: C, p: i32, n: i32, ml: f64| -> wasmtime::Result<()> {
            let key = read_str(&mut c, p, n)?;
            let s = c.data_mut();
            s.need(Permission::WorldWrite, "water a pot")?;
            if let Some(w) = s.world()
                && ml.is_finite()
            {
                w.water(&key, ml.clamp(0.0, 1e5));
            }
            Ok(())
        },
    )?;
    // ---- outputs
    l.func_wrap(
        "bb",
        "bb_emit",
        |mut c: C, p: i32, n: i32| -> wasmtime::Result<()> {
            let b = read(&mut c, p, n)?;
            let ev: DeviceEvent = json(&b, "bb_emit")?;
            let s = c.data_mut();
            match s.ctx {
                CallCtx::Device(_) => s.device("bb_emit")?.emit(ev),
                _ => {
                    if let DeviceEvent::Log { level, text } = ev {
                        s.out.logs.push((level, text));
                    }
                }
            }
            Ok(())
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_set_visual",
        |mut c: C, p: i32, n: i32| -> wasmtime::Result<()> {
            let b = read(&mut c, p, n)?;
            let v: serde_json::Value = json(&b, "bb_set_visual")?;
            c.data_mut().out.visual = Some(v);
            Ok(())
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_set_readings",
        |mut c: C, p: i32, n: i32| -> wasmtime::Result<()> {
            let b = read(&mut c, p, n)?;
            let v: serde_json::Value = json(&b, "bb_set_readings")?;
            let list: Vec<(String, f64)> = match v {
                serde_json::Value::Object(m) => m
                    .into_iter()
                    .filter_map(|(k, v)| v.as_f64().map(|x| (k, x)))
                    .collect(),
                serde_json::Value::Array(a) => a
                    .into_iter()
                    .filter_map(|x| {
                        let k = x.get(0)?.as_str()?.to_string();
                        Some((k, x.get(1)?.as_f64()?))
                    })
                    .collect(),
                _ => {
                    return Err(wasmtime::Error::msg(
                        "bb_set_readings: expected an object or [[name, value], …]",
                    ));
                }
            };
            c.data_mut().out.readings = Some(list);
            Ok(())
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_set_panel",
        |mut c: C, p: i32, n: i32| -> wasmtime::Result<()> {
            let b = read(&mut c, p, n)?;
            let v: serde_json::Value = json(&b, "bb_set_panel")?;
            let st = crate::panel::PanelState::from_json(&v)
                .ok_or_else(|| wasmtime::Error::msg("bb_set_panel: not a panel state"))?;
            let s = c.data_mut();
            if st.draws() {
                s.need(Permission::UiPanel, "draw on its screen")?;
            }
            s.out.panel = Some(v);
            Ok(())
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_result",
        |mut c: C, p: i32, n: i32| -> wasmtime::Result<()> {
            let b = read(&mut c, p, n)?;
            c.data_mut().out.result = Some(b);
            Ok(())
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_inbox_read",
        |mut c: C, out: i32, max: i32| -> wasmtime::Result<i32> {
            let data = std::mem::take(&mut c.data_mut().inbox);
            if data.len() > max.max(0) as usize {
                let n = data.len();
                c.data_mut().inbox = data;
                return Ok(-(n as i32));
            }
            write(&mut c, out, &data)?;
            Ok(data.len() as i32)
        },
    )?;
    // ---- solver
    l.func_wrap(
        "bb",
        "bb_spice_run",
        |mut c: C, p: i32, n: i32| -> wasmtime::Result<i32> {
            let text = read_str(&mut c, p, n)?;
            let out = crate::analysis::run_netlist(&text);
            let bytes = serde_json::to_vec(&out).unwrap_or_default();
            let len = bytes.len() as i32;
            let ok = out.error.is_none();
            c.data_mut().inbox = bytes;
            Ok(if ok { len } else { -len })
        },
    )?;
    // ---- radio (radio:space)
    l.func_wrap(
        "bb",
        "bb_mqtt_subscribe",
        |mut c: C, p: i32, n: i32| -> wasmtime::Result<i32> {
            let filter = read_str(&mut c, p, n)?;
            let s = c.data_mut();
            s.need(Permission::RadioSpace, "subscribe to MQTT")?;
            let id = s.plugin.clone();
            let key = s.mqtt;
            let Some((space, now)) = s.radio() else {
                return Ok(-1);
            };
            let k = key.unwrap_or_else(|| space.net.broker_mut().virtual_connect(&id, now));
            let out = space.net.broker_mut().virtual_subscribe(k, &filter, 1, now);
            space.net.apply_broker(out);
            s.mqtt = Some(k);
            Ok(0)
        },
    )?;
    l.func_wrap(
        "bb",
        "bb_mqtt_publish",
        |mut c: C,
         tp: i32,
         tn: i32,
         pp: i32,
         pn: i32,
         qos: i32,
         retain: i32|
         -> wasmtime::Result<i32> {
            let topic = read_str(&mut c, tp, tn)?;
            let payload = read(&mut c, pp, pn)?;
            let s = c.data_mut();
            s.need(Permission::RadioSpace, "publish to MQTT")?;
            let id = s.plugin.clone();
            let key = s.mqtt;
            let Some((space, now)) = s.radio() else {
                return Ok(-1);
            };
            let k = key.unwrap_or_else(|| space.net.broker_mut().virtual_connect(&id, now));
            let out = space.net.broker_mut().virtual_publish(
                k,
                Message::new(&topic, payload, qos.clamp(0, 1) as u8, retain != 0),
                now,
            );
            space.net.apply_broker(out);
            s.mqtt = Some(k);
            Ok(0)
        },
    )?;
    l.func_wrap("bb", "bb_mqtt_poll", |mut c: C| -> wasmtime::Result<i32> {
        let s = c.data_mut();
        s.need(Permission::RadioSpace, "read MQTT messages")?;
        if let (Some(k), Some((space, _))) = (s.mqtt, s.radio()) {
            let got = space.net.broker_mut().virtual_take(k);
            for r in got {
                s.mqtt_in.push_back((r.message.topic, r.message.payload));
            }
        }
        match s.mqtt_in.pop_front() {
            Some((topic, payload)) => {
                let v = serde_json::json!({
                    "topic": topic,
                    "payload": String::from_utf8_lossy(&payload),
                });
                let b = serde_json::to_vec(&v).unwrap_or_default();
                let n = b.len() as i32;
                s.inbox = b;
                Ok(n)
            }
            None => Ok(0),
        }
    })?;
    l.func_wrap(
        "bb",
        "bb_lan_bridge",
        |mut c: C, p: i32, n: i32| -> wasmtime::Result<i32> {
            let target = read_str(&mut c, p, n)?;
            let s = c.data_mut();
            s.need(Permission::NetLan, "bridge to the real LAN")?;
            let Some((space, _)) = s.radio() else {
                return Ok(-1);
            };
            if target.trim().is_empty() {
                space.set_lan_bridge(None);
                return Ok(0);
            }
            let Some((host, port)) = target.rsplit_once(':') else {
                return Ok(-1);
            };
            let Ok(port) = port.parse::<u16>() else {
                return Ok(-1);
            };
            space.set_lan_bridge(Some(bb_radio::bridge::BridgeConfig::new(host, port)));
            Ok(0)
        },
    )?;
    Ok(())
}

fn linker(wasi: bool) -> &'static Linker<HostState> {
    static PLAIN: OnceLock<Linker<HostState>> = OnceLock::new();
    static WASI: OnceLock<Linker<HostState>> = OnceLock::new();
    let cell = if wasi { &WASI } else { &PLAIN };
    cell.get_or_init(|| {
        let mut l = Linker::new(engine());
        if let Err(e) = add_bb(&mut l) {
            log::error!("plugin host functions: {e}");
        }
        if wasi
            && let Err(e) = wasmtime_wasi::p1::add_to_linker_sync(&mut l, |s: &mut HostState| {
                s.wasi
                    .as_mut()
                    .expect("WASI context exists for fs:project plugins")
            })
        {
            log::error!("WASI: {e}");
        }
        l
    })
}

/// One instantiated plugin (a part instance, a world model, an analysis run…).
pub struct Sandbox {
    store: Store<HostState>,
    instance: Instance,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    limits: Limits,
    pub plugin: String,
    /// Why the plugin was stopped (trap, budget, permission).
    pub faulted: Option<String>,
}

impl Sandbox {
    /// Instantiates `c`. `project_dir` is preopened for `fs:project` plugins.
    pub fn new(
        c: &Compiled,
        seed: u64,
        project_dir: Option<&Path>,
    ) -> Result<Sandbox, PluginError> {
        let id = c.manifest.id.clone();
        let wasi_on = c.perms.contains(&Permission::FsProject);
        let wasi = if wasi_on {
            let mut b = wasmtime_wasi::WasiCtxBuilder::new();
            if let Some(dir) = project_dir
                && let Err(e) = b.preopened_dir(dir, ".", wasmtime_wasi::FsPerms::ReadWrite)
            {
                return Err(PluginError::Abi {
                    plugin: id,
                    msg: format!("cannot open the project folder: {e}"),
                });
            }
            Some(b.build_p1())
        } else {
            None
        };
        let state = HostState {
            plugin: id.clone(),
            perms: c.perms.clone(),
            limits: StoreLimitsBuilder::new()
                .memory_size(c.limits.memory_bytes)
                .instances(1)
                .memories(1)
                .tables(4)
                .table_elements(65_536)
                .trap_on_grow_failure(true)
                .build(),
            ctx: CallCtx::None,
            out: Outputs::default(),
            wasi,
            rng: Rng::new(seed),
            inbox: Vec::new(),
            mqtt: None,
            mqtt_in: VecDeque::new(),
            violation: None,
        };
        let mut store = Store::new(engine(), state);
        store.limiter(|s| &mut s.limits);
        store.epoch_deadline_trap();
        let _ = store.set_fuel(c.limits.fuel_per_call);
        store.set_epoch_deadline(c.limits.epoch_ticks);
        let instance = linker(wasi_on)
            .instantiate(&mut store, &c.module)
            .map_err(|e| PluginError::Abi {
                plugin: id.clone(),
                msg: format!("cannot instantiate: {}", describe(&e, &store)),
            })?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| PluginError::Abi {
                plugin: id.clone(),
                msg: "exports no memory".into(),
            })?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "bb_alloc")
            .map_err(|e| PluginError::Abi {
                plugin: id.clone(),
                msg: format!("bb_alloc: {e}"),
            })?;
        let mut sb = Sandbox {
            store,
            instance,
            memory,
            alloc,
            limits: c.limits,
            plugin: id.clone(),
            faulted: None,
        };
        let v: i32 = sb
            .call(CallCtx::None, "bb_abi_version", ())?
            .ok_or_else(|| PluginError::Abi {
                plugin: id.clone(),
                msg: "does not export bb_abi_version".into(),
            })?;
        if v as u32 != crate::manifest::API_VERSION {
            return Err(PluginError::Abi {
                plugin: id,
                msg: format!(
                    "built for plugin ABI {v}, this Breadbox hosts ABI {}",
                    crate::manifest::API_VERSION
                ),
            });
        }
        Ok(sb)
    }

    fn refill(&mut self) {
        let _ = self.store.set_fuel(self.limits.fuel_per_call);
        self.store.set_epoch_deadline(self.limits.epoch_ticks);
    }

    /// A typed export (`None` if the plugin doesn't export it).
    pub fn func<P: WasmParams, R: WasmResults>(&mut self, name: &str) -> Option<TypedFunc<P, R>> {
        self.instance
            .get_typed_func::<P, R>(&mut self.store, name)
            .ok()
    }

    pub fn has(&mut self, name: &str) -> bool {
        self.instance.get_func(&mut self.store, name).is_some()
    }

    /// Calls `f` with `ctx` installed. A trap marks the plugin faulted and returns the reason.
    pub(crate) fn call_typed<P: WasmParams, R: WasmResults>(
        &mut self,
        ctx: CallCtx,
        f: &TypedFunc<P, R>,
        params: P,
    ) -> Result<R, PluginError> {
        if let Some(why) = &self.faulted {
            return Err(PluginError::Faulted {
                plugin: self.plugin.clone(),
                msg: why.clone(),
            });
        }
        self.refill();
        self.store.data_mut().ctx = ctx;
        self.store.data_mut().violation = None;
        let r = f.call(&mut self.store, params);
        self.store.data_mut().ctx = CallCtx::None;
        match r {
            Ok(v) => Ok(v),
            Err(e) => {
                let why = describe(&e, &self.store);
                self.faulted = Some(why.clone());
                let violation = self.store.data_mut().violation.take();
                Err(match violation {
                    Some(v) => PluginError::Violation {
                        plugin: self.plugin.clone(),
                        msg: v,
                    },
                    None => PluginError::Faulted {
                        plugin: self.plugin.clone(),
                        msg: why,
                    },
                })
            }
        }
    }

    /// Calls an export by name (`Ok(None)` if absent).
    pub(crate) fn call<P: WasmParams, R: WasmResults>(
        &mut self,
        ctx: CallCtx,
        name: &str,
        params: P,
    ) -> Result<Option<R>, PluginError> {
        let Some(f) = self.func::<P, R>(name) else {
            return Ok(None);
        };
        self.call_typed(ctx, &f, params).map(Some)
    }

    /// Copies bytes into guest memory (`bb_alloc`) → `(ptr, len)`.
    pub(crate) fn put(&mut self, bytes: &[u8]) -> Result<(i32, i32), PluginError> {
        if bytes.len() > MAX_MESSAGE {
            return Err(PluginError::Abi {
                plugin: self.plugin.clone(),
                msg: "message too large".into(),
            });
        }
        let alloc = self.alloc.clone();
        let ptr = self.call_typed(CallCtx::None, &alloc, bytes.len() as i32)?;
        if ptr < 0
            || (ptr as usize)
                .checked_add(bytes.len())
                .is_none_or(|end| end > self.memory.data_size(&self.store))
        {
            let msg = format!("bb_alloc returned an invalid pointer {ptr}");
            self.faulted = Some(msg.clone());
            return Err(PluginError::Faulted {
                plugin: self.plugin.clone(),
                msg,
            });
        }
        self.memory
            .write(&mut self.store, ptr as usize, bytes)
            .map_err(|e| PluginError::Faulted {
                plugin: self.plugin.clone(),
                msg: e.to_string(),
            })?;
        Ok((ptr, bytes.len() as i32))
    }

    /// Calls `name(ptr, len) -> i32` with `bytes` (`Ok(None)` if absent).
    pub(crate) fn call_bytes(
        &mut self,
        ctx: CallCtx,
        name: &str,
        bytes: &[u8],
    ) -> Result<Option<i32>, PluginError> {
        if let Some(f) = self.func::<(i32, i32), i32>(name) {
            let (p, n) = self.put(bytes)?;
            return self.call_typed(ctx, &f, (p, n)).map(Some);
        }
        if let Some(f) = self.func::<(i32, i32), ()>(name) {
            let (p, n) = self.put(bytes)?;
            return self.call_typed(ctx, &f, (p, n)).map(|_| Some(0));
        }
        Ok(None)
    }

    /// `bb_init(config)`; a nonzero return (or a trap) is an error carrying `bb_error`'s text.
    pub fn init(&mut self, config: &serde_json::Value) -> Result<(), PluginError> {
        let bytes = serde_json::to_vec(config).unwrap_or_default();
        match self.call_bytes(CallCtx::None, "bb_init", &bytes)? {
            None | Some(0) => Ok(()),
            Some(code) => {
                let msg = self
                    .out()
                    .error
                    .take()
                    .unwrap_or_else(|| format!("bb_init returned {code}"));
                self.faulted = Some(msg.clone());
                Err(PluginError::Faulted {
                    plugin: self.plugin.clone(),
                    msg,
                })
            }
        }
    }

    pub fn out(&mut self) -> &mut Outputs {
        &mut self.store.data_mut().out
    }

    /// Fuel left after the last call.
    pub fn fuel_left(&self) -> u64 {
        self.store.get_fuel().unwrap_or(0)
    }

    /// Bytes of linear memory in use.
    pub fn memory_size(&self) -> usize {
        self.memory.data_size(&self.store)
    }
}

/// Human-readable reason for a trap.
fn describe(e: &wasmtime::Error, store: &Store<HostState>) -> String {
    if let Some(t) = e.downcast_ref::<Trap>() {
        return match t {
            Trap::OutOfFuel => {
                "used up its CPU budget for one call (fuel) — stopped so the simulation keeps running".into()
            }
            Trap::Interrupt => {
                "ran longer than its time budget for one call — stopped so the simulation keeps running".into()
            }
            Trap::StackOverflow => "overflowed its stack".into(),
            Trap::MemoryOutOfBounds => "accessed memory out of bounds".into(),
            other => format!("trapped: {other}"),
        };
    }
    let msg = format!("{e:#}");
    if msg.contains("memory") && msg.contains("limit") {
        return format!("exceeded its memory cap ({msg})");
    }
    let _ = store;
    msg
}
