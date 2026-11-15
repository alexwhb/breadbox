use super::*;

fn garden() -> World {
    World::new(WorldConfig {
        pots: vec![PotConfig::default()],
        objects: vec![ObjectConfig::default()],
        ..WorldConfig::default()
    })
    .unwrap()
}

#[test]
fn evaporation_matches_design_calibration() {
    let w = garden();
    let s = w.snapshot();
    // design: "Evaporation · ≈ 1.4 %/h" at 21 °C, 45 %RH, 180 lx
    assert!(
        (s.pots[0].evaporation_pct_h - 1.45).abs() < 0.05,
        "{}",
        s.pots[0].evaporation_pct_h
    );
}

#[test]
fn pot_dries_and_drains() {
    let mut cfg = WorldConfig {
        pots: vec![PotConfig::default()],
        time_warp: 3600.0, // 1 sim second = 1 world hour
        ..WorldConfig::default()
    };
    cfg.pots[0].vwc = 40.0;
    let mut w = World::new(cfg).unwrap();
    for ms in 1..=1000 {
        w.step(SimTime::from_ms(ms));
    }
    let v = w.soil_vwc("pot1").unwrap();
    assert!((v - (40.0 - 1.4)).abs() < 0.15, "{v}");
    // hotter + drier air evaporates faster
    w.set("air.temp_c", 35.0);
    w.set("air.rh", 20.0);
    let r = w.var("pot1.evap_rate").unwrap();
    assert!(r > 2.5, "{r}");
    // above field capacity, excess drains with the rate constant
    w.set("pot1.vwc", 55.0);
    w.set("pot1.evaporation", 0.0);
    for ms in 1001..=2000 {
        w.step(SimTime::from_ms(ms));
    }
    let v = w.soil_vwc("pot1").unwrap();
    let expect = 45.0 + 10.0 * (-0.8f64).exp();
    assert!((v - expect).abs() < 0.05, "{v} vs {expect}");
}

#[test]
fn watering_adds_water_up_to_saturation() {
    let mut w = garden();
    w.set("moisture", 20.0);
    w.water("pot1", 100.0);
    assert!((w.soil_vwc("pot1").unwrap() - 30.0).abs() < 1e-9);
    w.water("pot1", 10_000.0);
    assert_eq!(w.soil_vwc("pot1").unwrap(), 60.0);
    assert_eq!(w.take_events().len(), 2);
}

#[test]
fn bench_light_sources_follow_inverse_square() {
    let mut w = garden();
    let ldr = Pos::new(0.10, 0.10);
    let base = w.lux_at(ldr);
    assert_eq!(base, 180.0);
    w.set_light_source("D1", Pos::new(0.12, 0.10), 0.05);
    let near = w.lux_at(ldr) - base;
    assert!((near - 0.05 / 0.0004).abs() < 1e-6, "{near}");
    w.set_light_source("D1", Pos::new(0.14, 0.10), 0.05);
    let far = w.lux_at(ldr) - base;
    assert!((near / far - 4.0).abs() < 1e-9);
    w.set_light_source("D1", Pos::new(0.14, 0.10), 0.0);
    assert_eq!(w.lux_at(ldr), base);
}

#[test]
fn solar_mode_follows_the_day() {
    let mut cfg = WorldConfig::default();
    cfg.light.mode = LightMode::Solar;
    cfg.light.indoor = false;
    cfg.light.cloud_cover = 0.0;
    cfg.light.start.hour = 0.0;
    cfg.time_warp = 3600.0;
    cfg.air.diurnal = true;
    let mut w = World::new(cfg).unwrap();
    let night = w.var("light.ambient_lux").unwrap();
    let cold = w.var("air.temp_c").unwrap();
    for ms in (1..=13_000).step_by(10) {
        w.step(SimTime::from_ms(ms));
    }
    let noon = w.var("light.ambient_lux").unwrap();
    let warm = w.var("air.temp_c").unwrap();
    assert!(night < 1.0, "{night}");
    assert!(noon > 80_000.0, "{noon}");
    assert!(warm > cold + 5.0, "{cold} → {warm}");
    assert!(w.var("sun.elevation_deg").unwrap() > 60.0);
}

#[test]
fn heat_sources_warm_nearby_sensors() {
    let mut w = garden();
    let p = Pos::new(0.05, 0.05);
    w.set_heat_source("R1", Pos::new(0.055, 0.05), 0.25);
    let t = w.temperature_at(p);
    assert!(t > 25.0 && t < 30.0, "{t}");
    assert!(w.temperature_at(Pos::new(0.5, 0.5)) < 21.5);
}

#[test]
fn objects_for_distance_motion_and_magnets() {
    let cfg = WorldConfig {
        objects: vec![
            ObjectConfig {
                id: "hand".into(),
                pos: Pos::new(0.30, 0.0),
                radius: 0.04,
                ..Default::default()
            },
            ObjectConfig {
                id: "mag".into(),
                kind: ObjectKind::Magnet,
                pos: Pos::new(0.0, 0.02),
                radius: 0.005,
                strength_mt: 50.0,
                ..Default::default()
            },
        ],
        ..WorldConfig::default()
    };
    let mut w = World::new(cfg).unwrap();
    let s = Pos::new(0.0, 0.0);
    let d = w.distance(s, 0.0, 15.0, 4.0).unwrap();
    assert!((d - 0.26).abs() < 1e-9);
    assert!(w.distance(s, 90.0, 15.0, 4.0).is_some_and(|d| d < 0.02)); // the magnet
    assert!(w.distance(s, 180.0, 15.0, 4.0).is_none());
    assert!(!w.motion(s, 1.0, 2.0));
    w.step(SimTime::from_ms(100));
    w.set("hand.x", 0.25);
    assert!(w.motion(s, 1.0, 2.0));
    w.step(SimTime::from_secs(5));
    assert!(!w.motion(s, 1.0, 2.0));
    let b = w.magnetic_field(s);
    assert!((b - 50.0 / 8.0).abs() < 1e-9, "{b}");
}

#[test]
fn scripts_and_timelines_drive_variables() {
    let cfg = WorldConfig {
        pots: vec![PotConfig::default()],
        time_warp: 3600.0,
        scripts: vec![Script {
            source: "moisture = 45 - 0.8*t_hours".into(),
            enabled: true,
        }],
        timelines: vec![Timeline {
            var: "light.ambient_lux".into(),
            keys: vec![
                Keyframe { t: 0.0, value: 0.0 },
                Keyframe {
                    t: 7200.0,
                    value: 1000.0,
                },
            ],
            interp: Interp::Linear,
            repeat: None,
            enabled: true,
        }],
        ..WorldConfig::default()
    };
    let mut w = World::new(cfg).unwrap();
    assert!((w.soil_vwc("pot1").unwrap() - 45.0).abs() < 1e-9);
    w.step(SimTime::from_secs(1)); // one world hour
    assert!((w.soil_vwc("pot1").unwrap() - 44.2).abs() < 1e-9);
    assert!((w.var("light.ambient_lux").unwrap() - 500.0).abs() < 1e-9);
    w.step(SimTime::from_secs(5));
    assert!((w.soil_vwc("pot1").unwrap() - 41.0).abs() < 1e-9);
    assert!((w.var("light.ambient_lux").unwrap() - 1000.0).abs() < 1e-9);
    // a bad script is rejected and the old one kept
    let err = w.set_scripts(vec![Script {
        source: "moisture = 45 -".into(),
        enabled: true,
    }]);
    assert!(err.is_err());
    // scripts may read other variables
    w.set_scripts(vec![Script {
        source: "temp = 10 + lux/100".into(),
        enabled: true,
    }])
    .unwrap();
    w.step(SimTime::from_secs(6));
    assert!((w.var("air.temp_c").unwrap() - 20.0).abs() < 1e-9);
}

#[test]
fn time_warp_and_determinism() {
    let run = || {
        let mut cfg = WorldConfig {
            pots: vec![PotConfig::default()],
            time_warp: 60.0,
            ..WorldConfig::default()
        };
        cfg.light.mode = LightMode::Solar;
        let mut w = World::new(cfg).unwrap();
        for ms in 1..=2000 {
            w.step(SimTime::from_ms(ms));
        }
        (w.world_secs(), w.snapshot())
    };
    let (a, sa) = run();
    let (b, sb) = run();
    assert!((a - 120.0).abs() < 1e-9);
    assert_eq!(a, b);
    assert_eq!(sa, sb);
}

#[test]
fn config_round_trips_through_json() {
    let mut w = garden();
    w.set("pot1.vwc", 33.0);
    w.set("light.ambient_lux", 420.0);
    let json = serde_json::to_string_pretty(w.config()).unwrap();
    let back: WorldConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, w.config());
    // missing fields take defaults
    let partial: WorldConfig = serde_json::from_str(r#"{"noise": false}"#).unwrap();
    assert!(!partial.noise);
    assert_eq!(partial.air.temp_c, 21.0);
    assert_eq!(w.controls()[0].stops[1].0, "Field capacity");
}

#[test]
fn objects_can_be_added_moved_and_removed_at_run_time() {
    let mut w = World::new(WorldConfig::default()).unwrap();
    let s = Pos::new(0.0, 0.0);
    assert!(w.distance(s, 0.0, 15.0, 4.0).is_none());
    w.step(SimTime::from_secs(10));
    w.set_objects(vec![ObjectConfig {
        id: "hand".into(),
        pos: Pos::new(0.5, 0.0),
        radius: 0.04,
        ..Default::default()
    }]);
    let d = w.distance(s, 0.0, 15.0, 4.0).unwrap();
    assert!((d - 0.46).abs() < 1e-9, "{d}");
    // appearing counts as motion; staying put does not
    assert!(w.motion(s, 1.0, 2.0));
    w.step(SimTime::from_secs(20));
    assert!(!w.motion(s, 1.0, 2.0));
    w.set_objects(vec![]);
    assert!(w.distance(s, 0.0, 15.0, 4.0).is_none());
}

#[test]
fn scripts_are_checked_without_a_world() {
    assert!(World::check_script("moisture = 45 - 0.8*t_hours").is_ok());
    assert!(
        World::check_script("# a comment\nlux = 100; air.temp_c = 20 + 5*sin(t_hours)").is_ok()
    );
    assert!(World::check_script("moisture 45").is_err());
    assert!(World::check_script("moisture = 45 *").is_err());
    assert!(World::check_script(" = 3").is_err());
}
