#![allow(dead_code)]
//! Shared helpers for bb-sim scenario tests.

use bb_circuit::builder::BenchBuilder;
use bb_circuit::project::Mode;
use bb_circuit::{Library, Project, PropValue};
use bb_sim::{Bench, BenchOptions};
use std::path::PathBuf;
use std::sync::Arc;

pub fn lib() -> Arc<Library> {
    Library::builtin()
}

pub fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

pub fn fixture(name: &str) -> bb_avr::FirmwareImage {
    let p = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
        .join(format!("{name}.uno.hex"));
    bb_sim::firmware::load_image(&p).unwrap_or_else(|e| panic!("{e}"))
}

pub fn n(v: f64) -> PropValue {
    PropValue::Num(v)
}
pub fn t(v: &str) -> PropValue {
    PropValue::Text(v.into())
}

/// A bench with an UNO (U1) at the origin and a 400-hole breadboard (BB1) next to it, its top
/// rails wired to 5V / GND.
pub fn uno_bench<'a>(lib: &'a Library, name: &str) -> (BenchBuilder<'a>, String, String) {
    let mut b = BenchBuilder::new(lib, name, Mode::Playground);
    let u1 = b.offboard("uno", "U1", 0.0, 4.0, &[]);
    let bb = b.breadboard("breadboard_400", 32.0, 2.0);
    b.wire((&u1, "5V"), (&bb, "t+1"), "red");
    b.wire((&u1, "GND"), (&bb, "t-1"), "black");
    (b, u1, bb)
}

pub fn start(project: Project, firmware: &[(&str, &str)]) -> Bench {
    start_with(project, firmware, BenchOptions::default())
}

pub fn start_with(project: Project, firmware: &[(&str, &str)], mut opts: BenchOptions) -> Bench {
    for (part, fx) in firmware {
        opts.firmware.insert(part.to_string(), fixture(fx));
    }
    Bench::new(project, lib(), opts).unwrap_or_else(|e| panic!("{e}"))
}

/// Firmware lines printed so far.
pub fn lines(b: &Bench, part: &str) -> Vec<String> {
    b.serial(part)
        .map(|s| s.data_lines().map(|l| l.text.clone()).collect())
        .unwrap_or_default()
}

/// Value of `key=` in a `k=v k2=v2` line.
pub fn field(line: &str, key: &str) -> Option<f64> {
    line.split_whitespace()
        .find_map(|tok| tok.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.trim_end_matches('%').parse().ok())
}
