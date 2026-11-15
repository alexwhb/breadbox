//! Parts and instruments from plugins as `bb_core::Device`s: [`WasmDevice`] forwards every
//! device hook to the plugin's wasm exports (`bb_on_start`, `bb_on_wake`, …) with the bench's
//! `DeviceCtx` installed for the call, and keeps the visual / readings / panel state the model
//! pushes. A plugin that traps (budget, permission, bug) is stopped and reported once; the
//! bench carries on without it.

use crate::sandbox::{CallCtx, Compiled, Sandbox};
use bb_circuit::netlist::DeviceSpec;
use bb_core::{Device, DeviceCtx, DeviceEvent, LogLevel, Look, UserInput, VisualState};
use std::path::PathBuf;
use std::sync::Arc;
use wasmtime::TypedFunc;

/// The JSON a device model receives in `bb_init`.
pub fn device_config(c: &Compiled, spec: &DeviceSpec) -> serde_json::Value {
    let m = &c.manifest;
    serde_json::json!({
        "plugin": m.id,
        "version": m.version,
        "role": "device",
        "part": {
            "id": spec.part,
            "reference": spec.reference,
            "def": spec.def,
            "behaviour": spec.behaviour,
            "pins": spec.pins.iter().map(|p| p.pin.clone()).collect::<Vec<_>>(),
            "props": spec.props,
            "pos": spec.pos,
        },
        "specs": m.specs,
        "permissions": m.permissions,
    })
}

/// The flat JSON a model receives in `bb_on_input`.
pub fn input_json(i: &UserInput) -> serde_json::Value {
    match i {
        UserInput::Press(on) => serde_json::json!({"kind": "press", "on": on}),
        UserInput::Toggle { index } => serde_json::json!({"kind": "toggle", "index": index}),
        UserInput::SetSwitch { index, on } => {
            serde_json::json!({"kind": "switch", "index": index, "on": on})
        }
        UserInput::Position(v) => serde_json::json!({"kind": "position", "value": v}),
        UserInput::Value { key, value } => {
            serde_json::json!({"kind": "value", "key": key, "value": value})
        }
    }
}

/// A visual pushed with `bb_set_visual`: a `VisualState`, a `Look`, or `{"text": "…"}`.
pub fn parse_visual(v: &serde_json::Value) -> Option<VisualState> {
    if let Ok(s) = serde_json::from_value::<VisualState>(v.clone()) {
        return Some(s);
    }
    if let Ok(l) = serde_json::from_value::<Look>(v.clone()) {
        return Some(l.into());
    }
    v.get("text")
        .and_then(|t| t.as_str())
        .map(|t| Look::Readout { text: t.into() }.into())
}

struct Hooks {
    start: Option<TypedFunc<(), ()>>,
    wake: Option<TypedFunc<i64, ()>>,
    edge: Option<TypedFunc<(i32, i32), ()>>,
    tick: Option<TypedFunc<(), ()>>,
}

/// A part / instrument whose behaviour is a plugin's wasm model.
pub struct WasmDevice {
    sb: Option<Sandbox>,
    plugin: String,
    behaviour: String,
    hooks: Hooks,
    visual: VisualState,
    readings: Vec<(String, f64)>,
    panel: Option<serde_json::Value>,
    error: Option<String>,
    reported: bool,
}

/// Erases the borrow lifetime of a context for the duration of one call.
fn ctx_ptr(ctx: &mut dyn DeviceCtx) -> CallCtx {
    let p: *mut (dyn DeviceCtx + '_) = ctx;
    // SAFETY: the pointer is only dereferenced by host functions during the synchronous call
    // that `WasmDevice::invoke` makes while `ctx` is mutably borrowed; `Sandbox` clears it when
    // the call returns.
    CallCtx::Device(unsafe {
        std::mem::transmute::<*mut (dyn DeviceCtx + '_), *mut (dyn DeviceCtx + 'static)>(p)
    })
}

impl WasmDevice {
    /// Instantiates the plugin for one part and runs `bb_init`.
    pub fn new(c: &Arc<Compiled>, spec: &DeviceSpec, project_dir: Option<PathBuf>) -> WasmDevice {
        let seed = spec.part.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
        });
        let mut d = WasmDevice {
            sb: None,
            plugin: c.manifest.id.clone(),
            behaviour: spec.behaviour.clone(),
            hooks: Hooks {
                start: None,
                wake: None,
                edge: None,
                tick: None,
            },
            visual: Look::Readout {
                text: c.manifest.display_name(),
            }
            .into(),
            readings: Vec::new(),
            panel: None,
            error: None,
            reported: false,
        };
        let sb = Sandbox::new(c, seed, project_dir.as_deref()).and_then(|mut sb| {
            sb.init(&device_config(c, spec))?;
            Ok(sb)
        });
        match sb {
            Ok(mut sb) => {
                d.hooks = Hooks {
                    start: sb.func("bb_on_start"),
                    wake: sb.func("bb_on_wake"),
                    edge: sb.func("bb_on_edge"),
                    tick: sb.func("bb_on_tick"),
                };
                d.sb = Some(sb);
                d.pull();
            }
            Err(e) => d.fail(e.to_string()),
        }
        d
    }

    /// Why the plugin stopped, if it did.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn fail(&mut self, msg: String) {
        self.visual = VisualState::new(Look::Readout {
            text: "plugin stopped".into(),
        });
        self.error = Some(msg);
    }

    /// Takes what the model pushed.
    fn pull(&mut self) {
        let Some(sb) = self.sb.as_mut() else { return };
        let out = sb.out();
        if let Some(v) = out.visual.take()
            && let Some(vs) = parse_visual(&v)
        {
            self.visual = vs;
        }
        if let Some(r) = out.readings.take() {
            self.readings = r;
        }
        if let Some(p) = out.panel.take() {
            self.panel = Some(p);
        }
    }

    /// Reports a failure once through the bench log.
    fn report(&mut self, ctx: &mut dyn DeviceCtx) {
        if self.reported {
            return;
        }
        if let Some(e) = &self.error {
            self.reported = true;
            ctx.emit(DeviceEvent::Log {
                level: LogLevel::Error,
                text: format!("plugin {}: {e}", self.plugin),
            });
        }
    }

    fn invoke<R>(
        &mut self,
        ctx: &mut dyn DeviceCtx,
        f: impl FnOnce(&mut Sandbox, CallCtx) -> Result<R, crate::PluginError>,
    ) -> Option<R> {
        if self.error.is_some() {
            self.report(ctx);
            return None;
        }
        let call = ctx_ptr(ctx);
        let sb = self.sb.as_mut()?;
        let r = f(sb, call);
        self.pull();
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.fail(e.to_string());
                self.report(ctx);
                None
            }
        }
    }
}

impl Device for WasmDevice {
    fn kind(&self) -> &str {
        &self.behaviour
    }
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {
        if let Some(f) = self.hooks.start.clone() {
            self.invoke(ctx, |sb, c| sb.call_typed(c, &f, ()));
        } else {
            self.report(ctx);
        }
    }
    fn on_wake(&mut self, ctx: &mut dyn DeviceCtx, token: u64) {
        if let Some(f) = self.hooks.wake.clone() {
            self.invoke(ctx, |sb, c| sb.call_typed(c, &f, token as i64));
        }
    }
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, high: bool) {
        if let Some(f) = self.hooks.edge.clone() {
            self.invoke(ctx, |sb, c| sb.call_typed(c, &f, (pin as i32, high as i32)));
        }
    }
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {
        if let Some(f) = self.hooks.tick.clone() {
            self.invoke(ctx, |sb, c| sb.call_typed(c, &f, ()));
        }
    }
    fn on_input(&mut self, ctx: &mut dyn DeviceCtx, input: &UserInput) {
        let bytes = serde_json::to_vec(&input_json(input)).unwrap_or_default();
        self.invoke(ctx, |sb, c| sb.call_bytes(c, "bb_on_input", &bytes));
    }
    fn set_param(&mut self, ctx: &mut dyn DeviceCtx, key: &str, value: f64) -> bool {
        let key = key.to_string();
        self.invoke(ctx, |sb, c| {
            let Some(f) = sb.func::<(i32, i32, f64), i32>("bb_set_param") else {
                return Ok(0);
            };
            let (p, n) = sb.put(key.as_bytes())?;
            sb.call_typed(c, &f, (p, n, value))
        })
        .is_some_and(|r| r != 0)
    }
    fn visual(&self) -> VisualState {
        self.visual.clone()
    }
    fn readings(&self) -> Vec<(String, f64)> {
        self.readings.clone()
    }
    fn panel(&self) -> Option<serde_json::Value> {
        self.panel.clone()
    }
    fn save_state(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
    fn i2c_transfer(
        &mut self,
        ctx: &mut dyn DeviceCtx,
        addr: u8,
        write: &[u8],
        read: usize,
    ) -> Option<Vec<u8>> {
        let data = write.to_vec();
        let r = self.invoke(ctx, |sb, c| {
            let Some(f) = sb.func::<(i32, i32, i32, i32), i32>("bb_i2c_transfer") else {
                return Ok(None);
            };
            sb.out().result = None;
            let (p, n) = sb.put(&data)?;
            let code = sb.call_typed(c, &f, (addr as i32, p, n, read as i32))?;
            if code < 0 {
                return Ok(None);
            }
            Ok(Some(sb.out().result.take().unwrap_or_default()))
        })?;
        r.map(|mut v| {
            v.resize(read, 0xff);
            v
        })
    }
}
