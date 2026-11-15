//! The app's handle on the running bench: creates the [`BenchLink`] for the open project via
//! the registered factory and accumulates what `poll` returns (serial history, latest debug /
//! radio / pin view models, events for the output panel).

use super::*;
use crate::project::Project;
use std::collections::HashMap;

pub struct BenchService {
    factory: BenchFactory,
    /// Bundled prebuilt firmware (passed to new benches).
    pub fixtures: Option<PathBuf>,
    /// Benches play audio (off in tests).
    pub audio: bool,
    link: Option<Box<dyn BenchLink>>,
    project_dir: Option<PathBuf>,
    pub speed: SimSpeed,
    /// Serial history (all targets), oldest first.
    pub serial: Vec<SerialLine>,
    pub debug: HashMap<String, DebugViewModel>,
    pub radio: Option<RadioPanelModel>,
    pub pins: HashMap<String, Vec<PinView>>,
    /// Live probe values (`V(A0)` → volts) for the Pro `.op` table's bench column.
    pub probes: HashMap<String, f64>,
    /// Unconsumed events (screens take them; bounded).
    pub events: Vec<BenchEvent>,
    /// Problems every screen must show (the app turns them into error toasts): a device or
    /// plugin error such as a sandbox permission violation (`PLUG-2`), the simulation stopping.
    pub alerts: Vec<String>,
    alerted: std::collections::HashSet<String>,
    pub running: bool,
    pub sim_time: SimTime,
}

const SERIAL_HISTORY: usize = 20_000;
/// Events kept for a screen that has not taken them yet (the Firmware output).
const EVENT_HISTORY: usize = 500;

impl BenchService {
    pub fn new(factory: BenchFactory, speed: SimSpeed) -> BenchService {
        BenchService {
            factory,
            fixtures: None,
            audio: false,
            link: None,
            project_dir: None,
            speed,
            serial: Vec::new(),
            debug: HashMap::new(),
            radio: None,
            pins: HashMap::new(),
            probes: HashMap::new(),
            events: Vec::new(),
            alerts: Vec::new(),
            alerted: Default::default(),
            running: false,
            sim_time: SimTime::ZERO,
        }
    }

    /// Replaces the factory (the `bb-sim` adapter registers itself here in phase 4).
    pub fn set_factory(&mut self, factory: BenchFactory) {
        self.factory = factory;
        self.shutdown();
    }

    /// The link for `project`, created on first use.
    pub fn link(&mut self, project: Option<&Project>, repaint: &Repaint) -> &mut dyn BenchLink {
        self.link_with(project, None, repaint)
    }

    /// The link for `project`; a new bench simulates `circuit` (the in-memory document) when
    /// given, else the project's `project.json`.
    pub fn link_with(
        &mut self,
        project: Option<&Project>,
        circuit: Option<bb_circuit::Project>,
        repaint: &Repaint,
    ) -> &mut dyn BenchLink {
        let dir = project.map(|p| p.dir.clone());
        if self.link.is_none() || self.project_dir != dir {
            self.shutdown();
            let spec = BenchSpec {
                project_dir: dir.clone(),
                targets: project.map(|p| p.bench_targets()).unwrap_or_default(),
                radios: project.map(|p| p.radios.clone()).unwrap_or_default(),
                repaint: repaint.clone(),
                speed: self.speed,
                circuit,
                fixtures: self.fixtures.clone(),
                audio: self.audio,
            };
            self.link = Some((self.factory)(&spec));
            self.project_dir = dir;
        }
        self.link.as_deref_mut().expect("bench link")
    }

    /// The link if one exists.
    pub fn existing(&mut self) -> Option<&mut (dyn BenchLink + 'static)> {
        self.link.as_deref_mut()
    }

    /// The co-simulation bench, if the link is one (and exists).
    pub fn sim(&mut self) -> Option<&mut super::sim::SimBench> {
        self.link.as_deref_mut().and_then(|l| l.as_sim())
    }

    pub fn is_active(&self) -> bool {
        self.link.is_some()
    }

    /// Stops and drops the bench (project closed / changed).
    pub fn shutdown(&mut self) {
        self.link = None;
        self.project_dir = None;
        self.serial.clear();
        self.debug.clear();
        self.radio = None;
        self.pins.clear();
        self.probes.clear();
        self.events.clear();
        self.alerts.clear();
        self.alerted.clear();
        self.running = false;
        self.sim_time = SimTime::ZERO;
    }

    /// Pulls the latest state from the link. Returns true if anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(link) = self.link.as_mut() else {
            return false;
        };
        let u = link.poll();
        let changed = !u.serial.is_empty()
            || !u.events.is_empty()
            || u.sim_time != self.sim_time
            || u.running != self.running;
        self.running = u.running;
        self.sim_time = u.sim_time;
        self.serial.extend(u.serial);
        if self.serial.len() > SERIAL_HISTORY {
            let n = self.serial.len() - SERIAL_HISTORY;
            self.serial.drain(..n);
        }
        for d in u.debug {
            self.debug.insert(d.target.clone(), d);
        }
        if u.radio.is_some() {
            self.radio = u.radio;
        }
        for (t, p) in u.pins {
            self.pins.insert(t, p);
        }
        for (k, v) in u.probes {
            self.probes.insert(k, v);
        }
        for e in &u.events {
            if let BenchEvent::Error(t) = e
                && !t.contains(" damaged: ")
                && self.alerted.insert(t.clone())
            {
                self.alerts.push(t.clone());
            }
        }
        // device / plugin errors (a wasm plugin touching the circuit without the permission
        // it declared traps and logs once at error level)
        if let Some(sim) = link.as_sim() {
            for ev in sim.recent_events() {
                if let bb_core::DeviceEvent::Log {
                    level: bb_core::LogLevel::Error,
                    text,
                } = &ev.event
                {
                    let t = format!("{}: {text}", ev.source);
                    if self.alerted.insert(t.clone()) {
                        self.alerts.push(t);
                    }
                }
            }
        }
        self.events.extend(u.events);
        if self.events.len() > EVENT_HISTORY {
            let n = self.events.len() - EVENT_HISTORY;
            self.events.drain(..n);
        }
        changed
    }

    pub fn take_events(&mut self) -> Vec<BenchEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn set_speed(&mut self, speed: SimSpeed) {
        self.speed = speed;
        if let Some(l) = self.link.as_mut() {
            l.set_speed(speed);
        }
    }
}
