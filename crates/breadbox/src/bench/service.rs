//! The app's handle on the running bench: creates the [`BenchLink`] for the open project via
//! the registered factory and accumulates what `poll` returns (serial history, latest debug /
//! radio / pin view models, events for the output panel).

use super::*;
use crate::project::Project;
use std::collections::HashMap;

pub struct BenchService {
    factory: BenchFactory,
    link: Option<Box<dyn BenchLink>>,
    project_dir: Option<PathBuf>,
    pub speed: SimSpeed,
    /// Serial history (all targets), oldest first.
    pub serial: Vec<SerialLine>,
    pub debug: HashMap<String, DebugViewModel>,
    pub radio: Option<RadioPanelModel>,
    pub pins: HashMap<String, Vec<PinView>>,
    /// Unconsumed events (screens take them).
    pub events: Vec<BenchEvent>,
    pub running: bool,
    pub sim_time: SimTime,
}

const SERIAL_HISTORY: usize = 20_000;

impl BenchService {
    pub fn new(factory: BenchFactory, speed: SimSpeed) -> BenchService {
        BenchService {
            factory,
            link: None,
            project_dir: None,
            speed,
            serial: Vec::new(),
            debug: HashMap::new(),
            radio: None,
            pins: HashMap::new(),
            events: Vec::new(),
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
        let dir = project.map(|p| p.dir.clone());
        if self.link.is_none() || self.project_dir != dir {
            self.shutdown();
            let spec = BenchSpec {
                project_dir: dir.clone(),
                targets: project.map(|p| p.bench_targets()).unwrap_or_default(),
                radios: project.map(|p| p.radios.clone()).unwrap_or_default(),
                repaint: repaint.clone(),
                speed: self.speed,
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
        self.events.clear();
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
        self.events.extend(u.events);
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
