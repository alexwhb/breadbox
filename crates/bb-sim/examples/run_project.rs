//! Runs a project folder headless and prints its serial monitor + events.
//!
//!     cargo run -p bb-sim --release --example run_project -- examples/garden-node 5 [image.hex]

use bb_circuit::{Library, Project};
use bb_core::SimTime;
use bb_sim::{Bench, BenchOptions};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "examples/garden-node".into());
    let secs: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5.0);
    let project = Project::load(&dir).expect("project");
    let lib = Library::builtin();
    // API-level boards (ESP32 / Pico): build their sketches with the host compiler (cached)
    let host_firmware = match bb_sim::firmware::compile_host_project(&project, &dir, &lib) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("host build: {e}");
            Default::default()
        }
    };
    let mut opts = BenchOptions {
        firmware: bb_sim::firmware::project_images(&project, &dir),
        host_firmware,
        ..Default::default()
    };
    if let Some(img) = args.get(3) {
        let mcu = project
            .parts
            .iter()
            .find(|p| p.firmware.is_some())
            .map(|p| p.id.clone())
            .unwrap_or_default();
        opts.firmware
            .insert(mcu, bb_sim::firmware::load_image(img).expect("image"));
    }
    let mut bench = Bench::new(project, lib, opts).expect("bench");
    let t0 = Instant::now();
    bench.run_until(SimTime::from_secs_f64(secs)).expect("run");
    let wall = t0.elapsed().as_secs_f64();
    for part in bench.mcu_parts() {
        for l in &bench.serial(&part).unwrap().lines {
            println!("{} {:?} {}", l.time, l.kind, l.text);
        }
    }
    for e in bench.take_events() {
        if !matches!(e.event, bb_core::DeviceEvent::Audio(_)) {
            println!("event {} {} {:?}", e.time, e.source, e.event);
        }
    }
    let tel = bench.telemetry();
    for p in &tel.probes {
        println!(
            "probe {:<22} {:>10}  min {:.3} max {:.3} avg {:.3}",
            p.label, p.text, p.min, p.max, p.avg
        );
    }
    for m in &tel.meters {
        println!("meter {} {}", m.id, m.display);
    }
    println!("{}", tel.erc_summary);
    for i in tel.erc.iter().chain(tel.drc.iter()) {
        println!("  {:?} {}: {}", i.severity, i.code, i.message);
    }
    println!("{:?}", bench.stats);
    println!(
        "simulated {secs} s in {wall:.3} s wall → {:.1}× real time",
        secs / wall
    );
}
