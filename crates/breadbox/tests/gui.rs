//! GUI end-to-end tests (`TEST-5`): drive the real app through AccessKit labels with
//! `egui_kittest` and save 1440×900 screenshots of each screen to `docs/screenshots/`.

mod common;

use common::*;
use egui_kittest::kittest::Queryable;

#[test]
fn home_screen_renders() {
    let mut h = harness();
    assert!(has_label(&h, "Breadbox"));
    assert!(has_label(&h, "New project"));
    assert!(has_label_containing(&h, "Open example garden-node"));
    screenshot(&mut h, "home");
}
