//! "Test circuit": a diagnosis of the current step — every goal evaluated, and for each one that
//! fails *what's wrong* in plain words, *where* on the bench (marks the app draws as glows,
//! pulsing rings and red markers) and a concrete next action.
//!
//! The engine is driven by connectivity (what each side of a goal reaches, in bench terms:
//! rails, columns, holes, part leads, board pins) and live simulator state (LED currents,
//! damage, meter readings and dial, pin voltages and frequencies, serial output). It knows the
//! common beginner mistakes in general — not per lesson:
//!
//! * a jumper that ends where nothing else is plugged in (wrong column, one column off, the
//!   other half of the board, only as far as a rail), a wire end not plugged into anything;
//! * a part not in holes (over the centre gap), both leads of a part in one strip (shorted), a
//!   part bypassed by a jumper, an LED in backwards, a supply shorted to GND;
//! * an open loop (and where it's open), a current too small to see, a burnt-out part, the
//!   bench paused;
//! * meters: the wrong dial setting, the wrong jack, wired in parallel where the current must
//!   flow through it (series), still settling;
//! * values out of range (`set_prop` / `choose` steps), sketches that don't compile, aren't
//!   loaded or blink at the wrong rate.
//!
//! Everything is named with [`crate::names::Namer`] (`USB +5 V`, `R1's lead in column 5`).

use crate::check::{self, CheckResult, range, reading_unit, sentence};
use crate::format::{Action, Check, Step};
use crate::guide::{self, control};
use crate::names::{self, Namer, Occupant, split};
use crate::session::StepState;
use bb_circuit::breadboard::{self, Hole};
use bb_circuit::connectivity::{Connectivity, NetId, TermRef};
use bb_circuit::part::{Layout, PinRole};
use bb_circuit::project::Endpoint;
use bb_circuit::{Library, Project};
use bb_sim::Bench;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};

/// How a mark is drawn on the bench.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tone {
    /// Already right: a green glow (what the goal's source already reaches).
    Ok,
    /// Where to go: a pulsing amber ring.
    Target,
    /// Something wrong here: a red marker with a tooltip.
    Wrong,
    /// Take this away (a wire to unplug, a part to remove).
    Remove,
    /// Look here (a part or control involved).
    Focus,
}

/// Where a mark is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Spot {
    /// A pin of a part or a breadboard hole.
    Pin {
        part: String,
        pin: String,
    },
    Part(String),
    Wire(String),
    /// A control of the step panel ([`crate::guide::control`] ids).
    Control(String),
}

/// A highlight on the bench or in the step panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    pub spot: Spot,
    pub tone: Tone,
    /// Tooltip / short label (`nothing else is in column 11`).
    pub note: String,
}

impl Mark {
    pub fn pin(t: &TermRef, tone: Tone, note: impl Into<String>) -> Mark {
        Mark {
            spot: Spot::Pin {
                part: t.part.clone(),
                pin: t.pin.clone(),
            },
            tone,
            note: note.into(),
        }
    }
    pub fn part(id: &str, tone: Tone, note: impl Into<String>) -> Mark {
        Mark {
            spot: Spot::Part(id.into()),
            tone,
            note: note.into(),
        }
    }
    pub fn wire(id: &str, tone: Tone, note: impl Into<String>) -> Mark {
        Mark {
            spot: Spot::Wire(id.into()),
            tone,
            note: note.into(),
        }
    }
    pub fn control(id: impl Into<String>, note: impl Into<String>) -> Mark {
        Mark {
            spot: Spot::Control(id.into()),
            tone: Tone::Target,
            note: note.into(),
        }
    }
}

/// The kind of mistake found (for tests, and for picking an icon).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mistake {
    /// A part the goal needs isn't on the bench.
    MissingPart,
    /// Nothing connects the two sides yet.
    NotWired,
    /// A jumper ends in a column where nothing else is plugged in.
    WrongColumn,
    /// … one column off.
    OneColumnOff,
    /// … in the right column but the other half of the board (across the centre gap).
    OtherHalf,
    /// A side reaches a rail and stops there.
    RailOnly,
    /// A jumper reaches other parts, not the goal's.
    WrongPlace,
    /// A wire end isn't plugged into anything.
    LooseEnd,
    /// A part's lead isn't in a hole (over the centre gap, between holes).
    NotInHole,
    /// Both leads of a part in one strip: the strip shorts it out.
    ShortedPart,
    /// A jumper connects both ends of a part.
    Bypassed,
    /// An LED is in backwards.
    Reversed,
    /// A supply's + is wired to its GND.
    SupplyShort,
    /// Two pins that must be apart are still connected.
    StillConnected,
    /// The loop is open (and where).
    OpenLoop,
    /// An LED carries too little current to see.
    TooDim,
    /// An LED is lit that should be dark.
    StillLit,
    /// A part burnt out.
    BurntOut,
    /// The bench isn't running.
    Paused,
    /// The meter's dial is on the wrong setting.
    MeterMode,
    /// The meter's lead is in the wrong jack (or no jack).
    MeterJack,
    /// The meter is in parallel where the current must flow through it.
    MeterParallel,
    /// Both meter leads on one net.
    MeterShorted,
    /// The meter is still auto-ranging.
    Settling,
    /// A value outside the step's range.
    WrongValue,
    /// A reading / voltage / result out of range.
    OutOfRange,
    /// The sketch isn't on the board yet.
    NoSketch,
    /// The sketch doesn't compile.
    CompileError,
    /// A pin switches at the wrong rate.
    WrongRate,
    /// A pin never changes.
    NeverChanges,
    /// Not done yet (nothing specific is wrong).
    NotYet,
}

/// A one-click fix the step panel can offer with a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fix {
    Repair,
    Run,
}

/// One thing that's wrong.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: Mistake,
    /// What's wrong and where (`Your red jumper reaches column 11, but R1's lead is in column 5.`).
    pub what: String,
    /// The next action (`Move it to any hole a5–e5.`).
    pub fix: String,
    pub marks: Vec<Mark>,
    pub action: Option<Fix>,
}

impl Finding {
    fn new(kind: Mistake, what: impl Into<String>, fix: impl Into<String>) -> Finding {
        Finding {
            kind,
            what: what.into(),
            fix: fix.into(),
            marks: Vec::new(),
            action: None,
        }
    }
    fn marks(mut self, m: Vec<Mark>) -> Finding {
        self.marks = m;
        self
    }
    fn action(mut self, a: Fix) -> Finding {
        self.action = Some(a);
        self
    }
    /// `what` and `fix` as one paragraph.
    pub fn text(&self) -> String {
        if self.fix.is_empty() {
            self.what.clone()
        } else {
            format!("{} {}", self.what, self.fix)
        }
    }
}

/// One goal with its explanation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalReport {
    pub label: String,
    pub ok: bool,
    pub detail: String,
    pub finding: Option<Finding>,
}

/// The result of "Test circuit".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Diagnosis {
    pub goals: Vec<GoalReport>,
    /// Mistakes on the bench not tied to one goal (a loose wire end, a shorted supply …).
    pub extra: Vec<Finding>,
    pub passed: bool,
    /// When every goal passes: `Loop closed — D1 is lit at 13.6 mA`.
    pub payoff: String,
}

impl Diagnosis {
    /// Every finding (goals first).
    pub fn findings(&self) -> impl Iterator<Item = &Finding> {
        self.goals
            .iter()
            .filter_map(|g| g.finding.as_ref())
            .chain(self.extra.iter())
    }
    /// Every mark of every finding.
    pub fn marks(&self) -> Vec<Mark> {
        self.findings().flat_map(|f| f.marks.clone()).collect()
    }
    /// `2 of 3 goals met`.
    pub fn summary(&self) -> String {
        let ok = self.goals.iter().filter(|g| g.ok).count();
        if self.passed {
            "All goals met".into()
        } else if self.goals.len() == 1 {
            "Not yet".into()
        } else {
            format!("{ok} of {} goals met", self.goals.len())
        }
    }
}

/// What the app knows that the bench doesn't.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The bench is running (a paused bench can't light anything).
    pub running: bool,
    /// Errors of the last compile of the student's sketch (`line 5: expected ';'`).
    pub compile_errors: Vec<String>,
}

impl Options {
    pub fn running() -> Options {
        Options {
            running: true,
            ..Default::default()
        }
    }
}

/// Diagnoses the step on the circuit (and the running bench).
pub fn diagnose(
    step: &Step,
    project: &Project,
    lib: &Library,
    bench: Option<&mut Bench>,
    state: &StepState,
    opts: &Options,
) -> Diagnosis {
    let conn = Connectivity::compute(project, lib);
    diagnose_with(step, project, lib, bench, state, opts, &conn)
}

/// The success line for passed goals: the step's `success` (a template over the step's
/// variables and live values) and the live values (`Loop closed — D1 is lit at 13.6 mA`).
pub fn payoff(step: &Step, n: &Namer, goals: &[CheckResult]) -> String {
    let mut vars = crate::text::Vars::default();
    for (k, v) in &step.vars {
        vars.set(k, *v);
    }
    let mut live = Vec::new();
    for g in goals.iter() {
        let Some(c) = g.check.and_then(|i| step.checks.get(i)) else {
            continue;
        };
        match c {
            Check::LedLit { part, .. } => {
                if let Some(ma) = g
                    .detail
                    .strip_suffix(" mA")
                    .and_then(|x| x.parse::<f64>().ok())
                {
                    vars.set(&format!("{part}.ma"), ma);
                }
                live.push(format!("{} is lit at {}", n.reference(part), g.detail));
            }
            Check::LedOff { part } => live.push(format!("{} is dark", n.reference(part))),
            Check::Damaged { part } => live.push(format!("{} is gone", n.reference(part))),
            Check::Reading { part, .. } => {
                if let Some(x) = g
                    .detail
                    .split(' ')
                    .next()
                    .and_then(|x| x.parse::<f64>().ok())
                {
                    vars.set(part, x);
                }
                live.push(format!("{} reads {}", n.part_short(part), g.detail));
            }
            Check::Voltage { a, .. } => live.push(format!("{} is at {}", n.pin_str(a), g.detail)),
            Check::PinFrequency { mcu, pin, .. } => {
                live.push(format!("{} switches at {}", n.pin(mcu, pin), g.detail))
            }
            Check::PinDuty { mcu, pin, .. } => live.push(format!(
                "{} is high {} of the time",
                n.pin(mcu, pin),
                g.detail
            )),
            Check::Spice { vector, .. } => {
                live.push(format!("{} is {}", vector.to_uppercase(), g.detail))
            }
            _ => {}
        }
    }
    let live = live.into_iter().next().unwrap_or_default();
    let own = crate::text::render(step.success.trim(), &vars);
    match (own.is_empty(), live.is_empty()) {
        (false, false) if !step.success.contains('{') => format!("{own} — {live}"),
        (false, _) => own,
        (true, false) => sentence(&live),
        (true, true) => "Step complete".into(),
    }
}

/// [`diagnose`] with the circuit's connectivity already computed.
pub fn diagnose_with(
    step: &Step,
    project: &Project,
    lib: &Library,
    mut bench: Option<&mut Bench>,
    state: &StepState,
    opts: &Options,
    conn: &Connectivity,
) -> Diagnosis {
    let results = check::evaluate_step_with(step, project, lib, bench.as_deref_mut(), state, conn);
    let n = Namer::new(project, lib, conn);
    let passed = !results.is_empty() && results.iter().all(|r| r.ok);
    let mut doc = Doctor {
        step,
        n,
        project,
        lib,
        conn,
        bench,
        state,
        opts,
    };
    let scan = doc.scan();
    let mut used: BTreeSet<usize> = BTreeSet::new();
    let mut goals = Vec::new();
    for r in &results {
        let finding = if r.ok {
            None
        } else {
            let mut f = match r.check.and_then(|i| step.checks.get(i)) {
                Some(c) => doc.explain(c, r, &scan, &mut used),
                None => doc.explain_implied(r),
            };
            if f.fix.is_empty() {
                if let Some(m) = guide::first_move(step, &doc.n) {
                    f.fix = format!("Next: {}.", lower_first(&m.text));
                    f.marks.extend(m.marks);
                } else {
                    let wait: f64 = step
                        .solution
                        .iter()
                        .map(|a| match a {
                            Action::Run { seconds } => *seconds,
                            _ => 0.0,
                        })
                        .sum::<f64>()
                        + step.run;
                    f.fix = if wait > 0.0 {
                        format!(
                            "Give it time: let the bench run about {} s more.",
                            crate::text::smart(wait.max(0.5))
                                .trim_end_matches('0')
                                .trim_end_matches('.')
                        )
                    } else {
                        "Use Show me to see what this step needs.".into()
                    };
                }
            }
            Some(f)
        };
        goals.push(GoalReport {
            label: r.label.clone(),
            ok: r.ok,
            detail: r.detail.clone(),
            finding,
        });
    }
    let extra = scan
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !used.contains(i))
        .map(|(_, f)| f)
        .collect();
    let payoff = if passed {
        payoff(step, &doc.n, &results)
    } else {
        String::new()
    };
    Diagnosis {
        goals,
        extra,
        passed,
        payoff,
    }
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Where a goal's other end is, for messages and marks.
struct Target {
    /// `R1's lead is in column 5`, `the goal is UNO pin D13`.
    is: String,
    /// `any hole a5–e5`, or the pin's name.
    to: String,
    /// Holes / pins to pulse.
    spots: Vec<TermRef>,
    /// (breadboard, column, top half) when the target is a terminal strip.
    column: Option<(String, u32, bool)>,
}

struct Doctor<'a> {
    step: &'a Step,
    n: Namer<'a>,
    project: &'a Project,
    lib: &'a Library,
    conn: &'a Connectivity,
    bench: Option<&'a mut Bench>,
    state: &'a StepState,
    opts: &'a Options,
}

fn term(s: &str) -> TermRef {
    let (p, n) = split(s);
    TermRef::new(p, n)
}

fn strip_column(h: &Hole) -> Option<(u32, bool)> {
    match *h {
        Hole::Strip { row, column } => Some((
            column,
            breadboard::ROWS.iter().position(|&r| r == row).unwrap_or(0) < 5,
        )),
        Hole::Rail { .. } => None,
    }
}

impl<'a> Doctor<'a> {
    // ------------------------------------------------------------------ circuit facts

    fn net(&self, t: &TermRef) -> Option<NetId> {
        self.conn.net_of(&t.part, &t.pin)
    }

    fn exists(&self, t: &TermRef) -> bool {
        self.n.def(&t.part).is_some_and(|d| d.pin(&t.pin).is_some())
    }

    /// Every pin and hole on a net (for green glows).
    fn net_terms(&self, net: NetId) -> Vec<TermRef> {
        let n = self.conn.net(net);
        n.pins.iter().chain(n.holes.iter()).cloned().collect()
    }

    fn ok_marks(&self, net: Option<NetId>, note: &str) -> Vec<Mark> {
        net.map(|n| {
            self.net_terms(n)
                .iter()
                .map(|t| Mark::pin(t, Tone::Ok, note))
                .collect()
        })
        .unwrap_or_default()
    }

    /// Wires with an end on a net.
    fn wires_on(&self, net: NetId) -> Vec<(String, TermRef, Endpoint)> {
        let mut out = Vec::new();
        for w in &self.project.wires {
            for (e, other) in [(&w.a, &w.b), (&w.b, &w.a)] {
                if let Endpoint::Pin { part, pin } = e
                    && self.conn.net_of(part, pin) == Some(net)
                {
                    out.push((w.id.clone(), TermRef::new(part, pin), other.clone()));
                }
            }
        }
        out
    }

    fn is_supply_pin(&self, t: &TermRef) -> bool {
        self.n.def(&t.part).is_some_and(|d| {
            (d.bench.off_board || d.is_mcu() || matches!(d.bench.layout, Some(Layout::Board(_))))
                && d.pin(&t.pin)
                    .is_some_and(|p| matches!(p.role, PinRole::PowerOut | PinRole::Ground))
        })
    }

    /// Wire ends on a net that land in a strip where nothing else is plugged in (`wire`, the
    /// hole). Wires from a supply or board pin to a rail don't count (powering a rail is fine).
    fn dead_ends(&self, net: NetId) -> Vec<(String, TermRef)> {
        let mut out = Vec::new();
        for w in &self.project.wires {
            for (e, other) in [(&w.a, &w.b), (&w.b, &w.a)] {
                let (Endpoint::Pin { part, pin }, Endpoint::Pin { part: op, pin: on }) = (e, other)
                else {
                    continue;
                };
                // the end in question is the one *away* from the net
                if self.conn.net_of(op, on) != Some(net) || !self.n.is_breadboard(part) {
                    continue;
                }
                let hole = TermRef::new(part, pin);
                if self.is_supply_pin(&TermRef::new(op, on))
                    && matches!(breadboard::parse_hole(pin), Some(Hole::Rail { .. }))
                {
                    continue;
                }
                let others = self
                    .n
                    .occupants(&hole)
                    .into_iter()
                    .filter(|o| !matches!(o, Occupant::WireEnd { wire, .. } if *wire == w.id))
                    .count();
                if others == 0 {
                    out.push((w.id.clone(), hole));
                }
            }
        }
        out
    }

    /// Wires whose removal separates `a` and `b` (the ones the step's solution unplugs first).
    fn bridges(&self, a: &TermRef, b: &TermRef) -> Vec<String> {
        let Some(net) = self.net(a) else {
            return Vec::new();
        };
        let mut out: Vec<String> = Vec::new();
        for w in &self.project.wires {
            let on = [&w.a, &w.b].iter().any(|e| match e {
                Endpoint::Pin { part, pin } => self.conn.net_of(part, pin) == Some(net),
                _ => false,
            });
            if !on {
                continue;
            }
            let mut p = self.project.clone();
            p.wires.retain(|x| x.id != w.id);
            let c = Connectivity::compute(&p, self.lib);
            if !c.connected((&a.part, &a.pin), (&b.part, &b.pin)) {
                out.push(w.id.clone());
            }
        }
        let preferred: Vec<String> = self
            .step
            .solution
            .iter()
            .filter_map(|x| match x {
                Action::Unwire { a, b } => {
                    let (ea, eb) = (
                        Endpoint::pin(split(a).0, split(a).1),
                        Endpoint::pin(split(b).0, split(b).1),
                    );
                    self.project
                        .wires
                        .iter()
                        .find(|w| (w.a == ea && w.b == eb) || (w.a == eb && w.b == ea))
                        .map(|w| w.id.clone())
                }
                _ => None,
            })
            .collect();
        // the solution's wire first, then one joining + to − (a red lead into the − rail),
        // then plain breadboard jumpers, then supply leads
        out.sort_by_key(|w| {
            let supply = self.project.wire(w).is_some_and(|x| {
                [&x.a, &x.b].iter().any(|e| match e {
                    Endpoint::Pin { part, pin } => self.is_supply_pin(&TermRef::new(part, pin)),
                    _ => false,
                })
            });
            let crossing = self.project.wire(w).is_some_and(|x| {
                let (pa, pb) = (self.polarity(&x.a), self.polarity(&x.b));
                pa.is_some() && pb.is_some() && pa != pb
            });
            (!preferred.contains(w), !crossing, supply)
        });
        out
    }

    /// `Some(true)` for a supply's + pin or a + rail hole, `Some(false)` for GND / a − rail.
    fn polarity(&self, e: &Endpoint) -> Option<bool> {
        let Endpoint::Pin { part, pin } = e else {
            return None;
        };
        if self.n.is_breadboard(part) {
            return match breadboard::parse_hole(pin) {
                Some(Hole::Rail { rail, .. }) => Some(rail.is_plus()),
                _ => None,
            };
        }
        let d = self.n.def(part)?;
        match d.pin(pin)?.role {
            PinRole::PowerOut => Some(true),
            PinRole::Ground => Some(false),
            _ => None,
        }
    }

    /// Where `t` is, as a goal target.
    fn target(&self, t: &TermRef) -> Target {
        let hole = if self.n.is_breadboard(&t.part) {
            Some(t.clone())
        } else {
            self.n
                .socket(&t.part, &t.pin)
                .filter(|h| self.n.is_breadboard(&h.part))
                .cloned()
        };
        match hole {
            Some(h) => {
                let mut spots = self.n.free_holes(&h);
                if spots.is_empty() {
                    spots.push(h.clone());
                }
                let place = self.n.place(&h);
                let range = self.n.strip_range(&h);
                let column = self
                    .n
                    .strip_of(&h)
                    .and_then(|(bb, _, hh)| strip_column(&hh).map(|(c, top)| (bb, c, top)));
                let is = if self.n.is_breadboard(&t.part) {
                    format!("the goal is {place}")
                } else {
                    let name = self.n.pin(&t.part, &t.pin);
                    // `R1's lead in column 5` → `R1's lead is in column 5`
                    match name.split_once(" in column") {
                        Some((who, _)) => format!("{who} is in {place}"),
                        None => format!("{name} is in {place}"),
                    }
                };
                let to = if range.starts_with("the ") {
                    format!("any hole on {range}")
                } else {
                    format!("any hole {range}")
                };
                Target {
                    is,
                    to,
                    spots,
                    column,
                }
            }
            None => {
                let name = self.n.pin(&t.part, &t.pin);
                Target {
                    is: format!("the goal is {name}"),
                    to: name,
                    spots: vec![t.clone()],
                    column: None,
                }
            }
        }
    }

    fn target_marks(&self, t: &Target, note: &str) -> Vec<Mark> {
        t.spots
            .iter()
            .map(|s| Mark::pin(s, Tone::Target, note))
            .collect()
    }

    fn bench_v(&mut self, t: &TermRef) -> Option<f64> {
        self.bench
            .as_deref()
            .and_then(|b| b.pin_voltage(&t.part, &t.pin))
    }

    fn reading(&self, part: &str, key: &str) -> Option<f64> {
        self.bench.as_deref().and_then(|b| {
            b.readings(part)
                .into_iter()
                .find(|(k, _)| k == key)
                .map(|x| x.1)
        })
    }

    /// The meter's dial (bench reading `mode`, else the part's `mode` property).
    fn dial(&self, part: &str) -> Option<usize> {
        if let Some(m) = self.reading(part, "mode") {
            return Some(m.round().max(0.0) as usize);
        }
        let p = self.project.part(part)?;
        let d = self.n.def(part)?;
        let mode = bb_circuit::Project::prop(&d, p, "mode")?.as_text();
        [
            "off",
            "ac_v",
            "dc_v",
            "mv",
            "ohm",
            "continuity",
            "cap",
            "a",
            "ma",
            "hz",
        ]
        .iter()
        .position(|m| *m == mode)
    }

    // ------------------------------------------------------------------ loop analysis

    /// Nets joined through parts (not the excluded one): adjacency of net ids with the part.
    fn neighbours(&self, net: NetId, exclude: &str) -> Vec<(NetId, String)> {
        let mut out = Vec::new();
        for p in &self.project.parts {
            if p.id == exclude || self.n.is_breadboard(&p.id) {
                continue;
            }
            let Some(d) = self.n.def(&p.id) else { continue };
            // supplies are where loops start and end, not a way through
            if self.n.is_meter(&p.id) {
                // a meter is a path only between its current jack and COM in current mode
                let jack = match self.dial(&p.id) {
                    Some(7) => "A",
                    Some(8) => "MA",
                    _ => continue,
                };
                let (j, c) = (
                    self.conn.net_of(&p.id, jack),
                    self.conn.net_of(&p.id, "COM"),
                );
                if let (Some(j), Some(c)) = (j, c) {
                    if j == net && c != net {
                        out.push((c, p.id.clone()));
                    } else if c == net && j != net {
                        out.push((j, p.id.clone()));
                    }
                }
                continue;
            }
            if d.pins
                .iter()
                .any(|x| matches!(x.role, PinRole::PowerOut | PinRole::Ground))
                && d.bench.off_board
            {
                continue;
            }
            let nets: Vec<NetId> = d
                .pins
                .iter()
                .filter_map(|x| self.conn.net_of(&p.id, &x.id))
                .collect();
            if nets.contains(&net) {
                for m in nets {
                    if m != net && !out.iter().any(|(x, _)| *x == m) {
                        out.push((m, p.id.clone()));
                    }
                }
            }
        }
        out
    }

    /// Nets that source current (supply +, board 5V/3V3, MCU outputs).
    fn is_source(&self, net: NetId) -> bool {
        self.conn.net(net).pins.iter().any(|t| {
            self.n.def(&t.part).is_some_and(|d| {
                d.pin(&t.pin).is_some_and(|p| {
                    p.role == PinRole::PowerOut
                        || (d.is_mcu() || matches!(d.bench.layout, Some(Layout::Board(_))))
                            && matches!(p.role, PinRole::Io | PinRole::Output)
                })
            })
        })
    }

    fn is_ground(&self, net: NetId) -> bool {
        let n = self.conn.net(net);
        n.name == "0"
            || n.pins.iter().any(|t| {
                self.n
                    .def(&t.part)
                    .is_some_and(|d| d.pin(&t.pin).is_some_and(|p| p.role == PinRole::Ground))
            })
    }

    /// Breadth-first search from a net to one that `goal` accepts, not through `exclude`.
    /// Returns (found, the nets reached in order).
    fn search(
        &self,
        from: NetId,
        exclude: &str,
        goal: impl Fn(NetId) -> bool,
    ) -> (bool, Vec<NetId>) {
        let mut seen = vec![from];
        let mut q = VecDeque::from([from]);
        while let Some(n) = q.pop_front() {
            if goal(n) {
                return (true, seen);
            }
            for (m, _) in self.neighbours(n, exclude) {
                if !seen.contains(&m) {
                    seen.push(m);
                    q.push_back(m);
                }
            }
        }
        (false, seen)
    }

    /// The farthest net on an open side with a single component pin (where the loop stops).
    fn open_end(&self, reached: &[NetId], exclude: &str) -> Option<TermRef> {
        reached
            .iter()
            .rev()
            .find(|n| {
                self.conn
                    .net(**n)
                    .pins
                    .iter()
                    .filter(|t| !self.n.is_meter(&t.part))
                    .count()
                    == 1
            })
            .and_then(|n| self.conn.net(*n).pins.first().cloned())
            .or_else(|| {
                reached.first().and_then(|n| {
                    self.conn
                        .net(*n)
                        .pins
                        .iter()
                        .find(|t| t.part == exclude)
                        .cloned()
                })
            })
    }

    /// The first supply + pin on the bench (`USB +5 V`).
    fn source_pin(&self) -> Option<TermRef> {
        self.project.parts.iter().find_map(|p| {
            let d = self.n.def(&p.id)?;
            if !d.bench.off_board {
                return None;
            }
            d.pins
                .iter()
                .find(|x| x.role == PinRole::PowerOut)
                .map(|x| TermRef::new(&p.id, &x.id))
        })
    }

    fn ground_pin(&self) -> Option<TermRef> {
        self.project.parts.iter().find_map(|p| {
            let d = self.n.def(&p.id)?;
            if !d.bench.off_board || !d.pins.iter().any(|x| x.role == PinRole::PowerOut) {
                return None;
            }
            d.pins
                .iter()
                .find(|x| x.role == PinRole::Ground)
                .map(|x| TermRef::new(&p.id, &x.id))
        })
    }

    /// Free holes on a net's rails (for "add a jumper to the − rail").
    fn rail_target(&self, net: NetId) -> Option<Target> {
        let h = self
            .conn
            .net(net)
            .holes
            .iter()
            .find(|h| matches!(breadboard::parse_hole(&h.pin), Some(Hole::Rail { .. })))?
            .clone();
        Some(self.target(&h))
    }

    // ------------------------------------------------------------------ the bench scan

    /// Mistakes anywhere on the bench.
    fn scan(&mut self) -> Vec<Finding> {
        let mut out = Vec::new();
        // a wire end not plugged into anything
        for w in &self.project.wires {
            let loose = [&w.a, &w.b].iter().any(|e| match e {
                Endpoint::Free { .. } => true,
                Endpoint::Pin { part, pin } => !self.exists(&TermRef::new(part, pin)),
            });
            if loose {
                out.push(
                    Finding::new(
                        Mistake::LooseEnd,
                        format!(
                            "One end of {} isn't plugged into anything.",
                            self.n.wire_short(&w.id)
                        ),
                        "Drag that end onto a hole or pin, or delete the jumper.",
                    )
                    .marks(vec![Mark::wire(&w.id, Tone::Wrong, "loose end")]),
                );
            }
        }
        // supplies shorted
        for p in &self.project.parts {
            let Some(d) = self.n.def(&p.id) else { continue };
            if !d.bench.off_board {
                continue;
            }
            let plus = d.pins.iter().find(|x| x.role == PinRole::PowerOut);
            let gnd = d.pins.iter().find(|x| x.role == PinRole::Ground);
            let (Some(plus), Some(gnd)) = (plus, gnd) else {
                continue;
            };
            let (tp, tg) = (TermRef::new(&p.id, &plus.id), TermRef::new(&p.id, &gnd.id));
            if self.net(&tp).is_some() && self.net(&tp) == self.net(&tg) {
                let culprits = self.bridges(&tp, &tg);
                let mut marks = vec![
                    Mark::pin(&tp, Tone::Wrong, "shorted to GND"),
                    Mark::pin(&tg, Tone::Wrong, "shorted to +"),
                ];
                let fix = match culprits.first() {
                    Some(w) => {
                        marks.push(Mark::wire(w, Tone::Remove, "this jumper shorts the supply"));
                        format!("Remove {}.", self.n.wire(w))
                    }
                    None => "Find the jumper that joins + and GND and remove it.".into(),
                };
                out.push(
                    Finding::new(
                        Mistake::SupplyShort,
                        format!(
                            "{} is wired straight to {}: a short circuit. The supply's fuse cuts the power.",
                            self.n.pin(&tp.part, &tp.pin),
                            self.n.pin(&tg.part, &tg.pin)
                        ),
                        fix,
                    )
                    .marks(marks),
                );
            }
        }
        // parts: not in holes, shorted by a strip, bypassed by a jumper
        for p in &self.project.parts {
            let Some(d) = self.n.def(&p.id) else { continue };
            if d.bench.off_board || d.bench.sockets || d.is_breadboard() || d.pins.len() != 2 {
                continue;
            }
            let leads: Vec<TermRef> = d.pins.iter().map(|x| TermRef::new(&p.id, &x.id)).collect();
            let sockets: Vec<Option<&TermRef>> = leads
                .iter()
                .map(|l| self.n.socket(&l.part, &l.pin))
                .collect();
            let reference = self.n.reference(&p.id);
            // a lead over the board but in no hole
            if sockets.iter().any(|s| s.is_some()) || self.over_a_board(&p.id) {
                for (l, s) in leads.iter().zip(sockets.iter()) {
                    if s.is_none() {
                        let gap = self.over_centre_gap(&p.id, &l.pin);
                        out.push(
                            Finding::new(
                                Mistake::NotInHole,
                                format!(
                                    "{} isn't in a hole{}.",
                                    self.n.pin(&l.part, &l.pin),
                                    if gap {
                                        " (it sits over the centre gap)"
                                    } else {
                                        ""
                                    }
                                ),
                                format!("Drag {reference} so both leads drop into holes."),
                            )
                            .marks(vec![Mark::part(
                                &p.id,
                                Tone::Wrong,
                                "a lead isn't in a hole",
                            )]),
                        );
                    }
                }
            }
            // both leads in one strip
            if let (Some(Some(a)), Some(Some(b))) = (sockets.first(), sockets.get(1))
                && let (Some((ba, ka, _)), Some((bb, kb, _))) =
                    (self.n.strip_of(a), self.n.strip_of(b))
                && ba == bb
                && ka == kb
            {
                out.push(
                    Finding::new(
                        Mistake::ShortedPart,
                        format!(
                            "Both of {}'s leads are in {}, so the strip connects them and the current goes around {reference}.",
                            reference,
                            self.n.place(a)
                        ),
                        "Move one lead to another column.",
                    )
                    .marks(vec![
                        Mark::part(&p.id, Tone::Wrong, "shorted by the strip"),
                        Mark::pin(a, Tone::Wrong, "same strip"),
                        Mark::pin(b, Tone::Wrong, "same strip"),
                    ]),
                );
                continue;
            }
            // a jumper across the part
            let (na, nb) = (self.net(&leads[0]), self.net(&leads[1]));
            if na.is_some() && na == nb {
                let culprits = self.bridges(&leads[0], &leads[1]);
                if let Some(w) = culprits.first() {
                    out.push(
                        Finding::new(
                            Mistake::Bypassed,
                            format!(
                                "{} connects both ends of {reference}, so the current skips it.",
                                sentence(&self.n.wire(w))
                            ),
                            "Remove that jumper.",
                        )
                        .marks(vec![
                            Mark::wire(w, Tone::Remove, format!("bypasses {reference}")),
                            Mark::part(&p.id, Tone::Focus, "bypassed"),
                        ]),
                    );
                }
            }
        }
        // jumpers that end where nothing else is plugged in
        let mut seen = BTreeSet::new();
        for net in 0..self.conn.nets.len() {
            for (w, hole) in self.dead_ends(NetId(net)) {
                if seen.insert((w.clone(), hole.clone())) {
                    out.push(
                        Finding::new(
                            Mistake::WrongColumn,
                            format!(
                                "{} ends in {}, where nothing else is plugged in.",
                                sentence(&self.n.wire_short(&w)),
                                self.n.place(&hole)
                            ),
                            "Move that end next to the part it should reach.",
                        )
                        .marks(vec![Mark::pin(
                            &hole,
                            Tone::Wrong,
                            format!("nothing else is in {}", self.n.place(&hole)),
                        )]),
                    );
                }
            }
        }
        // LEDs in backwards (with a running bench)
        let leds: Vec<String> = self
            .project
            .parts
            .iter()
            .filter(|p| p.part == "led")
            .map(|p| p.id.clone())
            .collect();
        for id in leds {
            if let Some(f) = self.reversed(&id) {
                out.push(f);
            }
        }
        out
    }

    /// Whether a part's body lies over a breadboard.
    fn over_a_board(&self, part: &str) -> bool {
        let Some(at) = self
            .project
            .part(part)
            .and_then(|p| p.bench.as_ref())
            .map(|b| (b.at.x, b.at.y))
        else {
            return false;
        };
        self.project.parts.iter().any(|b| {
            let Some(d) = self.n.def(&b.id) else {
                return false;
            };
            let Some(Layout::Breadboard { columns, .. }) = d.bench.layout else {
                return false;
            };
            let Some(bp) = b.bench.as_ref() else {
                return false;
            };
            let (w, h) = breadboard::size(columns);
            let (rx, ry) = (at.0 - bp.at.x, at.1 - bp.at.y);
            (0.0..=w).contains(&rx) && (0.0..=h).contains(&ry)
        })
    }

    /// A lead between rows e and f (the centre channel).
    fn over_centre_gap(&self, part: &str, pin: &str) -> bool {
        let Some((d, p)) = self.n.def(part).zip(self.project.part(part)) else {
            return false;
        };
        let Some((_, x, y)) = Project::bench_leads(&d, p)
            .unwrap_or_default()
            .into_iter()
            .find(|l| l.0 == pin)
        else {
            return false;
        };
        self.project.parts.iter().any(|b| {
            self.n.is_breadboard(&b.id)
                && b.bench.as_ref().is_some_and(|bp| {
                    let ry = y - bp.at.y;
                    let rx = x - bp.at.x;
                    rx > 0.0 && ry > 8.2 && ry < 10.8
                })
        })
    }

    fn reversed(&mut self, led: &str) -> Option<Finding> {
        let (a, k) = (TermRef::new(led, "a"), TermRef::new(led, "k"));
        let (va, vk) = (self.bench_v(&a)?, self.bench_v(&k)?);
        if vk - va < 1.0 {
            return None;
        }
        let r = self.n.reference(led);
        Some(
            Finding::new(
                Mistake::Reversed,
                format!(
                    "{} is in backwards: its short leg (−) faces the + side, so no current can flow.",
                    self.n.part(led)
                ),
                format!("Swap {r}'s legs: the long leg (+) goes toward +5 V, the short leg (−) toward GND."),
            )
            .marks(vec![Mark::part(led, Tone::Wrong, "in backwards")]),
        )
    }

    // ------------------------------------------------------------------ goals

    fn take(
        &self,
        scan: &[Finding],
        used: &mut BTreeSet<usize>,
        pred: impl Fn(&Finding) -> bool,
    ) -> Option<Finding> {
        let i = scan.iter().position(&pred)?;
        used.insert(i);
        Some(scan[i].clone())
    }

    fn explain(
        &mut self,
        c: &Check,
        r: &CheckResult,
        scan: &[Finding],
        used: &mut BTreeSet<usize>,
    ) -> Finding {
        match c {
            Check::Connected { a, b } => self.connected(&term(a), &term(b), scan, used),
            Check::NotConnected { a, b } => self.not_connected(&term(a), &term(b)),
            Check::LedLit { part, min_ma } => self.led_lit(part, *min_ma, scan, used),
            Check::LedOff { part } => self.led_off(part),
            Check::Reading {
                part,
                key,
                min,
                max,
            } => self.reading_goal(part, key, *min, *max, r),
            Check::Voltage { a, min, max, .. } => {
                let t = term(a);
                let floating = self
                    .net(&t)
                    .is_some_and(|n| !self.search(n, "", |m| self.is_source(m)).0);
                let what = format!(
                    "{} is at {}; it should be {}.",
                    self.n.pin(&t.part, &t.pin),
                    r.detail,
                    range(*min, *max, "V")
                );
                if floating {
                    let src = self
                        .source_pin()
                        .map(|s| self.n.pin(&s.part, &s.pin))
                        .unwrap_or_else(|| "the supply".into());
                    Finding::new(
                        Mistake::OpenLoop,
                        format!("{what} Nothing connects it to {src} yet."),
                        "",
                    )
                    .marks(vec![Mark::pin(&t, Tone::Focus, "")])
                } else {
                    Finding::new(Mistake::OutOfRange, what, "").marks(vec![Mark::pin(
                        &t,
                        Tone::Focus,
                        "",
                    )])
                }
            }
            Check::Current { part, min, max, .. } => Finding::new(
                Mistake::OutOfRange,
                format!(
                    "{} carries {}; it should be {}.",
                    self.n.part(part),
                    r.detail,
                    range(*min, *max, "A")
                ),
                "",
            )
            .marks(vec![Mark::part(part, Tone::Focus, "")]),
            Check::Prop {
                part,
                prop,
                min,
                max,
            } => {
                let unit = self
                    .n
                    .def(part)
                    .and_then(|d| d.prop(prop).map(|p| p.unit.clone()))
                    .unwrap_or_default();
                let want = self
                    .step
                    .solution
                    .iter()
                    .find_map(|a| match a {
                        Action::SetProp {
                            part: p,
                            prop: k,
                            value,
                        } if p == part && k == prop => match value {
                            toml::Value::Integer(i) => Some(*i as f64),
                            toml::Value::Float(f) => Some(*f),
                            _ => None,
                        },
                        _ => None,
                    })
                    .unwrap_or((min + max) / 2.0);
                let want_s = bb_core::si::format(want, &unit);
                Finding::new(
                    Mistake::WrongValue,
                    format!(
                        "{} is {}; this step needs {}.",
                        self.n.reference(part),
                        r.detail,
                        range(*min, *max, &unit)
                    ),
                    format!(
                        "Set {}'s {prop} to {want_s} with the field below.",
                        self.n.reference(part)
                    ),
                )
                .marks(vec![
                    Mark::control(control::prop(part, prop), format!("{prop} → {want_s}")),
                    Mark::part(part, Tone::Focus, format!("{prop} → {want_s}")),
                ])
            }
            Check::Part { id, def, .. } => {
                let reference = id.clone().unwrap_or_else(|| def.clone()).to_uppercase();
                Finding::new(
                    Mistake::MissingPart,
                    format!(
                        "{} isn't on the bench yet.",
                        lower_first(&r.label).replace(" on the bench", "")
                    ),
                    format!("Press Add {reference} below, then wire it."),
                )
                .marks(vec![Mark::control(control::add(&reference), "")])
            }
            Check::Damaged { part } => {
                let bypass = self.step.choice.as_ref().and_then(|ch| {
                    ch.options
                        .iter()
                        .position(|o| o.bypass)
                        .map(|i| (i, ch.options[i].label.clone()))
                });
                match bypass {
                    Some((i, label)) => Finding::new(
                        Mistake::NotYet,
                        format!("{} is still fine.", self.n.part(part)),
                        format!("Pick {label}: then nothing limits the current."),
                    )
                    .marks(vec![Mark::control(control::choice(i), "")]),
                    None => Finding::new(
                        Mistake::NotYet,
                        format!("{} is still fine.", self.n.part(part)),
                        "",
                    ),
                }
            }
            Check::Healthy { part } => Finding::new(
                Mistake::BurntOut,
                format!("{} burnt out.", self.n.part(part)),
                "Press Repair to put it back.",
            )
            .marks(vec![
                Mark::part(part, Tone::Wrong, "burnt out"),
                Mark::control(control::REPAIR, ""),
            ])
            .action(Fix::Repair),
            Check::SerialContains { mcu, text } => {
                if let Some(f) = self.sketch_problem(mcu) {
                    return f;
                }
                let last = self
                    .bench
                    .as_deref()
                    .and_then(|b| b.serial(mcu))
                    .and_then(|s| s.data_lines().last().map(|l| l.text.trim().to_string()))
                    .filter(|l| !l.is_empty());
                let what = match last {
                    Some(l) => format!(
                        "No “{}” from {} yet; the last line was “{}”.",
                        text.trim(),
                        self.n.part_short(mcu),
                        shorten(&l, 40)
                    ),
                    None => format!(
                        "{} hasn't printed anything yet.",
                        sentence(&self.n.part_short(mcu))
                    ),
                };
                Finding::new(Mistake::NotYet, what, "")
            }
            Check::PinFrequency {
                mcu,
                pin,
                hz,
                tolerance,
            } => self.frequency(mcu, pin, *hz, *tolerance),
            Check::PinDuty { mcu, pin, min, max } => {
                if let Some(f) = self.sketch_problem(mcu) {
                    return f;
                }
                Finding::new(
                    Mistake::OutOfRange,
                    format!(
                        "{} is high {} of the time; the goal is {:.0}–{:.0} %.",
                        self.n.pin(mcu, pin),
                        r.detail,
                        min * 100.0,
                        max * 100.0
                    ),
                    format!(
                        "With analogWrite, {:.0} % is about {:.0} out of 255.",
                        (min + max) * 50.0,
                        (min + max) / 2.0 * 255.0
                    ),
                )
                .marks(vec![Mark::part(mcu, Tone::Focus, "")])
            }
            Check::Firmware { mcu } => self.sketch_problem(mcu).unwrap_or_else(|| {
                Finding::new(
                    Mistake::NoSketch,
                    format!("Your sketch isn't on {} yet.", self.n.part_short(mcu)),
                    "Press Compile & upload.",
                )
                .marks(vec![Mark::control(control::COMPILE, "")])
            }),
            Check::World { key, min, max } => {
                let (name, unit) = check::world_name(key);
                Finding::new(
                    Mistake::OutOfRange,
                    format!(
                        "The {name} is {}; it should be {}.",
                        r.detail,
                        range(*min, *max, &unit)
                    ),
                    format!("Change the {name} with its control below."),
                )
                .marks(vec![Mark::control(control::world(key), "")])
            }
            Check::Mqtt { topic, .. } => Finding::new(
                Mistake::NotYet,
                format!("Nothing has arrived on {topic} yet."),
                "",
            ),
            Check::Phone { contains } => Finding::new(
                Mistake::NotYet,
                format!("The phone hasn't shown “{}” yet.", contains.trim()),
                "",
            ),
            Check::Prediction => match self.state.verdict {
                Some(false) => Finding::new(
                    Mistake::NotYet,
                    "Your prediction isn't right yet.",
                    self.state
                        .hint
                        .clone()
                        .unwrap_or_else(|| "Try again, or open Show me the working.".into()),
                )
                .marks(vec![Mark::control(control::PREDICT, "")]),
                _ => Finding::new(
                    Mistake::NotYet,
                    "No prediction yet.",
                    "Type your answer and press Check.",
                )
                .marks(vec![Mark::control(control::PREDICT, "")]),
            },
            Check::Measurements { min } => self.measurements(*min),
            Check::Quiz => Finding::new(Mistake::NotYet, "Not answered yet.", "Pick an answer."),
            Check::ErcClean { .. } => Finding::new(Mistake::NotYet, format!("{}.", r.detail), ""),
            Check::Spice {
                vector,
                min,
                max,
                stat,
                ..
            } => {
                let unit = if vector.to_ascii_lowercase().starts_with("i(") {
                    "A"
                } else if stat == "mag_db" {
                    "dB"
                } else if stat == "phase_deg" {
                    "°"
                } else {
                    "V"
                };
                let want = if unit == "dB" || unit == "°" {
                    format!("{min}–{max} {unit}")
                } else {
                    range(*min, *max, unit)
                };
                Finding::new(
                    Mistake::OutOfRange,
                    format!(
                        "{} is {}; the goal is {want}.",
                        vector.to_uppercase(),
                        r.detail
                    ),
                    "",
                )
            }
        }
    }

    /// Goals a step kind implies (quiz answer, sketch loaded, measurements).
    fn explain_implied(&mut self, r: &CheckResult) -> Finding {
        if let Some(code) = &self.step.code {
            if !self.opts.compile_errors.is_empty() {
                return self.compile_error();
            }
            if r.label.starts_with("Your sketch") {
                return Finding::new(
                    Mistake::NoSketch,
                    format!("Your sketch isn't on {} yet.", self.n.part_short(&code.mcu)),
                    "Press Compile & upload.",
                )
                .marks(vec![Mark::control(control::COMPILE, "")]);
            }
        }
        if let Some(t) = &self.step.table
            && r.label.ends_with("measurements")
        {
            return self.measurements(t.min_rows);
        }
        if self.step.quiz.is_some() {
            return Finding::new(Mistake::NotYet, "Not answered yet.", "Pick an answer.");
        }
        Finding::new(Mistake::NotYet, format!("{}: {}.", r.label, r.detail), "")
    }

    fn measurements(&self, min: usize) -> Finding {
        let n = self.state.rows.len();
        Finding::new(
            Mistake::NotYet,
            format!("{n} of {min} measurements so far."),
            "Pick another value; its row fills in once the meter settles.",
        )
    }

    fn compile_error(&self) -> Finding {
        let first = self
            .opts
            .compile_errors
            .first()
            .cloned()
            .unwrap_or_default();
        Finding::new(
            Mistake::CompileError,
            format!("Your sketch doesn't compile: {}", shorten(&first, 90)),
            "Fix that line, then press Compile & upload again.",
        )
        .marks(vec![Mark::control(control::COMPILE, "")])
    }

    /// The sketch isn't compiled / loaded.
    fn sketch_problem(&self, mcu: &str) -> Option<Finding> {
        let code = self.step.code.as_ref()?;
        if !self.opts.compile_errors.is_empty() {
            return Some(self.compile_error());
        }
        (!self.state.firmware && code.mcu == mcu).then(|| {
            Finding::new(
                Mistake::NoSketch,
                format!("Your sketch isn't on {} yet.", self.n.part_short(mcu)),
                "Press Compile & upload.",
            )
            .marks(vec![Mark::control(control::COMPILE, "")])
        })
    }

    fn frequency(&mut self, mcu: &str, pin: &str, hz: f64, tolerance: f64) -> Finding {
        if let Some(f) = self.sketch_problem(mcu) {
            return f;
        }
        let id = check::probe_id(mcu, pin, "f");
        let f = self.bench.as_deref_mut().and_then(|b| {
            b.telemetry()
                .probes
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.value)
        });
        let half_ms = |f: f64| 1000.0 / (2.0 * f);
        let name = self.n.pin(mcu, pin);
        match f {
            Some(f) if f.abs() > 1e-6 => {
                let ok = (f - hz).abs() <= hz * tolerance;
                if ok {
                    return Finding::new(Mistake::NotYet, "Give it a moment to settle.", "");
                }
                Finding::new(
                    Mistake::WrongRate,
                    format!(
                        "{name} switches at {}; the goal is {} (about {:.0} ms on, {:.0} ms off).",
                        names::hz(f),
                        names::hz(hz),
                        half_ms(hz),
                        half_ms(hz)
                    ),
                    format!(
                        "Your delays add up to about {:.0} ms per half; use delay({:.0}) for each.",
                        half_ms(f),
                        half_ms(hz)
                    ),
                )
                .marks(vec![Mark::part(mcu, Tone::Focus, "")])
            }
            _ => Finding::new(
                Mistake::NeverChanges,
                format!("{name} never changes, so nothing blinks."),
                "Check pinMode(…, OUTPUT) in setup() and that loop() switches the pin HIGH and LOW with a delay in between.",
            )
            .marks(vec![Mark::part(mcu, Tone::Focus, "")]),
        }
    }

    fn connected(
        &mut self,
        a: &TermRef,
        b: &TermRef,
        scan: &[Finding],
        used: &mut BTreeSet<usize>,
    ) -> Finding {
        for t in [a, b] {
            if !self.exists(t) {
                return Finding::new(
                    Mistake::MissingPart,
                    format!("{} isn't on the bench.", self.n.reference(&t.part)),
                    "",
                );
            }
        }
        let (na, nb) = (self.net(a), self.net(b));
        let an = self.n.pin(&a.part, &a.pin);
        let target = self.target(b);
        let mut marks = self.ok_marks(na, &format!("connected to {an}"));
        // a jumper from A's side that stops short of B
        let ends_a = na.map(|n| self.dead_ends(n)).unwrap_or_default();
        let ends_b = nb.map(|n| self.dead_ends(n)).unwrap_or_default();
        let near = |ends: &[(String, TermRef)], t: &Target| -> Option<(String, TermRef)> {
            ends.iter()
                .min_by_key(|(_, h)| {
                    match (
                        self.n.strip_of(h).and_then(|(_, _, hh)| strip_column(&hh)),
                        &t.column,
                    ) {
                        (Some((c, _)), Some((_, tc, _))) => (c as i64 - *tc as i64).unsigned_abs(),
                        (Some(_), None) => 500,
                        _ => 1000,
                    }
                })
                .cloned()
        };
        if let Some((w, hole)) = near(&ends_a, &target) {
            // the generic "ends where nothing else is plugged in" finding is this one
            let _ = self.take(scan, used, |f| {
                f.kind == Mistake::WrongColumn
                    && f.marks.iter().any(|m| {
                        m.spot
                            == Spot::Pin {
                                part: hole.part.clone(),
                                pin: hole.pin.clone(),
                            }
                    })
            });
            return self.stops_short(&w, &hole, &target, marks);
        }
        if let Some((w, hole)) = near(&ends_b, &self.target(a)) {
            let _ = self.take(scan, used, |f| {
                f.kind == Mistake::WrongColumn
                    && f.marks.iter().any(|m| {
                        m.spot
                            == Spot::Pin {
                                part: hole.part.clone(),
                                pin: hole.pin.clone(),
                            }
                    })
            });
            let ta = self.target(a);
            let mut marks = self.ok_marks(nb, "");
            marks.retain(|m| {
                m.spot
                    != Spot::Pin {
                        part: hole.part.clone(),
                        pin: hole.pin.clone(),
                    }
            });
            return self.stops_short(&w, &hole, &ta, marks);
        }
        let wires_a = na.map(|n| self.wires_on(n)).unwrap_or_default();
        let wires_b = nb.map(|n| self.wires_on(n)).unwrap_or_default();
        // A's side reaches other parts, not B's: the jumper end nearest to B
        let mut best: Option<(String, TermRef, u64)> = None;
        for (w, _, other) in &wires_a {
            if let Endpoint::Pin { part, pin } = other {
                let h = TermRef::new(part, pin);
                if let (Some((_, _, hh)), Some((_, tc, _))) = (self.n.strip_of(&h), &target.column)
                    && let Some((c, _)) = strip_column(&hh)
                {
                    let d = (c as i64 - *tc as i64).unsigned_abs();
                    if best.as_ref().is_none_or(|x| d < x.2) {
                        best = Some((w.clone(), h, d));
                    }
                }
            }
        }
        if let Some((w, hole, _)) = best {
            let who: Vec<String> = self
                .n
                .net_pins(&hole.part, &hole.pin)
                .iter()
                .filter(|t| t.part != a.part)
                .map(|t| self.n.pin(&t.part, &t.pin))
                .collect();
            let there = if who.is_empty() {
                String::new()
            } else {
                format!(" ({})", who.join(", "))
            };
            marks.push(Mark::pin(&hole, Tone::Wrong, "not the goal's column"));
            marks.extend(self.target_marks(&target, "here"));
            return Finding::new(
                Mistake::WrongPlace,
                format!(
                    "Your {} reaches {}{there}, but {}.",
                    self.n.wire_short(&w).trim_start_matches("the "),
                    self.n.place(&hole),
                    target.is
                ),
                format!("Move it to {}.", target.to),
            )
            .marks(marks);
        }
        // A's side already reaches a rail (the supply powers it): carry on from the rail
        let a_rail = na.and_then(|n| {
            self.conn
                .net(n)
                .holes
                .iter()
                .find(|h| matches!(breadboard::parse_hole(&h.pin), Some(Hole::Rail { .. })))
                .cloned()
        });
        if let Some(rail) = a_rail {
            let place = self.n.place(&rail);
            marks.extend(self.target_marks(&target, "end here"));
            return Finding::new(
                Mistake::RailOnly,
                format!(
                    "{an} reaches {place}, but nothing connects the rail to {}.",
                    self.n.pin(&b.part, &b.pin)
                ),
                format!("Add a jumper from {place} to {}.", target.to),
            )
            .marks(marks);
        }
        // nothing joins the sides yet; if B's side already reaches a rail, aim A at it
        let aim = if wires_a.is_empty() && !wires_b.is_empty() {
            nb.and_then(|n| self.rail_target(n)).unwrap_or(target)
        } else {
            target
        };
        marks.push(Mark::pin(a, Tone::Target, "start here"));
        marks.extend(self.target_marks(&aim, "and end here"));
        Finding::new(
            Mistake::NotWired,
            format!(
                "Nothing connects {an} to {} yet.",
                self.n.pin(&b.part, &b.pin)
            ),
            format!("Add a jumper from {an} to {}.", aim.to),
        )
        .marks(marks)
    }

    /// A jumper ends in an empty strip instead of reaching the target.
    fn stops_short(
        &self,
        wire: &str,
        hole: &TermRef,
        target: &Target,
        mut marks: Vec<Mark>,
    ) -> Finding {
        let jumper = format!(
            "Your {}",
            self.n.wire_short(wire).trim_start_matches("the ")
        );
        let place = self.n.place(hole);
        marks.retain(|m| {
            m.spot
                != Spot::Pin {
                    part: hole.part.clone(),
                    pin: hole.pin.clone(),
                }
        });
        marks.push(Mark::pin(
            hole,
            Tone::Wrong,
            format!("nothing else is in {place}"),
        ));
        marks.push(Mark::wire(wire, Tone::Focus, ""));
        marks.extend(self.target_marks(target, "move it here"));
        let fix = format!("Move it to {}.", target.to);
        let here = self.n.strip_of(hole).and_then(|(_, _, h)| strip_column(&h));
        let (kind, what) = match (here, &target.column) {
            (None, _) => (
                Mistake::RailOnly,
                format!(
                    "{jumper} reaches {place}, but nothing connects the rail onward: {}.",
                    target.is
                ),
            ),
            (Some((c, top)), Some((_, tc, ttop))) if c == *tc && top != *ttop => (
                Mistake::OtherHalf,
                format!(
                    "{jumper} is in hole {}, across the centre gap from {}. The two halves of the board aren't connected.",
                    hole.pin,
                    target.is.split(" is in ").next().unwrap_or("the goal")
                ),
            ),
            (Some((c, _)), Some((_, tc, _))) if (c as i64 - *tc as i64).abs() == 1 => (
                Mistake::OneColumnOff,
                format!("{jumper} is in {place}, one column off: {}.", target.is),
            ),
            _ => (
                Mistake::WrongColumn,
                format!("{jumper} reaches {place}, but {}.", target.is),
            ),
        };
        let fix = if kind == Mistake::RailOnly {
            format!("Add a jumper from the rail to {}.", target.to)
        } else {
            fix
        };
        Finding::new(kind, what, fix).marks(marks)
    }

    fn not_connected(&self, a: &TermRef, b: &TermRef) -> Finding {
        let (an, bn) = (self.n.pin(&a.part, &a.pin), self.n.pin(&b.part, &b.pin));
        let bridges = self.bridges(a, b);
        match bridges.first() {
            Some(w) => Finding::new(
                Mistake::StillConnected,
                format!("{an} still reaches {bn} through {}.", self.n.wire(w)),
                "Remove that jumper: click it, then press Delete.",
            )
            .marks(vec![Mark::wire(w, Tone::Remove, "remove this jumper")]),
            None => Finding::new(
                Mistake::StillConnected,
                format!("{an} is still connected to {bn}."),
                "",
            ),
        }
    }

    fn led_lit(
        &mut self,
        part: &str,
        min_ma: f64,
        scan: &[Finding],
        used: &mut BTreeSet<usize>,
    ) -> Finding {
        let name = self.n.part(part);
        let r = self.n.reference(part);
        if !self.exists(&TermRef::new(part, "a")) {
            return Finding::new(Mistake::MissingPart, format!("{r} isn't on the bench."), "");
        }
        if let Some(b) = self.bench.as_deref()
            && let bb_core::Health::Damaged { reason } = b.health(part)
        {
            return Finding::new(
                Mistake::BurntOut,
                format!("{name} burnt out ({reason})."),
                "Press Repair, and keep a resistor in its loop.",
            )
            .marks(vec![Mark::part(part, Tone::Wrong, "burnt out")])
            .action(Fix::Repair);
        }
        if !self.opts.running {
            return Finding::new(
                Mistake::Paused,
                "The bench is paused, so no current flows.",
                "Press Run.",
            )
            .action(Fix::Run);
        }
        // a mistake the scan found on this LED
        if let Some(f) = self.take(scan, used, |f| {
            matches!(
                f.kind,
                Mistake::Reversed | Mistake::ShortedPart | Mistake::Bypassed | Mistake::NotInHole
            ) && f.marks.iter().any(|m| m.spot == Spot::Part(part.into()))
        }) {
            return f;
        }
        if let Some(f) = self.meter_across(part) {
            return f;
        }
        if let Some(f) = self.take(scan, used, |f| f.kind == Mistake::SupplyShort) {
            return f;
        }
        let (a, k) = (TermRef::new(part, "a"), TermRef::new(part, "k"));
        let (Some(na), Some(nk)) = (self.net(&a), self.net(&k)) else {
            return Finding::new(Mistake::NotYet, format!("{name} isn't connected."), "");
        };
        // open loop: the + side
        let (plus, reached) = self.search(na, part, |n| self.is_source(n));
        if !plus {
            let src = self.source_pin();
            let src_name = src
                .as_ref()
                .map(|s| self.n.pin(&s.part, &s.pin))
                .unwrap_or_else(|| "the supply's +".into());
            let end = self.open_end(&reached, part).unwrap_or(a.clone());
            let t = self.target(&end);
            let mut marks = self.target_marks(&t, "connect + here");
            if let Some(s) = &src {
                marks.push(Mark::pin(s, Tone::Target, "from here"));
            }
            return Finding::new(
                Mistake::OpenLoop,
                format!(
                    "The loop is open at {}: nothing connects it to {src_name}.",
                    self.n.pin(&end.part, &end.pin)
                ),
                format!("Add a jumper from {src_name} to {}.", t.to),
            )
            .marks(marks);
        }
        let (gnd, reached) = self.search(nk, part, |n| self.is_ground(n));
        if !gnd {
            let end = self.open_end(&reached, part).unwrap_or(k.clone());
            let t = self.target(&end);
            let g = self.ground_pin();
            let rail = g
                .as_ref()
                .and_then(|g| self.net(g))
                .and_then(|n| self.rail_target(n));
            let to = rail
                .as_ref()
                .map(|r| r.to.clone())
                .or_else(|| g.as_ref().map(|g| self.n.pin(&g.part, &g.pin)))
                .unwrap_or_else(|| "GND".into());
            let mut marks = self.target_marks(&t, "from here");
            if let Some(r) = &rail {
                marks.extend(self.target_marks(r, "to GND"));
            }
            return Finding::new(
                Mistake::OpenLoop,
                format!(
                    "The loop is open at {}: nothing connects it back to GND.",
                    self.n.pin(&end.part, &end.pin)
                ),
                format!("Add a jumper from {} to {to}.", t.to),
            )
            .marks(marks);
        }
        // a closed loop with too little current
        let i = self
            .bench
            .as_deref()
            .and_then(|b| b.element_current(part, "D"))
            .unwrap_or(0.0)
            .abs();
        if i > 1e-6 && i * 1e3 < min_ma {
            let resistor = self
                .neighbours(na, part)
                .into_iter()
                .chain(self.neighbours(nk, part))
                .map(|(_, p)| p)
                .find(|p| self.project.part(p).is_some_and(|x| x.part == "resistor"));
            let fix = match resistor {
                Some(rp) => format!(
                    "{} is {}: a smaller resistor (220 Ω–1 kΩ) lets more current through.",
                    self.n.reference(&rp),
                    self.n
                        .part(&rp)
                        .split_once(" (")
                        .map(|x| x.1.trim_end_matches(')').to_string())
                        .unwrap_or_default()
                ),
                None => "Something in its loop limits the current too much.".into(),
            };
            return Finding::new(
                Mistake::TooDim,
                format!(
                    "{name} carries only {}, too little to see (it needs about {} mA).",
                    names::current(i),
                    crate::text::smart(min_ma).trim_end_matches(".00")
                ),
                fix,
            )
            .marks(vec![Mark::part(part, Tone::Focus, "too dim")]);
        }
        Finding::new(
            Mistake::NotYet,
            format!("{name} gets no current yet."),
            "Give the bench a moment, then follow the loop from + through the LED back to GND.",
        )
        .marks(vec![Mark::part(part, Tone::Focus, "")])
    }

    /// A meter in current mode wired across a part (in parallel): it carries the current
    /// instead of the part.
    fn meter_across(&self, part: &str) -> Option<Finding> {
        let d = self.n.def(part)?;
        let nets: Vec<NetId> = d
            .pins
            .iter()
            .filter_map(|x| self.conn.net_of(part, &x.id))
            .collect();
        for m in &self.project.parts {
            if !self.n.is_meter(&m.id) {
                continue;
            }
            let jack = match self.dial(&m.id) {
                Some(7) => "A",
                Some(8) => "MA",
                _ => continue,
            };
            let (j, c) = (
                self.conn.net_of(&m.id, jack),
                self.conn.net_of(&m.id, "COM"),
            );
            if let (Some(j), Some(c)) = (j, c)
                && j != c
                && nets.contains(&j)
                && nets.contains(&c)
            {
                return Some(
                    Finding::new(
                        Mistake::MeterParallel,
                        format!(
                            "The meter is wired across {}, in parallel. On mA it's almost a short, so the current goes through the meter instead of {}.",
                            self.n.part(part),
                            self.n.reference(part)
                        ),
                        "Put the meter in series: break the loop at one spot and let the meter close it (mA jack on one side, COM on the other).",
                    )
                    .marks(vec![
                        Mark::part(&m.id, Tone::Wrong, "in parallel"),
                        Mark::part(part, Tone::Focus, "bypassed by the meter"),
                    ]),
                );
            }
        }
        None
    }

    fn led_off(&mut self, part: &str) -> Finding {
        let name = self.n.part(part);
        let i = self
            .bench
            .as_deref()
            .and_then(|b| b.element_current(part, "D"))
            .unwrap_or(0.0)
            .abs();
        // the jumper whose removal opens the loop (the solution's first)
        let (a, k) = (TermRef::new(part, "a"), TermRef::new(part, "k"));
        let mut culprit = None;
        if let (Some(src), Some(gnd)) = (self.source_pin(), self.ground_pin()) {
            let mut c = self.bridges(&k, &gnd);
            c.extend(self.bridges(&src, &a));
            culprit = c.into_iter().next();
        }
        match culprit {
            Some(w) => Finding::new(
                Mistake::StillLit,
                format!(
                    "{name} is still lit ({}): its loop is still closed through {}.",
                    names::current(i),
                    self.n.wire(&w)
                ),
                "Remove that jumper to open the loop.",
            )
            .marks(vec![
                Mark::wire(&w, Tone::Remove, "remove this jumper"),
                Mark::part(part, Tone::Focus, "still lit"),
            ]),
            None => Finding::new(
                Mistake::StillLit,
                format!("{name} is still lit ({}).", names::current(i)),
                "",
            )
            .marks(vec![Mark::part(part, Tone::Focus, "still lit")]),
        }
    }

    fn reading_goal(
        &mut self,
        part: &str,
        key: &str,
        min: f64,
        max: f64,
        r: &CheckResult,
    ) -> Finding {
        let (scale, unit) = reading_unit(Some(self.step), part).unwrap_or((1.0, String::new()));
        let want = range(min * scale, max * scale, &unit);
        if self.n.is_meter(part) && key == "value" {
            // the dial
            let expected = self
                .step
                .solution
                .iter()
                .find_map(|a| match a {
                    Action::Control {
                        part: p,
                        key: k,
                        value,
                    } if p == part && k == "dial" => Some(value.round() as usize),
                    _ => None,
                })
                .or(match unit.as_str() {
                    "V" => Some(2),
                    "mV" => Some(3),
                    "mA" => Some(8),
                    "A" => Some(7),
                    "Ω" | "kΩ" => Some(4),
                    "Hz" => Some(9),
                    _ => None,
                });
            let dial = self.dial(part);
            if let (Some(e), Some(d)) = (expected, dial)
                && e != d
            {
                let (en, dn) = (guide::DIAL[e.min(9)], guide::DIAL[d.min(9)]);
                let what = if d == 0 {
                    format!("The meter is switched off; this step needs {en}.")
                } else {
                    format!("The meter's dial is on {dn}; this step needs {en}.")
                };
                return Finding::new(
                    Mistake::MeterMode,
                    what,
                    format!("Click the meter's dial until it points at {en}."),
                )
                .marks(vec![Mark::part(
                    part,
                    Tone::Target,
                    format!("dial → {en}"),
                )]);
            }
            let dial = dial.or(expected);
            if matches!(dial, Some(7) | Some(8))
                && let Some(f) = self.meter_series(part, dial == Some(7))
            {
                return f;
            }
            if self.reading(part, "settling").is_some_and(|s| s > 0.5) {
                return Finding::new(
                    Mistake::Settling,
                    "The meter is still finding its range.",
                    "Give it a second.",
                );
            }
            // a voltmeter with a lead nowhere
            for (jack, label) in [("VO", "red (VΩ)"), ("COM", "black (COM)")] {
                if matches!(dial, Some(1..=6) | Some(9))
                    && self.conn.net_of(part, jack).is_some_and(|n| {
                        self.conn.net(n).pins.len() <= 1 && self.conn.net(n).holes.is_empty()
                    })
                {
                    return Finding::new(
                        Mistake::MeterJack,
                        format!("The meter's {label} lead isn't connected to anything."),
                        "Clip it onto the point you want to measure.",
                    )
                    .marks(vec![Mark::pin(
                        &TermRef::new(part, jack),
                        Tone::Target,
                        "",
                    )]);
                }
            }
        }
        Finding::new(
            Mistake::OutOfRange,
            format!(
                "{} reads {}; this step wants {want}.",
                sentence(&self.n.part_short(part)),
                r.detail
            ),
            "",
        )
        .marks(vec![Mark::part(part, Tone::Focus, "")])
    }

    /// A current measurement: the meter must be in the loop (series).
    fn meter_series(&self, part: &str, amps: bool) -> Option<Finding> {
        let jack = if amps { "A" } else { "MA" };
        let jack_name = if amps { "10 A" } else { "mA" };
        let jn = self.conn.net_of(part, jack);
        let cn = self.conn.net_of(part, "COM");
        let alone = |n: Option<NetId>| {
            n.is_none_or(|n| self.conn.net(n).pins.len() <= 1 && self.conn.net(n).holes.is_empty())
        };
        if alone(jn) {
            let vo = self.conn.net_of(part, "VO");
            let in_vo = !alone(vo);
            return Some(
                Finding::new(
                    Mistake::MeterJack,
                    if in_vo {
                        format!("The meter's red lead is in the VΩ jack; current goes through the {jack_name} jack.")
                    } else {
                        format!("Nothing is plugged into the meter's {jack_name} jack.")
                    },
                    format!("Wire the side of the loop you opened to the {jack_name} jack."),
                )
                .marks(vec![Mark::pin(&TermRef::new(part, jack), Tone::Target, "this jack")]),
            );
        }
        let (Some(j), Some(c)) = (jn, cn) else {
            return None;
        };
        if j == c {
            return Some(
                Finding::new(
                    Mistake::MeterShorted,
                    format!("Both meter leads ({jack_name} and COM) are on the same net, so no current flows through the meter."),
                    "Move one lead so the current has to pass through the meter.",
                )
                .marks(vec![Mark::part(part, Tone::Wrong, "both leads on one net")]),
            );
        }
        // something else joins the two nets: the meter is in parallel with it
        for p in &self.project.parts {
            if p.id == part || self.n.is_breadboard(&p.id) {
                continue;
            }
            let Some(d) = self.n.def(&p.id) else { continue };
            let nets: Vec<NetId> = d
                .pins
                .iter()
                .filter_map(|x| self.conn.net_of(&p.id, &x.id))
                .collect();
            if nets.contains(&j) && nets.contains(&c) {
                let supply = d.bench.off_board;
                let what = if supply {
                    format!(
                        "The meter is wired straight across {}: on {jack_name} that's a short circuit.",
                        self.n.part(&p.id)
                    )
                } else {
                    format!(
                        "The meter is wired across {}, in parallel, so the current goes around the meter instead of through it.",
                        self.n.part(&p.id)
                    )
                };
                return Some(
                    Finding::new(
                        Mistake::MeterParallel,
                        what,
                        "Current is measured in series: break the loop at one spot and let the meter close it (mA jack on one side, COM on the other).",
                    )
                    .marks(vec![
                        Mark::part(part, Tone::Wrong, "in parallel"),
                        Mark::part(&p.id, Tone::Focus, "in parallel with the meter"),
                    ]),
                );
            }
        }
        None
    }
}

fn shorten(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n).collect();
        format!("{}…", t.trim_end())
    }
}
