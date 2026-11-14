//! Runs a netlist file and prints a summary: `cargo run -p bb-spice --example spice -- file.cir`.
use bb_spice::Netlist;

fn main() {
    let path = std::env::args().nth(1).expect("usage: spice <file.cir>");
    let nl = match Netlist::parse_file(&path) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    match nl.run() {
        Ok(r) => {
            for w in &r.warnings {
                eprintln!("{w}");
            }
            for run in &r.runs {
                if !run.label.is_empty() {
                    println!("== {}", run.label);
                }
                for p in &run.plots {
                    println!("-- {} ({} points)", p.kind_name(), p.len());
                    for v in &p.vectors {
                        let n = v.len();
                        if n == 0 {
                            continue;
                        }
                        if v.is_complex() {
                            let c = v.complex_at(n - 1);
                            println!("   {:<24} last = {:.6e} {:+.6e}j", v.name, c.re, c.im);
                        } else {
                            println!(
                                "   {:<24} first = {:.6e} last = {:.6e}",
                                v.name,
                                v.real_at(0),
                                v.real_at(n - 1)
                            );
                        }
                    }
                }
                for m in &run.measures {
                    println!("   meas {} = {:?}", m.name, m.value);
                }
            }
            println!("{}", r.summary());
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    }
}
