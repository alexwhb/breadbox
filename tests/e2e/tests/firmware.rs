//! TEST-4 · firmware on the live bench (MCU-4, MCU-5, SIM-7): blink compiled now by the
//! installed Arduino toolchain as a debug build (prebuilt ELF otherwise), flashed onto the
//! running bench, a breakpoint on `digitalWrite(LED_BUILTIN, HIGH)` hit, step over to the
//! `delay`, continue to the next hit exactly 2 s of simulated time later — all through the
//! engine commands the Firmware screen sends.

use bb_firmware::DebugInfo;
use bb_sim::{Command, DebugCommand, LineKind};
use breadbox_e2e::*;

fn line_at(info: &DebugInfo, sim: &Sim) -> (String, u32) {
    let pc = sim.tel().mcus[0].pc.expect("paused MCU reports its PC");
    let loc = info.location(pc).expect("PC maps to a source line");
    let file = loc
        .file
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    (file, loc.line)
}

#[test]
fn compile_flash_breakpoint_step_continue_on_the_live_bench() {
    let (p, dir) = example("blink");
    let (img, elf, src) = avr_firmware(&p, &dir, "u1", true);
    eprintln!("blink debug build: {src:?}");
    let elf = elf.expect("an ELF for source-level debugging");
    let info = DebugInfo::from_elf(&elf).unwrap();
    let mut sim = Sim::example("blink").unwrap();
    sim.run_secs(1.3);
    // Upload: flash + attach the debugger, bench state kept
    sim.send(Command::Flash {
        part: "u1".into(),
        image: Box::new(img),
    });
    sim.send(Command::Debug(DebugCommand::Attach {
        part: "u1".into(),
        elf: elf.clone(),
    }));
    sim.send(Command::Debug(DebugCommand::AddBreakpoint {
        part: "u1".into(),
        file: "blink.ino".into(),
        line: 8,
        hits: None,
    }));
    let t_flash = sim.time();
    let stop = sim.run_secs(5.0);
    let Stop::Paused(part, why) = stop else {
        panic!("breakpoint not hit: {stop:?}");
    };
    assert_eq!(part, "u1");
    assert!(why.to_ascii_lowercase().contains("breakpoint"), "{why}");
    let sys: Vec<String> = sim
        .lines_of("U1", LineKind::System)
        .iter()
        .map(|l| l.text.clone())
        .collect();
    assert!(
        sys.iter()
            .any(|l| l == "Flashed virtual U1 · core reset · bench state kept"),
        "{sys:#?}"
    );
    assert_eq!(line_at(&info, &sim), ("blink.ino".into(), 8));
    let t_hit1 = sim.time();
    assert!(
        t_hit1 > t_flash && t_hit1 - t_flash < ms(100),
        "first loop() right after the reset: {:?}",
        t_hit1 - t_flash
    );
    // step over: digitalWrite runs, we stop on the delay line, the LED is on
    sim.send(Command::Debug(DebugCommand::StepOver { part: "u1".into() }));
    let stop = sim.run_secs(0.5);
    assert!(matches!(stop, Stop::Paused(..)), "{stop:?}");
    assert_eq!(line_at(&info, &sim), ("blink.ino".into(), 9));
    assert_eq!(sim.probe("LED · D13").text, "HIGH");
    // continue: the next hit is one loop (2 s) later
    sim.send(Command::Debug(DebugCommand::Continue));
    let stop = sim.run_secs(5.0);
    assert!(matches!(stop, Stop::Paused(..)), "{stop:?}");
    assert_eq!(line_at(&info, &sim), ("blink.ino".into(), 8));
    let period = (sim.time() - t_hit1).as_secs_f64();
    assert!((period - 2.0).abs() < 0.01, "loop period {period} s");
    // the debugger's view: registers and the call stack through the engine thread
    let (sp, frames) = sim.with(|b| {
        let m = b.mcu("u1").unwrap();
        let (_, avr) = m.avr_debug().unwrap();
        (avr.sp(), avr.call_stack().len())
    });
    assert!(sp > 0x100 && sp <= 0x8ff, "SP {sp:#x}");
    assert!(frames >= 1, "loop() is on the call stack");
    // remove the breakpoint and run freely
    sim.send(Command::Debug(DebugCommand::RemoveBreakpoint {
        part: "u1".into(),
        id: 1,
    }));
    sim.send(Command::Debug(DebugCommand::Continue));
    let stop = sim.run_secs(3.0);
    if let Stop::Paused(_, why) = &stop {
        // breakpoint ids are the debugger's; clear all through the bench if 1 was not it
        panic!("still pausing after removing the breakpoint: {why}");
    }
    sim.shutdown();
}

/// A compile error from the real toolchain maps to the sketch tab, line and column the
/// Firmware screen squiggles (skips without a toolchain).
#[test]
fn compile_errors_map_to_the_sketch() {
    let Some(tc) = toolchain() else {
        eprintln!("no Arduino toolchain: skipped");
        return;
    };
    let scratch = TempDir::new("broken");
    let sketch = scratch.path().join("broken");
    std::fs::create_dir_all(&sketch).unwrap();
    std::fs::write(
        sketch.join("broken.ino"),
        "void setup() {\n  pinMode(13, OUTPUT);\n}\n\nvoid loop() {\n  digitalWrite(13, HIGH)\n  delay(10);\n}\n",
    )
    .unwrap();
    let out = tc
        .compile(
            &sketch,
            &bb_firmware::CompileOptions::new("arduino:avr:uno"),
        )
        .unwrap();
    assert!(!out.success);
    let d = out
        .diagnostics
        .iter()
        .find(|d| d.severity == bb_firmware::Severity::Error)
        .unwrap_or_else(|| panic!("{:#?}\n{}", out.diagnostics, out.log));
    assert_eq!(d.tab.as_deref(), Some("broken.ino"), "{d:#?}");
    assert!(d.line.is_some_and(|l| (6..=7).contains(&l)), "{d:#?}");
    // and the fixed sketch builds with an arduino-cli size report
    std::fs::write(
        sketch.join("broken.ino"),
        "void setup() {\n  pinMode(13, OUTPUT);\n}\n\nvoid loop() {\n  digitalWrite(13, HIGH);\n  delay(10);\n}\n",
    )
    .unwrap();
    let out = tc
        .compile(
            &sketch,
            &bb_firmware::CompileOptions::new("arduino:avr:uno"),
        )
        .unwrap();
    assert!(out.success, "{}", out.log);
    let lines = out.size.as_ref().expect("size report").arduino_lines();
    assert!(
        lines[0].starts_with("Sketch uses ") && lines[0].contains("bytes"),
        "{lines:?}"
    );
}
