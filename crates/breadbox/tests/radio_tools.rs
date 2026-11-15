//! Virtual phone (`RF-6`) + MQTT explorer (`RF-7`) GUI tests: ble-button's HM-10 is scanned,
//! connected and subscribed by the phone, which receives the button's notification; the
//! explorer subscribes with a wildcard and sees garden-node's `garden/pot1`. Saves
//! `docs/screenshots/phone.png` and `mqtt.png`.

mod common;

use breadbox::BreadboxApp;
use common::*;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

type H = Harness<'static, BreadboxApp>;

fn open_example(h: &mut H, name: &str) {
    h.get_by_label(&format!("Open example {name}")).click();
    h.run_steps(4);
}

fn first_label(h: &H, prefix: &str) -> Option<String> {
    h.query_all_by_label_contains(prefix).next().and_then(|n| {
        let a = n.accesskit_node();
        a.label().or_else(|| a.value())
    })
}

#[test]
fn phone_scans_connects_subscribes_and_gets_the_button_notify() {
    let mut h = harness();
    open_example(&mut h, "ble-button");
    h.get_by_label("Run").click();
    h.run_steps(2);
    h.get_by_label("Virtual phone").click();
    h.run_steps(3);
    assert!(
        wait_for(&mut h, 120.0, |h| first_label(h, "Phone: connect ")
            .is_some()),
        "no advertiser in the scan list"
    );
    assert!(first_label(&h, " RSSI ").is_some_and(|l| l.contains("dBm")));
    // connect once the sketch has set the module up (it then reports OK+CONN on its own line)
    assert!(wait_for(&mut h, 120.0, |h| h
        .state()
        .ctx
        .bench
        .serial
        .iter()
        .any(|l| l.text == "ble-button ready")));
    let c = first_label(&h, "Phone: connect ").unwrap();
    h.get_by_label(&c).click();
    assert!(
        wait_for(&mut h, 60.0, |h| first_label(h, "Phone: Connected to")
            .is_some()),
        "not connected"
    );
    assert!(
        wait_for(&mut h, 60.0, |h| first_label(h, "Notify ").is_some()),
        "no notifiable characteristic"
    );
    let n = first_label(&h, "Notify ").unwrap();
    h.get_by_label(&n).click();
    assert!(wait_for(&mut h, 30.0, |h| h
        .state()
        .ctx
        .bench
        .serial
        .iter()
        .any(|l| l.text == "phone connected")));
    // press the bench button: the sketch prints "pressed #1" to the HM-10 → notify
    let press = |h: &mut H, on: bool| {
        if let Some(sim) = h.state_mut().ctx.sim() {
            sim.send(bb_sim::Command::Input {
                part: "sw1".into(),
                input: bb_core::UserInput::Press(on),
            });
        }
    };
    press(&mut h, true);
    h.run_steps(20);
    std::thread::sleep(std::time::Duration::from_millis(100));
    press(&mut h, false);
    assert!(
        wait_for(&mut h, 60.0, |h| first_label(h, "Phone log notify")
            .is_some_and(|l| l.contains("pressed"))),
        "no notification: {:?} serial {:?}",
        h.query_all_by_label_contains("Phone log")
            .map(|n| {
                let a = n.accesskit_node();
                a.label().or_else(|| a.value())
            })
            .collect::<Vec<_>>(),
        h.state()
            .ctx
            .bench
            .serial
            .iter()
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
    );
    h.state_mut().ctx.toasts.clear();
    screenshot(&mut h, "phone");
}

#[test]
fn mqtt_explorer_sees_garden_pot1() {
    let mut h = harness();
    open_example(&mut h, "garden-node");
    h.get_by_label("Run").click();
    h.run_steps(2);
    h.get_by_label("MQTT explorer").click();
    h.run_steps(3);
    h.get_by_label("Subscribe topic").click();
    h.get_by_label("Subscribe topic").type_text("garden/#");
    h.run_steps(1);
    h.get_by_label("Subscribe").click();
    h.run_steps(3);
    assert!(
        wait_for(&mut h, 120.0, |h| first_label(h, "MQTT garden/pot1")
            .is_some()),
        "garden/pot1 never arrived"
    );
    let l = first_label(&h, "MQTT garden/pot1").unwrap();
    assert!(l.contains("\"m\":"), "{l}");
    // publish from the explorer: it shows up too
    h.get_by_label("Publish topic").click();
    h.get_by_label("Publish topic").type_text("garden/cmd");
    h.get_by_label("Publish payload").click();
    h.get_by_label("Publish payload").type_text("water");
    h.get_by_label("Publish").click();
    h.run_steps(3);
    assert!(
        wait_for(&mut h, 60.0, |h| first_label(h, "MQTT garden/cmd water")
            .is_some()),
        "own publish not seen"
    );
    h.state_mut().ctx.toasts.clear();
    screenshot(&mut h, "mqtt");
}
