//! Debug helper: compares crossing times of a vector between bb-spice and the ngspice golden
//! data. `cargo run -p bb-spice --example refdiff -- <name> <vector> <level>`
use bb_spice::export::read_raw;
use bb_spice::{Netlist, PlotKind};

fn crossings(t: &[f64], y: &[f64], lvl: f64) -> Vec<f64> {
    let mut out = Vec::new();
    for i in 1..t.len() {
        let (a, b) = (y[i - 1] - lvl, y[i] - lvl);
        if a * b < 0.0 || (b == 0.0 && a != 0.0) {
            out.push(t[i - 1] + (t[i] - t[i - 1]) * a / (a - b));
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/reference");
    let name = &args[1];
    let vec = &args[2];
    let lvl: f64 = args[3].parse().unwrap();
    let nl = Netlist::parse_file(dir.join(format!("{name}.cir"))).unwrap();
    let r = nl.run().unwrap();
    let g = read_raw(&std::fs::read_to_string(dir.join(format!("{name}.raw"))).unwrap()).unwrap();
    let gp = g.iter().find(|p| p.kind == PlotKind::Tran).unwrap();
    let op = r.plot(PlotKind::Tran).unwrap();
    let gc = crossings(&gp.scale_values(), &gp.real(vec).unwrap(), lvl);
    let oc = crossings(&op.scale_values(), &op.real(vec).unwrap(), lvl);
    println!("points: ours {} ngspice {}", op.len(), gp.len());
    let (gt, gy) = (gp.scale_values(), gp.real(vec).unwrap());
    let (ot, oy) = (op.scale_values(), op.real(vec).unwrap());
    let mut devs: Vec<(f64, f64, f64, f64)> = gt
        .iter()
        .zip(&gy)
        .map(|(&t, &y)| {
            let o = bb_spice::meas::interp(&ot, &oy, t);
            ((o - y).abs(), t, o, y)
        })
        .collect();
    devs.sort_by(|a, b| b.0.total_cmp(&a.0));
    for d in devs.iter().take(8) {
        println!(
            "  t={:.6e} ours {:.6e} ngspice {:.6e} |d|={:.3e}",
            d.1, d.2, d.3, d.0
        );
    }
    for i in 0..gc.len().max(oc.len()) {
        let a = oc.get(i).copied().unwrap_or(f64::NAN);
        let b = gc.get(i).copied().unwrap_or(f64::NAN);
        println!("{i:3}  ours {a:.6e}  ngspice {b:.6e}  diff {:+.3e}", a - b);
    }
}
