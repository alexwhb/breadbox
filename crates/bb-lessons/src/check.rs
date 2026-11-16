//! Checker evaluation (LES-3/LES-4): goals against the circuit (`bb_circuit` connectivity,
//! parts, properties, ERC) and the running bench (`bb_sim`: voltages, currents, device and
//! instrument readings — the DMM plugin's `value` —, LED state, damage, serial output, pin
//! frequency/duty via probes, MQTT traffic, world variables) or a SPICE analysis.

use crate::format::{Check, Step};
use crate::names::{self, Namer};
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
    /// What the goal is, for people (`USB +5 V reaches R1's lead in column 5`).
    pub label: String,
    pub ok: bool,
    /// What was found (`13.6 mA`, `not yet`).
    pub detail: String,
    /// Index of the step's check (`None` for the goals a quiz, code step or table imply).
    #[serde(default)]
    pub check: Option<usize>,
}

/// Everything a check may look at.
pub struct Ctx<'a> {
    pub project: &'a Project,
    pub lib: &'a Library,
    pub bench: Option<&'a mut Bench>,
    pub state: &'a StepState,
    /// The step the check belongs to (reading units, labels).
    pub step: Option<&'a Step>,
}

use crate::names::split;

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

/// `(scale, unit)` the step shows a part's reading in (`dmm1` in mA: 1000, `mA`).
pub fn reading_unit(step: Option<&Step>, part: &str) -> Option<(f64, String)> {
    let step = step?;
    if let Some(m) = &step.measure
        && m.part.eq_ignore_ascii_case(part)
        && !m.unit.is_empty()
    {
        return Some((m.scale, m.unit.clone()));
    }
    let p = step.predict.as_ref()?;
    (p.meter.as_deref()? == part)
        .then(|| (if p.unit == "mA" { 1000.0 } else { 1.0 }, p.unit.clone()))
}

/// `11–14.5 mA`, `4.85–5.05 V`.
pub fn range(min: f64, max: f64, unit: &str) -> String {
    let f = |v: f64| bb_core::si::format_sig(v, unit, 3);
    let (a, b) = (f(min), f(max));
    // `11 mA–14.5 mA` → `11–14.5 mA`
    let ua = a.split_once(' ').map(|x| x.1).unwrap_or("");
    let ub = b.split_once(' ').map(|x| x.1).unwrap_or("");
    if !ua.is_empty() && ua == ub {
        format!("{}–{b}", a.split(' ').next().unwrap_or(&a))
    } else {
        format!("{a}–{b}")
    }
}

fn in_unit(v: f64, scale: f64, unit: &str) -> String {
    let x = v * scale;
    if !x.is_finite() {
        return "—".into();
    }
    if x.abs() < 1e-9 {
        return format!("0 {unit}");
    }
    format!("{} {unit}", crate::text::smart(x))
}

/// Upper-cases the first letter (`the meter reads …` → `The meter reads …`).
pub fn sentence(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The unit of a numeric property (`Ω` for a resistance).
fn prop_unit(n: &Namer, part: &str, prop: &str) -> String {
    n.def(part)
        .and_then(|d| d.prop(prop).map(|p| p.unit.clone()))
        .unwrap_or_default()
}

/// A world variable in words (`light.ambient_lux` → `room light`).
pub fn world_name(key: &str) -> (String, String) {
    let k = key.to_ascii_lowercase();
    if k.ends_with("ambient_lux") || k.ends_with("lux") {
        ("room light".into(), "lux".into())
    } else if k.ends_with("temp_c") {
        ("air temperature".into(), "°C".into())
    } else if k.ends_with(".vwc") {
        (
            format!(
                "soil moisture in {}",
                k.split('.').next().unwrap_or("the pot")
            ),
            "%".into(),
        )
    } else if k.ends_with(".rh") || k.ends_with("humidity") {
        ("humidity".into(), "%".into())
    } else {
        (key.to_string(), String::new())
    }
}

/// The friendly label of a goal (the lesson's own `label` wins, see [`Step::label_of`]).
pub fn goal_label(c: &Check, n: &Namer, step: Option<&Step>) -> String {
    let s = match c {
        Check::Connected { a, b } => format!("{} reaches {}", n.pin_str(a), n.pin_str(b)),
        Check::NotConnected { a, b } => {
            format!("{} no longer reaches {}", n.pin_str(a), n.pin_str(b))
        }
        Check::Part { def, id, props, .. } => {
            let name = n
                .lib
                .get(def)
                .map(|d| d.name.to_lowercase())
                .unwrap_or(def.clone());
            let vals: Vec<String> = props
                .iter()
                .map(|(k, v)| {
                    let unit = n
                        .lib
                        .get(def)
                        .and_then(|d| d.prop(k).map(|p| p.unit.clone()))
                        .unwrap_or_default();
                    bb_core::si::format(*v, &unit)
                })
                .collect();
            let what = if vals.is_empty() {
                format!("a {name}")
            } else {
                format!("a {} {name}", vals.join(" "))
            };
            match id {
                Some(id) => format!("{}: {what} on the bench", id.to_uppercase()),
                None => format!("{what} on the bench"),
            }
        }
        Check::Prop {
            part,
            prop,
            min,
            max,
        } => {
            let unit = prop_unit(n, part, prop);
            format!(
                "{} {prop} set to {}",
                n.reference(part),
                range(*min, *max, &unit)
            )
        }
        Check::Voltage { a, b, min, max } => match b {
            Some(b) => format!(
                "{} to {} at {}",
                n.pin_str(a),
                n.pin_str(b),
                range(*min, *max, "V")
            ),
            None => format!("{} at {}", n.pin_str(a), range(*min, *max, "V")),
        },
        Check::Current { part, min, max, .. } => {
            format!("{} carries {}", n.part(part), range(*min, *max, "A"))
        }
        Check::Reading {
            part,
            key,
            min,
            max,
        } => {
            let who = n.part(part);
            match reading_unit(step, part) {
                Some((scale, unit)) if key == "value" => format!(
                    "{who} reads {}",
                    range(*min * scale, *max * scale, &unit)
                        .replace(&format!(" k{unit}"), &format!("k {unit}"))
                ),
                _ if key == "frequency" => format!("{who} shows {}", range(*min, *max, "Hz")),
                _ => format!("{who} {key} in {}", range(*min, *max, "")),
            }
        }
        Check::LedLit { part, .. } => format!("{} lights up", n.part(part)),
        Check::LedOff { part } => format!("{} is dark", n.part(part)),
        Check::Damaged { part } => format!("{} burns out", n.part(part)),
        Check::Healthy { part } => format!("{} still works", n.part(part)),
        Check::SerialContains { mcu, text } => {
            format!("{} prints “{}”", n.part_short(mcu), text.trim())
        }
        Check::PinFrequency { mcu, pin, hz, .. } => {
            format!("{} switches at {}", n.pin(mcu, pin), names::hz(*hz))
        }
        Check::PinDuty { mcu, pin, min, max } => format!(
            "{} high {:.0}–{:.0} % of the time",
            n.pin(mcu, pin),
            min * 100.0,
            max * 100.0
        ),
        Check::Firmware { mcu } => format!("Your sketch runs on {}", n.part_short(mcu)),
        Check::World { key, min, max } => {
            let (name, unit) = world_name(key);
            format!("{name} at {}", range(*min, *max, &unit))
        }
        Check::Mqtt { topic, contains } => match contains {
            Some(c) => format!("A message on {topic} with “{c}”"),
            None => format!("A message on {topic}"),
        },
        Check::Phone { contains } => format!("The phone shows “{}”", contains.trim()),
        Check::Prediction => "Your prediction is right".into(),
        Check::Measurements { min } => format!("{min} measurements"),
        Check::Quiz => "The right answer".into(),
        Check::ErcClean { .. } => "No wiring errors".into(),
        Check::Spice {
            analysis,
            vector,
            at,
            stat,
            min,
            max,
        } => {
            let v = vector.to_uppercase();
            let unit = if vector.to_ascii_lowercase().starts_with("i(") {
                "A"
            } else if stat == "mag_db" {
                "dB"
            } else if stat == "phase_deg" {
                "°"
            } else {
                "V"
            };
            let r = if unit == "dB" || unit == "°" {
                format!("{min}–{max} {unit}")
            } else {
                range(*min, *max, unit)
            };
            let when = match analysis.split_whitespace().next().unwrap_or("") {
                ".tran" if stat == "max" => " (peak)".to_string(),
                ".tran" => format!(" at {}", bb_core::si::format(*at, "s")),
                ".ac" => format!(" at {}", bb_core::si::format(*at, "Hz")),
                _ => String::new(),
            };
            format!("{v}{when} in {r}")
        }
    };
    sentence(&s)
}

/// Evaluates one check.
pub fn evaluate(c: &Check, ctx: &mut Ctx, conn: &Connectivity) -> CheckResult {
    let n = Namer::new(ctx.project, ctx.lib, conn);
    let label = goal_label(c, &n, ctx.step);
    let r = |ok: bool, detail: String| CheckResult {
        label: label.clone(),
        ok,
        detail,
        check: None,
    };
    if needs_bench(c) && ctx.bench.is_none() {
        return r(false, "the bench is not running".into());
    }
    match c {
        Check::Connected { a, b } => {
            let ok = conn.connected(split(a), split(b));
            r(
                ok,
                if ok {
                    "connected".into()
                } else {
                    "not yet".into()
                },
            )
        }
        Check::NotConnected { a, b } => {
            let ok = !conn.connected(split(a), split(b));
            r(
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
            let found = ctx
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
            r(
                found >= *count,
                if found >= *count {
                    "on the bench".into()
                } else {
                    "not yet".into()
                },
            )
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
            let unit = prop_unit(&n, part, prop);
            r(
                v.is_some_and(|v| range_ok(v, *min, *max)),
                v.map(|v| bb_core::si::format(v, &unit))
                    .unwrap_or_else(|| "missing".into()),
            )
        }
        Check::Prediction => r(
            ctx.state.verdict == Some(true),
            match ctx.state.verdict {
                Some(true) => "right".into(),
                Some(false) => "not yet".into(),
                None => "no prediction".into(),
            },
        ),
        Check::Measurements { min } => r(
            ctx.state.rows.len() >= *min,
            format!("{} recorded", ctx.state.rows.len()),
        ),
        Check::Quiz => r(
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
                Ok(v) => {
                    let unit = if vector.to_ascii_lowercase().starts_with("i(") {
                        "A"
                    } else {
                        "V"
                    };
                    let detail = match stat.as_str() {
                        "mag_db" => format!("{v:.1} dB"),
                        "phase_deg" => format!("{v:.1}°"),
                        _ => names::value(v, unit),
                    };
                    r(range_ok(v, *min, *max), detail)
                }
                Err(e) => r(false, e),
            }
        }
        _ => {
            let step = ctx.step;
            let b = ctx.bench.as_deref_mut().expect("checked above");
            let mut res = bench_check(c, b, step);
            res.label = label;
            res
        }
    }
}

fn bench_check(c: &Check, b: &mut Bench, step: Option<&Step>) -> CheckResult {
    let r = |ok: bool, detail: String| CheckResult {
        label: String::new(),
        ok,
        detail,
        check: None,
    };
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
                v.is_some_and(|v| range_ok(v, *min, *max)),
                v.map(names::voltage)
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
                i.is_some_and(|i| range_ok(i.abs(), *min, *max)),
                i.map(names::current).unwrap_or_else(|| "—".into()),
            )
        }
        Check::Reading {
            part,
            key,
            min,
            max,
        } => {
            let v = reading(b, part, key);
            let shown = match (v, reading_unit(step, part)) {
                (Some(v), Some((scale, unit))) if key == "value" => in_unit(v, scale, &unit),
                (Some(v), _) if key == "frequency" => names::hz(v),
                (Some(v), _) => format!("{v:.4}"),
                (None, _) => "no reading".into(),
            };
            r(v.is_some_and(|v| range_ok(v, *min, *max)), shown)
        }
        Check::LedLit { part, min_ma } => {
            let i = b.element_current(part, "D").unwrap_or(0.0).abs();
            let burnt = b.health(part).is_damaged();
            r(
                i * 1e3 >= *min_ma && !burnt,
                if burnt {
                    "burnt out".into()
                } else if i < 1e-4 {
                    "off".into()
                } else {
                    names::current(i)
                },
            )
        }
        Check::LedOff { part } => {
            let i = b.element_current(part, "D").unwrap_or(0.0).abs();
            r(
                i < 1e-4,
                if i < 1e-4 {
                    "off".into()
                } else {
                    format!("lit, {}", names::current(i))
                },
            )
        }
        Check::Damaged { part } => {
            let h = b.health(part);
            r(
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
                !h.is_damaged(),
                match h {
                    Health::Damaged { .. } => "burnt out".into(),
                    Health::Stressed { .. } => "stressed".into(),
                    Health::Ok => "fine".into(),
                },
            )
        }
        Check::SerialContains { mcu, text } => {
            let found = b.serial(mcu).is_some_and(|s| {
                s.lines.iter().any(|l| l.text.contains(text.as_str()))
                    || s.pending().contains(text.as_str())
            });
            r(
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
                f.is_some_and(|f| (f - hz).abs() <= hz * tolerance),
                f.map(|f| {
                    if f.abs() < 1e-6 {
                        "steady".into()
                    } else {
                        names::hz(f)
                    }
                })
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
                d.is_some_and(|d| range_ok(d, *min, *max)),
                d.map(|d| format!("{:.1} %", d * 100.0))
                    .unwrap_or_else(|| "no probe".into()),
            )
        }
        Check::Firmware { mcu } => {
            let has = b.mcu(mcu).is_some();
            let lines = b.serial(mcu).map(|s| s.data_lines().count()).unwrap_or(0);
            r(
                has,
                if lines == 1 {
                    "1 line printed".into()
                } else {
                    format!("{lines} lines printed")
                },
            )
        }
        Check::World { key, min, max } => {
            let v = bb_core::WorldView::var(b.world(), key);
            let (_, unit) = world_name(key);
            r(
                v.is_some_and(|v| range_ok(v, *min, *max)),
                v.map(|v| {
                    format!("{} {unit}", crate::text::smart(v))
                        .trim()
                        .to_string()
                })
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
                found,
                if found {
                    "received".into()
                } else {
                    "nothing yet".into()
                },
            )
        }
        _ => r(false, "not a bench check".into()),
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

/// Evaluates every check of a step: one result per check (labels: the lesson's own, else the
/// generated friendly ones), then the goals a quiz, code step or measurements table imply.
pub fn evaluate_step(
    step: &crate::format::Step,
    project: &Project,
    lib: &Library,
    bench: Option<&mut Bench>,
    state: &StepState,
) -> Vec<CheckResult> {
    let conn = Connectivity::compute(project, lib);
    evaluate_step_with(step, project, lib, bench, state, &conn)
}

/// [`evaluate_step`] with the circuit's connectivity already computed.
pub fn evaluate_step_with(
    step: &crate::format::Step,
    project: &Project,
    lib: &Library,
    mut bench: Option<&mut Bench>,
    state: &StepState,
    conn: &Connectivity,
) -> Vec<CheckResult> {
    let mut out = Vec::new();
    for (i, c) in step.checks.iter().enumerate() {
        let mut res = if let Check::Quiz = c {
            let ok = step
                .quiz
                .as_ref()
                .is_some_and(|q| state.quiz == Some(q.answer));
            CheckResult {
                label: "The right answer".into(),
                ok,
                detail: if ok { "right".into() } else { "not yet".into() },
                check: None,
            }
        } else {
            let mut ctx = Ctx {
                project,
                lib,
                bench: bench.as_deref_mut(),
                state,
                step: Some(step),
            };
            evaluate(c, &mut ctx, conn)
        };
        res.check = Some(i);
        if let Some(l) = step.label_of(i) {
            res.label = l.to_string();
        }
        out.push(res);
    }
    // implied goals of the step kinds
    if let Some(q) = &step.quiz
        && !step.checks.iter().any(|c| matches!(c, Check::Quiz))
    {
        let ok = state.quiz == Some(q.answer);
        out.push(CheckResult {
            label: "The right answer".into(),
            ok,
            detail: if ok { "right".into() } else { "not yet".into() },
            check: None,
        });
    }
    if step.code.is_some() {
        out.push(CheckResult {
            label: "Your sketch is loaded".into(),
            ok: state.firmware,
            detail: if state.firmware {
                "running".into()
            } else {
                "not yet".into()
            },
            check: None,
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
            check: None,
        });
    }
    out
}
