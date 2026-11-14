//! MCU-3: ≥ 3× real time for a busy 16 MHz sketch (release build).
//!
//! `cargo test -p bb-avr --release --test perf -- --ignored --nocapture`

mod common;
use bb_avr::{StopOn, StopReason};
use common::*;
use std::time::Instant;

fn measure(stop_on_pins: bool) -> f64 {
    let mut avr = load("busy", "uno");
    avr.set_stop_on(StopOn {
        pin_change: stop_on_pins,
        ..StopOn::default()
    });
    let mut io = Volts::default();
    io.map.insert(bb_avr::Pin::new('C', 0), 2.5);
    let sim_secs = 10.0;
    let end = ms(sim_secs * 1000.0);
    let mut stops = 0u64;
    let mut events = 0usize;
    let t0 = Instant::now();
    loop {
        match avr.run(end, &mut io) {
            StopReason::ReachedCycle => break,
            StopReason::PinChange => stops += 1,
            other => panic!("{other:?}"),
        }
        events += avr.take_pin_events().len();
        io.samples.clear();
    }
    events += avr.take_pin_events().len();
    let wall = t0.elapsed().as_secs_f64();
    let out = usart_text(&mut avr, 0);
    assert!(
        out.lines().count() >= 90,
        "sketch ran: {} lines",
        out.lines().count()
    );
    let ratio = sim_secs / wall;
    println!(
        "busy sketch, stop on pin change = {stop_on_pins}: {sim_secs} s simulated in {wall:.3} s → {ratio:.1}× real time \
         ({:.1} MIPS, {stops} stops, {events} pin events)",
        avr.instructions() as f64 / wall / 1e6
    );
    ratio
}

#[test]
#[ignore]
fn busy_sketch_runs_at_least_3x_real_time() {
    let logged = measure(false);
    let stopping = measure(true);
    assert!(logged >= 3.0, "{logged:.2}× real time");
    assert!(
        stopping >= 3.0,
        "{stopping:.2}× real time with co-sim style stops"
    );
}
