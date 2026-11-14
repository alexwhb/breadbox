//! Debug helper: worst relative DC/OP deviations vs golden. `--example dcdiff -- <name> [kind#]`
use bb_spice::export::read_raw;
use bb_spice::{Netlist, PlotKind};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/reference");
    let name = &args[1];
    let nl = Netlist::parse_file(dir.join(format!("{name}.cir"))).unwrap();
    let r = nl.run().unwrap();
    let g = read_raw(&std::fs::read_to_string(dir.join(format!("{name}.raw"))).unwrap()).unwrap();
    for kind in [PlotKind::Op, PlotKind::Dc] {
        let gs: Vec<_> = g.iter().filter(|p| p.kind == kind).collect();
        let os: Vec<_> = r
            .runs
            .iter()
            .flat_map(|r| r.plots.iter())
            .filter(|p| p.kind == kind)
            .collect();
        for (gp, op) in gs.iter().zip(os) {
            for gv in &gp.vectors {
                let Some(ov) = op.vector(&gv.name) else {
                    continue;
                };
                if ov.len() != gv.len() || gv.name.contains("sweep") {
                    continue;
                }
                let mut worst = (0.0, 0usize);
                for i in 0..gv.len() {
                    let (a, b) = (ov.real_at(i), gv.real_at(i));
                    let floor = if gv.name.starts_with("i(") {
                        1e-9
                    } else {
                        1e-6
                    };
                    let e = (a - b).abs() / (1e-3 * b.abs()).max(floor);
                    if e > worst.0 {
                        worst = (e, i);
                    }
                }
                let i = worst.1;
                println!(
                    "{:?} {:<12} worst {:.3} at [{i}] scale={:.4e}: ours {:.9e} ngspice {:.9e}",
                    kind,
                    gv.name,
                    worst.0,
                    gp.scale_at(i),
                    ov.real_at(i),
                    gv.real_at(i)
                );
            }
        }
    }
}
