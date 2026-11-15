//! Debugger memory views (`MCU-5`) driven through the GUI: blink paused at a breakpoint shows
//! SRAM rows with the stack / pointer registers, the I/O register view sees PORTB bit 5 flip
//! between two pauses, Go to address scrolls the hex view, EEPROM and flash render. API-level
//! boards get an explanation instead. Saves `docs/screenshots/firmware-memory.png` with
//! `BREADBOX_UPDATE_SCREENSHOTS=1`.

mod common;

use breadbox::BreadboxApp;
use common::*;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

type H = Harness<'static, BreadboxApp>;

fn open_example(h: &mut H, name: &str) {
    h.get_by_label(&format!("Open example {name}")).click();
    h.run_steps(3);
}

fn go(h: &mut H, section: &str) {
    h.get_by_label("Switch screen").click();
    h.run_steps(2);
    h.get_by_label(section).click();
    h.run_steps(3);
}

fn toolchain_settled(h: &H) -> bool {
    !matches!(
        h.state().ctx.toolchain.state,
        breadbox::toolchain::ToolchainState::Detecting
    )
}

fn paused_at(h: &H) -> Option<u32> {
    let d = h.state().ctx.bench.debug.get("U1")?;
    if d.is_paused() {
        d.location.as_ref().map(|l| l.line)
    } else {
        None
    }
}

/// Accessible name of the first node whose label contains `part`.
fn first_label(h: &H, part: &str) -> Option<String> {
    h.query_all_by_label_contains(part).next().and_then(|n| {
        let a = n.accesskit_node();
        a.label().or_else(|| a.value())
    })
}

/// `PORTB 0x25 0x20 0010 0000` → 0x20.
fn io_value(label: &str) -> u8 {
    let hex = label.split_whitespace().nth(2).expect("value column");
    u8::from_str_radix(hex.trim_start_matches("0x"), 16).expect("hex value")
}

fn type_into(h: &mut H, label: &str, text: &str) {
    h.get_by_label(label).click();
    h.run_steps(1);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::A);
    h.get_by_label(label).type_text(text);
    h.run_steps(1);
}

#[test]
fn memory_and_io_views_follow_blink_through_two_pauses() {
    let mut h = harness();
    open_example(&mut h, "blink");
    go(&mut h, "Firmware");
    h.get_by_label("Step · breakpoints").click();
    h.run_steps(2);
    h.get_by_label("Breakpoint line 8").click();
    h.run_steps(2);
    assert!(wait_for(&mut h, 30.0, |h| toolchain_settled(h)));
    h.get_by_label("Upload to sim").click();
    assert!(
        wait_for(&mut h, 240.0, |h| paused_at(h) == Some(8)),
        "breakpoint on line 8 never hit"
    );
    // the pause carries a memory snapshot of the 328P
    assert!(wait_for(&mut h, 30.0, |h| {
        h.state()
            .ctx
            .bench
            .debug
            .get("U1")
            .and_then(|d| d.memory.as_ref())
            .is_some_and(|m| m.sram.len() == 2048 && m.eeprom.len() == 1024)
    }));

    // Memory view: SRAM rows from 0x0100, the pointer registers listed
    h.get_by_label("Memory").click();
    h.run_steps(3);
    assert!(
        wait_for(&mut h, 30.0, |h| has_label_containing(h, "SRAM 0x0100: ")),
        "no SRAM rows"
    );
    assert!(has_label_containing(&h, "SP 0x08"));
    for p in ["X 0x", "Y 0x", "Z 0x"] {
        assert!(has_label_containing(&h, p), "{p}");
    }

    // I/O registers: PORTB before and after `digitalWrite(LED_BUILTIN, HIGH)`
    h.get_by_label("I/O registers").click();
    h.run_steps(2);
    type_into(&mut h, "Filter registers", "Port B");
    h.run_steps(2);
    let before = first_label(&h, "PORTB 0x25 ").expect("PORTB row");
    assert!(
        has_label_containing(&h, "DDRB 0x24 0x20 0010 0000"),
        "PB5 is an output"
    );
    h.get_by_label("Step over").click();
    assert!(
        wait_for(&mut h, 60.0, |h| paused_at(h) == Some(9)),
        "step over did not land on line 9"
    );
    assert!(
        wait_for(&mut h, 30.0, |h| first_label(h, "PORTB 0x25 ")
            .is_some_and(|l| l != before)),
        "PORTB did not change between the pauses ({before})"
    );
    let after = first_label(&h, "PORTB 0x25 ").unwrap();
    assert_eq!(
        (io_value(&before) ^ io_value(&after)) & 0x20,
        0x20,
        "bit 5 (the LED) toggled: {before} → {after}"
    );
    assert_eq!(after, "PORTB 0x25 0x20 0010 0000");
    // the view model marks it changed since the previous pause
    let changed = h.state().ctx.bench.debug["U1"]
        .memory
        .as_ref()
        .unwrap()
        .io
        .iter()
        .find(|r| r.name == "PORTB")
        .unwrap()
        .changed();
    assert!(changed);
    // without a filter every peripheral of the 328P is listed
    h.get_by_label("Filter registers").click();
    h.run_steps(1);
    h.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::A);
    h.key_press(eframe::egui::Key::Backspace);
    h.run_steps(3);
    for reg in ["PORTD 0x2b ", "TCCR0A 0x44 ", "UDR0 0xc6 ", "SREG 0x5f "] {
        assert!(has_label_containing(&h, reg), "{reg}");
    }

    // back to the hex view: Go to the stack pointer, then back to the start of SRAM
    h.get_by_label("Memory").click();
    h.run_steps(2);
    let sp = h.state().ctx.bench.debug["U1"].memory.as_ref().unwrap().sp;
    type_into(&mut h, "Go to address", "SP");
    h.get_by_label("Go").click();
    h.run_steps(3);
    let sp_row = format!("SRAM 0x{:04x}: ", sp & !0xF);
    assert!(has_label_containing(&h, &sp_row), "{sp_row} visible");
    assert!(
        !has_label_containing(&h, "SRAM 0x0100: "),
        "scrolled away from the start"
    );
    h.state_mut().ctx.toasts.clear();
    screenshot(&mut h, "firmware-memory");
    type_into(&mut h, "Go to address", "0x0100");
    h.get_by_label("Go").click();
    h.run_steps(3);
    assert!(has_label_containing(&h, "SRAM 0x0100: "), "Go to 0x0100");
    // a bad address explains itself
    type_into(&mut h, "Go to address", "0x9000");
    h.get_by_label("Go").click();
    h.run_steps(2);
    assert!(has_label_containing(&h, "outside SRAM"));

    // EEPROM: blink never writes it (erased)
    h.get_by_label("EEPROM").click();
    h.run_steps(3);
    assert!(
        has_label_containing(&h, "EEPROM 0x0000: ff ff ff ff"),
        "EEPROM rows"
    );
    // flash: the vector table starts with `jmp` (0x940c)
    h.get_by_label("Flash").click();
    h.run_steps(3);
    assert!(
        wait_for(&mut h, 30.0, |h| has_label_containing(
            h,
            "Flash 0x0000: 0c 94"
        )),
        "flash rows: {:?}",
        first_label(&h, "Flash 0x")
    );
    assert!(has_label_containing(&h, "bytes programmed"));
}

#[test]
fn api_level_boards_explain_the_missing_hex_views() {
    let mut h = harness();
    open_example(&mut h, "pico-blink");
    go(&mut h, "Firmware");
    h.get_by_label("Debug").click();
    h.run_steps(2);
    h.get_by_label("Memory").click();
    h.run_steps(2);
    assert!(has_label(&h, "Hex views are AVR-only"));
    h.get_by_label("I/O registers").click();
    h.run_steps(2);
    assert!(has_label(&h, "Hex views are AVR-only"));
}
