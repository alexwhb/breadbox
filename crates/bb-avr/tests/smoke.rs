mod common;
use bb_avr::Pin;
use common::*;

#[test]
fn smoke_blink() {
    let mut avr = load("blink", "uno");
    let ev = run_ms(&mut avr, 5000.0);
    let e = edges(&ev, Pin::new('B', 5));
    for w in e.windows(2) {
        println!(
            "{} -> {} : {:.4} ms",
            w[0].0,
            w[1].0,
            (w[1].0 - w[0].0) as f64 / 16000.0
        );
    }
    println!("instr {}", avr.instructions());
    assert!(e.len() >= 4);
}

#[test]
fn smoke_serial() {
    let mut avr = load("serial_hello", "uno");
    let _ev = run_ms(&mut avr, 300.0);
    let s = usart_text(&mut avr, 0);
    println!("{s}");
    assert!(s.contains("Hello, Breadbox!"));
}
