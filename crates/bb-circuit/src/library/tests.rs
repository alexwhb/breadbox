use super::*;

#[test]
fn builtin_library_loads_and_validates() {
    let mut lib = Library::default();
    for (name, text) in BUILTIN {
        lib.add_json(text, name, "builtin")
            .unwrap_or_else(|e| panic!("{e}"));
    }
    assert!(lib.len() >= 80, "{}", lib.len());
    assert_eq!(Library::builtin().len(), lib.len());
}

#[test]
fn simple_set_is_the_25_starter_parts() {
    let lib = Library::builtin();
    let simple = lib.simple();
    assert_eq!(simple.len(), 25);
    for p in &simple {
        assert!(p.simple, "{} not flagged simple", p.id);
    }
    let flagged = lib.parts().filter(|p| p.simple).count();
    assert_eq!(flagged, 25);
    assert_eq!(simple[8].display_name(true), "Knob (pot)");
    let cats = lib.categories(true);
    assert_eq!(cats[0].0, Category::Boards);
    assert_eq!(cats[0].1.len(), 3);
}

#[test]
fn every_spec_part_exists() {
    let lib = Library::builtin();
    for id in [
        // PART-1
        "uno",
        "nano",
        "mega",
        "esp32_devkit",
        "pico",
        // PART-2
        "breadboard_830",
        "breadboard_400",
        "jumper_wire",
        "resistor",
        "capacitor",
        "capacitor_electrolytic",
        "inductor",
        "push_button",
        "slide_switch",
        "dip_switch",
        "potentiometer",
        "trimmer",
        "battery_pack",
        "usb_power",
        "barrel_jack",
        // PART-3
        "led",
        "rgb_led",
        "seven_segment",
        "led_bar",
        "ws2812_strip",
        "buzzer_passive",
        "buzzer_active",
        "speaker",
        // PART-4
        "ldr",
        "capsoil",
        "soil_resistive",
        "tmp36",
        "dht11",
        "dht22",
        "ds18b20",
        "bme280",
        "hc_sr04",
        "pir",
        "tilt_switch",
        "hall_sensor",
        "microphone",
        "thermistor",
        // PART-5
        "diode",
        "npn",
        "pnp",
        "nmos",
        "opamp_lm358",
        "opamp_tl072",
        "opamp_ideal",
        "ne555",
        "hc595",
        "hc00",
        "hc04",
        "hc08",
        "hc32",
        "uln2003",
        "l293d",
        "reg_7805",
        "ams1117_3v3",
        "pc817",
        // PART-6
        "lcd_1602",
        "lcd_1602_i2c",
        "ssd1306",
        "dc_motor",
        "servo",
        "stepper_28byj48",
        "relay_module",
        "vibration_motor",
        // PART-7
        "esp01",
        "hm10",
        "hc05",
    ] {
        assert!(lib.get(id).is_some(), "missing {id}");
    }
    // LED colours and diode / BJT / MOSFET variants
    let led = lib.get("led").unwrap();
    let PropKind::Choice { options, .. } = &led.prop("color").unwrap().kind else {
        panic!()
    };
    assert_eq!(options.len(), 6);
}

#[test]
fn uno_header_geometry_and_bonds() {
    let lib = Library::builtin();
    let uno = lib.get("uno").unwrap();
    assert!(uno.pin("D13").is_some());
    assert_eq!(uno.pin("D9").unwrap().mcu.as_deref(), Some("D9"));
    assert_eq!(uno.pin("A0").unwrap().mcu.as_deref(), Some("A0"));
    assert_eq!(uno.pin("GND").unwrap().role, PinRole::Ground);
    assert!(uno.pin("GND.3").is_some());
    assert_eq!(uno.pin("3V3").unwrap().label(), "3.3V");
    let lead = |p: &str| uno.bench.leads.iter().find(|l| l.pin == p).unwrap().clone();
    // 160 mil gap between D7 and D8
    assert!((lead("D7").x - lead("D8").x - 1.6).abs() < 1e-9);
    assert_eq!(lead("D0").x, 26.0);
    assert!(
        uno.bonds
            .iter()
            .any(|b| b.contains(&"GND".into()) && b.len() == 3)
    );
    assert!(
        uno.bonds
            .iter()
            .any(|b| b == &vec!["A4".to_string(), "SDA".to_string()])
    );
    let mega = lib.get("mega").unwrap();
    assert!(mega.pin("D53").is_some() && mega.pin("A15").is_some());
    assert!(
        mega.bonds
            .iter()
            .any(|b| b.contains(&"D20".into()) && b.contains(&"SDA".into()))
    );
    let nano = lib.get("nano").unwrap();
    assert!(nano.pin("A7").is_some());
    assert_eq!(nano.bench.leads.len(), 30);
}

#[test]
fn breadboard_has_830_holes() {
    let lib = Library::builtin();
    let bb = lib.get("breadboard_830").unwrap();
    assert_eq!(bb.pins.len(), 830);
    assert!(bb.is_breadboard());
    assert_eq!(lib.get("breadboard_400").unwrap().pins.len(), 400);
}

#[test]
fn layouts_and_symbols_are_derived() {
    let lib = Library::builtin();
    let r = lib.get("resistor").unwrap();
    assert_eq!(r.bench.leads.len(), 2);
    assert_eq!(r.bench.leads[1].x, 4.0);
    assert_eq!(r.schematic.pins.len(), 2);
    let dip = lib.get("ne555").unwrap();
    // pin 1 bottom-left, pin 8 top-left
    let l1 = dip.bench.leads.iter().find(|l| l.pin == "GND").unwrap();
    let l8 = dip.bench.leads.iter().find(|l| l.pin == "VCC").unwrap();
    assert_eq!((l1.x, l1.y), (0.0, 3.0));
    assert_eq!((l8.x, l8.y), (0.0, 0.0));
    assert_eq!(dip.schematic.symbol, "box");
    assert_eq!(dip.schematic.pins.len(), 8);
    // every non-hidden, pinned part has leads and symbol anchors for its pins
    for p in lib.parts() {
        if p.pins.is_empty() || p.bench.hidden || p.id == "spice_card" {
            continue;
        }
        assert_eq!(p.bench.leads.len(), p.pins.len(), "{} leads", p.id);
        if p.schematic.symbol != "none" {
            assert!(!p.schematic.pins.is_empty(), "{} symbol", p.id);
        }
    }
}

#[test]
fn search_finds_by_name_number_and_model() {
    let lib = Library::builtin();
    let r = lib.search("gl5528", false);
    assert_eq!(r[0].id, "ldr");
    let r = lib.search("2n2222", false);
    assert!(r.iter().any(|p| p.id == "npn"));
    let r = lib.search("led", true);
    assert_eq!(r[0].id, "led");
    assert!(lib.search("zzzz", false).is_empty());
}

#[test]
fn plugin_parts_use_the_same_schema() {
    let mut lib = Library::with_builtin();
    let json = r#"{
        "id": "hx711", "name": "HX711 load cell amp", "category": "sensors", "ref_prefix": "U",
        "description": "24-bit ADC", "pins": ["E+:power_out", "E-:ground", "A+:input", "A-:input", "DT:output", "SCK:input"],
        "model": {"behaviour": "plugin:hx711"},
        "bench": {"layout": "row"}, "schematic": {"symbol": "box"}
    }"#;
    let ids = lib
        .add_json(json, "plugin.json", "com.example.hx711")
        .unwrap();
    assert_eq!(ids, vec!["hx711"]);
    let p = lib.get("hx711").unwrap();
    assert_eq!(p.bench.leads.len(), 6);
    assert_eq!(p.source, "com.example.hx711");
    // bad templates are rejected
    let bad = r#"{"id":"x","name":"x","category":"sensors","ref_prefix":"X","description":"",
        "pins":["1","2"],"model":{"spice":["$(el:R) $(1) $(3) 1k"]}}"#;
    assert!(lib.add_json(bad, "bad.json", "t").is_err());
    // round trip through the serialized form
    let all = lib.to_json();
    let mut lib2 = Library::default();
    lib2.add_json(&all, "all.json", "rt").unwrap();
    assert_eq!(lib2.len(), lib.len());
    assert_eq!(lib2.get("uno").unwrap().pins, lib.get("uno").unwrap().pins);
}
