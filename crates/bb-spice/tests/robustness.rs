//! No panics on user input: thousands of deterministic mutations of the reference netlists must
//! parse, elaborate and solve (or fail with an error) without panicking.

use bb_spice::{Analysis, Netlist};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const REPLACEMENTS: &[&str] = &[
    "0",
    "-1",
    "1e300",
    "-1e-300",
    "abc",
    "{",
    "}",
    "(",
    ")",
    "=",
    "{1/0}",
    "{sqrt(-1)}",
    "'",
    "1meg",
    "NaN",
    "inf",
    "v(",
    "i(x)",
    "params:",
    ".ends",
    "PULSE(",
    "POLY(3)",
    "+",
    ",",
    "{ext(\"q\")}",
    "dc",
    "ac",
    "off",
    "ic=",
    "x1",
    "{a b}",
    "1e",
    "µ",
    "Ω",
    "5%",
];

fn mutate(text: &str, rng: &mut Lcg) -> String {
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    if lines.len() < 2 {
        return text.to_string();
    }
    for _ in 0..1 + rng.below(3) {
        let li = 1 + rng.below(lines.len() - 1);
        let mut toks: Vec<String> = lines[li]
            .split_whitespace()
            .map(|t| t.to_string())
            .collect();
        match rng.below(7) {
            0 if !toks.is_empty() => {
                let i = rng.below(toks.len());
                toks.remove(i);
            }
            1 if !toks.is_empty() => {
                let i = rng.below(toks.len());
                toks[i] = REPLACEMENTS[rng.below(REPLACEMENTS.len())].to_string();
            }
            2 if !toks.is_empty() => {
                let i = rng.below(toks.len());
                let t = toks[i].clone();
                toks.insert(i, t);
            }
            3 => {
                let i = rng.below(toks.len() + 1);
                toks.insert(i, REPLACEMENTS[rng.below(REPLACEMENTS.len())].to_string());
            }
            4 if !toks.is_empty() => {
                let i = rng.below(toks.len());
                let t = &toks[i];
                let cut = rng.below(t.len().max(1));
                toks[i] = t.chars().take(cut).collect();
            }
            5 => {
                let other = lines[1 + rng.below(lines.len() - 1)].clone();
                lines.insert(li, other);
            }
            _ => {
                lines.remove(li);
                if lines.len() < 2 {
                    break;
                }
                continue;
            }
        }
        lines[li] = toks.join(" ");
    }
    lines.join("\n") + "\n"
}

#[test]
fn mutated_netlists_never_panic() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/reference");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "cir"))
        .collect();
    files.sort();
    let mut rng = Lcg(0x5eed);
    let mut cases = 0;
    let mut parsed = 0;
    let mut solved = 0;
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        for _ in 0..60 {
            let m = mutate(&text, &mut rng);
            cases += 1;
            if std::env::var("BB_FUZZ_TRACE").is_ok() {
                eprintln!("CASE {cases} {}\n{m}\nEND", f.display());
            }
            let r = catch_unwind(AssertUnwindSafe(|| {
                let Ok(nl) = Netlist::parse(&m) else {
                    return (false, false);
                };
                // operating point only: mutants may ask for absurd transients
                let ok = nl.run_analysis(&Analysis::Op).is_ok();
                (true, ok)
            }));
            match r {
                Ok((p, s)) => {
                    parsed += p as usize;
                    solved += s as usize;
                }
                Err(_) => panic!("panic on mutated netlist from {}:\n{m}", f.display()),
            }
        }
    }
    eprintln!("{cases} mutants: {parsed} parsed, {solved} solved");
    assert!(cases > 2000);
    assert!(parsed > cases / 4 && solved > 0);
}

#[test]
fn hostile_expressions_and_values() {
    let cases = [
        "t\nV1 a 0 {1/0}\nR1 a 0 1k\n",
        "t\nV1 a 0 1\nR1 a 0 {-1}\n",
        "t\nV1 a 0 1\nR1 a 0 0\n",
        "t\nV1 a 0 1\nC1 a 0 -1n\n",
        "t\nB1 a 0 V=ln(v(a)-1)\nR1 a 0 1\n",
        "t\nB1 a 0 V=sqrt(v(a)-5)\nR1 a 0 1\n",
        "t\nB1 a 0 I=exp(1000*v(a))\nR1 a 0 1\n",
        "t\n.param a={b}\n.param b={a}\nR1 x 0 {a}\n",
        "t\n.func f(x) {f(x)}\nR1 a 0 {f(1)}\n",
        "t\n.subckt s a\nX1 a s\n.ends\nX1 n s\nR1 n 0 1\n",
        "t\nX1 a b\n",
        "t\nK1 L1 L2 2\nL1 a 0 1m\nL2 b 0 1m\n",
        "t\nV1 a 0 PWL(1 0 0 1)\nR1 a 0 1\n",
        "t\nT1 a 0 b 0 Z0=-50 TD=1n\n",
        "t\nM1 d g s b m L=0\n.model m NMOS\nV1 d 0 1\n",
        "t\nF1 a 0 Vnone 2\nR1 a 0 1\n",
        "t\nW1 a 0 R1 sw\n.model sw CSW\nR1 a 0 1\n",
        "t\nE1 a 0 POLY(2) b 0 c 0 1 2 3\nR1 a 0 1\n",
        "t\nI1 0 a 1\n",
        "t\n.dc V1 0 1 0\nV1 a 0 0\nR1 a 0 1\n",
        "t\n.dc V1 1 0 0.1\nV1 a 0 0\nR1 a 0 1\n",
        "t\n.ac dec 0 1 10\nV1 a 0 AC 1\nR1 a 0 1\n",
        "t\n.noise v(a) V9 dec 1 1 10\nV1 a 0 AC 1\nR1 a 0 1\n",
        "t\n.tf v(zz) V1\nV1 a 0 1\nR1 a 0 1\n",
        "t\n.meas tran x avg v(nope)\n.tran 1u 2u\nV1 a 0 1\nR1 a 0 1\n",
        "t\n.step param zz list\n",
        "t\n.mc 0\n",
    ];
    for text in cases {
        let r = catch_unwind(|| {
            if let Ok(nl) = Netlist::parse(text) {
                let _ = nl.run();
                let _ = nl.run_analysis(&Analysis::Op);
            }
        });
        assert!(r.is_ok(), "panicked on:\n{text}");
    }
}
