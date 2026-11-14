//! Register-level peripheral features on real firmware (see periph_misc.ino).

mod common;
use bb_avr::{Pin, ResetCause, StopOn, StopReason};
use common::*;

#[test]
fn register_level_peripheral_phases() {
    let mut avr = load("periph_misc", "uno");
    avr.set_stop_on(StopOn {
        pin_change: false,
        reset: true,
        ..StopOn::default()
    });
    let mut ev = Vec::new();
    let mut out = String::new();
    // A2 for the ADC noise-reduction phase; comparator inputs start with AIN0 < AIN1.
    avr.set_pin_voltage(Pin::new('C', 2), 2.5);
    avr.set_pin_voltage(Pin::new('D', 6), 1.0);
    avr.set_pin_voltage(Pin::new('D', 7), 2.0);
    avr.set_pin_input(Pin::new('D', 2), Some(true));

    // 1. EE_READY interrupt fires once when the 3.4 ms write completes.
    let line = run_until_line(&mut avr, 0, "ee_irqs=", 100.0, &mut out, &mut ev);
    assert_eq!(line, "ee_irqs=1 ee5=5A");
    assert_eq!(avr.eeprom()[5], 0x5A);

    // 2. 100 rising edges on T0 (D4) → TCNT0 = 100.
    run_until_text(&mut avr, 0, "T0?\r\n", 50.0, &mut out, &mut ev);
    let t0 = Pin::new('D', 4);
    let mut t = avr.cycle() + 100;
    for _ in 0..100 {
        avr.schedule_pin_input(t0, Some(true), t);
        avr.schedule_pin_input(t0, Some(false), t + 40);
        t += 80;
    }
    run_until(&mut avr, t + 100, &mut (), &mut ev);
    avr.usart_inject(0, b"g");
    run_until_text(&mut avr, 0, "t0=100\r\n", 20.0, &mut out, &mut ev);

    // 3. Two FOC2A strobes toggle OC2A (D11) twice, 50 µs apart.
    run_until_text(&mut avr, 0, "foc\r\n", 20.0, &mut out, &mut ev);
    let oc2a: Vec<(u64, bool)> = edges(&ev, Pin::new('B', 3));
    let toggles: Vec<&(u64, bool)> = oc2a.iter().filter(|e| e.1).collect();
    assert!(!toggles.is_empty(), "{oc2a:?}");
    // pinMode → low, FOC → high, FOC 50 µs later → low.
    let levels: Vec<bool> = oc2a.iter().map(|e| e.1).collect();
    assert_eq!(levels, vec![false, true, false], "{oc2a:?}");
    let dt = (oc2a[2].0 - oc2a[1].0) as f64 / 16.0;
    assert!((45.0..60.0).contains(&dt), "FOC strobes {dt} µs apart");

    // 4. Comparator rising edges at known cycles → Timer1 ICR differences (clk/8).
    run_until_text(&mut avr, 0, "AC?\r\n", 20.0, &mut out, &mut ev);
    avr.usart_inject(0, b"g");
    run_for(&mut avr, ms(1.0), &mut ev);
    let (ain0, mut stamps) = (Pin::new('D', 6), Vec::new());
    for gap in [0u64, 16_000, 24_000] {
        run_for(&mut avr, gap.max(100), &mut ev);
        avr.set_pin_voltage(ain0, 3.0);
        stamps.push(avr.cycle());
        run_for(&mut avr, 2_000, &mut ev);
        avr.set_pin_voltage(ain0, 1.0);
    }
    let line = run_until_line(&mut avr, 0, "ac ", 20.0, &mut out, &mut ev);
    let d: Vec<i64> = line[3..]
        .split(' ')
        .map(|x| x.trim().parse().unwrap())
        .collect();
    for (k, got) in d.iter().enumerate() {
        let want = ((stamps[k + 1] - stamps[k]) / 8) as i64;
        assert!((got - want).abs() <= 1, "capture diff {got} vs {want}");
    }

    // 5. ADC noise reduction: 2.5 V on A2 → 512, ADC ISR woke the core.
    let line = run_until_line(&mut avr, 0, "adcnr=", 50.0, &mut out, &mut ev);
    assert_eq!(line, "adcnr=512 1");

    // 6. INT0 FALLING: three falling edges; CHANGE: four changes.
    let d2 = Pin::new('D', 2);
    run_until_text(&mut avr, 0, "INT?\r\n", 20.0, &mut out, &mut ev);
    let mut t = avr.cycle() + 200;
    for _ in 0..3 {
        avr.schedule_pin_input(d2, Some(false), t);
        avr.schedule_pin_input(d2, Some(true), t + 1_000);
        t += 2_000;
    }
    run_until(&mut avr, t + 100, &mut (), &mut ev);
    avr.usart_inject(0, b"g");
    run_until_text(&mut avr, 0, "fall=3\r\n", 30.0, &mut out, &mut ev);
    run_until_text(&mut avr, 0, "CHG?\r\n", 20.0, &mut out, &mut ev);
    let mut t = avr.cycle() + 200;
    for k in 0..4 {
        avr.schedule_pin_input(d2, Some(k % 2 == 1), t);
        t += 1_000;
    }
    run_until(&mut avr, t + 100, &mut (), &mut ev);
    avr.usart_inject(0, b"g");
    run_until_text(&mut avr, 0, "chg=4\r\n", 30.0, &mut out, &mut ev);

    // 7. TSM halts Timer1; after release it counts 100 µs / 4 µs = 25 ticks.
    let line = run_until_line(&mut avr, 0, "tsm ", 20.0, &mut out, &mut ev);
    let v: Vec<i64> = line[4..]
        .split(' ')
        .map(|x| x.trim().parse().unwrap())
        .collect();
    assert_eq!(v[0], 0, "halted");
    assert!((24..=27).contains(&v[1]), "{line}");

    // 8. CLKPR /2: the 100 ms delay takes 200 ms of real time.
    run_until_text(&mut avr, 0, "clkdone\r\n", 400.0, &mut out, &mut ev);
    let d7 = edges(&ev, Pin::new('D', 7));
    let rise = d7.iter().find(|e| e.1).unwrap().0;
    let fall = d7.iter().rfind(|e| !e.1).unwrap().0;
    let dur = (fall - rise) as f64 / 16_000.0;
    assert!(
        (dur - 200.0).abs() < 2.5,
        "delay(100) at clk/2 took {dur} ms"
    );

    // 9. Watchdog interrupt + reset: interrupt after 16 ms, reset 16 ms later.
    run_until_text(&mut avr, 0, "wdt irq\r\n", 100.0, &mut out, &mut ev);
    let irq_at = avr.cycle();
    match avr.run(avr.cycle() + ms(100.0), &mut ()) {
        StopReason::Reset {
            cause: ResetCause::Watchdog,
        } => {}
        other => panic!("{other:?}"),
    }
    let gap = (avr.cycle() - irq_at) as f64 / 16_000.0;
    assert!(gap < 17.0, "second timeout {gap} ms after the interrupt");
    let mut again = String::new();
    run_until_text(&mut avr, 0, "start\r\n", 50.0, &mut again, &mut ev);
}
