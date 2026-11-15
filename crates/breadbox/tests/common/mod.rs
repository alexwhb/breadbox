//! Shared helpers for the GUI tests (`TEST-5`).
#![allow(dead_code)]

use breadbox::{AppOptions, BreadboxApp};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const W: f32 = 1440.0;
pub const H: f32 = 900.0;

pub fn app_harness(opts: AppOptions) -> Harness<'static, BreadboxApp> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(W, H))
        .with_pixels_per_point(1.0)
        .with_max_steps(64)
        .wgpu()
        .build_eframe(move |cc| BreadboxApp::new(cc, opts));
    h.run_steps(3);
    h
}

pub fn harness() -> Harness<'static, BreadboxApp> {
    app_harness(AppOptions::test())
}

/// Steps until `cond` holds (background jobs and the bench run on their own threads).
pub fn wait_for(h: &mut Harness<'static, BreadboxApp>, secs: f64, mut cond: impl FnMut(&mut Harness<'static, BreadboxApp>) -> bool) -> bool {
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        h.run_steps(1);
        if cond(h) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    false
}

pub fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Renders and saves `docs/screenshots/<name>.png` (1440×900).
pub fn screenshot(h: &mut Harness<'static, BreadboxApp>, name: &str) {
    h.run_steps(2);
    match h.render() {
        Ok(img) => {
            let path = repo().join("docs/screenshots").join(format!("{name}.png"));
            img.save(&path).expect("save screenshot");
        }
        Err(e) => panic!("render failed: {e}"),
    }
}

pub fn has_label(h: &Harness<'static, BreadboxApp>, label: &str) -> bool {
    h.query_by_label(label).is_some()
}

pub fn has_label_containing(h: &Harness<'static, BreadboxApp>, label: &str) -> bool {
    h.query_by_label_contains(label).is_some()
}
