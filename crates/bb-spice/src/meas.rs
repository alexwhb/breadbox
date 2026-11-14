//! `.meas` / `.measure` post-processing of analysis results.
//!
//! Supported forms (ngspice syntax): `avg`, `rms`, `pp`, `min`, `max`, `min_at`, `max_at`,
//! `integ` (with `from=`/`to=`), `find … at=`, `find … when …`, `when …`, `deriv … at=`,
//! `deriv … when …`, `trig … targ …` and `param='expr'`. Conditions take `val=`, `td=`,
//! `rise=`/`fall=`/`cross=` (a count or `last`). Values are interpolated linearly between
//! output points.

use crate::expr::{Expr, UnOp};
use crate::results::Plot;

/// Which crossing of a condition to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Rise(usize),
    Fall(usize),
    Cross(usize),
    RiseLast,
    FallLast,
    CrossLast,
}

/// A trigger/target/when condition: either a fixed abscissa (`at=`) or the moment `expr`
/// crosses `val`.
#[derive(Debug, Clone, PartialEq)]
pub struct Trigger {
    pub at: Option<f64>,
    pub expr: Option<Expr>,
    pub val: Option<Expr>,
    pub td: f64,
    pub edge: Edge,
}

/// Statistic over an interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatFunc {
    Avg,
    Rms,
    Pp,
    Min,
    Max,
    MinAt,
    MaxAt,
    Integ,
}

/// Measurement kind.
#[derive(Debug, Clone, PartialEq)]
pub enum MeasKind {
    Stat {
        func: StatFunc,
        expr: Expr,
        from: Option<f64>,
        to: Option<f64>,
    },
    FindAt {
        expr: Expr,
        at: f64,
    },
    FindWhen {
        expr: Expr,
        when: Trigger,
    },
    When(Trigger),
    DerivAt {
        expr: Expr,
        at: f64,
    },
    DerivWhen {
        expr: Expr,
        when: Trigger,
    },
    TrigTarg {
        trig: Trigger,
        targ: Trigger,
    },
    Param(Expr),
}

/// Evaluates an output expression (`v(out)`, `v(a)-v(b)`, `vdb(out)`, …) over a plot,
/// returning one real value per point. Complex vectors are reduced to magnitude unless wrapped
/// in `db()/mag()/ph()/real()/imag()`.
pub fn eval_vector(
    plot: &Plot,
    e: &Expr,
    params: &dyn Fn(&str) -> Option<f64>,
) -> Result<Vec<f64>, String> {
    let n = plot.len();
    let out: Result<Vec<f64>, String> = (0..n).map(|i| eval_point(plot, e, i, params)).collect();
    out
}

fn vector_value(plot: &Plot, name: &str, i: usize, mode: &str) -> Result<f64, String> {
    let v = plot
        .vector(name)
        .ok_or_else(|| format!("no vector `{name}` in {} results", plot.kind_name()))?;
    Ok(match mode {
        "db" => 20.0 * v.complex_at(i).abs().log10(),
        "ph" => v.complex_at(i).arg().to_degrees(),
        "real" => v.complex_at(i).re,
        "imag" => v.complex_at(i).im,
        _ => {
            if v.is_complex() {
                v.complex_at(i).abs()
            } else {
                v.real_at(i)
            }
        }
    })
}

fn probe_name(e: &Expr) -> Option<String> {
    match e {
        Expr::Volt(a, None) => Some(format!("v({a})")),
        Expr::Volt(a, Some(b)) => Some(format!("v({a},{b})")),
        Expr::Curr(a) => Some(format!("i({a})")),
        _ => None,
    }
}

fn eval_point(
    plot: &Plot,
    e: &Expr,
    i: usize,
    params: &dyn Fn(&str) -> Option<f64>,
) -> Result<f64, String> {
    match e {
        Expr::Volt(a, Some(b)) => {
            if plot.vector(&format!("v({a},{b})")).is_some() {
                return vector_value(plot, &format!("v({a},{b})"), i, "");
            }
            let va = vector_value(plot, &format!("v({a})"), i, "")?;
            let vb = if b == "0" {
                0.0
            } else {
                vector_value(plot, &format!("v({b})"), i, "")?
            };
            Ok(va - vb)
        }
        Expr::Volt(..) | Expr::Curr(_) => {
            vector_value(plot, &probe_name(e).unwrap_or_default(), i, "")
        }
        Expr::UserCall(f, args)
            if args.len() == 1 && matches!(f.as_str(), "db" | "mag" | "ph" | "real" | "imag") =>
        {
            if let Some(name) = probe_name(&args[0]) {
                if let Expr::Volt(a, Some(b)) = &args[0] {
                    if plot.vector(&name).is_none() {
                        // complex difference
                        let va = plot
                            .vector(&format!("v({a})"))
                            .ok_or_else(|| format!("no vector `v({a})`"))?
                            .complex_at(i);
                        let vb = if b == "0" {
                            Default::default()
                        } else {
                            plot.vector(&format!("v({b})"))
                                .ok_or_else(|| format!("no vector `v({b})`"))?
                                .complex_at(i)
                        };
                        let d = va - vb;
                        return Ok(match f.as_str() {
                            "db" => 20.0 * d.abs().log10(),
                            "ph" => d.arg().to_degrees(),
                            "real" => d.re,
                            "imag" => d.im,
                            _ => d.abs(),
                        });
                    }
                }
                return vector_value(plot, &name, i, f);
            }
            let x = eval_point(plot, &args[0], i, params)?;
            Ok(match f.as_str() {
                "db" => 20.0 * x.abs().log10(),
                "mag" => x.abs(),
                _ => x,
            })
        }
        Expr::Num(n) => Ok(*n),
        Expr::Ident(id) => {
            if let Some(v) = params(id) {
                return Ok(v);
            }
            // bare vector name (e.g. a node) or the scale
            if plot.vector(id).is_some() {
                return vector_value(plot, id, i, "");
            }
            if plot.vector(&format!("v({id})")).is_some() {
                return vector_value(plot, &format!("v({id})"), i, "");
            }
            Err(format!("unknown name `{id}` in measurement"))
        }
        Expr::Rt(_) => Ok(plot.scale_at(i)),
        Expr::Unary(op, a) => {
            let x = eval_point(plot, a, i, params)?;
            Ok(match op {
                UnOp::Neg => -x,
                UnOp::Not => (x == 0.0) as i32 as f64,
            })
        }
        Expr::Binary(..) | Expr::Cond(..) | Expr::Call(..) => {
            // Evaluate leaves first, then the scalar expression.
            let resolved = e.rewrite(&mut |n| match n {
                Expr::Volt(..)
                | Expr::Curr(_)
                | Expr::UserCall(..)
                | Expr::Ident(_)
                | Expr::Rt(_) => eval_point(plot, n, i, params).map(|v| Some(Expr::Num(v))),
                _ => Ok(None),
            })?;
            resolved.eval_with(&|_| None)
        }
        Expr::Ext(_) => Err("ext() is not available in measurements".into()),
        Expr::UserCall(f, _) => Err(format!("unknown function `{f}` in measurement")),
    }
}

/// Linear interpolation of `y` at abscissa `x` (clamped).
pub fn interp(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    if xs.is_empty() {
        return f64::NAN;
    }
    if x <= xs[0] {
        return ys[0];
    }
    let n = xs.len();
    if x >= xs[n - 1] {
        return ys[n - 1];
    }
    let i = xs.partition_point(|&t| t <= x).max(1);
    let (x0, x1) = (xs[i - 1], xs[i]);
    if x1 == x0 {
        return ys[i];
    }
    ys[i - 1] + (ys[i] - ys[i - 1]) * (x - x0) / (x1 - x0)
}

/// Crossing times of `ys` through `level` (with direction: +1 rising, -1 falling).
fn crossings(xs: &[f64], ys: &[f64], level: &[f64], td: f64) -> Vec<(f64, i32)> {
    let mut out = Vec::new();
    for i in 1..xs.len() {
        if xs[i] < td {
            continue;
        }
        let a = ys[i - 1] - level[i - 1];
        let b = ys[i] - level[i];
        if (a < 0.0 && b >= 0.0) || (a > 0.0 && b <= 0.0) {
            // avoid double counting when a sample lands exactly on the level
            if a != 0.0 {
                let frac = a / (a - b);
                let x = xs[i - 1] + frac * (xs[i] - xs[i - 1]);
                if x >= td {
                    out.push((x, if b > a { 1 } else { -1 }));
                }
            }
        } else if a == 0.0 && i == 1 && xs[0] >= td {
            // starts exactly on the level: not a crossing
        }
    }
    out
}

fn pick(cross: &[(f64, i32)], edge: Edge) -> Option<f64> {
    let filt = |dir: i32| {
        cross
            .iter()
            .filter(move |c| dir == 0 || c.1 == dir)
            .map(|c| c.0)
    };
    match edge {
        Edge::Rise(n) => filt(1).nth(n.max(1) - 1),
        Edge::Fall(n) => filt(-1).nth(n.max(1) - 1),
        Edge::Cross(n) => filt(0).nth(n.max(1) - 1),
        Edge::RiseLast => filt(1).last(),
        Edge::FallLast => filt(-1).last(),
        Edge::CrossLast => filt(0).last(),
    }
}

fn trigger_time(
    plot: &Plot,
    t: &Trigger,
    params: &dyn Fn(&str) -> Option<f64>,
) -> Result<f64, String> {
    if let Some(at) = t.at {
        return Ok(at);
    }
    let xs = plot.scale_values();
    let e = t.expr.as_ref().ok_or("condition without a variable")?;
    let ys = eval_vector(plot, e, params)?;
    let level = match &t.val {
        Some(v) => eval_vector(plot, v, params)?,
        None => vec![0.0; xs.len()],
    };
    let c = crossings(&xs, &ys, &level, t.td);
    pick(&c, t.edge).ok_or_else(|| "condition never met".to_string())
}

/// Evaluates one measurement over `plot`. `params` resolves names of earlier measurements and
/// `.param`s for `param=` expressions.
pub fn measure(
    plot: &Plot,
    kind: &MeasKind,
    params: &dyn Fn(&str) -> Option<f64>,
) -> Result<f64, String> {
    let xs = plot.scale_values();
    if xs.is_empty() {
        return Err("no data".into());
    }
    match kind {
        MeasKind::Param(e) => e.eval_with(params),
        MeasKind::FindAt { expr, at } => Ok(interp(&xs, &eval_vector(plot, expr, params)?, *at)),
        MeasKind::DerivAt { expr, at } => Ok(deriv(&xs, &eval_vector(plot, expr, params)?, *at)),
        MeasKind::When(t) => trigger_time(plot, t, params),
        MeasKind::FindWhen { expr, when } => {
            let x = trigger_time(plot, when, params)?;
            Ok(interp(&xs, &eval_vector(plot, expr, params)?, x))
        }
        MeasKind::DerivWhen { expr, when } => {
            let x = trigger_time(plot, when, params)?;
            Ok(deriv(&xs, &eval_vector(plot, expr, params)?, x))
        }
        MeasKind::TrigTarg { trig, targ } => {
            Ok(trigger_time(plot, targ, params)? - trigger_time(plot, trig, params)?)
        }
        MeasKind::Stat {
            func,
            expr,
            from,
            to,
        } => {
            let ys = eval_vector(plot, expr, params)?;
            let a = from.unwrap_or(xs[0]).max(xs[0]);
            let b = to.unwrap_or(xs[xs.len() - 1]).min(xs[xs.len() - 1]);
            if b < a {
                return Err("empty interval (from > to)".into());
            }
            // Build the sample set inside [a, b] including interpolated end points.
            let mut px = vec![a];
            let mut py = vec![interp(&xs, &ys, a)];
            for (i, &x) in xs.iter().enumerate() {
                if x > a && x < b {
                    px.push(x);
                    py.push(ys[i]);
                }
            }
            if b > a {
                px.push(b);
                py.push(interp(&xs, &ys, b));
            }
            let integ = |f: &dyn Fn(f64) -> f64| -> f64 {
                let mut s = 0.0;
                for i in 1..px.len() {
                    s += 0.5 * (f(py[i]) + f(py[i - 1])) * (px[i] - px[i - 1]);
                }
                s
            };
            let span = b - a;
            Ok(match func {
                StatFunc::Avg => {
                    if span > 0.0 {
                        integ(&|y| y) / span
                    } else {
                        py[0]
                    }
                }
                StatFunc::Rms => {
                    if span > 0.0 {
                        // trapezoid on y² (ngspice integrates y² the same way)
                        (integ(&|y| y * y) / span).sqrt()
                    } else {
                        py[0].abs()
                    }
                }
                StatFunc::Integ => integ(&|y| y),
                StatFunc::Min => py.iter().copied().fold(f64::INFINITY, f64::min),
                StatFunc::Max => py.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                StatFunc::Pp => {
                    py.iter().copied().fold(f64::NEG_INFINITY, f64::max)
                        - py.iter().copied().fold(f64::INFINITY, f64::min)
                }
                StatFunc::MinAt => {
                    let i = (0..py.len())
                        .min_by(|&i, &j| py[i].total_cmp(&py[j]))
                        .unwrap_or(0);
                    px[i]
                }
                StatFunc::MaxAt => {
                    let i = (0..py.len())
                        .max_by(|&i, &j| py[i].total_cmp(&py[j]))
                        .unwrap_or(0);
                    px[i]
                }
            })
        }
    }
}

fn deriv(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    let n = xs.len();
    if n < 2 {
        return 0.0;
    }
    let i = xs.partition_point(|&t| t <= x).clamp(1, n - 1);
    let dx = xs[i] - xs[i - 1];
    if dx == 0.0 {
        0.0
    } else {
        (ys[i] - ys[i - 1]) / dx
    }
}
