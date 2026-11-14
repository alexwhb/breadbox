//! Prints the `.cir` text produced by `CircuitBuilder` (night-light example).
use bb_spice::{Analysis, CircuitBuilder, ModelKind};

fn main() {
    let mut b = CircuitBuilder::new("led night light");
    b.vsource_dc("1", "vcc", "0", 5.0);
    b.resistor_expr(
        "ldr",
        "vcc",
        "a1",
        "350k*pwr(max(ext(\"J2.lux\"),0.01),-0.7)",
    )
    .unwrap();
    b.resistor("1", "a1", "0", 10e3);
    b.model(
        "led_amber",
        ModelKind::D,
        &[("is", 1e-19), ("n", 1.8), ("rs", 6.0)],
    );
    b.diode("1", "a1", "0", "led_amber");
    b.analysis(Analysis::Op);
    print!("{}", b.build().to_spice_text());
}
