//! Checker evaluation (LES-3/LES-4): goals against the circuit (`bb_circuit` connectivity,
//! parts, properties, ERC) and the running bench (`bb_sim`: voltages, currents, device and
//! instrument readings — the DMM plugin's `value` —, LED state, damage, serial output, pin
//! frequency/duty via probes, MQTT traffic, world variables) or a SPICE analysis.

use crate::format::Check;
use crate::session::StepState;
use bb_circuit::connectivity::Connectivity;
use bb_circuit::project::Quantity;
use bb_circuit::{Library, Project};
use bb_core::Health;
use bb_sim::Bench;
use serde::{Deserialize, Serialize};

/// One evaluated goal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    /// What the goal is, for people (`D1.a connected to R1.2`).
    pub label: String,
    pub ok: bool,
    /// What was found (`12.9 mA`, `not connected`).
    pub detail: String,
}

/// Everything a check may look at.
pub struct Ctx<'a> {
    pub project: &'a Project,
    pub lib: &'a Library,
    pub bench: Option<&'a mut Bench>,
    pub state: &'a StepState,
}

fn split(pin: &str) -> (&str, &str) {
    pin.split_once('.').unwrap_or((pin, ""))
}

fn range_ok(v: f64, min: f64, max: f64) -> bool {
    v.is_finite() && v >= min && v <= max
}

fn reading(b: &Bench, part: &str, key: &str) -> Option<f64> {
    b.readings(part)
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|x| x.1)
        .filter(|v| v.is_finite())
}

/// Probe id used for pin frequency / duty checks.
pub fn probe_id(mcu: &str, pin: &str, q: &str) -> String {
    format!("lesson-{mcu}-{pin}-{q}")
}

/// Adds the probes the checks need (frequency/duty) before the bench runs.
pub fn prepare(checks: &[Check], bench: &mut Bench) {
    let built = bench.netlist().clone();
    for c in checks {
        let (mcu, pin, q, quantity) = match c {
            Check::PinFrequency { mcu, pin, .. } => (mcu, pin, "f", Quantity::Frequency),
            Check::PinDuty { mcu, pin, .. } => (mcu, pin, "d", Quantity::Duty),
            _ => continue,
        };
        bench.instruments_mut().add_probe(
            bb_circuit::project::ProbeDef {
                id: probe_id(mcu, pin, q),
                label: format!("{mcu}.{pin}"),
                target: bb_circuit::project::Endpoint::pin(mcu, pin),
                quantity,
                pinned: None,
            },
            &built,
        );
    }
}

fn needs_bench(c: &Check) -> bool {
    !matches!(
        c,
        Check::Connected { .. }
            | Check::NotConnected { .. }
            | Check::Part { .. }
            | Check::Prop { .. }
            | Check::Prediction
            | Check::Measurements { .. }
            | Check::Quiz
            | Check::ErcClean { .. }
            | Check::Spice { .. }
    )
}

/// Evaluates one check.
pub fn evaluate(c: &Check, ctx: &mut Ctx, conn: &Connectivity) -> CheckResult {
    let r = |label: String, ok: bool, detail: String| CheckResult { label, ok, detail };
    if needs_bench(c) && ctx.bench.is_none() {
        return r(format!("{c:?}"), false, "the bench is not running".into());
    }
    match c {
        Check::Connected { a, b } => {
            let ok = conn.connected(split(a), split(b));
            r(
                format!("{} connected to {}", a.to_uppercase(), b.to_uppercase()),
                ok,
                if ok {
                    "connected".into()
                } else {
                    "not connected".into()
                },
            )
        }
        Check::NotConnected { a, b } => {
            let ok = !conn.connected(split(a), split(b));
            r(
                format!("{} not connected to {}", a.to_uppercase(), b.to_uppercase()),
                ok,
                if ok {
                    "separate".into()
                } else {
                    "still connected".into()
                },
            )
        }
        Check::Part {
            def,
            id,
            props,
            count,
        } => {
            let n = ctx
                .project
                .parts
                .iter()
                .filter(|p| p.part == *def)
                .filter(|p| id.as_ref().is_none_or(|i| p.id.eq_ignore_ascii_case(i)))
                .filter(|p| {
                    props.iter().all(|(k, want)| {
                        ctx.lib
                            .get(&p.part)
                            .and_then(|d| Project::prop_f64(d, p, k))
                            .is_some_and(|v| (v - want).abs() <= want.abs() * 0.01 + 1e-12)
                    })
                })
                .count();
            r(format!("{count}× {def}"), n >= *count, format!("{n} found"))
        }
        Check::Prop {
            part,
            prop,
            min,
            max,
        } => {
            let v = ctx
                .project
                .part(part)
                .and_then(|p| Project::prop_f64(ctx.lib.get(&p.part)?, p, prop));
            r(
                format!("{}.{prop} in {min}…{max}", part.to_uppercase()),
                v.is_some_and(|v| range_ok(v, *min, *max)),
                v.map(|v| bb_core::si::format(v, ""))
                    .unwrap_or_else(|| "missing".into()),
            )
        }
        Check::Prediction => r(
            "your prediction is right".into(),
            ctx.state.verdict == Some(true),
            match ctx.state.verdict {
                Some(true) => "right".into(),
                Some(false) => "not yet".into(),
                None => "no prediction".into(),
            },
        ),
        Check::Measurements { min } => r(
            format!("{min} measurements"),
            ctx.state.rows.len() >= *min,
            format!("{} recorded", ctx.state.rows.len()),
        ),
        Check::Quiz => r(
            "the right answer".into(),
            false, // decided by the session (needs the quiz); see `evaluate_step`
            String::new(),
        ),
        Check::ErcClean { strict } => {
            let erc = bb_circuit::erc::erc(ctx.project, ctx.lib, conn);
            let bad: Vec<_> = erc
                .iter()
                .filter(|i| *strict || i.severity == bb_circuit::erc::ErcSeverity::Error)
                .collect();
            r(
                "electrical rules".into(),
                bad.is_empty(),
                bad.first()
                    .map(|i| i.message.clone())
                    .unwrap_or_else(|| "clean".into()),
            )
        }
        Check::Spice {
            analysis,
            vector,
            at,
            stat,
            min,
            max,
            ..
        } => {
            let v = spice_value(ctx.project, ctx.lib, analysis, vector, *at, stat);
            match v {
                Ok(v) => r(
                    format!("{stat} of {vector} ({analysis})"),
                    range_ok(v, *min, *max),
                    bb_core::si::format(v, ""),
                ),
                Err(e) => r(format!("{stat} of {vector}"), false, e),
            }
        }
        _ => {
            let b = ctx.bench.as_deref_mut().expect("checked above");
            bench_check(c, b)
        }
    }
}

fn bench_check(c: &Check, b: &mut Bench) -> CheckResult {
    let r = |label: String, ok: bool, detail: String| CheckResult { label, ok, detail };
    match c {
        Check::Voltage {
            a,
            b: bpin,
            min,
            max,
        } => {
            let (pa, na) = split(a);
            let va = b.pin_voltage(pa, na);
            let vb = match bpin {
                Some(x) => {
                    let (p, n) = split(x);
                    b.pin_voltage(p, n)
                }
                None => Some(0.0),
            };
            let v = va.zip(vb).map(|(x, y)| x - y);
            r(
                format!(
                    "V({}) in {}…{}",
                    a.to_uppercase(),
                    bb_core::si::format(*min, "V"),
                    bb_core::si::format(*max, "V")
                ),
                v.is_some_and(|v| range_ok(v, *min, *max)),
                v.map(|v| bb_core::si::format(v, "V"))
                    .unwrap_or_else(|| "no such pin".into()),
            )
        }
        Check::Current {
            part,
            element,
            min,
            max,
        } => {
            let i = b.element_current(part, element);
            r(
                format!("current through {}", part.to_uppercase()),
                i.is_some_and(|i| range_ok(i.abs(), *min, *max)),
                i.map(|i| bb_core::si::format(i.abs(), "A"))
                    .unwrap_or_else(|| "—".into()),
            )
        }
        Check::Reading {
            part,
            key,
            min,
            max,
        } => {
            let v = reading(b, part, key);
            r(
                format!("{} {key}", part.to_uppercase()),
                v.is_some_and(|v| range_ok(v, *min, *max)),
                v.map(|v| format!("{v:.4}"))
                    .unwrap_or_else(|| "no reading".into()),
            )
        }
        Check::LedLit { part, min_ma } => {
            let i = b.element_current(part, "D").unwrap_or(0.0);
            r(
                format!("{} is lit", part.to_uppercase()),
                i * 1e3 >= *min_ma && !b.health(part).is_damaged(),
                bb_core::si::format(i, "A"),
            )
        }
        Check::LedOff { part } => {
            let i = b.element_current(part, "D").unwrap_or(0.0);
            r(
                format!("{} is off", part.to_uppercase()),
                i < 1e-4,
                bb_core::si::format(i, "A"),
            )
        }
        Check::Damaged { part } => {
            let h = b.health(part);
            r(
                format!("{} destroyed", part.to_uppercase()),
                h.is_damaged(),
                match h {
                    Health::Damaged { reason } => reason,
                    Health::Stressed { reason } => format!("stressed: {reason}"),
                    Health::Ok => "fine".into(),
                },
            )
        }
        Check::Healthy { part } => {
            let h = b.health(part);
            r(
                format!("{} works", part.to_uppercase()),
                !h.is_damaged(),
                format!("{h:?}"),
            )
        }
        Check::SerialContains { mcu, text } => {
            let found = b.serial(mcu).is_some_and(|s| {
                s.lines.iter().any(|l| l.text.contains(text.as_str()))
                    || s.pending().contains(text.as_str())
            });
            r(
                format!("{} prints “{text}”", mcu.to_uppercase()),
                found,
                if found {
                    "seen".into()
                } else {
                    "not yet".into()
                },
            )
        }
        Check::PinFrequency {
            mcu,
            pin,
            hz,
            tolerance,
        } => {
            let id = probe_id(mcu, pin, "f");
            let f = b
                .telemetry()
                .probes
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.value);
            r(
                format!(
                    "{}.{pin} at {} ± {:.0} %",
                    mcu.to_uppercase(),
                    bb_core::si::format(*hz, "Hz"),
                    tolerance * 100.0
                ),
                f.is_some_and(|f| (f - hz).abs() <= hz * tolerance),
                f.map(|f| bb_core::si::format(f, "Hz"))
                    .unwrap_or_else(|| "no probe".into()),
            )
        }
        Check::PinDuty { mcu, pin, min, max } => {
            let id = probe_id(mcu, pin, "d");
            let d = b
                .telemetry()
                .probes
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.value / 100.0);
            r(
                format!("{}.{pin} duty", mcu.to_uppercase()),
                d.is_some_and(|d| range_ok(d, *min, *max)),
                d.map(|d| format!("{:.1} %", d * 100.0))
                    .unwrap_or_else(|| "no probe".into()),
            )
        }
        Check::Firmware { mcu } => {
            let has = b.mcu(mcu).is_some();
            let lines = b.serial(mcu).map(|s| s.data_lines().count()).unwrap_or(0);
            r(
                format!("{} runs your sketch", mcu.to_uppercase()),
                has,
                format!("{lines} lines printed"),
            )
        }
        Check::World { key, min, max } => {
            let v = bb_core::WorldView::var(b.world(), key);
            r(
                format!("{key} in {min}…{max}"),
                v.is_some_and(|v| range_ok(v, *min, *max)),
                v.map(|v| format!("{v:.2}"))
                    .unwrap_or_else(|| "unknown".into()),
            )
        }
        Check::Mqtt { topic, contains } => {
            let tel = b.telemetry();
            let found = tel.radio.as_ref().is_some_and(|r| {
                r.mqtt.iter().any(|m| {
                    topic_matches(topic, &m.topic)
                        && contains
                            .as_ref()
                            .is_none_or(|c| m.payload.contains(c.as_str()))
                })
            });
            r(
                format!("MQTT {topic}"),
                found,
                if found {
                    "received".into()
                } else {
                    "nothing yet".into()
                },
            )
        }
        Check::Phone { contains } => {
            let tel = b.telemetry();
            let found = tel.radio.as_ref().is_some_and(|r| {
                r.phone.terminal.contains(contains.as_str())
                    || r.phone
                        .log
                        .iter()
                        .any(|l| l.text.contains(contains.as_str()))
            });
            r(
                format!("the phone shows “{contains}”"),
                found,
                if found {
                    "received".into()
                } else {
                    "nothing yet".into()
                },
            )
        }
        _ => r(format!("{c:?}"), false, "not a bench check".into()),
    }
}

/// MQTT topic filter match (`+`, `#`).
pub fn topic_matches(filter: &str, topic: &str) -> bool {
    let f: Vec<&str> = filter.split('/').collect();
    let t: Vec<&str> = topic.split('/').collect();
    for (i, part) in f.iter().enumerate() {
        match *part {
            "#" => return true,
            "+" => {
                if i >= t.len() {
                    return false;
                }
            }
            p => {
                if t.get(i) != Some(&p) {
                    return false;
                }
            }
        }
    }
    f.len() == t.len()
}

/// Runs a SPICE analysis card on the project's Pro netlist and reads one value.
pub fn spice_value(
    project: &Project,
    lib: &Library,
    analysis: &str,
    vector: &str,
    at: f64,
    stat: &str,
) -> Result<f64, String> {
    let mut nl = bb_circuit::spice_io::pro_netlist(project, lib).map_err(|e| e.to_string())?;
    // only the check's analysis runs (the project's `.param`/`.ic`/`.options` stay)
    nl.analyses.clear();
    nl.steps.clear();
    nl.mc = None;
    nl.measures.clear();
    bb_circuit::spice_io::merge_directives(&mut nl, analysis).map_err(|e| e.to_string())?;
    let res = nl.run().map_err(|e| e.to_string())?;
    let kind = match analysis.split_whitespace().next().unwrap_or("") {
        ".op" => bb_spice::results::PlotKind::Op,
        ".tran" => bb_spice::results::PlotKind::Tran,
        ".ac" => bb_spice::results::PlotKind::Ac,
        ".dc" => bb_spice::results::PlotKind::Dc,
        other => return Err(format!("unsupported analysis {other}")),
    };
    let run = res.runs.first().ok_or("no results")?;
    let plot = run.plot(kind).ok_or("no plot")?;
    let want = vector.to_ascii_lowercase();
    let v = plot
        .vectors
        .iter()
        .find(|v| v.name.to_ascii_lowercase() == want)
        .ok_or_else(|| format!("no vector {vector}"))?;
    if kind == bb_spice::results::PlotKind::Op {
        return Ok(v.reals().first().copied().unwrap_or(f64::NAN));
    }
    let scale = plot.vectors[0].reals();
    let n = scale.len();
    let idx = (0..n)
        .min_by(|a, b| (scale[*a] - at).abs().total_cmp(&(scale[*b] - at).abs()))
        .ok_or("empty plot")?;
    Ok(match stat {
        "max" => v.reals().iter().copied().fold(f64::NEG_INFINITY, f64::max),
        "min" => v.reals().iter().copied().fold(f64::INFINITY, f64::min),
        "mag_db" => {
            let c = v.complex_at(idx);
            20.0 * c.abs().log10()
        }
        "phase_deg" => v.complex_at(idx).arg().to_degrees(),
        "mag" => v.complex_at(idx).abs(),
        _ => {
            if v.is_complex() {
                v.complex_at(idx).abs()
            } else {
                v.reals()[idx]
            }
        }
    })
}

/// Evaluates every check of a step.
pub fn evaluate_step(
    step: &crate::format::Step,
    project: &Project,
    lib: &Library,
    mut bench: Option<&mut Bench>,
    state: &StepState,
) -> Vec<CheckResult> {
    let conn = Connectivity::compute(project, lib);
    let mut out = Vec::new();
    for c in &step.checks {
        if let Check::Quiz = c {
            let ok = step
                .quiz
                .as_ref()
                .is_some_and(|q| state.quiz == Some(q.answer));
            out.push(CheckResult {
                label: "the right answer".into(),
                ok,
                detail: if ok { "right".into() } else { "not yet".into() },
            });
            continue;
        }
        let mut ctx = Ctx {
            project,
            lib,
            bench: bench.as_deref_mut(),
            state,
        };
        out.push(evaluate(c, &mut ctx, &conn));
    }
    // implied goals of the step kinds
    if let Some(q) = &step.quiz
        && !step.checks.iter().any(|c| matches!(c, Check::Quiz))
    {
        let ok = state.quiz == Some(q.answer);
        out.push(CheckResult {
            label: "the right answer".into(),
            ok,
            detail: if ok { "right".into() } else { "not yet".into() },
        });
    }
    if step.code.is_some() {
        out.push(CheckResult {
            label: "your sketch is loaded".into(),
            ok: state.firmware,
            detail: if state.firmware {
                "running".into()
            } else {
                "upload your sketch".into()
            },
        });
    }
    if let Some(t) = &step.table
        && t.min_rows > 0
        && !step
            .checks
            .iter()
            .any(|c| matches!(c, Check::Measurements { .. }))
    {
        out.push(CheckResult {
            label: format!("{} measurements", t.min_rows),
            ok: state.rows.len() >= t.min_rows,
            detail: format!("{} recorded", state.rows.len()),
        });
    }
    out
}
