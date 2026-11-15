//! The Analyses sidebar (`PRO-3`) as text operations on the netlist: each card is a dot-card
//! line (`.tran 0 10m 0 1u`). Disabling a card comments it out as `*off .ac …` so its
//! configuration survives; the config popover rewrites the line.

use crate::widgets::plot::fmt_si;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnalysisKind {
    Tran,
    Dc,
    Ac,
    Mc,
    Temp,
    Noise,
    Op,
}

impl AnalysisKind {
    /// Design order.
    pub const ALL: [AnalysisKind; 7] = [
        AnalysisKind::Tran,
        AnalysisKind::Dc,
        AnalysisKind::Ac,
        AnalysisKind::Mc,
        AnalysisKind::Temp,
        AnalysisKind::Noise,
        AnalysisKind::Op,
    ];

    pub fn title(self) -> &'static str {
        match self {
            AnalysisKind::Tran => "Transient (.tran)",
            AnalysisKind::Dc => "DC sweep (.dc)",
            AnalysisKind::Ac => "AC / Bode (.ac)",
            AnalysisKind::Mc => "Monte Carlo (.mc)",
            AnalysisKind::Temp => "Temperature sweep",
            AnalysisKind::Noise => "Noise (.noise)",
            AnalysisKind::Op => "Op point (.op)",
        }
    }

    pub fn keyword(self) -> &'static str {
        match self {
            AnalysisKind::Tran => ".tran",
            AnalysisKind::Dc => ".dc",
            AnalysisKind::Ac => ".ac",
            AnalysisKind::Mc => ".mc",
            AnalysisKind::Temp => ".temp",
            AnalysisKind::Noise => ".noise",
            AnalysisKind::Op => ".op",
        }
    }

    /// Card inserted when the user enables an analysis the netlist doesn't have.
    pub fn default_args(self) -> &'static str {
        match self {
            AnalysisKind::Tran => "0 10m 0 1u",
            AnalysisKind::Dc => "V1 0 5 0.1",
            AnalysisKind::Ac => "dec 20 1 1meg",
            AnalysisKind::Mc => "100 tol=5% seed=1",
            AnalysisKind::Temp => "-20 0 25 50 85",
            AnalysisKind::Noise => "v(out) V1 dec 10 10 100k",
            AnalysisKind::Op => "",
        }
    }

    /// Editable fields of the config popover: `(label, hint)`.
    pub fn fields(self) -> &'static [(&'static str, &'static str)] {
        match self {
            AnalysisKind::Tran => &[("Stop time", "10m"), ("Max step", "1u"), ("Start time", "0")],
            AnalysisKind::Dc => &[("Sweep", "V1 / lux / temp"), ("Start", "1"), ("Stop", "10k"), ("Mode", "dec / lin / oct"), ("Points or step", "40")],
            AnalysisKind::Ac => &[("Mode", "dec / lin / oct"), ("Points", "20"), ("Start", "1"), ("Stop", "1meg")],
            AnalysisKind::Mc => &[("Runs", "100"), ("Tolerance", "5%"), ("Seed", "7")],
            AnalysisKind::Temp => &[("Temperatures °C", "-20 0 25 50 85")],
            AnalysisKind::Noise => &[("Output", "v(out)"), ("Input source", "V1"), ("Mode", "dec"), ("Points", "10"), ("Start", "10"), ("Stop", "100k")],
            AnalysisKind::Op => &[],
        }
    }

    /// Splits card arguments into the popover's field values.
    pub fn to_fields(self, args: &[String]) -> Vec<String> {
        let g = |i: usize| args.get(i).cloned().unwrap_or_default();
        let kv = |k: &str| {
            args.iter()
                .find_map(|a| a.to_ascii_lowercase().strip_prefix(&format!("{k}=")).map(str::to_string))
                .unwrap_or_default()
        };
        match self {
            AnalysisKind::Tran => {
                // .tran tstep tstop [tstart [tmax]]
                let max = if args.len() >= 4 { g(3) } else { g(0) };
                vec![g(1), max, if args.len() >= 3 { g(2) } else { "0".into() }]
            }
            AnalysisKind::Dc => {
                if args.get(3).is_some_and(|m| ["dec", "lin", "oct"].contains(&m.to_ascii_lowercase().as_str())) {
                    vec![g(0), g(1), g(2), g(3), g(4)]
                } else {
                    vec![g(0), g(1), g(2), "lin".into(), g(3)]
                }
            }
            AnalysisKind::Ac => vec![g(0), g(1), g(2), g(3)],
            AnalysisKind::Mc => vec![g(0), kv("tol"), kv("seed")],
            AnalysisKind::Temp => vec![args.join(" ")],
            AnalysisKind::Noise => vec![g(0), g(1), g(2), g(3), g(4), g(5)],
            AnalysisKind::Op => vec![],
        }
    }

    /// Composes card arguments from field values.
    pub fn from_fields(self, f: &[String]) -> String {
        let g = |i: usize| f.get(i).map(|s| s.trim().to_string()).unwrap_or_default();
        match self {
            AnalysisKind::Tran => format!("0 {} {} {}", or(&g(0), "10m"), or(&g(2), "0"), or(&g(1), "1u")),
            AnalysisKind::Dc => {
                if g(3).eq_ignore_ascii_case("lin") || g(3).is_empty() {
                    format!("{} {} {} {}", g(0), g(1), g(2), g(4))
                } else {
                    format!("{} {} {} {} {}", g(0), g(1), g(2), g(3), g(4))
                }
            }
            AnalysisKind::Ac => format!("{} {} {} {}", or(&g(0), "dec"), or(&g(1), "20"), or(&g(2), "1"), or(&g(3), "1meg")),
            AnalysisKind::Mc => {
                let mut s = or(&g(0), "100");
                if !g(1).is_empty() {
                    s.push_str(&format!(" tol={}", g(1)));
                }
                if !g(2).is_empty() {
                    s.push_str(&format!(" seed={}", g(2)));
                }
                s
            }
            AnalysisKind::Temp => g(0),
            AnalysisKind::Noise => f.iter().map(|s| s.trim()).collect::<Vec<_>>().join(" "),
            AnalysisKind::Op => String::new(),
        }
    }
}

fn or(s: &str, d: &str) -> String {
    if s.is_empty() { d.to_string() } else { s.to_string() }
}

/// A dot-card found in the netlist.
#[derive(Debug, Clone, PartialEq)]
pub struct CardLine {
    pub kind: AnalysisKind,
    /// 0-based line index.
    pub line: usize,
    pub enabled: bool,
    pub args: Vec<String>,
}

const OFF: &str = "*off ";

fn classify(line: &str) -> Option<(AnalysisKind, bool, Vec<String>)> {
    let t = line.trim();
    let (enabled, body) = match t.strip_prefix(OFF) {
        Some(b) => (false, b.trim()),
        None => (true, t),
    };
    let mut it = body.split_whitespace();
    let kw = it.next()?.to_ascii_lowercase();
    let args: Vec<String> = it.map(str::to_string).collect();
    let kind = match kw.as_str() {
        ".tran" => AnalysisKind::Tran,
        ".dc" => AnalysisKind::Dc,
        ".ac" => AnalysisKind::Ac,
        ".mc" => AnalysisKind::Mc,
        ".noise" => AnalysisKind::Noise,
        ".op" => AnalysisKind::Op,
        // a temperature *sweep* is a multi-value .temp
        ".temp" if args.len() > 1 || !enabled => AnalysisKind::Temp,
        _ => return None,
    };
    Some((kind, enabled, args))
}

/// All analysis cards in the text.
pub fn scan(text: &str) -> Vec<CardLine> {
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| {
            classify(l).map(|(kind, enabled, args)| CardLine {
                kind,
                line: i,
                enabled,
                args,
            })
        })
        .collect()
}

/// The card for `kind` (first occurrence).
pub fn find(text: &str, kind: AnalysisKind) -> Option<CardLine> {
    scan(text).into_iter().find(|c| c.kind == kind)
}

/// Card subtitle as in the design (`0 → 10 ms · step 1 µs`), `off` when disabled/missing.
pub fn summary(card: Option<&CardLine>) -> String {
    let Some(c) = card else {
        return "off".into();
    };
    if !c.enabled {
        return "off".into();
    }
    let v = |s: &str| bb_core::si::parse(s).ok();
    let a = &c.args;
    match c.kind {
        AnalysisKind::Tran => {
            let stop = a.get(1).and_then(|s| v(s)).unwrap_or(0.0);
            let start = a.get(2).and_then(|s| v(s)).unwrap_or(0.0);
            let step = a
                .get(3)
                .and_then(|s| v(s))
                .or_else(|| a.first().and_then(|s| v(s)))
                .unwrap_or(0.0);
            format!("{} → {} · step {}", fmt_si(start, "s").trim_end_matches(" s"), fmt_si(stop, "s"), fmt_si(step, "s"))
                .replace("0 s →", "0 →")
        }
        AnalysisKind::Dc => {
            let var = a.first().cloned().unwrap_or_default();
            let lo = a.get(1).and_then(|s| v(s)).unwrap_or(0.0);
            let hi = a.get(2).and_then(|s| v(s)).unwrap_or(0.0);
            match a.get(3).map(|s| s.to_ascii_lowercase()) {
                Some(m) if m == "dec" || m == "oct" => {
                    format!("{var} {} → {} · {}/{m}", fmt_si(lo, ""), fmt_si(hi, ""), a.get(4).cloned().unwrap_or_default())
                }
                _ => format!("{var} {} → {} · step {}", fmt_si(lo, ""), fmt_si(hi, ""), a.get(3).cloned().unwrap_or_default()),
            }
        }
        AnalysisKind::Ac => {
            let n = a.get(1).cloned().unwrap_or_default();
            let lo = a.get(2).and_then(|s| v(s)).unwrap_or(0.0);
            let hi = a.get(3).and_then(|s| v(s)).unwrap_or(0.0);
            let m = a.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
            format!("{} → {} · {n}/{m}", fmt_si(lo, "Hz"), fmt_si(hi, "Hz"))
        }
        AnalysisKind::Mc => {
            let runs = a.first().cloned().unwrap_or_default();
            let seed = a
                .iter()
                .find_map(|x| x.to_ascii_lowercase().strip_prefix("seed=").map(str::to_string));
            match seed {
                Some(s) => format!("{runs} runs · seed {s}"),
                None => format!("{runs} runs"),
            }
        }
        AnalysisKind::Temp => {
            let vals: Vec<f64> = a.iter().filter_map(|s| v(s)).collect();
            match (vals.first(), vals.last()) {
                (Some(lo), Some(hi)) => format!("{lo} … {hi} °C · {} points", vals.len()),
                _ => "off".into(),
            }
        }
        AnalysisKind::Noise => {
            let out = a.first().cloned().unwrap_or_default();
            let src = a.get(1).cloned().unwrap_or_default();
            let lo = a.get(4).and_then(|s| v(s)).unwrap_or(0.0);
            let hi = a.get(5).and_then(|s| v(s)).unwrap_or(0.0);
            format!("{out} ← {src} · {} → {}", fmt_si(lo, "Hz"), fmt_si(hi, "Hz"))
        }
        AnalysisKind::Op => "bias point".into(),
    }
}

/// Enables / disables a card (comment toggling), inserting a default card if missing.
pub fn set_enabled(text: &str, kind: AnalysisKind, on: bool) -> String {
    match find(text, kind) {
        Some(c) => {
            let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
            let l = &mut lines[c.line];
            let t = l.trim_start().to_string();
            *l = if on {
                t.strip_prefix(OFF).unwrap_or(&t).to_string()
            } else if t.starts_with(OFF) {
                t
            } else {
                format!("{OFF}{t}")
            };
            join(lines, text)
        }
        None if on => insert_card(text, &format!("{} {}", kind.keyword(), kind.default_args())),
        None => text.to_string(),
    }
}

/// Replaces (or inserts) the card for `kind` with `args`, enabled.
pub fn set_card(text: &str, kind: AnalysisKind, args: &str) -> String {
    let card = format!("{} {}", kind.keyword(), args).trim_end().to_string();
    match find(text, kind) {
        Some(c) => {
            let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
            lines[c.line] = card;
            join(lines, text)
        }
        None => insert_card(text, &card),
    }
}

fn insert_card(text: &str, card: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let at = lines
        .iter()
        .rposition(|l| l.trim().eq_ignore_ascii_case(".end"))
        .unwrap_or(lines.len());
    lines.insert(at, card.to_string());
    join(lines, text)
}

fn join(lines: Vec<String>, original: &str) -> String {
    let mut s = lines.join("\n");
    if original.ends_with('\n') {
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const NL: &str = "* t\nV1 a 0 5\n.tran 0 10m 0 1u\n.dc   lux 1 10k dec 40\n.step param R1v list 4.7k 10k 22k\n.mc   100 tol=5%  seed=7\n.op\n.end\n";

    #[test]
    fn summaries_match_the_design() {
        let s = |k| summary(find(NL, k).as_ref());
        assert_eq!(s(AnalysisKind::Tran), "0 → 10 ms · step 1 µs");
        assert_eq!(s(AnalysisKind::Dc), "lux 1 → 10k · 40/dec");
        assert_eq!(s(AnalysisKind::Ac), "off");
        assert_eq!(s(AnalysisKind::Mc), "100 runs · seed 7");
        assert_eq!(s(AnalysisKind::Temp), "off");
        assert_eq!(s(AnalysisKind::Op), "bias point");
    }

    #[test]
    fn toggling_comments_out_and_back() {
        let off = set_enabled(NL, AnalysisKind::Mc, false);
        assert!(off.contains("*off .mc   100 tol=5%  seed=7"));
        assert_eq!(summary(find(&off, AnalysisKind::Mc).as_ref()), "off");
        let on = set_enabled(&off, AnalysisKind::Mc, true);
        assert_eq!(on, NL);
        // enabling a missing analysis inserts its default card before .end
        let ac = set_enabled(NL, AnalysisKind::Ac, true);
        assert!(ac.contains(".ac dec 20 1 1meg\n.end"));
        assert_eq!(summary(find(&ac, AnalysisKind::Ac).as_ref()), "1 Hz → 1 MHz · 20/dec");
    }

    #[test]
    fn popover_fields_round_trip() {
        let c = find(NL, AnalysisKind::Tran).unwrap();
        let f = AnalysisKind::Tran.to_fields(&c.args);
        assert_eq!(f, vec!["10m", "1u", "0"]);
        let mut f2 = f.clone();
        f2[0] = "20m".into();
        let t = set_card(NL, AnalysisKind::Tran, &AnalysisKind::Tran.from_fields(&f2));
        assert!(t.contains(".tran 0 20m 0 1u\n"));
        let dc = AnalysisKind::Dc.to_fields(&find(NL, AnalysisKind::Dc).unwrap().args);
        assert_eq!(dc, vec!["lux", "1", "10k", "dec", "40"]);
        assert_eq!(AnalysisKind::Dc.from_fields(&dc), "lux 1 10k dec 40");
        let mc = AnalysisKind::Mc.to_fields(&find(NL, AnalysisKind::Mc).unwrap().args);
        assert_eq!(mc, vec!["100", "5%", "7"]);
        // the modified netlist still parses
        assert!(bb_spice::Netlist::parse(&t).is_ok());
    }

    #[test]
    fn temperature_sweep_is_a_multi_value_temp() {
        let t = set_enabled(NL, AnalysisKind::Temp, true);
        assert!(t.contains(".temp -20 0 25 50 85"));
        assert_eq!(summary(find(&t, AnalysisKind::Temp).as_ref()), "-20 … 85 °C · 5 points");
        assert!(find("x\n.temp 27\n", AnalysisKind::Temp).is_none());
    }
}
