//! Source-level debugging of real compiled firmware (`debug_target.ino`, built -Og by
//! scripts/build-test-firmware.sh).

use bb_avr::{Avr, Mcu, StopOn, StopReason};
use bb_firmware::{DebugInfo, Debugger, Decision, HitCondition, PauseReason, Value};
use std::sync::Arc;

const SKETCH: &str = "debug_target.ino";

fn setup() -> (Avr, Debugger) {
    let elf = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/debug_target.uno.elf"
    ))
    .unwrap();
    let mut avr = Avr::with_mcu(Mcu::Atmega328p);
    avr.set_stop_on(StopOn {
        pin_change: false,
        ..StopOn::default()
    });
    avr.load_elf(&elf).unwrap();
    let info = Arc::new(DebugInfo::from_elf(&elf).unwrap());
    (avr, Debugger::new(info))
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(i) => *i,
        Value::UInt(u) => *u as i64,
        other => panic!("not an integer: {other:?}"),
    }
}

fn local(d: &Debugger, avr: &Avr, name: &str) -> Value {
    d.locals(avr, 0)
        .into_iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no local {name}"))
        .value
}

fn global(d: &Debugger, avr: &Avr, name: &str) -> Value {
    d.read(avr, name, 0)
        .unwrap_or_else(|| panic!("no global {name}"))
        .value
}

#[test]
fn breakpoint_in_loop_hits_n_times_then_step_over_and_read_variables() {
    let (mut avr, mut dbg) = setup();
    let bp = dbg
        .add_breakpoint(&mut avr, SKETCH, 28, HitCondition::Always)
        .unwrap();
    assert_eq!(bp.line, 28);
    assert!(bp.file.ends_with(SKETCH));
    for k in 1..=5i64 {
        let p = dbg
            .run_until_pause(&mut avr, &mut (), 16_000_000)
            .expect("breakpoint hit");
        assert_eq!(
            p.reason,
            PauseReason::Breakpoint {
                id: bp.id,
                hits: k as u64
            }
        );
        let loc = p.location.unwrap();
        assert!(loc.file.ends_with(SKETCH));
        assert_eq!(loc.line, 28);
        assert_eq!(p.function.as_deref(), Some("loop"));
        // counter++ ran on line 25 of this iteration.
        assert_eq!(int(&global(&dbg, &avr, "counter")), k);
        // step = counter * 3 was computed before the increment.
        let step = 3 * (k - 1);
        assert_eq!(int(&local(&dbg, &avr, "step")), step);
        // total accumulates square_plus(step, 7) = step² + 7.
        let expect_total: i64 = (0..k).map(|i| (3 * i) * (3 * i) + 7).sum();
        assert_eq!(int(&global(&dbg, &avr, "total")), expect_total);
    }
    assert_eq!(dbg.breakpoints()[0].hits, 5);
    // Step over line 28 → line 29; then over 29 (flag = !flag) → line 30.
    let flag_before = global(&dbg, &avr, "flag");
    dbg.step_over(&mut avr);
    let p = dbg.run_until_pause(&mut avr, &mut (), 1_000_000).unwrap();
    assert_eq!(p.reason, PauseReason::Step);
    assert_eq!(p.location.unwrap().line, 29);
    dbg.step_over(&mut avr);
    let p = dbg.run_until_pause(&mut avr, &mut (), 1_000_000).unwrap();
    assert_eq!(p.location.unwrap().line, 30);
    let flag_after = global(&dbg, &avr, "flag");
    assert_ne!(flag_before, flag_after, "line 29 toggled flag");
    // history[counter & 3] = step was stored on line 28.
    let Value::Array(h) = global(&dbg, &avr, "history") else {
        panic!()
    };
    assert_eq!(h.len(), 4);
    assert_eq!(int(&h[5 & 3]), 12);
    // Other basic types.
    assert!(matches!(global(&dbg, &avr, "ratio"), Value::Float(_)));
    let g = dbg.read(&avr, "greeting", 0).unwrap();
    assert_eq!(g.type_name, "const char*");
    assert!(
        matches!(g.value, Value::Pointer { string: Some(ref s), .. } if s == "hi"),
        "{:?}",
        g.value
    );
    let names: Vec<String> = dbg.globals(&avr).into_iter().map(|v| v.name).collect();
    for n in [
        "counter", "total", "ratio", "flag", "letter", "history", "greeting",
    ] {
        assert!(names.contains(&n.to_string()), "{n} missing from {names:?}");
    }
    assert!(!names.iter().any(|n| n == "UDR0"), "SFRs are not variables");
}

#[test]
fn conditional_hit_counts() {
    let (mut avr, mut dbg) = setup();
    let every = dbg
        .add_breakpoint(&mut avr, SKETCH, 30, HitCondition::Every(5))
        .unwrap();
    let p = dbg.run_until_pause(&mut avr, &mut (), 16_000_000).unwrap();
    assert_eq!(
        p.reason,
        PauseReason::Breakpoint {
            id: every.id,
            hits: 5
        }
    );
    assert_eq!(int(&global(&dbg, &avr, "counter")), 5);
    let p = dbg.run_until_pause(&mut avr, &mut (), 16_000_000).unwrap();
    assert_eq!(
        p.reason,
        PauseReason::Breakpoint {
            id: every.id,
            hits: 10
        }
    );
    dbg.remove_breakpoint(&mut avr, every.id);
    let eq = dbg
        .add_breakpoint(&mut avr, SKETCH, 24, HitCondition::Equals(24))
        .unwrap();
    let p = dbg.run_until_pause(&mut avr, &mut (), 16_000_000).unwrap();
    assert_eq!(
        p.reason,
        PauseReason::Breakpoint {
            id: eq.id,
            hits: 24
        }
    );
    assert_eq!(int(&global(&dbg, &avr, "counter")), 10 + 23);
    assert!(
        dbg.run_until_pause(&mut avr, &mut (), 2_000_000).is_none(),
        "Equals(24) fires once"
    );
}

#[test]
fn step_into_call_stack_and_step_out() {
    let (mut avr, mut dbg) = setup();
    dbg.add_breakpoint(&mut avr, SKETCH, 26, HitCondition::Equals(3))
        .unwrap();
    dbg.run_until_pause(&mut avr, &mut (), 16_000_000).unwrap();
    let step = int(&local(&dbg, &avr, "step"));
    assert_eq!(step, 6);
    dbg.step_into(&mut avr);
    let p = dbg.run_until_pause(&mut avr, &mut (), 1_000_000).unwrap();
    assert_eq!(p.function.as_deref(), Some("square_plus"));
    let line = p.location.unwrap().line;
    assert!((11..=12).contains(&line), "landed on line {line}");
    assert_eq!(int(&local(&dbg, &avr, "x")), 6);
    assert_eq!(int(&local(&dbg, &avr, "bias")), 7);
    let stack = dbg.call_stack(&avr);
    let names: Vec<String> = stack.iter().filter_map(|f| f.function.clone()).collect();
    assert_eq!(&names[..3], &["square_plus", "loop", "main"], "{names:?}");
    assert_eq!(
        stack[1].location.as_ref().unwrap().line,
        26,
        "caller frame at the call site"
    );
    // Outer frame locals are still readable through the CFA/CFI.
    assert!(dbg.locals(&avr, 1).iter().any(|v| v.name == "step"));
    // Step through the function body and back out.
    dbg.step_over(&mut avr);
    let p = dbg.run_until_pause(&mut avr, &mut (), 1_000_000).unwrap();
    assert!(p.location.unwrap().line > line);
    dbg.step_out(&mut avr);
    let p = dbg.run_until_pause(&mut avr, &mut (), 1_000_000).unwrap();
    assert_eq!(p.function.as_deref(), Some("loop"));
    assert_eq!(
        dbg.call_stack(&avr).len(),
        3,
        "back in loop (← main ← reset)"
    );
    // The return address is either the rest of line 26 or already the first statement of 27.
    let mut line = p.location.unwrap().line;
    if line == 26 {
        dbg.step_over(&mut avr);
        line = dbg
            .run_until_pause(&mut avr, &mut (), 1_000_000)
            .unwrap()
            .location
            .unwrap()
            .line;
    }
    assert_eq!(line, 27);
    assert_eq!(int(&local(&dbg, &avr, "r")), 6 * 6 + 7);
}

#[test]
fn debugger_as_policy_layer_under_an_outer_scheduler() {
    // A co-sim style driver: 1 µs quanta, stops on every pin change, asks the debugger about
    // every stop.
    let (mut avr, mut dbg) = setup();
    avr.set_stop_on(StopOn {
        pin_change: true,
        ..StopOn::default()
    });
    dbg.add_breakpoint(&mut avr, SKETCH, 32, HitCondition::AtLeast(3))
        .unwrap();
    let mut t = 0;
    let mut pauses = 0;
    let mut pin_stops = 0;
    while pauses < 2 && t < 32_000_000 {
        t += 16;
        loop {
            let r = avr.run(t, &mut ());
            if r == StopReason::PinChange {
                pin_stops += 1;
            }
            match dbg.on_stop(&mut avr, &r) {
                Decision::Pause(p) => {
                    pauses += 1;
                    assert_eq!(p.location.unwrap().line, 32);
                    assert_eq!(int(&global(&dbg, &avr, "counter")), 2 + pauses);
                }
                Decision::Continue => {}
            }
            if matches!(r, StopReason::ReachedCycle) {
                break;
            }
        }
    }
    assert_eq!(pauses, 2);
    assert!(
        pin_stops > 0,
        "digitalWrite(13) toggles were delivered to the scheduler"
    );
}
