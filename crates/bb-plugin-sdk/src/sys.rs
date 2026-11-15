//! Raw host imports (module `bb`). Every string / buffer is a `(ptr, len)` pair in the
//! plugin's linear memory; times are `i64` picoseconds unless the name ends in `_s`. See
//! `docs/PLUGIN-SDK.md` for the permission each one needs.

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "bb")]
unsafe extern "C" {
    pub fn bb_log(level: i32, ptr: i32, len: i32);
    pub fn bb_error(ptr: i32, len: i32);
    pub fn bb_now() -> i64;
    pub fn bb_now_s() -> f64;
    pub fn bb_rng() -> f64;
    pub fn bb_pin_voltage(pin: i32) -> f64;
    pub fn bb_pin_current(pin: i32) -> f64;
    pub fn bb_element_current(ptr: i32, len: i32) -> f64;
    pub fn bb_element_charge(ptr: i32, len: i32) -> f64;
    pub fn bb_element_energy(ptr: i32, len: i32) -> f64;
    pub fn bb_track_element(ptr: i32, len: i32);
    pub fn bb_watch(pin: i32, lo: f64, hi: f64);
    pub fn bb_capture(pin: i32, period_ps: i64);
    pub fn bb_samples(pin: i32, from_ps: i64, to_ps: i64, out: i32, max: i32) -> i32;
    pub fn bb_set_drive(pin: i32, kind: i32, v: f64, r: f64);
    pub fn bb_set_ext(ptr: i32, len: i32, value: f64);
    pub fn bb_wake_at(t_ps: i64, token: i64);
    pub fn bb_cancel_wake(token: i64);
    pub fn bb_tick_every(period_ps: i64);
    pub fn bb_world_get(ptr: i32, len: i32) -> f64;
    pub fn bb_world_set(ptr: i32, len: i32, value: f64);
    pub fn bb_world_water(ptr: i32, len: i32, ml: f64);
    pub fn bb_world_secs() -> f64;
    pub fn bb_emit(ptr: i32, len: i32);
    pub fn bb_set_visual(ptr: i32, len: i32);
    pub fn bb_set_readings(ptr: i32, len: i32);
    pub fn bb_set_panel(ptr: i32, len: i32);
    pub fn bb_result(ptr: i32, len: i32);
    pub fn bb_spice_run(ptr: i32, len: i32) -> i32;
    pub fn bb_inbox_read(out: i32, max: i32) -> i32;
    pub fn bb_mqtt_subscribe(ptr: i32, len: i32) -> i32;
    pub fn bb_mqtt_publish(
        tptr: i32,
        tlen: i32,
        pptr: i32,
        plen: i32,
        qos: i32,
        retain: i32,
    ) -> i32;
    pub fn bb_mqtt_poll() -> i32;
    pub fn bb_lan_bridge(ptr: i32, len: i32) -> i32;
}

/// Native builds (docs, `cargo check` on the host): inert stand-ins.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::missing_safety_doc)]
mod native {
    pub unsafe fn bb_log(_: i32, _: i32, _: i32) {}
    pub unsafe fn bb_error(_: i32, _: i32) {}
    pub unsafe fn bb_now() -> i64 {
        0
    }
    pub unsafe fn bb_now_s() -> f64 {
        0.0
    }
    pub unsafe fn bb_rng() -> f64 {
        0.5
    }
    pub unsafe fn bb_pin_voltage(_: i32) -> f64 {
        0.0
    }
    pub unsafe fn bb_pin_current(_: i32) -> f64 {
        0.0
    }
    pub unsafe fn bb_element_current(_: i32, _: i32) -> f64 {
        f64::NAN
    }
    pub unsafe fn bb_element_charge(_: i32, _: i32) -> f64 {
        f64::NAN
    }
    pub unsafe fn bb_element_energy(_: i32, _: i32) -> f64 {
        f64::NAN
    }
    pub unsafe fn bb_track_element(_: i32, _: i32) {}
    pub unsafe fn bb_watch(_: i32, _: f64, _: f64) {}
    pub unsafe fn bb_capture(_: i32, _: i64) {}
    pub unsafe fn bb_samples(_: i32, _: i64, _: i64, _: i32, _: i32) -> i32 {
        0
    }
    pub unsafe fn bb_set_drive(_: i32, _: i32, _: f64, _: f64) {}
    pub unsafe fn bb_set_ext(_: i32, _: i32, _: f64) {}
    pub unsafe fn bb_wake_at(_: i64, _: i64) {}
    pub unsafe fn bb_cancel_wake(_: i64) {}
    pub unsafe fn bb_tick_every(_: i64) {}
    pub unsafe fn bb_world_get(_: i32, _: i32) -> f64 {
        f64::NAN
    }
    pub unsafe fn bb_world_set(_: i32, _: i32, _: f64) {}
    pub unsafe fn bb_world_water(_: i32, _: i32, _: f64) {}
    pub unsafe fn bb_world_secs() -> f64 {
        0.0
    }
    pub unsafe fn bb_emit(_: i32, _: i32) {}
    pub unsafe fn bb_set_visual(_: i32, _: i32) {}
    pub unsafe fn bb_set_readings(_: i32, _: i32) {}
    pub unsafe fn bb_set_panel(_: i32, _: i32) {}
    pub unsafe fn bb_result(_: i32, _: i32) {}
    pub unsafe fn bb_spice_run(_: i32, _: i32) -> i32 {
        0
    }
    pub unsafe fn bb_inbox_read(_: i32, _: i32) -> i32 {
        0
    }
    pub unsafe fn bb_mqtt_subscribe(_: i32, _: i32) -> i32 {
        -1
    }
    pub unsafe fn bb_mqtt_publish(_: i32, _: i32, _: i32, _: i32, _: i32, _: i32) -> i32 {
        -1
    }
    pub unsafe fn bb_mqtt_poll() -> i32 {
        0
    }
    pub unsafe fn bb_lan_bridge(_: i32, _: i32) -> i32 {
        -1
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub use native::*;
