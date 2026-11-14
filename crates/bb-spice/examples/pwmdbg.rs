use bb_core::PinDrive;
use bb_spice::{CircuitBuilder, ModelKind, Session};
fn main() {
    let mut b = CircuitBuilder::new("pwm led + rc");
    let pin = b.port("d9", "d9", "0", PinDrive::HighZ);
    b.resistor("2", "d9", "n1", 220.0);
    b.model(
        "led_amber",
        ModelKind::D,
        &[("is", 1e-19), ("n", 1.8), ("rs", 6.0)],
    );
    b.diode("1", "n1", "0", "led_amber");
    b.resistor("f", "d9", "f", 10e3);
    b.capacitor("f", "f", "0", 1e-6);
    let nl = b.build();
    let mut s = Session::new(&nl).unwrap();
    let period = 1.0 / 490.0;
    let mut t = 0.0;
    let mut next_edge = 0.0;
    let mut high = false;
    let mut edges = 0;
    for k in 0..20000u64 {
        t = (k as f64 + 1.0) * 1e-6;
        if t >= next_edge {
            high = !high;
            edges += 1;
            s.set_port(
                pin,
                PinDrive::Thevenin {
                    v: if high { 5.0 } else { 0.0 },
                    r: 25.0,
                },
            );
            let cycle = (next_edge / period).floor();
            next_edge = if high {
                cycle * period + 0.478 * period
            } else {
                (cycle + 1.0) * period
            };
        }
        s.advance_to(t).unwrap();
        if k % 2000 == 0 {
            println!(
                "t={t:.6} edges={edges} {:?} vf={:.4}",
                s.stats(),
                s.voltage("f").unwrap()
            );
        }
    }
}
