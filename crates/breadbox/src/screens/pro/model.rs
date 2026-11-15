//! Running a netlist and turning `bb_spice::Results` into what the Analysis tab draws
//! (traces, `.step` families, Monte Carlo histogram, Bode, operating-point rows). Pure data so
//! it is unit-tested without a GUI.

use super::demo;
use crate::theme::{DARK, Palette};
use crate::widgets::plot::{Histogram, Trace, fmt_readout, fmt_si};
use bb_spice::{CancelToken, Netlist, ParseOptions, Plot, PlotKind, Results, RunOptions};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Per-netlist presentation hints (the built-in demo uses the design's titles).
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub tran_title: Option<String>,
    pub tran_traces: Vec<String>,
    pub dc_vector: Option<String>,
    pub dc_title: Option<String>,
    pub step_label: Option<(String, String)>,
    pub mc_vector: Option<String>,
    pub mc_threshold: Option<(f64, String)>,
    pub mc_note: Option<String>,
    /// Order of the `.op` rows (quantities), others follow.
    pub op_rows: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TranView {
    pub title: String,
    pub traces: Vec<Trace>,
    pub y_unit: String,
    pub y2_unit: Option<String>,
    /// `(t1, t2, duty)` from the primary trace.
    pub cursors: Option<(f64, f64, f64)>,
    pub plot: Plot,
}

#[derive(Debug, Clone)]
pub struct DcView {
    pub title: String,
    pub traces: Vec<Trace>,
    pub x_unit: String,
    pub x_log: bool,
    pub marker: Option<(f64, f64, String)>,
    pub x_name: String,
}

#[derive(Debug, Clone)]
pub struct McView {
    pub title: String,
    pub stats: String,
    pub hist: Histogram,
    pub vector: String,
}

#[derive(Debug, Clone)]
pub struct AcView {
    pub title: String,
    pub mag: Vec<Trace>,
    pub phase: Vec<Trace>,
    pub plot: Plot,
    /// First −3 dB crossing relative to the peak gain (a filter's corner), Hz.
    pub corner_hz: Option<f64>,
}

/// The first frequency where `|H|` falls 3.01 dB below its peak (log-interpolated), after
/// the peak for a low-pass / band-pass, before it for a high-pass.
pub fn corner_frequency(mag_db: &[[f64; 2]]) -> Option<f64> {
    if mag_db.len() < 2 {
        return None;
    }
    let (ipk, pk) = mag_db
        .iter()
        .enumerate()
        .max_by(|a, b| a.1[1].total_cmp(&b.1[1]))
        .map(|(i, p)| (i, p[1]))?;
    let level = pk - 10.0 * 2f64.log10();
    let cross = |a: &[f64; 2], b: &[f64; 2]| -> f64 {
        let t = (level - a[1]) / (b[1] - a[1]);
        let (la, lb) = (a[0].max(1e-30).ln(), b[0].max(1e-30).ln());
        (la + t * (lb - la)).exp()
    };
    // falling edge after the peak
    for w in mag_db[ipk..].windows(2) {
        if w[0][1] >= level && w[1][1] < level {
            return Some(cross(&w[0], &w[1]));
        }
    }
    // rising edge before it (high-pass)
    for w in mag_db[..=ipk].windows(2).rev() {
        if w[0][1] < level && w[1][1] >= level {
            return Some(cross(&w[0], &w[1]));
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct NoiseView {
    pub title: String,
    pub traces: Vec<Trace>,
    pub total: Option<String>,
}

/// One `.meas` / `.tf` result row.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasRow {
    /// `.meas` name or `.tf` quantity (`transfer_function`, `output_impedance_at_v(out)`).
    pub name: String,
    /// `.step` run label (empty without `.step`).
    pub run: String,
    /// Formatted value, or why the measurement failed.
    pub value: String,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpRow {
    /// `V(VCC)`, `I(D1) avg`, `P(R2)`.
    pub quantity: String,
    pub value: String,
    pub raw: f64,
}

#[derive(Debug, Clone)]
pub struct MixedInfo {
    pub sync: String,
    pub solver: String,
    pub integration: String,
}

/// Everything the tab shows after a run.
#[derive(Debug, Clone)]
pub struct ViewData {
    pub summary: String,
    pub tran: Option<TranView>,
    pub dc: Option<DcView>,
    pub mc: Option<McView>,
    pub ac: Option<AcView>,
    pub noise: Option<NoiseView>,
    pub op: Vec<OpRow>,
    /// `.meas` results (per `.step` run when stepped) and `.tf` values (`SPICE-3`).
    pub measures: Vec<MeasRow>,
    pub warnings: Vec<String>,
    pub dc_family: Vec<(String, Plot)>,
}

/// Outcome of a background run.
pub type RunOutcome = Result<Arc<ViewData>, RunError>;

#[derive(Debug, Clone, PartialEq)]
pub struct RunError {
    pub line: Option<u32>,
    pub message: String,
}

/// Prepares the netlist text for the solver: Breadbox parts that only the co-simulation
/// knows (`CAPSOIL`, `ATMEGA328P`) get stand-in subcircuits, and their non-numeric
/// parameters (`fw=garden-node.hex`) are dropped for the analog-only run.
pub fn prepare(text: &str) -> (String, Option<Arc<bb_spice::Library>>) {
    let lower = text.to_ascii_lowercase();
    let needs: Vec<&str> = demo::STUB_PARTS
        .iter()
        .filter(|(name, _)| {
            lower.contains(&name.to_ascii_lowercase())
                && !lower.contains(&format!(".subckt {}", name.to_ascii_lowercase()))
        })
        .map(|(name, _)| *name)
        .collect();
    if needs.is_empty() {
        // file-name instance parameters (`fw=garden-node.hex`) are for the co-simulation only
        return (strip_text_params(text), None);
    }
    let lib_text: String = demo::STUB_PARTS
        .iter()
        .filter(|(n, _)| needs.contains(n))
        .map(|(_, body)| *body)
        .collect();
    let lib = bb_spice::Library::parse(&lib_text).ok().map(Arc::new);
    (strip_text_params(text), lib)
}

/// Removes `key=value` tokens whose value is a file name (e.g. `fw=garden-node.hex`) from
/// subcircuit instance lines and their continuations.
pub fn strip_text_params(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_x = false;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        let first = t.chars().next().map(|c| c.to_ascii_uppercase());
        if first == Some('X') {
            in_x = true;
        } else if first != Some('+') {
            in_x = false;
        }
        if in_x {
            let nl = line.ends_with('\n');
            let kept: Vec<&str> = line
                .split_whitespace()
                .filter(|tok| match tok.split_once('=') {
                    Some((_, v)) => {
                        let fileish = v.contains('.')
                            && v.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
                        !fileish
                    }
                    None => true,
                })
                .collect();
            out.push_str(&kept.join("  "));
            if nl {
                out.push('\n');
            }
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Parses and runs (blocking; call from a job thread).
pub fn run(
    text: &str,
    file_name: &str,
    base_dir: Option<PathBuf>,
    profile: &Profile,
    cancel: CancelToken,
    progress: Arc<dyn Fn(f32) + Send + Sync>,
) -> RunOutcome {
    let (prepared, lib) = prepare(text);
    let opts = ParseOptions {
        file_name: Some(file_name.to_string()),
        base_dir,
        libraries: lib.into_iter().collect(),
        ..Default::default()
    };
    let nl = Netlist::parse_with(&prepared, &opts).map_err(|e| RunError {
        line: Some(e.span.line),
        message: e.to_string(),
    })?;
    let results = nl
        .run_with(&RunOptions {
            cancel: Some(cancel),
            progress: Some(progress),
            ..Default::default()
        })
        .map_err(|e| RunError {
            line: None,
            message: e.to_string(),
        })?;
    Ok(Arc::new(build_view(&results, &nl, text, profile, &DARK)))
}

fn is_top_node(name: &str) -> bool {
    name.starts_with("v(") && !name[2..].contains('.') && !name.contains(',')
}

fn variation(v: &[f64]) -> f64 {
    let (lo, hi) = v
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| {
            (a.min(*x), b.max(*x))
        });
    if lo.is_finite() { hi - lo } else { 0.0 }
}

fn upper(name: &str) -> String {
    // v(a1) → V(A1)
    match name.split_once('(') {
        Some((f, rest)) => format!("{}({}", f.to_ascii_uppercase(), rest.to_ascii_uppercase()),
        None => name.to_ascii_uppercase(),
    }
}

/// Index of the run at the netlist's nominal `.param` values (else 0).
fn nominal_run(results: &Results, text: &str) -> usize {
    let params = nominal_params(text);
    results
        .runs
        .iter()
        .position(|r| {
            r.params.iter().all(|(k, v)| {
                params
                    .get(&k.to_ascii_lowercase())
                    .is_some_and(|nv| ((nv - v) / nv.abs().max(1e-30)).abs() < 1e-6)
            })
        })
        .unwrap_or(0)
}

/// `.param name=value` numeric values (lower-case names).
pub fn nominal_params(text: &str) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    for l in text.lines() {
        let t = l.trim();
        if !t.to_ascii_lowercase().starts_with(".param") {
            continue;
        }
        for tok in t.split_whitespace().skip(1) {
            if let Some((k, v)) = tok.split_once('=')
                && let Ok(x) = bb_core::si::parse(v)
            {
                m.insert(k.to_ascii_lowercase(), x);
            }
        }
    }
    m
}

fn series(plot: &Plot, name: &str) -> Option<Vec<[f64; 2]>> {
    let x = plot.scale_values();
    let y = plot.real(name)?;
    Some(x.into_iter().zip(y).map(|(a, b)| [a, b]).collect())
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

/// Time-weighted average over a transient plot.
fn time_avg(t: &[f64], v: &[f64]) -> f64 {
    if t.len() < 2 {
        return mean(v);
    }
    let mut acc = 0.0;
    for i in 1..t.len().min(v.len()) {
        acc += (v[i] + v[i - 1]) * 0.5 * (t[i] - t[i - 1]);
    }
    acc / (t[t.len() - 1] - t[0]).max(1e-30)
}

pub fn build_view(
    results: &Results,
    nl: &Netlist,
    text: &str,
    profile: &Profile,
    p: &Palette,
) -> ViewData {
    let nom = nominal_run(results, text);
    let palette = [p.accent, p.info, p.text, p.trace_blue, p.ok, p.trace_yellow];
    let mut warnings: Vec<String> = results.warnings.iter().map(|w| w.to_string()).collect();
    warnings.truncate(20);

    // ---- transient
    let tran = results
        .runs
        .get(nom)
        .and_then(|r| r.plot(PlotKind::Tran))
        .map(|plot| {
            let names: Vec<String> = plot.names().map(str::to_string).collect();
            let mut chosen: Vec<String> = profile
                .tran_traces
                .iter()
                .filter(|n| names.contains(n))
                .cloned()
                .collect();
            if chosen.is_empty() {
                let best_v = names
                    .iter()
                    .filter(|n| is_top_node(n))
                    .max_by(|a, b| {
                        variation(&plot.real(a).unwrap_or_default())
                            .total_cmp(&variation(&plot.real(b).unwrap_or_default()))
                    })
                    .cloned();
                let best_i = names
                    .iter()
                    .filter(|n| {
                        n.starts_with("i(d") || n.starts_with("i(q") || n.starts_with("i(r")
                    })
                    .filter(|n| !n.contains('.'))
                    .max_by(|a, b| {
                        variation(&plot.real(a).unwrap_or_default())
                            .total_cmp(&variation(&plot.real(b).unwrap_or_default()))
                    })
                    .cloned();
                chosen.extend(best_v);
                chosen
                    .extend(best_i.filter(|n| variation(&plot.real(n).unwrap_or_default()) > 1e-9));
            }
            let mut traces = Vec::new();
            let mut has_current = false;
            for (k, n) in chosen.iter().enumerate() {
                let Some(pts) = series(plot, n) else { continue };
                let current = n.starts_with("i(");
                let mut t = Trace::new(upper(n), palette[k % palette.len()], pts)
                    .width(if current { 1.6 } else { 2.0 });
                if current && k > 0 {
                    t = t.right();
                    has_current = true;
                }
                traces.push(t);
            }
            let cursors = traces
                .first()
                .and_then(|t| crate::widgets::plot::auto_cursors(&t.points));
            let title = match &profile.tran_title {
                Some(t) => format!(".tran — {t}"),
                None => format!(
                    ".tran — {}",
                    traces
                        .iter()
                        .map(|t| t.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            TranView {
                title,
                y_unit: if traces.first().is_some_and(|t| t.name.starts_with('I')) {
                    "A".into()
                } else {
                    "V".into()
                },
                y2_unit: has_current.then(|| "mA".into()),
                traces,
                cursors,
                plot: plot.clone(),
            }
        });

    // ---- DC sweep (+ .step family)
    let family = results.family(PlotKind::Dc);
    let dc = family.first().map(|(_, first)| {
        let names: Vec<String> = first.names().map(str::to_string).collect();
        let vector = profile
            .dc_vector
            .clone()
            .filter(|v| names.contains(v))
            .or_else(|| {
                names
                    .iter()
                    .filter(|n| is_top_node(n))
                    .max_by(|a, b| {
                        variation(&first.real(a).unwrap_or_default())
                            .total_cmp(&variation(&first.real(b).unwrap_or_default()))
                    })
                    .cloned()
            })
            .unwrap_or_default();
        let x_name = first
            .scale()
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "sweep".into());
        let x_log = text.lines().any(|l| {
            let t = l.trim().to_ascii_lowercase();
            t.starts_with(".dc") && (t.contains(" dec ") || t.contains(" oct "))
        });
        let colors = [p.trace_blue, p.accent, p.text, p.ok, p.info, p.trace_yellow];
        let mut traces = Vec::new();
        for (k, (run, plot)) in family.iter().enumerate() {
            let Some(pts) = series(plot, &vector) else {
                continue;
            };
            let label = if run.params.is_empty() {
                upper(&vector)
            } else {
                run.params
                    .iter()
                    .map(|(n, v)| {
                        let name = match &profile.step_label {
                            Some((pn, disp)) if pn.eq_ignore_ascii_case(n) => disp.clone(),
                            _ => n.clone(),
                        };
                        format!("{name} {}", fmt_si(*v, ""))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let w = if k == nom { 2.4 } else { 2.0 };
            traces.push(Trace::new(label, colors[k % colors.len()], pts).width(w));
        }
        // op marker at the nominal sweep value (`op: 180 lx → 2.60 V`)
        let params = nominal_params(text);
        let marker = params.get(&x_name.to_ascii_lowercase()).and_then(|x0| {
            let pts = &traces.get(nom.min(traces.len().saturating_sub(1)))?.points;
            let y = interp(pts, *x0)?;
            let unit = if x_name.eq_ignore_ascii_case("lux") {
                " lx"
            } else {
                ""
            };
            Some((
                *x0,
                y,
                format!("op: {}{unit} → {:.2} V", fmt_si(*x0, ""), y),
            ))
        });
        let stepped = !results
            .runs
            .first()
            .map(|r| r.params.is_empty())
            .unwrap_or(true);
        let title = format!(
            ".dc{} — {}",
            if stepped { " + .step" } else { "" },
            profile
                .dc_title
                .clone()
                .unwrap_or_else(|| format!("{} vs {x_name}", upper(&vector)))
        );
        DcView {
            title,
            traces,
            x_unit: if x_name.eq_ignore_ascii_case("lux") {
                "lx".into()
            } else if x_name.eq_ignore_ascii_case("temp") {
                "°C".into()
            } else {
                String::new()
            },
            x_log,
            marker,
            x_name,
        }
    });

    // ---- Monte Carlo
    let mc = results.mc.as_ref().and_then(|mc| {
        let first = mc.runs.first()?.plots.first()?;
        let names: Vec<String> = first.names().map(str::to_string).collect();
        let vector = profile
            .mc_vector
            .clone()
            .filter(|v| names.contains(v))
            .or_else(|| {
                names
                    .iter()
                    .filter(|n| is_top_node(n))
                    .filter_map(|n| mc.distribution(n).map(|d| (n.clone(), d.sigma)))
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|x| x.0)
            })?;
        let d = mc.distribution(&vector)?;
        let bins = d.histogram(12);
        let tol = nl
            .mc
            .as_ref()
            .and_then(|m| m.tol)
            .map(|t| format!("{}% R tol", (t * 100.0).round()));
        let mut stats = format!("μ {:.2} V · σ {:.2} V", d.mean, d.sigma);
        if let Some(t) = tol {
            stats.push_str(&format!(" · {t}"));
        }
        if let Some(n) = &profile.mc_note {
            stats.push_str(&format!(", {n}"));
        }
        Some(McView {
            title: format!(".mc — {} spread, {} runs", upper(&vector), mc.runs.len()),
            stats,
            hist: Histogram {
                bins,
                unit: "V".into(),
                threshold: profile.mc_threshold.clone(),
            },
            vector,
        })
    });

    // ---- AC / Bode
    let ac = results
        .runs
        .get(nom)
        .and_then(|r| r.plot(PlotKind::Ac))
        .map(|plot| {
            let names: Vec<String> = plot.names().map(str::to_string).collect();
            let out = names
                .iter()
                .filter(|n| is_top_node(n))
                .filter_map(|n| plot.vector(n).map(|v| (n.clone(), v)))
                .max_by(|a, b| {
                    let spread = |v: &bb_spice::Vector| {
                        let m: Vec<f64> = (0..v.len()).map(|i| v.complex_at(i).abs()).collect();
                        variation(&m)
                    };
                    spread(a.1).total_cmp(&spread(b.1))
                })
                .map(|x| x.0)
                .unwrap_or_default();
            let f = plot.scale_values();
            let (mut mag, mut ph) = (Vec::new(), Vec::new());
            if let Some(v) = plot.vector(&out) {
                for (i, fi) in f.iter().enumerate() {
                    let c = v.complex_at(i);
                    mag.push([*fi, 20.0 * c.abs().max(1e-30).log10()]);
                    ph.push([*fi, c.arg().to_degrees()]);
                }
            }
            let corner_hz = corner_frequency(&mag);
            let title = match corner_hz {
                Some(fc) => format!(
                    ".ac — Bode of {} · −3 dB at {}",
                    upper(&out),
                    fmt_readout(fc, "Hz", 2)
                ),
                None => format!(".ac — Bode of {}", upper(&out)),
            };
            AcView {
                title,
                mag: vec![Trace::new(format!("|{}| dB", upper(&out)), p.accent, mag)],
                phase: vec![Trace::new(format!("φ {} °", upper(&out)), p.info, ph).width(1.6)],
                plot: plot.clone(),
                corner_hz,
            }
        });

    // ---- noise
    let noise = results
        .runs
        .get(nom)
        .and_then(|r| r.plot(PlotKind::Noise))
        .map(|plot| {
            let traces: Vec<Trace> = plot
                .names()
                .filter(|n| n.contains("onoise") && !n.contains('_'))
                .take(2)
                .filter_map(|n| series(plot, n).map(|pts| Trace::new(n.to_string(), p.accent, pts)))
                .collect();
            let total = results
                .runs
                .get(nom)
                .and_then(|r| r.plot(PlotKind::NoiseTotal))
                .and_then(|t| t.value("onoise_total"))
                .map(|v| format!("integrated output noise {}", fmt_readout(v, "V", 2)));
            NoiseView {
                title: ".noise — output noise density".into(),
                traces,
                total,
            }
        });

    // ---- operating point
    let mut op = Vec::new();
    if let Some(opp) = results.runs.get(nom).and_then(|r| r.plot(PlotKind::Op)) {
        for n in opp.names().filter(|n| is_top_node(n)) {
            if let Some(v) = opp.value(n) {
                op.push(OpRow {
                    quantity: upper(n),
                    value: fmt_readout(v, "V", 3),
                    raw: v,
                });
            }
        }
    }
    if let Some(tp) = results.runs.get(nom).and_then(|r| r.plot(PlotKind::Tran)) {
        let t = tp.scale_values();
        let names: Vec<String> = tp.names().map(str::to_string).collect();
        for n in names
            .iter()
            .filter(|n| n.starts_with("i(d") && !n.contains('.'))
        {
            let avg = time_avg(&t, &tp.real(n).unwrap_or_default());
            if avg.abs() > 1e-6 {
                op.push(OpRow {
                    quantity: format!("{} avg", upper(n)),
                    value: fmt_readout(avg, "A", 2),
                    raw: avg,
                });
            }
        }
        // resistor power: avg(v(a,b) · i(r)) — needs the element's nodes
        for (name, a, b) in resistor_nodes(text) {
            let key = format!("i({})", name.to_ascii_lowercase());
            let (Some(i), Some(va)) = (tp.real(&key), node_series(tp, &a)) else {
                continue;
            };
            let vb = node_series(tp, &b).unwrap_or_else(|| vec![0.0; va.len()]);
            let pw: Vec<f64> = (0..i.len().min(va.len()).min(vb.len()))
                .map(|k| (va[k] - vb[k]) * i[k])
                .collect();
            let avg = time_avg(&t, &pw);
            if avg.abs() > 1e-3 {
                op.push(OpRow {
                    quantity: format!("P({})", name.to_ascii_uppercase()),
                    value: fmt_readout(avg, "W", 1),
                    raw: avg,
                });
            }
        }
    }
    if !profile.op_rows.is_empty() {
        let rank = |q: &str| {
            profile
                .op_rows
                .iter()
                .position(|r| r == q)
                .unwrap_or(usize::MAX)
        };
        op.sort_by_key(|r| rank(&r.quantity));
    }

    // ---- .meas + .tf
    let mut measures = Vec::new();
    let stepped = results.runs.len() > 1;
    for r in &results.runs {
        let run = if stepped {
            r.label.clone()
        } else {
            String::new()
        };
        for m in &r.measures {
            let (value, ok) = match &m.value {
                Ok(v) => (fmt_measure(*v), true),
                Err(e) => (e.clone(), false),
            };
            measures.push(MeasRow {
                name: m.name.clone(),
                run: run.clone(),
                value,
                ok,
            });
        }
        if let Some(tf) = r.plot(PlotKind::Tf) {
            for n in tf.names().map(str::to_string).collect::<Vec<_>>() {
                if let Some(v) = tf.value(&n) {
                    let unit = if n.contains("impedance") { "Ω" } else { "" };
                    measures.push(MeasRow {
                        name: format!(".tf {n}"),
                        run: run.clone(),
                        value: if unit.is_empty() {
                            fmt_measure(v)
                        } else {
                            fmt_readout(v, unit, 3)
                        },
                        ok: true,
                    });
                }
            }
        }
        if !stepped {
            break;
        }
    }

    ViewData {
        summary: results.summary(),
        tran,
        dc,
        mc,
        ac,
        noise,
        op,
        measures,
        warnings,
        dc_family: family
            .iter()
            .map(|(r, p)| (r.label.clone(), (*p).clone()))
            .collect(),
    }
}

/// A unitless measurement value (`.meas` results carry no unit): 4 significant digits.
fn fmt_measure(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return format!("{v}");
    }
    let mag = v.abs().log10();
    if (-3.0..6.0).contains(&mag) {
        let decimals = (3 - mag.floor() as i32).clamp(0, 6) as usize;
        format!("{v:.decimals$}")
    } else {
        format!("{v:.3e}")
    }
}

fn node_series(plot: &Plot, node: &str) -> Option<Vec<f64>> {
    if node == "0" || node.eq_ignore_ascii_case("gnd") {
        return Some(vec![0.0; plot.len()]);
    }
    plot.real(&format!("v({})", node.to_ascii_lowercase()))
}

/// `(name, node a, node b)` of top-level resistors.
fn resistor_nodes(text: &str) -> Vec<(String, String, String)> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let name = it.next()?;
            if !name.to_ascii_uppercase().starts_with('R') {
                return None;
            }
            Some((
                name.to_string(),
                it.next()?.to_string(),
                it.next()?.to_string(),
            ))
        })
        .collect()
}

fn interp(pts: &[[f64; 2]], x: f64) -> Option<f64> {
    pts.windows(2).find_map(|w| {
        let (a, b) = (w[0], w[1]);
        ((a[0] <= x && x <= b[0]) || (b[0] <= x && x <= a[0])).then(|| {
            if (b[0] - a[0]).abs() < 1e-300 {
                a[1]
            } else {
                a[1] + (x - a[0]) / (b[0] - a[0]) * (b[1] - a[1])
            }
        })
    })
}

/// Mixed-signal panel values from the netlist text (`PRO-5`).
pub fn mixed_info(text: &str) -> MixedInfo {
    let lower = text.to_ascii_lowercase();
    let sync = lower
        .split_whitespace()
        .find_map(|t| t.strip_prefix("sync="))
        .and_then(|v| bb_core::si::parse(v).ok())
        .map(|v| fmt_si(v, "s"))
        .unwrap_or_else(|| "1 µs".into());
    let method = lower
        .split_whitespace()
        .find_map(|t| t.strip_prefix("method="))
        .map(|m| {
            if m.starts_with("gear") {
                "Gear-2".to_string()
            } else {
                m.to_string()
            }
        })
        .unwrap_or_else(|| "trap".into());
    let reltol = lower
        .split_whitespace()
        .find_map(|t| t.strip_prefix("reltol="))
        .unwrap_or("1e-3")
        .to_string();
    MixedInfo {
        sync,
        solver: "Newton · gmin step".into(),
        integration: format!("{method}, reltol {reltol}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_of_a_first_order_low_pass_and_high_pass() {
        let fc = 1000.0;
        let lp: Vec<[f64; 2]> = (0..=60)
            .map(|i| {
                let f = 10f64.powf(1.0 + i as f64 / 15.0);
                [f, -10.0 * (1.0 + (f / fc).powi(2)).log10()]
            })
            .collect();
        let got = corner_frequency(&lp).unwrap();
        assert!((got / fc - 1.0).abs() < 0.01, "{got}");
        let hp: Vec<[f64; 2]> = lp
            .iter()
            .map(|p| [p[0], -10.0 * (1.0 + (fc / p[0]).powi(2)).log10()])
            .collect();
        let got = corner_frequency(&hp).unwrap();
        assert!((got / fc - 1.0).abs() < 0.01, "{got}");
        let flat: Vec<[f64; 2]> = lp.iter().map(|p| [p[0], 0.0]).collect();
        assert_eq!(corner_frequency(&flat), None);
    }

    #[test]
    fn text_params_are_stripped_from_instances_only() {
        let t =
            "XU1 VCC 0 A0 ATMEGA328P\n+     fw=garden-node.hex  sync=1u\nR1 a b 1k\n.param f=1.5\n";
        let s = strip_text_params(t);
        assert!(s.contains("sync=1u") && !s.contains("fw="), "{s}");
        assert!(s.contains("R1 a b 1k\n.param f=1.5\n"));
    }

    #[test]
    fn demo_runs_and_matches_the_design_numbers() {
        let profile = demo::profile();
        let v = run(
            demo::NETLIST,
            "garden-node.cir",
            None,
            &profile,
            CancelToken::new(),
            Arc::new(|_| {}),
        )
        .expect("demo runs");
        assert!(v.summary.starts_with("3 steps · "), "{}", v.summary);
        let tran = v.tran.as_ref().unwrap();
        assert_eq!(tran.title, ".tran — night LED drive");
        assert_eq!(tran.traces[0].name, "V(D9)");
        assert_eq!(tran.traces[1].name, "I(D1)");
        let (a, b, duty) = tran.cursors.unwrap();
        assert!(
            ((1.0 / (b - a)) - 490.0).abs() < 2.0,
            "f = {}",
            1.0 / (b - a)
        );
        assert!((duty - 0.478).abs() < 0.01, "duty {duty}");
        let dc = v.dc.as_ref().unwrap();
        assert_eq!(dc.traces.len(), 3);
        assert_eq!(dc.traces[0].name, "R1 4.7k");
        assert!(dc.x_log);
        let (x, y, label) = dc.marker.clone().unwrap();
        assert_eq!(x, 180.0);
        assert!((y - 2.598).abs() < 0.01, "{y}");
        assert_eq!(label, "op: 180 lx → 2.60 V");
        let mc = v.mc.as_ref().unwrap();
        assert_eq!(mc.title, ".mc — V(A1) spread, 100 runs");
        assert!(
            mc.stats.starts_with("μ 2.59") || mc.stats.starts_with("μ 2.60"),
            "{}",
            mc.stats
        );
        let q: Vec<&str> = v.op.iter().map(|r| r.quantity.as_str()).collect();
        assert_eq!(&q[..3], &["V(VCC)", "V(A0)", "V(A1)"]);
        assert!(q.contains(&"I(D1) avg") && q.contains(&"P(R2)"), "{q:?}");
        let a1 = v.op.iter().find(|r| r.quantity == "V(A1)").unwrap();
        assert!(
            a1.value.starts_with("2.59") || a1.value.starts_with("2.60"),
            "{}",
            a1.value
        );
    }

    #[test]
    fn parse_errors_carry_the_line() {
        let e = run(
            "t\nR1 a\n.op\n",
            "x.cir",
            None,
            &Profile::default(),
            CancelToken::new(),
            Arc::new(|_| {}),
        )
        .unwrap_err();
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn ac_and_bode() {
        let v = run(
            "rc\nV1 in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 100n\n.ac dec 10 10 100k\n.end\n",
            "rc.cir",
            None,
            &Profile::default(),
            CancelToken::new(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let ac = v.ac.clone().unwrap();
        assert_eq!(ac.title, ".ac — Bode of V(OUT)");
        let mag = &ac.mag[0].points;
        assert!(mag.first().unwrap()[1].abs() < 0.1);
        assert!(mag.last().unwrap()[1] < -20.0);
    }

    #[test]
    fn mixed_info_from_netlist() {
        let m = mixed_info(demo::NETLIST);
        assert_eq!(m.sync, "1 µs");
        assert_eq!(m.integration, "trap, reltol 1e-3");
    }
}
