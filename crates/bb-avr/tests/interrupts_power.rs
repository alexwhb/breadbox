//! External / pin-change interrupts, sleep, watchdog, EEPROM, resets and brown-out on real
//! firmware.

mod common;
use bb_avr::{
    Avr, AvrConfig, Bootloader, Mcu, Pin, PinState, ResetCause, SleepMode, StopOn, StopReason,
};
use common::*;

fn first_event_after(ev: &[bb_avr::PinEvent], pin: Pin, after: u64) -> Option<bb_avr::PinEvent> {
    ev.iter()
        .filter(|e| e.pin == pin && e.cycle >= after)
        .min_by_key(|e| e.cycle)
        .copied()
}

#[test]
fn attach_interrupt_and_pcint_latency() {
    let mut avr = load("interrupts", "uno");
    let (d2, d4, d5, d6, d8, a0) = (
        Pin::new('D', 2),
        Pin::new('D', 4),
        Pin::new('D', 5),
        Pin::new('D', 6),
        Pin::new('B', 0),
        Pin::new('C', 0),
    );
    for p in [d2, d8, a0] {
        avr.set_pin_input(p, Some(false));
    }
    let mut ev = Vec::new();
    let mut out = String::new();
    run_until_text(&mut avr, 0, "ready\r\n", 100.0, &mut out, &mut ev);

    // attachInterrupt(INT0, RISING): Arduino's ISR wrapper + user function toggles D4.
    let t = avr.cycle() + 5_000;
    avr.schedule_pin_input(d2, Some(true), t);
    avr.schedule_pin_input(d2, Some(false), t + 20_000);
    avr.schedule_pin_input(d2, Some(true), t + 40_000);
    run_until(&mut avr, t + ms(3.0), &mut (), &mut ev);
    let e = first_event_after(&ev, d4, t).unwrap();
    let lat = e.cycle - t;
    println!("attachInterrupt latency {lat} cycles");
    assert!((30..=120).contains(&lat), "INT0 → D4 in {lat} cycles");
    let toggles = ev.iter().filter(|x| x.pin == d4 && x.cycle >= t).count();
    assert_eq!(toggles, 2, "RISING only: two rising edges → two toggles");
    run_until_text(&mut avr, 0, "int0=2\r\n", 20.0, &mut out, &mut ev);

    // PCINT0 on D8 fires on both edges.
    let t = avr.cycle() + 1_000;
    avr.schedule_pin_input(d8, Some(true), t);
    avr.schedule_pin_input(d8, Some(false), t + 3_000);
    run_until(&mut avr, t + 10_000, &mut (), &mut ev);
    assert_eq!(ev.iter().filter(|x| x.pin == d5 && x.cycle >= t).count(), 2);

    // Naked PCINT1 ISR (sbi PORTD,6 ; reti): latency = rest of current instruction + 4 (entry)
    // + 3 (JMP in the vector table) + 2 (SBI). Measure at 40 different phases.
    let mut lats = Vec::new();
    for k in 0..40u64 {
        let t = avr.cycle() + 1_000 + k * 7;
        avr.schedule_pin_input(a0, Some(k % 2 == 0), t);
        run_until(&mut avr, t + 200, &mut (), &mut ev);
        let e = first_event_after(&ev, d6, t).expect("D6 set by ISR");
        assert_eq!(e.state, PinState::Output { high: true });
        lats.push(e.cycle - t);
        avr.poke(0x2B, avr.peek(0x2B) & !0x40);
        ev.clear();
    }
    let min = *lats.iter().min().unwrap();
    let max = *lats.iter().max().unwrap();
    println!("naked PCINT latency {min}..{max} cycles");
    assert_eq!(min, 9, "minimum latency 4 + 3 + 2");
    assert!(
        max <= 12,
        "max {max}: a pending interrupt waits at most 3 extra cycles"
    );
    assert!(max > min, "latency depends on the interrupted instruction");
}

#[test]
fn power_down_sleep_and_wake_on_int0_level() {
    let mut avr = load("sleep_wake", "uno");
    let d2 = Pin::new('D', 2);
    let mut ev = Vec::new();
    let mut out = String::new();
    run_until_text(&mut avr, 0, "sleeping at", 200.0, &mut out, &mut ev);
    run_for(&mut avr, ms(5.0), &mut ev);
    out.push_str(&usart_text(&mut avr, 0));
    assert_eq!(avr.sleep_mode(), Some(SleepMode::PowerDown));
    let asleep_at: u64 = out.trim().split(' ').next_back().unwrap().parse().unwrap();
    // Nothing executes while powered down (Timer0 is stopped too).
    let i0 = avr.instructions();
    run_for(&mut avr, ms(500.0), &mut ev);
    assert_eq!(avr.instructions(), i0, "core ran while powered down");
    // INT0 is configured as LOW level; pulling D2 low wakes the core after the 16K CK start-up.
    let t = avr.cycle();
    avr.set_pin_input(d2, Some(false));
    run_until(&mut avr, t + 16_384, &mut (), &mut ev);
    assert_eq!(avr.instructions(), i0, "still in oscillator start-up");
    run_until(&mut avr, t + 16_384 + 4 + 50, &mut (), &mut ev);
    assert!(avr.instructions() > i0, "awake after start-up");
    avr.set_pin_input(d2, None);
    let mut rest = String::new();
    run_until_text(&mut avr, 0, "idle wakes=", 300.0, &mut rest, &mut ev);
    run_for(&mut avr, ms(5.0), &mut ev);
    rest.push_str(&usart_text(&mut avr, 0));
    let woke_at: u64 = rest
        .lines()
        .next()
        .unwrap()
        .trim()
        .split(' ')
        .next_back()
        .unwrap()
        .parse()
        .unwrap();
    // millis() did not advance during the 500 ms in power-down.
    assert!(woke_at - asleep_at <= 3, "millis {asleep_at} → {woke_at}");
    // Idle sleep keeps Timer0 running; every overflow (1.024 ms) wakes the core.
    let wakes: u64 = rest
        .lines()
        .nth(1)
        .unwrap()
        .trim()
        .split('=')
        .next_back()
        .unwrap()
        .parse()
        .unwrap();
    assert!((95..=130).contains(&wakes), "idle wakes {wakes}");
}

#[test]
fn watchdog_interrupt_mode_and_reset() {
    let mut avr = load("watchdog", "uno");
    avr.set_stop_on(StopOn {
        pin_change: false,
        reset: true,
        ..StopOn::default()
    });
    let mut ev = Vec::new();
    let reset_at;
    loop {
        match avr.run(ms(3_000.0), &mut ()) {
            StopReason::Reset { cause } => {
                assert_eq!(cause, ResetCause::Watchdog);
                reset_at = avr.cycle();
                break;
            }
            StopReason::ReachedCycle => panic!("no watchdog reset"),
            other => panic!("{other:?}"),
        }
    }
    ev.extend(avr.take_pin_events());
    let out = usart_text(&mut avr, 0);
    assert!(out.starts_with("boot\r\nwdt_irqs="), "{out}");
    // 16 ms interrupt mode for 500 ms → 31 interrupts.
    let irqs: u32 = out
        .lines()
        .nth(1)
        .unwrap()
        .split('=')
        .nth(1)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!((30..=32).contains(&irqs), "wdt irqs {irqs}");
    assert_eq!(out.matches("fed").count(), 5);
    // 250 ms setting = 32K cycles of the 128 kHz oscillator = 256 ms after the last WDR, which
    // happened 100 ms (delay) before the last D7 toggle.
    let marks = edges(&ev, Pin::new('D', 7));
    // (the reset itself tri-states D7, so take the last toggle before it)
    let last_feed_mark = marks.iter().filter(|m| m.0 < reset_at).last().unwrap().0;
    let gap_ms = (reset_at - last_feed_mark) as f64 / 16_000.0;
    assert!(
        (155.0..=157.0).contains(&gap_ms),
        "reset {gap_ms} ms after last mark"
    );
    assert_eq!(avr.last_reset_cause(), ResetCause::Watchdog);
    // The (emulated) Arduino bootloader disables the watchdog: the sketch boots again normally.
    let mut out2 = String::new();
    run_until_text(&mut avr, 0, "boot", 100.0, &mut out2, &mut ev);
}

#[test]
fn eeprom_write_timing_and_persistence_across_reset() {
    let mut avr = load("eeprom_persist", "uno");
    let mut ev = Vec::new();
    let mut out = String::new();
    run_until_text(&mut avr, 0, "written dt=", 200.0, &mut out, &mut ev);
    run_for(&mut avr, ms(20.0), &mut ev);
    out.push_str(&usart_text(&mut avr, 0));
    // 4 writes: each waits for the previous 3.4 ms erase+write; the last returns at once.
    let dt: u64 = out.trim().split('=').nth(1).unwrap().parse().unwrap();
    assert!(
        (10_200..=10_500).contains(&dt),
        "EEPROM writes took {dt} µs"
    );
    assert_eq!(&avr.eeprom()[0..4], &[0x42, b'B', b'B', 7]);
    assert_eq!(&avr.eeprom()[10..14], &3.25f32.to_le_bytes());
    avr.reset(ResetCause::External);
    let mut out2 = String::new();
    run_until_text(&mut avr, 0, "\r\n", 200.0, &mut out2, &mut ev);
    assert_eq!(out2, "read BB 7 3.25\r\n");
}

fn bare(name: &str) -> Avr {
    let mut cfg = AvrConfig::new(Mcu::Atmega328p);
    cfg.bootloader = Bootloader::None;
    cfg.stop_on = StopOn {
        pin_change: false,
        ..StopOn::default()
    };
    let mut avr = Avr::new(cfg);
    avr.load_hex(&std::fs::read_to_string(fixture_path(name, "uno", "hex")).unwrap())
        .unwrap();
    avr
}

#[test]
fn reset_causes_brown_out_and_reset_pin() {
    let mut avr = bare("mcusr");
    // load_hex performs an external reset (like an upload); power-cycle for PORF.
    avr.set_vcc(0.0);
    avr.set_vcc(5.0);
    let mut ev = Vec::new();
    let mut out = String::new();
    run_until_text(&mut avr, 0, "mcusr=", 200.0, &mut out, &mut ev);
    run_for(&mut avr, ms(2.0), &mut ev);
    out.push_str(&usart_text(&mut avr, 0));
    assert_eq!(out, "mcusr=1\r\n", "power-on reset");

    // Reset pin: held low → pins tri-stated, core stopped; released → EXTRF.
    avr.set_reset_pin(true);
    assert!(avr.in_reset());
    assert_eq!(
        avr.pin_info(Pin::new('D', 1)).state,
        PinState::Input { pullup: false }
    );
    run_for(&mut avr, ms(1.0), &mut ev);
    avr.set_reset_pin(false);
    let mut out = String::new();
    run_until_text(&mut avr, 0, "\r\n", 200.0, &mut out, &mut ev);
    assert_eq!(out, "mcusr=2\r\n", "external reset");

    // Brown-out: Vcc below 2.7 V holds the core in reset; restoring it restarts after the
    // reset delay (65 ms + 16K CK) with BORF.
    avr.set_vcc(2.5);
    assert!(avr.in_reset());
    run_for(&mut avr, ms(10.0), &mut ev);
    avr.set_vcc(4.8);
    let t = avr.cycle();
    run_until(&mut avr, t + ms(65.0), &mut (), &mut ev);
    assert!(avr.in_reset(), "still in the start-up delay");
    let mut out = String::new();
    run_until_text(&mut avr, 0, "\r\n", 200.0, &mut out, &mut ev);
    assert_eq!(out, "mcusr=4\r\n", "brown-out reset");
    assert_eq!(avr.last_reset_cause(), ResetCause::BrownOut);
    // A dip that stays above the BOD level does nothing.
    avr.set_vcc(3.0);
    assert!(!avr.in_reset());
}
