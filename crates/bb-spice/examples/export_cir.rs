//! Re-exports a netlist file through bb-spice's writer: `--example export_cir -- in.cir`.
fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: export_cir <file.cir>");
    let nl = bb_spice::Netlist::parse_file(path).unwrap_or_else(|e| panic!("{e}"));
    print!("{}", nl.to_spice_text());
}
