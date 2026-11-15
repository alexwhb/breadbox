//! `MCU-6`: host-compiled sketches (ESP32 / Pico API-level cores) in lock-step with simulated
//! time. Needs a C++ compiler (Xcode command line tools); skipped otherwise.

use bb_core::SimTime;
use bb_firmware::host::{
    HostBoard, HostConfig, HostEvent, HostIo, HostKind, HostPinState, HostStop, I2cStatus,
    compile_host, cxx,
};
use std::path::PathBuf;
use std::time::Instant;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn build(dir: PathBuf, kind: HostKind) -> Option<PathBuf> {
    if cxx().is_none() {
        eprintln!("no C++ compiler; skipping");
        return None;
    }
    let tmp = std::env::temp_dir().join(format!(
        "bb-host-test-{}-{}",
        dir.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let b = compile_host(&dir, kind, Some(&tmp)).unwrap();
    assert!(b.success, "{}\n{:#?}", b.log, b.diagnostics);
    b.exe
}

fn serial_text(b: &mut HostBoard) -> Vec<(SimTime, String)> {
    let mut lines = vec![];
    let mut cur = String::new();
    for (t, c) in b.take_serial(0) {
        if c == b'\n' {
            lines.push((
                t,
                std::mem::take(&mut cur).trim_end_matches('\r').to_string(),
            ));
        } else {
            cur.push(c as char);
        }
    }
    lines
}

#[test]
fn pico_blink_follows_sim_time_and_reads_adc() {
    let Some(exe) = build(
        root().join("examples/pico-blink/firmware/U1/pico-blink"),
        HostKind::Pico,
    ) else {
        return;
    };
    let mut b = HostBoard::spawn(&exe, HostConfig::new(HostKind::Pico)).unwrap();
    b.set_pin_voltage(26, 1.65);
    let wall = Instant::now();
    assert_eq!(
        b.run(SimTime::from_ms(2_100), &mut ()),
        HostStop::ReachedTime
    );
    let ev = b.take_events();
    let led: Vec<(SimTime, bool)> = ev
        .iter()
        .filter_map(|e| match e {
            HostEvent::Pin {
                time,
                pin: 25,
                state: HostPinState::Output { high },
            } => Some((*time, *high)),
            _ => None,
        })
        .collect();
    assert!(led.len() >= 8, "{led:?}");
    for w in led.windows(2).skip(1) {
        let dt = (w[1].0 - w[0].0).as_secs_f64() * 1e3;
        assert!((dt - 250.0).abs() < 2.1, "LED period {dt} ms");
        assert_ne!(w[0].1, w[1].1);
    }
    let lines = serial_text(&mut b);
    assert!(lines.len() >= 3);
    assert!(
        lines.iter().all(|l| l.1 == "A0 raw=2048 1.650 V"),
        "{lines:?}"
    );
    // 2.1 simulated seconds don't take 2.1 real seconds.
    assert!(wall.elapsed().as_secs_f64() < 1.5, "{:?}", wall.elapsed());
    // The analog value follows the pushed voltage.
    b.set_pin_voltage(26, 0.5);
    b.run(SimTime::from_ms(2_700), &mut ());
    assert!(
        serial_text(&mut b)
            .iter()
            .any(|l| l.1 == "A0 raw=620 0.500 V")
    );
}

struct Bench;

impl HostIo for Bench {
    fn i2c_write(&mut self, _bus: u8, addr: u8, data: &[u8], _at: SimTime) -> I2cStatus {
        if addr == 0x48 && data == [0] {
            I2cStatus::Ok
        } else {
            I2cStatus::AddrNack
        }
    }
    fn i2c_read(&mut self, _bus: u8, addr: u8, len: usize, _at: SimTime) -> Option<Vec<u8>> {
        (addr == 0x48).then(|| vec![0x19, 0x80][..len.min(2)].to_vec())
    }
    fn spi_transfer(&mut self, _bus: u8, data: &[u8], _at: SimTime) -> Vec<u8> {
        data.iter().map(|b| !b).collect()
    }
}

#[test]
fn esp32_core_features_interrupts_buses_nvs_restart() {
    let Some(exe) = build(
        root().join("crates/bb-firmware/tests/host/features"),
        HostKind::Esp32DevKit,
    ) else {
        return;
    };
    let nvs = std::env::temp_dir().join(format!("bb-nvs-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&nvs);
    let mut cfg = HostConfig::new(HostKind::Esp32DevKit);
    cfg.nvs_path = Some(nvs.clone());
    let mut b = HostBoard::spawn(&exe, cfg).unwrap();
    // A 1234 µs pulse on GPIO16 at 0.7 s; button presses (falling edges) on GPIO15.
    b.schedule_pin_input(16, Some(false), SimTime::ZERO);
    b.schedule_pin_input(16, Some(true), SimTime::from_ms(700));
    b.schedule_pin_input(
        16,
        Some(false),
        SimTime::from_ms(700) + SimTime::from_us(1234),
    );
    for (i, t) in [1000u64, 1500, 1520].iter().enumerate() {
        let _ = i;
        b.schedule_pin_input(15, Some(false), SimTime::from_ms(*t));
        b.schedule_pin_input(15, Some(true), SimTime::from_ms(*t + 5));
    }
    let mut io = Bench;
    assert_eq!(b.run(SimTime::from_secs(5), &mut io), HostStop::ReachedTime);
    let lines: Vec<String> = serial_text(&mut b).into_iter().map(|l| l.1).collect();
    let want = [
        "boot 1",
        "i2c st=0 data=1980",
        "i2c nack=2",
        "spi 5a",
        "busy waited 500 ms",
        "pulse 1234 us",
        "ABC42X",
        "3.142",
        "127",
        "presses=1",
        "presses=3",
        "restarting",
        "boot 2",
    ];
    for w in want {
        assert!(
            lines.iter().any(|l| l == w),
            "missing {w:?} in {lines:#?}\nstderr: {}",
            b.stderr()
        );
    }
    assert!(
        b.take_events()
            .iter()
            .any(|e| matches!(e, HostEvent::Restarted { .. }))
    );
    assert!(
        std::fs::read_to_string(&nvs)
            .unwrap()
            .contains("bench\tboots\t32")
    );
    let _ = std::fs::remove_file(&nvs);
}

#[test]
fn compile_errors_map_to_the_ino() {
    if cxx().is_none() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("bbhost-err-{}/broken", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("broken.ino"),
        "void setup() {\n  Serial.begin(115200);\n  undefinedThing(3);\n}\nvoid loop() {}\n",
    )
    .unwrap();
    let b = compile_host(&dir, HostKind::Esp32DevKit, Some(&dir.join("build"))).unwrap();
    assert!(!b.success);
    let d = b
        .diagnostics
        .iter()
        .find(|d| d.severity == bb_firmware::Severity::Error)
        .unwrap();
    assert_eq!(d.tab.as_deref(), Some("broken.ino"));
    assert_eq!((d.line, d.column), (Some(3), Some(3)));
    assert!(d.message.contains("undefinedThing"), "{}", d.message);
}
