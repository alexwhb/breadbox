//! Sandbox guarantees (PLUG-2) with hand-written WebAssembly: permission-checked imports,
//! fuel and epoch budgets stopping runaway plugins without hurting the bench, memory caps, no
//! WASI without `fs:project`, clear ABI errors.

mod common;
use bb_core::SimTime;
use bb_plugin::sandbox::{Compiled, Sandbox};
use bb_plugin::{Manifest, PluginError, PluginHost};
use common::*;
use std::sync::Arc;

/// A plugin part between the UNO's 5V and GND: R = ext `r` (1 kΩ by default).
const PARTS: &str = r#"[{
  "id": "plugres", "name": "Plugin resistor", "category": "building_blocks",
  "ref_prefix": "R", "description": "test", "pins": ["1:passive", "2:passive"],
  "model": {"spice": ["$(el:R) $(1) $(2) {$(ext:r)}"], "ext": [{"name": "r", "default": 1000}],
            "behaviour": "wasm"},
  "bench": {"layout": "row", "off_board": true, "size": [4, 2]}
}]"#;

/// Common WAT: memory, abi version, bump allocator, a string "r" at 16.
fn module(body: &str, imports: &str) -> String {
    format!(
        r#"(module
  {imports}
  (memory (export "memory") 1)
  (data (i32.const 16) "r")
  (global $heap (mut i32) (i32.const 4096))
  (func (export "bb_abi_version") (result i32) i32.const 1)
  (func (export "bb_alloc") (param $n i32) (result i32) (local $p i32)
    global.get $heap local.set $p
    global.get $heap local.get $n i32.add global.set $heap
    local.get $p)
  {body}
)"#
    )
}

fn bench_with(dir: &std::path::Path) -> (bb_sim::Bench, PluginHost) {
    let host = PluginHost::from_dirs(&[dir.to_path_buf()], None);
    assert!(host.errors.is_empty(), "{:?}", host.errors);
    let lib = host.library();
    let mut b =
        bb_circuit::builder::BenchBuilder::new(&lib, "t", bb_circuit::project::Mode::Playground);
    let u1 = b.offboard("uno", "U1", 0.0, 0.0, &[]);
    let r = b.offboard("plugres", "R1", 20.0, 0.0, &[]);
    b.wire((&u1, "5V"), (&r, "1"), "red");
    b.wire((&u1, "GND"), (&r, "2"), "black");
    let p = b.finish();
    let bench = bb_sim::Bench::new(p, lib, host.bench_options()).unwrap();
    (bench, host)
}

fn current(b: &bb_sim::Bench) -> f64 {
    b.element_current("r1", "R").unwrap()
}

fn errors(b: &mut bb_sim::Bench) -> Vec<String> {
    b.take_events()
        .into_iter()
        .filter_map(|e| match e.event {
            bb_core::DeviceEvent::Log { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn a_plugin_without_inject_load_cannot_load_the_circuit() {
    let tmp = Tmp::new("noload");
    // 1. declaring a load without the permission is rejected
    let d = tmp.plugin(
        "test.noload",
        r#"["instrument"]"#,
        r#"["read:nets"]"#,
        &module("", ""),
        Some(PARTS),
        r#""spice": {"load": "R 1k"},"#,
    );
    let e = Manifest::load(&d).unwrap_err();
    assert!(
        matches!(e, PluginError::PermissionDenied { ref action, .. } if action.contains("load")),
        "{e}"
    );
    // 2. an instrument whose part puts elements across its probes needs it too
    let d = tmp.plugin(
        "test.noload2",
        r#"["instrument"]"#,
        r#"["read:nets"]"#,
        &module("", ""),
        Some(PARTS),
        "",
    );
    let host = PluginHost::from_dirs(&[d], None);
    assert!(
        host.errors.iter().any(|e| matches!(
            e,
            PluginError::PermissionDenied {
                permission: bb_plugin::Permission::InjectLoad,
                ..
            }
        )),
        "{:?}",
        host.errors
    );
    // 3. at run time: changing its load without the permission stops the plugin, the circuit
    //    keeps its old value and the bench keeps running
    let wat = module(
        r#"(func (export "bb_on_start") i32.const 16 i32.const 1 f64.const 10 call $set_ext)"#,
        r#"(import "bb" "bb_set_ext" (func $set_ext (param i32 i32 f64)))"#,
    );
    let d = tmp.plugin("test.sneaky", r#"["part"]"#, "[]", &wat, Some(PARTS), "");
    let (mut b, _h) = bench_with(&d);
    b.run_for(SimTime::from_ms(20)).unwrap();
    let i = current(&b);
    assert!((i - 5.0e-3).abs() < 0.3e-3, "R stayed 1 kΩ: {i}");
    let log = errors(&mut b);
    assert!(
        log.iter()
            .any(|l| l.contains("permission denied") && l.contains("inject:load")),
        "{log:?}"
    );
    // the same plugin with the permission works
    let d = tmp.plugin(
        "test.allowed",
        r#"["part"]"#,
        r#"["inject:load"]"#,
        &wat,
        Some(PARTS),
        "",
    );
    let (mut b, _h) = bench_with(&d);
    b.run_for(SimTime::from_ms(20)).unwrap();
    let i = current(&b);
    assert!(i > 0.1, "R is now 10 Ω: {i}");
}

#[test]
fn fuel_stops_a_runaway_plugin_without_hurting_the_sim() {
    let tmp = Tmp::new("fuel");
    let wat = module(
        r#"(func (export "bb_on_start") i64.const 1000000000 call $tick)
           (func (export "bb_on_tick") (loop br 0))"#,
        r#"(import "bb" "bb_tick_every" (func $tick (param i64)))"#,
    );
    let d = tmp.plugin("test.spin", r#"["part"]"#, "[]", &wat, Some(PARTS), "");
    let (mut b, _h) = bench_with(&d);
    let t0 = std::time::Instant::now();
    b.run_until(SimTime::from_ms(50)).unwrap();
    assert!(t0.elapsed().as_secs_f64() < 5.0);
    assert_eq!(b.time(), SimTime::from_ms(50), "the bench kept going");
    let log = errors(&mut b);
    assert!(log.iter().any(|l| l.contains("CPU budget")), "{log:?}");
    // the circuit is intact
    assert!((current(&b) - 5.0e-3).abs() < 0.3e-3);
    let tel = b.telemetry();
    let p = tel.parts.iter().find(|p| p.part == "r1").unwrap();
    assert_eq!(
        p.visual.look,
        bb_core::Look::Readout {
            text: "plugin stopped".into()
        }
    );
}

#[test]
fn epoch_deadline_stops_a_slow_call() {
    let tmp = Tmp::new("epoch");
    // maximum fuel, but every iteration is a slow host call: the wall-clock budget (epoch)
    // stops it long before the fuel would
    let wat = module(
        r#"(func (export "bb_on_start") (loop call $rng drop br 0))"#,
        r#"(import "bb" "bb_rng" (func $rng (result f64)))"#,
    );
    let d = tmp.plugin(
        "test.slow",
        r#"["part"]"#,
        "[]",
        &wat,
        Some(PARTS),
        r#""limits": {"fuel_per_call": 2000000000000},"#,
    );
    let t0 = std::time::Instant::now();
    let (mut b, _h) = bench_with(&d);
    assert!(t0.elapsed().as_secs_f64() < 10.0, "{:?}", t0.elapsed());
    let log = errors(&mut b);
    assert!(log.iter().any(|l| l.contains("time budget")), "{log:?}");
    b.run_until(SimTime::from_ms(5)).unwrap();
}

#[test]
fn memory_cap_is_enforced() {
    let tmp = Tmp::new("mem");
    // grows by 64 pages (4 MiB) with a 1 MiB cap
    let wat = module(
        r#"(func (export "bb_on_start") i32.const 64 memory.grow drop)"#,
        "",
    );
    let d = tmp.plugin(
        "test.hungry",
        r#"["part"]"#,
        "[]",
        &wat,
        Some(PARTS),
        r#""limits": {"memory_mb": 1},"#,
    );
    let (mut b, _h) = bench_with(&d);
    b.run_for(SimTime::from_ms(1)).unwrap();
    let log = errors(&mut b);
    assert!(log.iter().any(|l| l.contains("memory")), "{log:?}");
    // a module asking for too much memory up front cannot even start
    let big = r#"(module (memory (export "memory") 64)
        (func (export "bb_abi_version") (result i32) i32.const 1)
        (func (export "bb_alloc") (param i32) (result i32) i32.const 0))"#;
    let d = tmp.plugin(
        "test.big",
        r#"["part"]"#,
        "[]",
        big,
        Some(PARTS),
        r#""limits": {"memory_mb": 1},"#,
    );
    let m = Arc::new(Manifest::load(&d).unwrap());
    let c = Compiled::new(m, d.clone(), &std::fs::read(d.join("model.wasm")).unwrap()).unwrap();
    let e = Sandbox::new(&c, 1, None)
        .err()
        .expect("instantiation must fail");
    assert!(e.to_string().contains("instantiate"), "{e}");
}

#[test]
fn no_wasi_without_fs_project_and_clear_abi_errors() {
    let tmp = Tmp::new("wasi");
    let wasi = module(
        "",
        r#"(import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))"#,
    );
    let d = tmp.plugin("test.wasi", r#"["part"]"#, "[]", &wasi, Some(PARTS), "");
    let m = Arc::new(Manifest::load(&d).unwrap());
    let bytes = std::fs::read(d.join("model.wasm")).unwrap();
    let e = Compiled::new(m, d.clone(), &bytes).unwrap_err();
    assert!(
        matches!(
            e,
            PluginError::PermissionDenied {
                permission: bb_plugin::Permission::FsProject,
                ..
            }
        ),
        "{e}"
    );
    // with fs:project the WASI imports link (preopened to the project folder only)
    let d = tmp.plugin(
        "test.wasi2",
        r#"["part"]"#,
        r#"["fs:project"]"#,
        &wasi,
        Some(PARTS),
        "",
    );
    let m = Arc::new(Manifest::load(&d).unwrap());
    let c = Compiled::new(m, d.clone(), &bytes).unwrap();
    let proj = tmp.dir("project");
    assert!(Sandbox::new(&c, 1, Some(&proj)).is_ok());
    // unknown host function
    let bad = module("", r#"(import "bb" "bb_format_disk" (func))"#);
    let d = tmp.plugin("test.bad", r#"["part"]"#, "[]", &bad, Some(PARTS), "");
    let m = Arc::new(Manifest::load(&d).unwrap());
    let e = Compiled::new(m, d.clone(), &std::fs::read(d.join("model.wasm")).unwrap()).unwrap_err();
    assert!(e.to_string().contains("bb.bb_format_disk"), "{e}");
    // wrong ABI version
    let v2 = r#"(module (memory (export "memory") 1)
        (func (export "bb_abi_version") (result i32) i32.const 2)
        (func (export "bb_alloc") (param i32) (result i32) i32.const 0))"#;
    let d = tmp.plugin("test.v2", r#"["part"]"#, "[]", v2, Some(PARTS), "");
    let m = Arc::new(Manifest::load(&d).unwrap());
    let c = Compiled::new(m, d.clone(), &std::fs::read(d.join("model.wasm")).unwrap()).unwrap();
    let e = Sandbox::new(&c, 1, None).err().unwrap();
    assert!(e.to_string().contains("ABI 2"), "{e}");
    // missing exports
    let bare = r#"(module (memory (export "memory") 1))"#;
    let d = tmp.plugin("test.bare", r#"["part"]"#, "[]", bare, Some(PARTS), "");
    let m = Arc::new(Manifest::load(&d).unwrap());
    let e = Compiled::new(m, d.clone(), &std::fs::read(d.join("model.wasm")).unwrap()).unwrap_err();
    assert!(e.to_string().contains("bb_abi_version"), "{e}");
}

#[test]
fn reading_nets_needs_read_nets() {
    let tmp = Tmp::new("read");
    let wat = module(
        r#"(func (export "bb_on_start") i32.const 0 call $v drop)"#,
        r#"(import "bb" "bb_pin_voltage" (func $v (param i32) (result f64)))"#,
    );
    let d = tmp.plugin("test.peek", r#"["part"]"#, "[]", &wat, Some(PARTS), "");
    let (mut b, _h) = bench_with(&d);
    let log = errors(&mut b);
    assert!(log.iter().any(|l| l.contains("read:nets")), "{log:?}");
}
