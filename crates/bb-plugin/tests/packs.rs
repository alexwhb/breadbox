//! World / radio packs and the SDK examples end to end (PLUG-5, PLUG-7): soil & weather
//! evaporation under time-warp, LoRa node → gateway → MQTT (+ decoder) and back, a template
//! part, a generator plugin, an analysis plugin calling the solver, a declarative board
//! variant running real firmware, and a theme.

mod common;
use bb_circuit::builder::BenchBuilder;
use bb_circuit::project::{FirmwareRef, Mode};
use bb_circuit::{Project, PropValue};
use bb_core::SimTime;
use bb_plugin::PluginHost;
use bb_sim::Bench;
use bb_sim::radio::RadioCommand;
use common::*;

fn host(dirs: &[&str]) -> PluginHost {
    let h = PluginHost::from_dirs(
        &dirs
            .iter()
            .map(|d| root().join("plugins").join(d))
            .collect::<Vec<_>>(),
        None,
    );
    assert!(h.errors.is_empty(), "{:?}", h.errors);
    h
}

fn moisture(b: &Bench) -> Vec<(f64, f64)> {
    b.serial("u1")
        .unwrap()
        .data_lines()
        .filter_map(|l| {
            let m = l
                .text
                .split_whitespace()
                .find_map(|t| t.strip_prefix("moisture="))?
                .trim_end_matches('%')
                .parse()
                .ok()?;
            Some((l.time.as_secs_f64(), m))
        })
        .collect()
}

#[test]
fn soil_pack_evaporation_dries_the_pot_over_time_warped_hours() {
    let h = host(&["soil-weather"]);
    let dir = root().join("examples/garden-node");
    let mut p = Project::load(&dir).unwrap();
    p.world.time_warp = 3600.0; // one simulated second = one hour
    p.world.noise = false;
    let mut opts = h.bench_options();
    opts.firmware = bb_sim::firmware::project_images(&p, &dir);
    let mut b = Bench::new(p, h.library(), opts).unwrap();
    b.run_until(SimTime::from_secs(14)).unwrap();
    let m = moisture(&b);
    let first = m.iter().find(|x| x.0 > 2.0).unwrap().1;
    let last = m.last().unwrap().1;
    // ≈ 1.2 %VWC per hour for ~12 hours
    assert!(first - last > 8.0, "moisture {first} → {last}: {m:?}");
    use bb_core::WorldView;
    let w = b.world();
    assert_eq!(
        w.var("pot1.evaporation_scale"),
        Some(0.0),
        "the pack took over"
    );
    let rate = w.var("weather.evap_rate_pct_h").unwrap();
    assert!((0.6..1.6).contains(&rate), "rate {rate} %/h");
    let vwc = w.var("pot1.vwc").unwrap();
    let avg = (42.0 - vwc) / 14.0;
    assert!((0.7..1.4).contains(&avg), "vwc {vwc}: {avg} %/h on average");
    // the sun moved across the sky (8:00 → 22:00 on June 21st in Ashland)
    let elev = w.var("weather.sun_elevation_deg").unwrap();
    assert!(elev < 5.0, "late evening sun at {elev}°");
    assert!(w.var("sun.azimuth_deg").unwrap() > 270.0);
    // no rain indoors
    assert_eq!(w.var("weather.rain_mm_h"), Some(0.0));
}

#[test]
fn soil_pack_rains_on_outdoor_pots() {
    let h = host(&["soil-weather"]);
    let lib = h.library();
    // the garden-node world (pot 1), outdoors, on an otherwise empty bench
    let garden = Project::load(root().join("examples/garden-node")).unwrap();
    let mut p = Project::new("outdoor", Mode::Playground);
    p.world = garden.world;
    p.world.time_warp = 36_000.0; // ten hours per second
    for pot in p.world.pots.iter_mut() {
        pot.indoor = false;
        pot.vwc = 20.0;
    }
    let mut b = Bench::new(p, lib, h.bench_options()).unwrap();
    let mut rained = false;
    let mut peak: f64 = 0.0;
    for k in 1..=60 {
        b.run_until(SimTime::from_secs(k)).unwrap();
        use bb_core::WorldView;
        let r = b.world().var("weather.rain_mm_h").unwrap_or(0.0);
        if r > 0.0 {
            rained = true;
        }
        peak = peak.max(b.world().var("pot1.vwc").unwrap());
    }
    // 600 hours outdoors: storms every ~3 days
    assert!(rained, "no rain in 25 days");
    assert!(peak > 20.5, "rain never wetted the pot (peak {peak})");
}

#[test]
fn lora_pack_node_to_gateway_to_mqtt_and_back() {
    if bb_firmware::host::cxx().is_none() {
        eprintln!("no C++ compiler; skipping");
        return;
    }
    let h = host(&["lora-gateway"]);
    let lib = h.library();
    let sketch = root().join("crates/bb-plugin/tests/firmware/lora_node");
    let build =
        bb_firmware::host::compile_host(&sketch, bb_firmware::host::HostKind::Esp32DevKit, None)
            .unwrap();
    assert!(build.success, "{}", build.log);
    let mut bb = BenchBuilder::new(&lib, "lora", Mode::Playground);
    let u1 = bb.offboard("esp32_devkit", "U1", 0.0, 0.0, &[]);
    let m = bb.offboard("rylr896", "U2", 20.0, 0.0, &[]);
    bb.wire((&u1, "3V3"), (&m, "VDD"), "red");
    bb.wire((&u1, "GND"), (&m, "GND"), "black");
    bb.wire((&u1, "TX2"), (&m, "RXD"), "blue");
    bb.wire((&u1, "RX2"), (&m, "TXD"), "yellow");
    let mut p = bb.finish();
    p.part_mut(&u1).unwrap().firmware = Some(FirmwareRef {
        sketch: "firmware/U1/lora_node".into(),
        fqbn: Some("breadbox:host:esp32".into()),
        image: None,
    });
    let mut opts = h.bench_options();
    opts.host_firmware.insert(u1.clone(), build.exe.unwrap());
    let mut b = Bench::new(p, lib, opts).unwrap();
    b.radio_command(RadioCommand::ExplorerSubscribe("lora/#".into()))
        .unwrap();
    b.run_until(SimTime::from_secs(5)).unwrap();
    let tel = b.telemetry();
    let r = tel.radio.as_ref().expect("radio");
    assert!(r.has_lora);
    let up: Vec<_> = r.mqtt.iter().filter(|m| m.topic == "lora/0/7/up").collect();
    assert!(!up.is_empty(), "{:#?}\n{:?}", r.mqtt, lines(&b));
    let v: serde_json::Value = serde_json::from_str(&up[0].payload).unwrap();
    assert_eq!(v["data"], "t=21.5,h=40");
    assert!(v["rssi"].as_f64().unwrap() < -40.0);
    // the pack's decoder
    let dec: Vec<_> = r
        .mqtt
        .iter()
        .filter(|m| m.topic == "lora/0/7/decoded")
        .collect();
    assert!(!dec.is_empty(), "{:#?}", r.mqtt);
    let d: serde_json::Value = serde_json::from_str(&dec[0].payload).unwrap();
    assert_eq!(d["t"], 21.5);
    assert_eq!(d["h"], 40.0);
    assert!(d["_rssi"].is_number());
    // the packet log has LoRa rows and the card shows the module
    assert!(r.log.iter().any(|row| row.proto == "LoRa"), "{:#?}", r.log);
    assert!(r.cards.iter().any(|c| c.label == "U2"));
    // downlink: dashboard → gateway → module → sketch
    b.radio_command(RadioCommand::ExplorerPublish {
        topic: "lora/0/7/down".into(),
        payload: b"led on".to_vec(),
        qos: 0,
        retain: false,
    })
    .unwrap();
    b.run_until(SimTime::from_secs(8)).unwrap();
    let l = lines(&b);
    assert!(
        l.iter()
            .any(|x| x.starts_with("rx: +RCV=") && x.contains("led on")),
        "{l:#?}"
    );
}

fn lines(b: &Bench) -> Vec<String> {
    b.serial("u1")
        .map(|s| s.data_lines().map(|l| l.text.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn template_part_blinks_an_led() {
    let h = host(&["examples/template"]);
    let lib = h.library();
    let mut bb = BenchBuilder::new(&lib, "blinker", Mode::Playground);
    let u1 = bb.offboard("uno", "U1", 0.0, 0.0, &[]);
    let chip = bb.offboard(
        "blinker_chip",
        "U2",
        20.0,
        0.0,
        &[("rate", PropValue::Num(4.0))],
    );
    let led = bb.offboard("led", "D1", 30.0, 0.0, &[]);
    let r = bb.offboard(
        "resistor",
        "R1",
        40.0,
        0.0,
        &[("resistance", PropValue::Num(330.0))],
    );
    bb.wire((&u1, "5V"), (&chip, "VCC"), "red");
    bb.wire((&u1, "GND"), (&chip, "GND"), "black");
    bb.wire((&chip, "OUT"), (&led, "a"), "yellow");
    bb.wire((&led, "k"), (&r, "1"), "yellow");
    bb.wire((&r, "2"), (&u1, "GND"), "black");
    let mut b = Bench::new(bb.finish(), lib, h.bench_options()).unwrap();
    let mut on = Vec::new();
    for k in 0..8 {
        // sample mid-phase: toggles every 125 ms at 4 Hz
        b.run_until(SimTime::from_ms(60 + 125 * k)).unwrap();
        on.push(b.element_current("d1", "D").unwrap() > 1e-3);
    }
    assert_eq!(
        on,
        [false, true, false, true, false, true, false, true],
        "{on:?}"
    );
    let t = b.readings("u2");
    assert!(t.iter().any(|(k, v)| k == "toggles" && *v >= 7.0), "{t:?}");
    // live property change: 10 Hz
    b.set_part_param("u2", "rate", 10.0).unwrap();
    let before = t.iter().find(|x| x.0 == "toggles").unwrap().1;
    b.run_for(SimTime::from_secs(1)).unwrap();
    let after = b
        .readings("u2")
        .iter()
        .find(|x| x.0 == "toggles")
        .unwrap()
        .1;
    assert!((after - before - 20.0).abs() <= 2.0, "{before} → {after}");
}

#[test]
fn generator_plugin_becomes_a_spice_source() {
    let h = host(&["examples/sawtooth"]);
    let lib = h.library();
    let def = lib.get("sawtooth_source").expect("generated part");
    assert_eq!(def.pins.len(), 2);
    let mut bb = BenchBuilder::new(&lib, "saw", Mode::Playground);
    let g = bb.offboard(
        "sawtooth_source",
        "V1",
        0.0,
        0.0,
        &[("gain", PropValue::Num(2.0))],
    );
    let r = bb.offboard(
        "resistor",
        "R1",
        20.0,
        0.0,
        &[("resistance", PropValue::Num(1e6))],
    );
    bb.wire((&g, "OUT"), (&r, "1"), "red");
    bb.wire((&r, "2"), (&g, "COM"), "black");
    let mut b = Bench::new(bb.finish(), lib, h.bench_options()).unwrap();
    // 1 kHz sawtooth × gain 2: rises to 2 V over 0.9 ms, falls in 0.1 ms
    let at = |b: &mut Bench, us: u64| {
        b.run_until(SimTime::from_us(us)).unwrap();
        b.pin_voltage("r1", "1").unwrap() - b.pin_voltage("r1", "2").unwrap()
    };
    let v1 = at(&mut b, 3_450);
    let v2 = at(&mut b, 3_800);
    let v3 = at(&mut b, 3_950);
    assert!((v1 - 2.0 * 0.45 / 0.9).abs() < 0.03, "{v1}");
    assert!((v2 - 2.0 * 0.8 / 0.9).abs() < 0.03, "{v2}");
    assert!((v3 - 2.0 * 0.5).abs() < 0.05, "{v3}");
}

#[test]
fn analysis_plugin_runs_the_solver_on_a_netlist() {
    let h = host(&["examples/sensitivity"]);
    let a = h.analyses();
    assert_eq!(a[0].1.name, "DC sensitivity");
    let out = h
        .run_analysis(
            "example.sensitivity",
            "divider\nV1 in 0 10\nR1 in out 1k\nR2 out 0 3k\n.tran 1u 1m\n.end\n",
            &serde_json::json!({"node": "out"}),
        )
        .unwrap();
    assert!(
        out.summary.starts_with("Sensitivity of V(out) = 7.5000 V"),
        "{}",
        out.summary
    );
    assert_eq!(out.table.len(), 3);
    // ∂V/∂R1·1 % = −10·R2·R1/(R1+R2)²·0.01 ≈ −18.7 mV; R2: +18.6 mV
    let r1 = out
        .table
        .iter()
        .find(|r| r[0].eq_ignore_ascii_case("r1"))
        .unwrap_or_else(|| panic!("{:?}", out.table));
    assert!(
        r1[2].starts_with("-18.") || r1[2].starts_with("−18."),
        "{r1:?}"
    );
    assert_eq!(out.plots[0].series[0].values.len(), 2);
    // a broken netlist is a clean error
    let e = h
        .run_analysis(
            "example.sensitivity",
            "x\nR1 a\n.end",
            &serde_json::json!({}),
        )
        .unwrap_err();
    assert!(!e.to_string().is_empty());
}

#[test]
fn board_variant_runs_real_firmware() {
    let h = PluginHost::from_dirs(&[root().join("plugins/examples/pro-mini")], None);
    assert!(h.errors.is_empty(), "{:?}", h.errors);
    let lib = h.library();
    let def = lib.get("pro_mini").expect("board part");
    assert!(def.model.mcu.is_some());
    let mut bb = BenchBuilder::new(&lib, "promini", Mode::Playground);
    let u1 = bb.offboard("uno", "U1", 0.0, 0.0, &[]);
    let pm = bb.offboard("pro_mini", "U2", 30.0, 0.0, &[]);
    bb.wire((&u1, "5V"), (&pm, "VCC"), "red");
    bb.wire((&u1, "GND"), (&pm, "GND"), "black");
    let p = bb.finish();
    let mut opts = h.bench_options();
    let hex = root().join("examples/fixtures/blink.uno.hex");
    opts.firmware
        .insert(pm.clone(), bb_sim::firmware::load_image(&hex).unwrap());
    let mut b = Bench::new(p, lib, opts).unwrap();
    // blink: D13 high 1 s, low 1 s
    let level = |b: &mut Bench, ms: u64| {
        b.run_until(SimTime::from_ms(ms)).unwrap();
        let m = b.mcu(&pm).unwrap();
        let i = m.pin_index("D13").unwrap();
        matches!(
            m.pin_drive(i),
            bb_sim::mcu::LogicDrive::Output { high: true }
        )
    };
    let s: Vec<bool> = [500, 1500, 2500, 3500]
        .iter()
        .map(|t| level(&mut b, *t))
        .collect();
    assert_eq!(s, [true, false, true, false], "{s:?}");
    let tel = b.telemetry();
    assert!(
        tel.mcus
            .iter()
            .any(|m| m.part == pm && m.label.contains("ATmega328P"))
    );
}

#[test]
fn theme_plugin_overrides_tokens() {
    let h = host(&["examples/solarized"]);
    let t = h.themes();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].1.base, "light");
    assert_eq!(t[0].1.colors()["bg"], [0xFD, 0xF6, 0xE3, 0xFF]);
}

/// 12 V (`vsource`) → the example plugin's LM317 (`.subckt LM317BB` shipped in its
/// parts.json) with R1 = 240 Ω (OUT → ADJ), R2 = 720 Ω (ADJ → GND) and a 100 Ω load.
fn lm317_bench(h: &PluginHost, vin: f64) -> (bb_circuit::Project, Bench) {
    let lib = h.library();
    let mut bb = BenchBuilder::new(&lib, "lm317", Mode::Playground);
    let v1 = bb.offboard(
        "vsource",
        "V1",
        0.0,
        0.0,
        &[("spec", PropValue::Text(format!("DC {vin}")))],
    );
    // the bench's ground: a board's GND (a source's minus is not ground by itself)
    let uno = bb.offboard("uno", "U9", -40.0, 0.0, &[]);
    bb.wire((&v1, "minus"), (&uno, "GND"), "black");
    let u1 = bb.offboard("lm317_bb", "U1", 20.0, 0.0, &[]);
    let r = |bb: &mut BenchBuilder, name: &str, x: f64, ohms: f64| {
        bb.offboard(
            "resistor",
            name,
            x,
            0.0,
            &[
                ("resistance", PropValue::Num(ohms)),
                ("power", PropValue::Num(5.0)),
            ],
        )
    };
    let r1 = r(&mut bb, "R1", 30.0, 240.0);
    let r2 = r(&mut bb, "R2", 40.0, 720.0);
    let rl = r(&mut bb, "R3", 50.0, 100.0);
    bb.wire((&v1, "plus"), (&u1, "IN"), "red");
    bb.wire((&u1, "OUT"), (&r1, "1"), "orange");
    bb.wire((&r1, "2"), (&u1, "ADJ"), "yellow");
    bb.wire((&r2, "1"), (&u1, "ADJ"), "yellow");
    bb.wire((&r2, "2"), (&v1, "minus"), "black");
    bb.wire((&rl, "1"), (&u1, "OUT"), "orange");
    bb.wire((&rl, "2"), (&v1, "minus"), "black");
    let p = bb.finish();
    let b = Bench::new(p.clone(), lib, h.bench_options()).unwrap();
    (p, b)
}

/// PLUG-7: a plugin part that ships its own SPICE subcircuit (the `example.lm317` plugin,
/// built by scripts/build-plugins.sh): it regulates on the bench, its wasm side sees it, it
/// drops out with a warning, and Pro / `.cir` netlists carry the same `.subckt`.
#[test]
fn plugin_part_with_a_subckt_model_regulates_on_the_bench_and_in_pro() {
    let h = host(&["examples/lm317"]);
    let lib = h.library();
    let def = lib.get("lm317_bb").expect("the plugin's part");
    assert!(
        def.model
            .library
            .iter()
            .any(|l| l.starts_with(".subckt LM317BB")),
        "{:?}",
        def.model.library
    );
    // Vout = 1.25 V × (1 + 720 / 240) + 50 µA × 720 Ω
    let want = 1.25 * (1.0 + 720.0 / 240.0) + 50e-6 * 720.0;
    let (p, mut b) = lm317_bench(&h, 12.0);
    b.run_until(SimTime::from_ms(50)).unwrap();
    let vout = b.pin_voltage("u1", "OUT").unwrap();
    assert!((vout - want).abs() < 0.01, "bench {vout} V, want {want} V");
    let r = |b: &Bench, k: &str| {
        b.readings("u1")
            .into_iter()
            .find(|x| x.0 == k)
            .map(|x| x.1)
            .unwrap_or(f64::NAN)
    };
    assert_eq!(r(&b, "regulating"), 1.0, "{:?}", b.readings("u1"));
    assert!(
        (r(&b, "vref") - 1.25).abs() < 0.005,
        "{:?}",
        b.readings("u1")
    );
    assert!((r(&b, "headroom") - (12.0 - want)).abs() < 0.02);
    // the same subcircuit in Pro: .op on the circuit's netlist agrees with the bench
    let nl = bb_circuit::spice_io::pro_netlist(&p, &lib).unwrap();
    let text = nl.to_spice_text();
    assert!(
        text.to_ascii_lowercase()
            .contains(".subckt lm317bb in adj out"),
        "{text}"
    );
    let mut s = bb_spice::Session::new(&nl).unwrap();
    s.op().unwrap();
    let out_net = {
        let conn = bb_circuit::Connectivity::compute(&p, &lib);
        conn.net(conn.net_of("u1", "OUT").unwrap()).name.clone()
    };
    let v_pro = s.voltage(&out_net).unwrap();
    assert!(
        (v_pro - vout).abs() < 1e-3,
        "Pro {v_pro} V vs bench {vout} V"
    );
    let cir = bb_circuit::spice_io::export_cir(&p, &lib).unwrap();
    assert!(
        cir.to_ascii_lowercase().contains(".subckt lm317bb"),
        "{cir}"
    );
    // 6 V in: not enough headroom — dropout, with a warning on the bench
    let (_, mut b) = lm317_bench(&h, 6.0);
    b.run_until(SimTime::from_ms(50)).unwrap();
    let vout = b.pin_voltage("u1", "OUT").unwrap();
    assert!(vout < want - 0.5 && vout > 3.0, "dropout {vout} V");
    assert_eq!(r(&b, "dropout"), 1.0, "{:?}", b.readings("u1"));
    assert_eq!(r(&b, "regulating"), 0.0);
    let warned = b.take_events().into_iter().any(|e| {
        matches!(&e.event, bb_core::DeviceEvent::Log { text, .. } if text.contains("LM317 in dropout"))
    });
    assert!(warned, "dropout warning");
}
