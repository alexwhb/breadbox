//! Behavioural devices (`bb_core::Device` implementations) and the registry that maps a part's
//! behaviour id to a factory. Plugins and the radio/host-board glue register their own
//! factories (or replace built-in ones, e.g. a full ESP-AT modem for `esp01`).

mod basic;
mod i2c;
mod lcd;
mod logic;
mod motion;
mod onewire;
mod uart;
mod ws2812;

pub use i2c::{I2cRegs, I2cSlave};
pub use lcd::Hd44780;
pub use uart::{AtStub, UartPort};

use bb_circuit::PropValue;
use bb_circuit::netlist::DeviceSpec;
use bb_core::{Device, DeviceCtx, Look, VisualState};
use std::collections::HashMap;
use std::sync::Arc;

/// Creates a device for a part.
pub type Factory = Arc<dyn Fn(&DeviceSpec) -> Box<dyn Device> + Send + Sync>;

/// Behaviour id → factory.
#[derive(Clone, Default)]
pub struct DeviceRegistry {
    map: HashMap<String, Factory>,
}

impl DeviceRegistry {
    /// Every built-in behaviour.
    pub fn builtin() -> DeviceRegistry {
        let mut r = DeviceRegistry::default();
        basic::register(&mut r);
        motion::register(&mut r);
        logic::register(&mut r);
        onewire::register(&mut r);
        i2c::register(&mut r);
        lcd::register(&mut r);
        ws2812::register(&mut r);
        uart::register(&mut r);
        r
    }
    pub fn register(&mut self, id: &str, f: Factory) {
        self.map.insert(id.into(), f);
    }
    pub fn register_fn(
        &mut self,
        id: &str,
        f: impl Fn(&DeviceSpec) -> Box<dyn Device> + Send + Sync + 'static,
    ) {
        self.map.insert(id.into(), Arc::new(f));
    }
    pub fn create(&self, spec: &DeviceSpec) -> Option<Box<dyn Device>> {
        self.map.get(&spec.behaviour).map(|f| f(spec))
    }
    pub fn contains(&self, id: &str) -> bool {
        self.map.contains_key(id)
    }
    pub fn ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.map.keys().cloned().collect();
        v.sort();
        v
    }
}

/// A device that does nothing (unknown behaviours, removed parts, instruments).
pub struct NullDevice;

impl Device for NullDevice {
    fn kind(&self) -> &str {
        "null"
    }
    fn visual(&self) -> VisualState {
        VisualState::new(Look::None)
    }
}

/// Index of a pin by id (`usize::MAX` if absent — device methods then no-op).
pub fn pin_of(spec: &DeviceSpec, id: &str) -> usize {
    spec.pins
        .iter()
        .position(|p| p.pin == id)
        .unwrap_or(usize::MAX)
}

pub fn prop_f64(spec: &DeviceSpec, key: &str, default: f64) -> f64 {
    spec.props
        .get(key)
        .and_then(|v| v.as_f64())
        .unwrap_or(default)
}

pub fn prop_text(spec: &DeviceSpec, key: &str, default: &str) -> String {
    spec.props
        .get(key)
        .map(|v| v.as_text())
        .unwrap_or_else(|| default.to_string())
}

pub fn prop_bool(spec: &DeviceSpec, key: &str, default: bool) -> bool {
    match spec.props.get(key) {
        Some(PropValue::Bool(b)) => *b,
        Some(v) => v.as_f64().map(|x| x != 0.0).unwrap_or(default),
        None => default,
    }
}

/// Pin helper used by devices: valid index?
pub(crate) fn ok(pin: usize) -> bool {
    pin != usize::MAX
}

/// Reads a pin as a logic level against a fraction of a supply pin's voltage.
pub(crate) fn logic(ctx: &dyn DeviceCtx, pin: usize, vcc: f64) -> bool {
    ok(pin) && ctx.pin_voltage(pin) > 0.5 * vcc
}
