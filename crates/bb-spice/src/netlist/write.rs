//! [`Netlist`] → ngspice-compatible text.
//!
//! * `ext("name")` references become `.param ext_name=<value>` (ngspice has no external
//!   variables); values come from the map passed to [`netlist_to_text_with`].
//! * Breadbox ports become an equivalent `V` + series `R` (Thevenin/pull), an `I` source, or a
//!   high-value leakage `R` (Hi-Z), each preceded by a `* breadbox port` comment.
//! * Breadbox-only cards — `.step`, `.mc`, several `.temp` values, `.dc` over a `.param` or with
//!   dec/oct/list spacing — are written as `*!bb <card>`: a comment for ngspice (so the file
//!   still runs there), read back as the card by bb-spice.

use super::*;
use crate::expr::{Expr, fmt_g};
use crate::meas::{Edge, MeasKind, StatFunc, Trigger};
use std::collections::BTreeMap;
use std::fmt::Write;

pub(crate) fn netlist_to_text(nl: &Netlist) -> String {
    netlist_to_text_with(nl, &HashMap::new())
}

/// Sanitised parameter name for an external variable.
pub fn ext_param_name(name: &str) -> String {
    let mut s = String::from("ext_");
    for c in name.chars() {
        s.push(if c.is_ascii_alphanumeric() {
            c.to_ascii_lowercase()
        } else {
            '_'
        });
    }
    s
}

/// Renders an expression for netlist output: numbers plain, everything else in braces, with
/// `ext("x")` replaced by its parameter.
pub(crate) fn expr_text(e: &Expr) -> String {
    match e {
        Expr::Num(v) => fmt_g(*v),
        _ => format!("{{{}}}", expr_inner(e)),
    }
}

fn expr_inner(e: &Expr) -> String {
    let r = e
        .rewrite(&mut |n| {
            Ok(if let Expr::Ext(name) = n {
                Some(Expr::Ident(ext_param_name(name)))
            } else {
                None
            })
        })
        .unwrap_or_else(|_| e.clone());
    let s = r.to_string();
    // strip one outer pair of parentheses for readability
    if s.starts_with('(') && s.ends_with(')') && balanced(&s[1..s.len() - 1]) {
        s[1..s.len() - 1].to_string()
    } else {
        s
    }
}

fn balanced(s: &str) -> bool {
    let mut d = 0i32;
    for c in s.chars() {
        if c == '(' {
            d += 1;
        } else if c == ')' {
            d -= 1;
            if d < 0 {
                return false;
            }
        }
    }
    d == 0
}

fn collect_ext(nl: &Netlist) -> Vec<String> {
    let mut names = Vec::new();
    let mut add = |e: &Expr| {
        e.visit(&mut |n| {
            if let Expr::Ext(x) = n
                && !names.contains(x)
            {
                names.push(x.clone());
            }
        })
    };
    fn walk(s: &Scope, add: &mut dyn FnMut(&Expr)) {
        for (_, e, _) in &s.params {
            add(e);
        }
        for f in &s.funcs {
            add(&f.body);
        }
        for m in &s.models {
            for (_, e) in &m.params {
                add(e);
            }
        }
        for el in &s.elements {
            for (_, e) in &el.params {
                add(e);
            }
            match &el.kind {
                ElemKind::Resistor { value: Some(v), .. }
                | ElemKind::Capacitor { value: Some(v), .. }
                | ElemKind::Inductor { value: Some(v), .. } => add(v),
                ElemKind::Behavioral { expr, .. } => add(expr),
                ElemKind::Vsource(s) | ElemKind::Isource(s) => {
                    if let Some(d) = &s.dc {
                        add(d);
                    }
                }
                ElemKind::Vcvs(c) | ElemKind::Vccs(c) | ElemKind::Cccs(c) | ElemKind::Ccvs(c) => {
                    match c {
                        Control::Linear { gain, .. } => add(gain),
                        Control::Value(e) => add(e),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        for sub in &s.subckts {
            walk(sub, add);
        }
    }
    walk(&nl.top, &mut add);
    names
}

/// Writes the netlist, giving `ext()` parameters the supplied values (0 when missing).
pub fn netlist_to_text_with(nl: &Netlist, ext: &HashMap<String, f64>) -> String {
    netlist_to_text_opts(nl, ext, false)
}

/// `strict`: the file is meant for ngspice itself (Share → `.cir`), so Breadbox / LTspice
/// conveniences ngspice rejects are rewritten (`.tran 0 10m` → a positive TSTEP).
pub(crate) fn netlist_to_text_opts(
    nl: &Netlist,
    ext: &HashMap<String, f64>,
    strict: bool,
) -> String {
    let mut o = String::new();
    let title = if nl.title.is_empty() {
        "* breadbox netlist".to_string()
    } else {
        nl.title.clone()
    };
    let _ = writeln!(o, "{title}");
    let exts = collect_ext(nl);
    if !exts.is_empty() {
        let _ = writeln!(
            o,
            "* external variables (Breadbox ext()) frozen at their current values"
        );
        for n in &exts {
            let _ = writeln!(
                o,
                ".param {}={}",
                ext_param_name(n),
                fmt_g(ext.get(n).copied().unwrap_or(0.0))
            );
        }
    }
    write_scope(&mut o, &nl.top, "");
    if !nl.globals.is_empty() {
        let _ = writeln!(o, ".global {}", nl.globals.join(" "));
    }
    let mut opts: BTreeMap<String, String> = BTreeMap::new();
    let mut flags = Vec::new();
    for (k, v, _) in &nl.options {
        match v {
            Some(v) => {
                opts.insert(k.clone(), fmt_g(*v));
            }
            None => {
                if let Some((a, b)) = k.split_once('=') {
                    opts.insert(a.to_string(), b.to_string());
                } else {
                    flags.push(k.clone());
                }
            }
        }
    }
    if !opts.is_empty() || !flags.is_empty() {
        let mut line = String::from(".options");
        for (k, v) in &opts {
            let _ = write!(line, " {k}={v}");
        }
        for f in &flags {
            let _ = write!(line, " {f}");
        }
        let _ = writeln!(o, "{line}");
    }
    if !nl.temps.is_empty() {
        // ngspice accepts a single .temp; several values are a Breadbox temperature loop
        let prefix = if nl.temps.len() > 1 { BB_DIRECTIVE } else { "" };
        let _ = writeln!(
            o,
            "{prefix}.temp {}",
            nl.temps
                .iter()
                .map(|t| fmt_g(*t))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    for (kw, list) in [(".ic", &nl.ics), (".nodeset", &nl.nodesets)] {
        if !list.is_empty() {
            let items: Vec<String> = list
                .iter()
                .map(|(n, e, _)| format!("v({n})={}", expr_text(e)))
                .collect();
            let _ = writeln!(o, "{kw} {}", items.join(" "));
        }
    }
    for (a, _) in &nl.analyses {
        let prefix = if ngspice_can_run(nl, a) {
            ""
        } else {
            BB_DIRECTIVE
        };
        let card = match a {
            Analysis::Tran(t) if strict && t.tstep <= 0.0 => {
                // ngspice: "TSTEP is invalid, must be greater than zero"
                let span = (t.tstop - t.tstart).max(t.tstop);
                let mut t = t.clone();
                t.tstep = t.tmax.filter(|m| *m > 0.0).unwrap_or(span / 1000.0);
                analysis_text(&Analysis::Tran(t))
            }
            _ => analysis_text(a),
        };
        let _ = writeln!(o, "{prefix}{card}");
    }
    for s in &nl.steps {
        let var = match &s.var {
            SweepVar::Param(p) => format!("param {p}"),
            SweepVar::Temp => "temp".into(),
            SweepVar::Name(n) => n.clone(),
        };
        let _ = writeln!(
            o,
            "{BB_DIRECTIVE}.step {var} list {}",
            s.values
                .iter()
                .map(|v| fmt_g(*v))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    if let Some(mc) = &nl.mc {
        let mut line = format!("{BB_DIRECTIVE}.mc {}", mc.runs);
        if let Some(t) = mc.tol {
            let _ = write!(line, " tol={}%", fmt_g(t * 100.0));
        }
        let _ = write!(line, " seed={}", mc.seed);
        if mc.dist == Dist::Uniform {
            line.push_str(" dist=uniform");
        }
        for a in &mc.analyses {
            let _ = write!(line, " analysis={a}");
        }
        let _ = writeln!(o, "{line}");
    }
    for m in &nl.measures {
        let _ = writeln!(o, "{}", meas_text(m));
    }
    if !nl.saves.is_empty() {
        let _ = writeln!(o, ".save {}", nl.saves.join(" "));
    }
    let _ = writeln!(o, ".end");
    o
}

fn write_scope(o: &mut String, s: &Scope, indent: &str) {
    for (n, e, _) in &s.params {
        let _ = writeln!(o, "{indent}.param {n}={}", expr_text(e));
    }
    for f in &s.funcs {
        let _ = writeln!(
            o,
            "{indent}.func {}({}) {{{}}}",
            f.name,
            f.args.join(","),
            expr_inner(&f.body)
        );
    }
    for m in &s.models {
        let ps: Vec<String> = m
            .params
            .iter()
            .map(|(k, e)| format!("{k}={}", expr_text(e)))
            .collect();
        let _ = writeln!(
            o,
            "{indent}.model {} {}({})",
            m.name,
            m.kind.keyword(),
            ps.join(" ")
        );
    }
    for sub in &s.subckts {
        let mut line = format!("{indent}.subckt {} {}", sub.name, sub.ports.join(" "));
        if !sub.defaults.is_empty() {
            line.push_str(" params:");
            for (k, e) in &sub.defaults {
                let _ = write!(line, " {k}={}", expr_text(e));
            }
        }
        let _ = writeln!(o, "{line}");
        write_scope(o, sub, &format!("{indent}  "));
        let _ = writeln!(o, "{indent}.ends {}", sub.name);
    }
    for el in &s.elements {
        let _ = writeln!(o, "{indent}{}", element_text(el));
    }
}

fn params_text(p: &ParamList) -> String {
    let mut s = String::new();
    let mut i = 0;
    while i < p.len() {
        let (k, e) = &p[i];
        // re-join list params (ic, ic.1, …)
        let mut vals = vec![expr_text(e)];
        while i + 1 < p.len() && p[i + 1].0.starts_with(&format!("{k}.")) {
            i += 1;
            vals.push(expr_text(&p[i].1));
        }
        let _ = write!(s, " {k}={}", vals.join(","));
        i += 1;
    }
    s
}

fn wave_text(w: &Waveform) -> String {
    let join = |a: &[Expr]| a.iter().map(expr_text).collect::<Vec<_>>().join(" ");
    match w {
        Waveform::Pwl {
            points,
            repeat,
            delay,
        } => {
            let mut s = format!("PWL({})", join(points));
            if let Some(r) = repeat {
                let _ = write!(s, " r={}", expr_text(r));
            }
            if let Some(d) = delay {
                let _ = write!(s, " td={}", expr_text(d));
            }
            s
        }
        Waveform::Pulse(a)
        | Waveform::Sin(a)
        | Waveform::Exp(a)
        | Waveform::Sffm(a)
        | Waveform::Am(a) => {
            format!("{}({})", w.keyword(), join(a))
        }
    }
}

fn source_text(s: &SourceSpec) -> String {
    let mut t = String::new();
    if let Some(d) = &s.dc {
        let _ = write!(t, " DC {}", expr_text(d));
    }
    if let Some(m) = &s.ac_mag {
        let _ = write!(t, " AC {}", expr_text(m));
        if let Some(p) = &s.ac_phase {
            let _ = write!(t, " {}", expr_text(p));
        }
    }
    if let Some(w) = &s.wave {
        let _ = write!(t, " {}", wave_text(w));
    }
    if t.is_empty() {
        t.push_str(" DC 0");
    }
    t
}

fn control_text(c: &Control, current_ctrl: bool) -> String {
    match c {
        Control::Linear {
            ctrl_nodes,
            ctrl_src,
            gain,
        } => {
            if current_ctrl {
                format!(
                    "{} {}",
                    ctrl_src.clone().unwrap_or_default(),
                    expr_text(gain)
                )
            } else {
                format!("{} {}", ctrl_nodes.join(" "), expr_text(gain))
            }
        }
        Control::Poly {
            dims,
            ctrl_nodes,
            ctrl_srcs,
            coeffs,
        } => {
            let ctrl = if current_ctrl {
                ctrl_srcs.join(" ")
            } else {
                ctrl_nodes.join(" ")
            };
            format!(
                "POLY({dims}) {ctrl} {}",
                coeffs.iter().map(expr_text).collect::<Vec<_>>().join(" ")
            )
        }
        Control::Value(e) => format!("VALUE={{{}}}", expr_inner(e)),
        Control::Table { input, points } => {
            let pts: Vec<String> = points
                .iter()
                .map(|(x, y)| format!("({},{})", expr_text(x), expr_text(y)))
                .collect();
            format!("TABLE {{{}}} = {}", expr_inner(input), pts.join(" "))
        }
    }
}

/// Instance parameters ngspice does not know (Monte Carlo `tol=`): written after
/// [`super::parse::BB_INLINE`] so ngspice reads a comment and bb-spice the parameter.
const BB_ONLY_PARAMS: &[&str] = &["tol"];

/// One element card as text.
pub(crate) fn element_text(el: &ElementCard) -> String {
    let (bb_only, std): (ParamList, ParamList) = el
        .params
        .iter()
        .cloned()
        .partition(|(k, _)| BB_ONLY_PARAMS.contains(&k.to_ascii_lowercase().as_str()));
    let mut s = element_text_with(el, &std);
    if !bb_only.is_empty() {
        s.push_str(" ;!bb");
        s.push_str(&params_text(&bb_only));
    }
    s
}

fn element_text_with(el: &ElementCard, params: &ParamList) -> String {
    let n = el.nodes.join(" ");
    let p = params_text(params);
    let name = &el.name;
    match &el.kind {
        ElemKind::Resistor { value, model }
        | ElemKind::Capacitor { value, model }
        | ElemKind::Inductor { value, model } => {
            let mut s = format!("{name} {n}");
            if let Some(v) = value {
                let _ = write!(s, " {}", expr_text(v));
            }
            if let Some(m) = model {
                let _ = write!(s, " {m}");
            }
            s + &p
        }
        ElemKind::Coupling { inductors, k } => {
            format!("{name} {} {}", inductors.join(" "), expr_text(k))
        }
        ElemKind::Vsource(s) | ElemKind::Isource(s) => format!("{name} {n}{}{p}", source_text(s)),
        ElemKind::Vcvs(c) | ElemKind::Vccs(c) => {
            format!("{name} {n} {}{p}", control_text(c, false))
        }
        ElemKind::Cccs(c) | ElemKind::Ccvs(c) => format!("{name} {n} {}{p}", control_text(c, true)),
        ElemKind::Behavioral { is_current, expr } => {
            format!(
                "{name} {n} {}={{{}}}",
                if *is_current { "I" } else { "V" },
                expr_inner(expr)
            )
        }
        ElemKind::Switch { model, init_on } | ElemKind::CSwitch { model, init_on, .. } => {
            let ctrl = match &el.kind {
                ElemKind::CSwitch { ctrl, .. } => format!(" {ctrl}"),
                _ => String::new(),
            };
            let st = match init_on {
                Some(true) => " ON",
                Some(false) => " OFF",
                None => "",
            };
            format!("{name} {n}{ctrl} {model}{st}{p}")
        }
        ElemKind::Diode { model, area, off }
        | ElemKind::Jfet { model, area, off }
        | ElemKind::Bjt { model, area, off } => {
            let mut s = format!("{name} {n} {model}");
            if let Some(a) = area {
                let _ = write!(s, " {}", expr_text(a));
            }
            if *off {
                s.push_str(" off");
            }
            s + &p
        }
        ElemKind::Mosfet { model, off } => {
            format!("{name} {n} {model}{}{p}", if *off { " off" } else { "" })
        }
        ElemKind::Tline => format!("{name} {n}{p}"),
        ElemKind::Subckt {
            name: sub,
            text_params,
        } => {
            let mut s = if params.is_empty() && text_params.is_empty() {
                format!("{name} {n} {sub}")
            } else {
                format!("{name} {n} {sub} params:{p}")
            };
            for (k, v) in text_params {
                let _ = write!(s, " {k}={v}");
            }
            s
        }
        ElemKind::Port { drive } => {
            let (a, b) = (
                &el.nodes[0],
                el.nodes.get(1).map(|s| s.as_str()).unwrap_or("0"),
            );
            let base = name.trim_start_matches(['p', 'P']);
            match drive {
                bb_core::PinDrive::HighZ => {
                    format!("* breadbox port {name}: high-Z\nR{base}_port {a} {b} 1e12")
                }
                bb_core::PinDrive::Thevenin { v, r } | bb_core::PinDrive::Pull { v, r } => {
                    let kind = if matches!(drive, bb_core::PinDrive::Pull { .. }) {
                        "pull"
                    } else {
                        "drive"
                    };
                    format!(
                        "* breadbox port {name}: {kind} {} V behind {} ohm\nV{base}_port {name}_int {b} DC {}\nR{base}_port {name}_int {a} {}",
                        fmt_g(*v),
                        fmt_g(*r),
                        fmt_g(*v),
                        fmt_g(r.max(1e-6))
                    )
                }
                bb_core::PinDrive::Current { i } => {
                    format!(
                        "* breadbox port {name}: current {} A\nI{base}_port {b} {a} DC {}",
                        fmt_g(*i),
                        fmt_g(*i)
                    )
                }
            }
        }
    }
}

/// Whether ngspice can run an analysis card as written (its `.dc` only sweeps sources, `temp` and
/// resistors linearly).
fn ngspice_can_run(nl: &Netlist, a: &Analysis) -> bool {
    match a {
        Analysis::Dc(sweeps) => sweeps.iter().all(|s| {
            let linear = matches!(s.kind, SweepKind::Lin { .. } | SweepKind::LinPoints(_));
            let var_ok = match &s.var {
                SweepVar::Temp => true,
                SweepVar::Param(_) => false,
                SweepVar::Name(n) => nl.top.elements.iter().any(|e| {
                    &e.name == n
                        && matches!(
                            e.kind,
                            ElemKind::Vsource(_) | ElemKind::Isource(_) | ElemKind::Resistor { .. }
                        )
                }),
            };
            linear && var_ok
        }),
        _ => true,
    }
}

fn sweep_text(s: &Sweep) -> String {
    match &s.kind {
        SweepKind::Lin { step } => format!("{} {} {}", fmt_g(s.start), fmt_g(s.stop), fmt_g(*step)),
        SweepKind::LinPoints(n) => format!("lin {n} {} {}", fmt_g(s.start), fmt_g(s.stop)),
        SweepKind::Dec(n) => format!("dec {n} {} {}", fmt_g(s.start), fmt_g(s.stop)),
        SweepKind::Oct(n) => format!("oct {n} {} {}", fmt_g(s.start), fmt_g(s.stop)),
        SweepKind::List(v) => format!(
            "list {}",
            v.iter().map(|x| fmt_g(*x)).collect::<Vec<_>>().join(" ")
        ),
    }
}

pub(crate) fn analysis_text(a: &Analysis) -> String {
    match a {
        Analysis::Op => ".op".into(),
        Analysis::Tran(t) => {
            let mut s = format!(".tran {} {}", fmt_g(t.tstep), fmt_g(t.tstop));
            if t.tstart != 0.0 || t.tmax.is_some() {
                let _ = write!(s, " {}", fmt_g(t.tstart));
            }
            if let Some(m) = t.tmax {
                let _ = write!(s, " {}", fmt_g(m));
            }
            if t.uic {
                s.push_str(" uic");
            }
            s
        }
        Analysis::Ac(sw) => format!(".ac {}", sweep_text(sw)),
        Analysis::Dc(sweeps) => {
            let mut s = String::from(".dc");
            for sw in sweeps {
                let var = match &sw.var {
                    SweepVar::Param(p) => format!("param {p}"),
                    SweepVar::Temp => "temp".into(),
                    SweepVar::Name(n) => n.clone(),
                };
                match &sw.kind {
                    SweepKind::Lin { step } => {
                        let _ = write!(
                            s,
                            " {var} {} {} {}",
                            fmt_g(sw.start),
                            fmt_g(sw.stop),
                            fmt_g(*step)
                        );
                    }
                    SweepKind::List(v) => {
                        let _ = write!(
                            s,
                            " {var} list {}",
                            v.iter().map(|x| fmt_g(*x)).collect::<Vec<_>>().join(" ")
                        );
                    }
                    SweepKind::Dec(n) => {
                        let _ = write!(s, " {var} {} {} dec {n}", fmt_g(sw.start), fmt_g(sw.stop));
                    }
                    SweepKind::Oct(n) => {
                        let _ = write!(s, " {var} {} {} oct {n}", fmt_g(sw.start), fmt_g(sw.stop));
                    }
                    SweepKind::LinPoints(n) => {
                        let step = if *n > 1 {
                            (sw.stop - sw.start) / (*n - 1) as f64
                        } else {
                            0.0
                        };
                        let _ = write!(
                            s,
                            " {var} {} {} {}",
                            fmt_g(sw.start),
                            fmt_g(sw.stop),
                            fmt_g(step)
                        );
                    }
                }
            }
            s
        }
        Analysis::Noise {
            output,
            input,
            sweep,
            pts_per_summary,
        } => {
            let mut s = format!(".noise {} {input} {}", output.label(), sweep_text(sweep));
            if *pts_per_summary > 0 {
                let _ = write!(s, " {pts_per_summary}");
            }
            s
        }
        Analysis::Tf { output, input } => format!(".tf {} {input}", output.label()),
    }
}

fn trig_text(t: &Trigger, when: bool) -> String {
    if let Some(at) = t.at {
        return format!("at={}", fmt_g(at));
    }
    let mut s = String::new();
    let e = t.expr.as_ref().map(expr_inner).unwrap_or_default();
    match (&t.val, when) {
        (Some(v), true) => {
            let _ = write!(s, "{e}={}", expr_inner(v));
        }
        (Some(v), false) => {
            let _ = write!(s, "{e} val={}", expr_inner(v));
        }
        (None, _) => s.push_str(&e),
    }
    if t.td != 0.0 {
        let _ = write!(s, " td={}", fmt_g(t.td));
    }
    let edge = match t.edge {
        Edge::Rise(n) => format!("rise={n}"),
        Edge::Fall(n) => format!("fall={n}"),
        Edge::Cross(n) => format!("cross={n}"),
        Edge::RiseLast => "rise=last".into(),
        Edge::FallLast => "fall=last".into(),
        Edge::CrossLast => "cross=last".into(),
    };
    let _ = write!(s, " {edge}");
    s
}

fn meas_text(m: &MeasSpec) -> String {
    let body = match &m.kind {
        MeasKind::Stat {
            func,
            expr,
            from,
            to,
        } => {
            let f = match func {
                StatFunc::Avg => "avg",
                StatFunc::Rms => "rms",
                StatFunc::Pp => "pp",
                StatFunc::Min => "min",
                StatFunc::Max => "max",
                StatFunc::MinAt => "min_at",
                StatFunc::MaxAt => "max_at",
                StatFunc::Integ => "integ",
            };
            let mut s = format!("{f} {}", expr_inner(expr));
            if let Some(a) = from {
                let _ = write!(s, " from={}", fmt_g(*a));
            }
            if let Some(b) = to {
                let _ = write!(s, " to={}", fmt_g(*b));
            }
            s
        }
        MeasKind::FindAt { expr, at } => format!("find {} at={}", expr_inner(expr), fmt_g(*at)),
        MeasKind::DerivAt { expr, at } => format!("deriv {} at={}", expr_inner(expr), fmt_g(*at)),
        MeasKind::FindWhen { expr, when } => {
            format!("find {} when {}", expr_inner(expr), trig_text(when, true))
        }
        MeasKind::DerivWhen { expr, when } => {
            format!("deriv {} when {}", expr_inner(expr), trig_text(when, true))
        }
        MeasKind::When(t) => format!("when {}", trig_text(t, true)),
        MeasKind::TrigTarg { trig, targ } => format!(
            "trig {} targ {}",
            trig_text(trig, false),
            trig_text(targ, false)
        ),
        MeasKind::Param(e) => format!("param='{}'", expr_inner(e)),
    };
    format!(".meas {} {} {body}", m.analysis, m.name)
}
