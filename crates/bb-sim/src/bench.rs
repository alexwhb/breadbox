//! A runnable bench built from a project: the analog session, MCUs, behavioural devices, the
//! world, instruments — and the co-simulation scheduler (ARCHITECTURE §6) that advances them
//! together.

use crate::damage::Damage;
use crate::devices::{DeviceRegistry, NullDevice};
use crate::instruments::Instruments;
use crate::mcu::{AvrMcu, IdleMcu, LogicDrive, McuCore, McuPinEvent, PinModel};
use crate::serial::SerialMonitor;
use crate::{SimError, SimEvent};
use bb_circuit::netlist::{
    self, BuiltNetlist, DamageEffect, DeviceSpec, McuSpec, NetKind, NetlistOptions,
};
use bb_circuit::{Library, NetId, Project};
use bb_core::{
    Device, DeviceEvent, Health, LogLevel, PinDrive, Rng, SimTime, UserInput, VisualState,
    WorldMut, WorldView,
};
use bb_spice::{NodeId, PortId, Session, SessionOptions, WatchId};
use bb_world::World;
use std::any::Any;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, VecDeque};
use std::sync::Arc;

/// Something the scheduler steps besides devices (radio space, plugin hosts). Registered with
/// [`Bench::add_service`]; devices reach it through `DeviceCtx`-side downcasting via
/// [`Bench::service_mut`].
pub trait Service: Send {
    fn name(&self) -> &str;
    /// Next time the service needs [`Service::step`] (`None` = only when asked).
    fn next_wake(&self) -> Option<SimTime>;
    fn step(&mut self, now: SimTime);
    fn as_any(&mut self) -> &mut dyn Any;
    /// Serves a radio request from the API-level board `part` (see [`crate::host`]); `None` =
    /// not this service's board.
    fn host_service(
        &mut self,
        part: &str,
        channel: u8,
        request: &[u8],
        at: SimTime,
    ) -> Option<Vec<u8>> {
        let _ = (part, channel, request, at);
        None
    }
    /// Device wake-ups the service needs (`(part id, time, token)` → `Device::on_wake`), e.g.
    /// a radio module whose modem produced UART frames while the service stepped.
    fn take_device_wakes(&mut self) -> Vec<(String, SimTime, u64)> {
        Vec::new()
    }
    /// Serial-monitor annotations (RF-8) produced since the last call.
    fn take_notes(&mut self) -> Vec<ServiceNote> {
        Vec::new()
    }
}

/// An annotation from a service (`wifi: AT+CWJAP="SIM-AP" → OK …`). The bench shows it in the
/// serial monitor of the MCU(s) wired to `part` (all MCUs when unknown) and emits it as a
/// [`crate::SimEvent`] from `source`.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceNote {
    pub time: SimTime,
    /// Part id of the radio (module or API-level board), if known.
    pub part: Option<String>,
    /// Designator shown as the event source (`U2`).
    pub source: String,
    /// `wifi`, `mqtt`, `net`, `ble`, `lora`.
    pub channel: String,
    pub text: String,
}

/// Creates a service for a new bench (called by [`Bench::new`], so services survive a full
/// reset of a [`crate::SimHandle`]). Radio glue registers one that builds a
/// [`crate::RadioService`] from the project's radio settings.
pub type ServiceFactory = Arc<dyn Fn(&Project) -> Box<dyn Service> + Send + Sync>;

/// Creates a world model for a new bench (world-model plugins, WORLD-5).
pub type WorldModelFactory = Arc<dyn Fn(&Project) -> Box<dyn bb_world::WorldModel> + Send + Sync>;

/// A waveform a device records (scope plugins): every change on digital nets, at least every
/// `period` on analog nets (the scheduler syncs that often while it is active).
#[derive(Debug, Clone)]
pub(crate) struct Capture {
    pub part: String,
    pub pin: usize,
    pub net: Option<NetId>,
    pub period: SimTime,
    pub next: SimTime,
    pub buf: VecDeque<(SimTime, f64)>,
}

/// Samples kept per capture.
const CAPTURE_CAP: usize = 1 << 19;

/// Bench options.
#[derive(Clone)]
pub struct BenchOptions {
    pub seed: Option<u64>,
    /// Sync quantum while the analog circuit is moving (SIM-1, default from the project: 1 µs).
    pub quantum: Option<SimTime>,
    /// Longest quantum while the circuit is quiescent.
    pub max_quantum: SimTime,
    /// World step period.
    pub world_dt: SimTime,
    /// Behaviour id → device factory (built-ins + plugins + radio glue).
    pub registry: Arc<DeviceRegistry>,
    /// Firmware images by part id (`None` → the MCU runs erased flash until flashed).
    pub firmware: HashMap<String, bb_avr::FirmwareImage>,
    /// Host-compiled sketch executables for API-level boards (ESP32 / Pico) by part id
    /// (see [`crate::firmware::compile_host_project`]).
    pub host_firmware: HashMap<String, std::path::PathBuf>,
    /// Services created with every bench. The default holds the radio space
    /// ([`crate::radio::service_factory`]); plugin hosts add theirs.
    pub services: Vec<ServiceFactory>,
    /// World models stepped with the world (world-model plugins).
    pub world_models: Vec<WorldModelFactory>,
}

impl Default for BenchOptions {
    fn default() -> Self {
        BenchOptions {
            seed: None,
            quantum: None,
            max_quantum: SimTime::from_ms(1),
            world_dt: SimTime::from_ms(10),
            registry: Arc::new(DeviceRegistry::builtin()),
            firmware: HashMap::new(),
            host_firmware: HashMap::new(),
            services: vec![crate::radio::service_factory()],
            world_models: Vec::new(),
        }
    }
}

/// Where a threshold crossing goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgeTarget {
    McuPin { mcu: usize, pin: usize },
    DevicePin { dev: usize, pin: usize },
    McuReset { mcu: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Driver {
    Mcu { mcu: usize, pin: usize },
    Device { dev: usize, pin: usize },
}

#[derive(Debug, Clone)]
pub(crate) struct Listener {
    pub target: EdgeTarget,
    pub lo: f64,
    pub hi: f64,
    pub high: bool,
}

/// Runtime state of one net.
#[derive(Debug, Clone)]
pub(crate) struct NetRt {
    pub kind: NetKind,
    pub node: Option<NodeId>,
    pub name: String,
    /// Closed-form (digital) nets: every drive on the net.
    pub drives: Vec<(Driver, PinDrive)>,
    pub voltage: f64,
    pub listeners: Vec<Listener>,
}

#[derive(Debug, Clone)]
pub(crate) struct McuPinRt {
    pub name: String,
    pub net: Option<NetId>,
    pub hi: Option<PortId>,
    pub lo: Option<PortId>,
    pub drive: LogicDrive,
    pub damaged: bool,
    pub watched: bool,
    /// Last sampled voltage (adaptive quantum: slope towards the thresholds).
    pub last_v: f64,
    pub last_t: SimTime,
}

pub(crate) struct McuRt {
    pub spec: McuSpec,
    pub core: Box<dyn McuCore>,
    pub model: PinModel,
    pub pins: Vec<McuPinRt>,
    pub vcc_node: Option<NodeId>,
    pub vcc: f64,
    pub pending: VecDeque<McuPinEvent>,
    pub adc_request: Option<(usize, SimTime)>,
    pub reset_watch: bool,
    pub serial: SerialMonitor,
    pub paused: Option<String>,
    /// High-time integration for the board's L LED (pin D13 / LED_BUILTIN).
    pub led_pin: Option<usize>,
    pub led_on_since: Option<SimTime>,
    pub led_accum: f64,
    pub led_window_start: SimTime,
    pub led_level: f32,
    pub tx_until: SimTime,
    pub rx_until: SimTime,
    pub firmware_name: Option<String>,
}

pub(crate) struct DevRt {
    pub spec: DeviceSpec,
    pub dev: Box<dyn Device>,
    pub rng: Rng,
    pub ports: Vec<Option<PortId>>,
    pub nets: Vec<Option<NetId>>,
    pub drives: Vec<PinDrive>,
    pub watches: Vec<Option<(f64, f64)>>,
    pub watch_ids: Vec<Option<WatchId>>,
    pub tick: SimTime,
    pub tick_gen: u64,
    pub token_gen: HashMap<u64, u64>,
    pub health: Health,
    pub smoke: f32,
    pub smoke_at: SimTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum WakeKind {
    Device {
        dev: usize,
        token: u64,
        generation: u64,
    },
    Tick {
        dev: usize,
        generation: u64,
    },
    World,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Wake {
    pub time: SimTime,
    pub seq: u64,
    pub kind: WakeKind,
}

/// Integrated current/power of a SPICE element.
#[derive(Debug, Clone)]
pub(crate) struct Tracked {
    pub element: String,
    pub a: Option<NodeId>,
    pub b: Option<NodeId>,
    pub i: f64,
    pub p: f64,
    pub v: f64,
    pub charge: f64,
    pub energy: f64,
    pub last: SimTime,
}

pub(crate) struct Analog {
    pub session: Option<Session>,
    /// Bench time at session time 0.
    pub t0: SimTime,
    pub watch_targets: Vec<EdgeTarget>,
}

/// Counters for the status line and performance tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BenchStats {
    pub sync_points: u64,
    pub mcu_stops: u64,
    pub adc_answered_inline: u64,
    pub adc_stops: u64,
    pub edges: u64,
    pub late_edges: u64,
    pub device_calls: u64,
    pub rebuilds: u64,
}

/// A runnable bench.
pub struct Bench {
    pub(crate) lib: Arc<Library>,
    pub(crate) project: Project,
    pub(crate) built: BuiltNetlist,
    pub(crate) analog: Analog,
    pub(crate) nets: Vec<NetRt>,
    pub(crate) mcus: Vec<McuRt>,
    pub(crate) devices: Vec<DevRt>,
    pub(crate) world: World,
    pub(crate) heap: BinaryHeap<Reverse<Wake>>,
    pub(crate) seq: u64,
    pub(crate) time: SimTime,
    pub(crate) quantum: SimTime,
    pub(crate) max_quantum: SimTime,
    pub(crate) world_dt: SimTime,
    pub(crate) seed: u64,
    /// Sketch executables of API-level boards by part id.
    pub(crate) host_firmware: HashMap<String, std::path::PathBuf>,
    pub(crate) edges: VecDeque<(SimTime, EdgeTarget, bool)>,
    pub(crate) in_edges: bool,
    pub(crate) tracked: Vec<Tracked>,
    pub(crate) tracked_index: HashMap<String, usize>,
    pub(crate) damage: Damage,
    pub(crate) instruments: Instruments,
    pub(crate) events: Vec<SimEvent>,
    pub(crate) routed: usize,
    pub(crate) services: Vec<Box<dyn Service>>,
    pub(crate) registry: Arc<DeviceRegistry>,
    pub(crate) ext_values: BTreeMap<String, f64>,
    pub(crate) damaged: BTreeMap<String, DamageEffect>,
    pub(crate) loads: Vec<bb_circuit::netlist::Load>,
    pub(crate) erc_cache: Option<(Vec<bb_circuit::ErcItem>, Vec<bb_circuit::ErcItem>)>,
    pub(crate) paused: Option<String>,
    /// Designator → MCUs whose I/O pins share a net with that part (annotation routing).
    pub(crate) routes: HashMap<String, Vec<usize>>,
    pub(crate) captures: Vec<Capture>,
    /// Sync again one quantum after an MCU drive change on an analog net.
    pub(crate) settle_next: bool,
    pub stats: BenchStats,
}

fn net_target_level(v: f64, lo: f64, hi: f64, was_high: bool) -> bool {
    if was_high { v >= lo } else { v > hi }
}

impl Bench {
    /// Builds a bench for a project.
    pub fn new(project: Project, lib: Arc<Library>, opts: BenchOptions) -> Result<Bench, SimError> {
        let seed = opts.seed.unwrap_or(project.settings.seed);
        let quantum = opts.quantum.unwrap_or_else(|| {
            SimTime::from_secs_f64(project.settings.sync_quantum_us.max(0.01) * 1e-6)
        });
        let mut world =
            World::new(project.world.clone()).map_err(|e| SimError::Build(e.to_string()))?;
        for f in &opts.world_models {
            world.add_model(f(&project));
        }
        let built = netlist::build(&project, &lib, &NetlistOptions::bench())
            .map_err(|e| SimError::Build(e.to_string()))?;
        let mut bench = Bench {
            lib,
            built,
            analog: Analog {
                session: None,
                t0: SimTime::ZERO,
                watch_targets: Vec::new(),
            },
            nets: Vec::new(),
            mcus: Vec::new(),
            devices: Vec::new(),
            world,
            heap: BinaryHeap::new(),
            seq: 0,
            time: SimTime::ZERO,
            quantum,
            max_quantum: opts.max_quantum.max(quantum),
            world_dt: opts.world_dt,
            seed,
            edges: VecDeque::new(),
            in_edges: false,
            tracked: Vec::new(),
            tracked_index: HashMap::new(),
            damage: Damage::default(),
            instruments: Instruments::default(),
            events: Vec::new(),
            routed: 0,
            services: Vec::new(),
            registry: opts.registry.clone(),
            ext_values: BTreeMap::new(),
            damaged: BTreeMap::new(),
            loads: Vec::new(),
            erc_cache: None,
            host_firmware: opts.host_firmware.clone(),
            paused: None,
            routes: HashMap::new(),
            captures: Vec::new(),
            settle_next: false,
            stats: BenchStats::default(),
            project,
        };
        for (k, v) in &bench.built.ext {
            bench.ext_values.insert(k.clone(), *v);
        }
        for f in &opts.services {
            let svc = f(&bench.project);
            bench.services.push(svc);
        }
        // MCUs
        let specs = bench.built.mcus.clone();
        for (i, spec) in specs.iter().enumerate() {
            let m = bench.make_mcu(spec, i, opts.firmware.get(&spec.part))?;
            bench.mcus.push(m);
        }
        // devices
        let dspecs = bench.built.devices.clone();
        for spec in dspecs {
            let dev = bench.make_device(&spec);
            bench.devices.push(dev);
        }
        bench.instruments = Instruments::from_project(&bench.project);
        bench.wire_up(None)?;
        // initial operating point first, so devices see real supply voltages in on_start
        bench.commit_analog(SimTime::ZERO)?;
        for mi in 0..bench.mcus.len() {
            bench.update_vcc(mi);
        }
        // devices start (initial drives / watches / ticks) at t = 0, each seeing the circuit
        // as the previous ones left it
        for i in 0..bench.devices.len() {
            bench.with_device(i, SimTime::ZERO, |d, ctx| d.on_start(ctx));
            bench.commit_analog(SimTime::ZERO)?;
        }
        bench.settle(SimTime::ZERO)?;
        bench.push_wake(bench.world_dt, WakeKind::World);
        Ok(bench)
    }

    fn make_mcu(
        &self,
        spec: &McuSpec,
        i: usize,
        image: Option<&bb_avr::FirmwareImage>,
    ) -> Result<McuRt, SimError> {
        let noise = self.world.config().noise;
        let core: Box<dyn McuCore> = if spec.core == "avr" {
            let mut m = AvrMcu::new(
                &spec.board,
                self.seed.wrapping_add(i as u64 * 7919),
                if noise { 0.5 } else { 0.0 },
            )
            .map_err(SimError::Build)?;
            if let Some(img) = image {
                m.flash(img).map_err(SimError::Build)?;
            }
            Box::new(m)
        } else {
            let names: Vec<String> = spec.pins.iter().map(|p| p.mcu.clone()).collect();
            let idle = |why: String| -> Box<dyn McuCore> {
                Box::new(IdleMcu::new(&spec.label, names.clone(), &why))
            };
            match self.host_firmware.get(&spec.part) {
                Some(exe) => match crate::host::HostMcu::spawn(
                    &spec.board,
                    exe,
                    self.seed.wrapping_add(i as u64 * 7919),
                ) {
                    Ok(h) => Box::new(h),
                    Err(e) => idle(format!("{} · cannot start the sketch: {e}", spec.reference)),
                },
                None => idle(format!(
                    "{} · {} · no sketch build yet — compile to run it",
                    spec.reference, spec.label
                )),
            }
        };
        let model = if spec.core == "avr" {
            PinModel::AVR
        } else {
            PinModel::ESP32
        };
        let label = core.label();
        let mut serial = SerialMonitor::new(&spec.reference);
        serial.seen_resets = core.reset_count();
        serial.system(
            self.time,
            &format!("Breadbox sim · {label} · reset cause: power-on"),
        );
        let led_pin = if spec.core == "avr" {
            core.pin_index("D13")
        } else {
            core.pin_index("LED_BUILTIN")
        };
        Ok(McuRt {
            spec: spec.clone(),
            core,
            model,
            pins: Vec::new(),
            vcc_node: None,
            vcc: if spec.core == "avr" { 5.0 } else { 3.3 },
            pending: VecDeque::new(),
            adc_request: None,
            reset_watch: false,
            serial,
            paused: None,
            led_pin,
            led_on_since: None,
            led_accum: 0.0,
            led_window_start: self.time,
            led_level: 0.0,
            tx_until: SimTime::ZERO,
            rx_until: SimTime::ZERO,
            firmware_name: spec
                .firmware
                .as_ref()
                .map(|f| f.sketch.rsplit('/').next().unwrap_or("").to_string()),
        })
    }

    /// Injects a load between two pins' nets (instrument input impedance); rebuilds.
    pub fn add_load(&mut self, load: bb_circuit::netlist::Load) -> Result<(), SimError> {
        self.loads.retain(|l| l.id != load.id);
        self.loads.push(load);
        self.rebuild()
    }

    pub fn remove_load(&mut self, id: &str) -> Result<(), SimError> {
        self.loads.retain(|l| l.id != id);
        self.rebuild()
    }

    fn make_device(&mut self, spec: &DeviceSpec) -> DevRt {
        let dev: Box<dyn Device> = match self.registry.create(spec) {
            Some(d) => d,
            None => {
                self.log(
                    LogLevel::Warn,
                    &spec.reference,
                    &format!("no simulation model for `{}` in this build", spec.behaviour),
                );
                Box::new(NullDevice)
            }
        };
        let n = spec.pins.len();
        DevRt {
            rng: Rng::derive(self.seed, &spec.part),
            spec: spec.clone(),
            dev,
            ports: vec![None; n],
            nets: vec![None; n],
            drives: vec![PinDrive::HighZ; n],
            watches: vec![None; n],
            watch_ids: vec![None; n],
            tick: SimTime::ZERO,
            tick_gen: 0,
            token_gen: HashMap::new(),
            health: Health::Ok,
            smoke: 0.0,
            smoke_at: SimTime::ZERO,
        }
    }

    /// (Re)creates the analog session, nets, ports and watches from `self.built`, keeping
    /// MCU/device state. `ic` carries node voltages from a previous session (rebuilds).
    fn wire_up(&mut self, ic: Option<HashMap<String, f64>>) -> Result<(), SimError> {
        let built = &self.built;
        let has_analog = !built.netlist.top.elements.is_empty();
        let mut session = None;
        if has_analog {
            let mut nl = built.netlist.clone();
            if let Some(ic) = &ic {
                for (node, v) in ic {
                    if built
                        .net_kind
                        .iter()
                        .any(|k| matches!(k, NetKind::Analog(n) if n.eq_ignore_ascii_case(node)))
                    {
                        nl.nodesets.push((
                            node.to_ascii_lowercase(),
                            bb_spice::expr::Expr::Num(*v),
                            Default::default(),
                        ));
                    }
                }
            }
            let ext: Vec<(String, f64)> = self
                .ext_values
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            let opts = SessionOptions {
                max_step: (self.quantum.as_secs_f64() * 10.0).clamp(1e-6, 1e-4),
                ..SessionOptions::default()
            };
            session = Some(
                Session::with_ext(&nl, opts, &ext).map_err(|e| SimError::Analog(e.to_string()))?,
            );
        }
        self.analog = Analog {
            session,
            t0: self.time,
            watch_targets: Vec::new(),
        };
        // nets
        self.nets = built
            .connectivity
            .nets
            .iter()
            .map(|n| {
                let kind = built.net_kind[n.id.0].clone();
                let node = match (&kind, &self.analog.session) {
                    (NetKind::Analog(name), Some(s)) => s.node(name),
                    _ => None,
                };
                NetRt {
                    kind,
                    node,
                    name: n.name.clone(),
                    drives: Vec::new(),
                    voltage: 0.0,
                    listeners: Vec::new(),
                }
            })
            .collect();
        // MCU pins
        for (mi, m) in self.mcus.iter_mut().enumerate() {
            let spec = built.mcus.iter().find(|s| s.part == m.spec.part).cloned();
            if let Some(s) = spec {
                m.spec = s;
            }
            let names = m.core.pin_names();
            let old: Vec<McuPinRt> = std::mem::take(&mut m.pins);
            m.pins = names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let ps = m
                        .spec
                        .pins
                        .iter()
                        .find(|p| p.mcu.eq_ignore_ascii_case(name));
                    let port = |n: &Option<String>| {
                        n.as_ref()
                            .and_then(|n| self.analog.session.as_ref().and_then(|s| s.port(n)))
                    };
                    McuPinRt {
                        name: name.clone(),
                        net: ps.map(|p| p.net),
                        hi: ps.and_then(|p| port(&p.hi_port)),
                        lo: ps.and_then(|p| port(&p.lo_port)),
                        drive: m.core.pin_drive(i),
                        damaged: old.get(i).is_some_and(|o| o.damaged),
                        watched: false,
                        last_v: 0.0,
                        last_t: SimTime::ZERO,
                    }
                })
                .collect();
            m.vcc_node = match (&m.spec.vcc_node, &self.analog.session) {
                (Some(n), Some(s)) => s.node(n),
                _ => None,
            };
            let _ = mi;
        }
        // device pins
        for d in self.devices.iter_mut() {
            if let Some(s) = built.devices.iter().find(|s| s.part == d.spec.part) {
                d.spec = s.clone();
            }
            let n = d.spec.pins.len();
            d.ports.resize(n, None);
            d.nets.resize(n, None);
            d.drives.resize(n, PinDrive::HighZ);
            d.watches.resize(n, None);
            d.watch_ids = vec![None; n];
            for (i, p) in d.spec.pins.iter().enumerate() {
                d.nets[i] = p.net;
                d.ports[i] = p
                    .port
                    .as_ref()
                    .and_then(|name| self.analog.session.as_ref().and_then(|s| s.port(name)));
            }
        }
        // MCU drives into the circuit + supply
        for mi in 0..self.mcus.len() {
            for pi in 0..self.mcus[mi].pins.len() {
                let d = self.mcus[mi].pins[pi].drive;
                self.apply_mcu_drive_now(mi, pi, d, self.time);
            }
        }
        // device drives + watches (re-applied after a rebuild)
        for di in 0..self.devices.len() {
            for pi in 0..self.devices[di].drives.len() {
                let d = self.devices[di].drives[pi];
                if d != PinDrive::HighZ {
                    self.set_device_drive(di, pi, d, self.time);
                }
                if let Some((lo, hi)) = self.devices[di].watches[pi] {
                    self.add_device_watch(di, pi, lo, hi);
                }
            }
        }
        // MCU input watches
        for mi in 0..self.mcus.len() {
            self.watch_mcu_inputs(mi);
        }
        // tracked elements (keep integrals by name)
        let old = std::mem::take(&mut self.tracked);
        self.tracked_index.clear();
        for t in old {
            self.track(&t.element);
            if let Some(&i) = self.tracked_index.get(&t.element) {
                self.tracked[i].charge = t.charge;
                self.tracked[i].energy = t.energy;
            }
        }
        let ratings = self.built.ratings.clone();
        for r in &ratings {
            self.track(&r.element);
        }
        self.damage.bind(&self.built, &self.mcus);
        self.instruments.bind(&self.built, &self.nets);
        for mi in 0..self.mcus.len() {
            // TX waveform only when something listens on the TX pin
            let tx = self.mcus[mi].core.serial_tx_pin(0);
            let wired = tx
                .and_then(|p| self.mcus[mi].pins.get(p))
                .and_then(|p| p.net)
                .is_some_and(|n| !matches!(self.nets[n.0].kind, NetKind::Isolated));
            self.mcus[mi].core.set_serial_waveform(0, wired);
        }
        self.compute_routes();
        for c in self.captures.iter_mut() {
            c.net = self
                .devices
                .iter()
                .find(|d| d.spec.part == c.part)
                .and_then(|d| d.nets.get(c.pin).copied().flatten());
        }
        Ok(())
    }

    /// Starts/stops a capture for device `di`'s pin (`DeviceCtx::capture`).
    pub(crate) fn set_capture(&mut self, di: usize, pin: usize, period: SimTime, now: SimTime) {
        let part = self.devices[di].spec.part.clone();
        if period == SimTime::ZERO {
            self.captures.retain(|c| !(c.part == part && c.pin == pin));
            return;
        }
        let net = self.devices[di].nets.get(pin).copied().flatten();
        let v = self.device_pin_voltage(di, pin);
        match self
            .captures
            .iter_mut()
            .find(|c| c.part == part && c.pin == pin)
        {
            Some(c) => {
                c.period = period;
                c.next = c.next.min(now + period);
            }
            None => {
                let mut buf = VecDeque::new();
                buf.push_back((now, v));
                self.captures.push(Capture {
                    part,
                    pin,
                    net,
                    period,
                    next: now + period,
                    buf,
                });
            }
        }
    }

    /// Samples of a capture between `from` and `to` (plus the last one before `from`).
    pub(crate) fn capture_samples(
        &self,
        di: usize,
        pin: usize,
        from: SimTime,
        to: SimTime,
    ) -> Vec<(SimTime, f64)> {
        let part = &self.devices[di].spec.part;
        let Some(c) = self
            .captures
            .iter()
            .find(|c| &c.part == part && c.pin == pin)
        else {
            return Vec::new();
        };
        let i0 = c.buf.partition_point(|s| s.0 < from).saturating_sub(1);
        c.buf
            .iter()
            .skip(i0)
            .take_while(|s| s.0 <= to)
            .copied()
            .collect()
    }

    /// Earliest analog capture sample due.
    pub(crate) fn next_capture(&self) -> Option<SimTime> {
        self.captures
            .iter()
            .filter(|c| {
                c.net
                    .is_none_or(|n| !matches!(self.nets[n.0].kind, NetKind::Digital))
            })
            .map(|c| c.next)
            .min()
    }

    /// Records analog captures due at `t` and every capture on a digital net that changed.
    pub(crate) fn record_captures(&mut self, t: SimTime, changed: Option<(NetId, f64)>) {
        if self.captures.is_empty() {
            return;
        }
        for i in 0..self.captures.len() {
            let net = self.captures[i].net;
            let (due, v) = match (changed, net) {
                (Some((n, v)), Some(cn)) if n == cn => (true, v),
                (Some(_), _) => (false, 0.0),
                (None, Some(n)) if matches!(self.nets[n.0].kind, NetKind::Digital) => (false, 0.0),
                (None, n) => {
                    let due = self.captures[i].next <= t;
                    let v = match n {
                        Some(n) if !matches!(self.nets[n.0].kind, NetKind::Isolated) => {
                            self.net_voltage(n)
                        }
                        _ => 0.0,
                    };
                    (due, v)
                }
            };
            if !due {
                continue;
            }
            let c = &mut self.captures[i];
            if changed.is_none() {
                c.next = (c.next + c.period).max(t + SimTime(c.period.0 / 2));
            }
            if c.buf.back().is_some_and(|b| b.0 > t) {
                continue;
            }
            c.buf.push_back((t, v));
            if c.buf.len() > CAPTURE_CAP {
                c.buf.pop_front();
            }
        }
    }

    /// Which MCU(s) a part's annotations belong to: those sharing a signal net with it.
    fn compute_routes(&mut self) {
        let mut routes: HashMap<String, Vec<usize>> = HashMap::new();
        let supply: Vec<NetId> = self.mcus.iter().filter_map(|m| m.spec.vcc_net).collect();
        for (mi, m) in self.mcus.iter().enumerate() {
            routes.entry(m.spec.reference.clone()).or_default().push(mi);
        }
        for d in &self.devices {
            let roles = self.lib.get(&d.spec.def);
            let mut nets = Vec::new();
            for p in &d.spec.pins {
                let power = roles
                    .and_then(|def| def.pins.iter().find(|x| x.id == p.pin))
                    .is_some_and(|x| {
                        matches!(
                            x.role,
                            bb_circuit::part::PinRole::Ground
                                | bb_circuit::part::PinRole::PowerIn
                                | bb_circuit::part::PinRole::PowerOut
                        )
                    });
                if let Some(n) = p.net
                    && !power
                    && !supply.contains(&n)
                    && !matches!(&self.nets[n.0].kind, NetKind::Analog(name) if name == "0")
                {
                    nets.push(n);
                }
            }
            let mcus: Vec<usize> = self
                .mcus
                .iter()
                .enumerate()
                .filter(|(_, m)| {
                    m.pins
                        .iter()
                        .any(|p| p.net.is_some_and(|n| nets.contains(&n)))
                })
                .map(|(i, _)| i)
                .collect();
            if !mcus.is_empty() {
                routes.insert(d.spec.reference.clone(), mcus);
            }
        }
        self.routes = routes;
    }

    /// Schedules `Device::on_wake(token)` for the device of `part` at `at` (services).
    pub(crate) fn wake_part(&mut self, part: &str, at: SimTime, token: u64) {
        let Some(dev) = self.devices.iter().position(|d| d.spec.part == part) else {
            return;
        };
        let generation = *self.devices[dev].token_gen.entry(token).or_insert(0);
        self.push_wake(
            at.max(self.time),
            WakeKind::Device {
                dev,
                token,
                generation,
            },
        );
    }

    /// Delivers service wake-ups and annotations.
    pub(crate) fn service_outputs(&mut self) {
        for i in 0..self.services.len() {
            let wakes = self.services[i].take_device_wakes();
            for (part, at, token) in wakes {
                self.wake_part(&part, at, token);
            }
            for n in self.services[i].take_notes() {
                let source = n
                    .part
                    .as_ref()
                    .and_then(|p| self.project.part(p))
                    .map(|p| p.reference.clone())
                    .unwrap_or(n.source);
                self.events.push(SimEvent {
                    time: n.time,
                    source,
                    event: DeviceEvent::Annotation {
                        channel: n.channel,
                        text: n.text,
                    },
                });
            }
        }
    }

    fn watch_mcu_inputs(&mut self, mi: usize) {
        let vcc = self.mcus[mi].vcc;
        let model = self.mcus[mi].model;
        let (lo, hi) = (model.vil * vcc, model.vih * vcc);
        for pi in 0..self.mcus[mi].pins.len() {
            let Some(net) = self.mcus[mi].pins[pi].net else {
                continue;
            };
            let target = EdgeTarget::McuPin { mcu: mi, pin: pi };
            match self.nets[net.0].kind.clone() {
                NetKind::Analog(name) => {
                    if let Some(s) = self.analog.session.as_mut()
                        && let Ok(w) = s.watch(&name, lo, hi)
                    {
                        if w.0 >= self.analog.watch_targets.len() {
                            self.analog.watch_targets.resize(w.0 + 1, target);
                        }
                        self.analog.watch_targets[w.0] = target;
                        self.mcus[mi].pins[pi].watched = true;
                    }
                }
                NetKind::Digital => {
                    self.nets[net.0].listeners.push(Listener {
                        target,
                        lo,
                        hi,
                        high: false,
                    });
                    self.mcus[mi].pins[pi].watched = true;
                }
                NetKind::Isolated => {
                    self.mcus[mi].core.set_input(pi, None);
                }
            }
        }
        // reset line
        if let Some(node) = self.mcus[mi].spec.reset_node.clone()
            && let Some(s) = self.analog.session.as_mut()
            && let Ok(w) = s.watch(&node, 0.3 * vcc, 0.6 * vcc)
        {
            let target = EdgeTarget::McuReset { mcu: mi };
            if w.0 >= self.analog.watch_targets.len() {
                self.analog.watch_targets.resize(w.0 + 1, target);
            }
            self.analog.watch_targets[w.0] = target;
            self.mcus[mi].reset_watch = true;
        }
    }

    /// Solves the circuit at `t` after drive changes and delivers the resulting edges; sets
    /// every MCU's supply and every watched input from the solution.
    pub(crate) fn settle(&mut self, t: SimTime) -> Result<(), SimError> {
        self.commit_analog(t)?;
        // initial levels for all watched MCU inputs
        for mi in 0..self.mcus.len() {
            self.update_vcc(mi);
            for pi in 0..self.mcus[mi].pins.len() {
                if !self.mcus[mi].pins[pi].watched {
                    continue;
                }
                let Some(net) = self.mcus[mi].pins[pi].net else {
                    continue;
                };
                let v = self.net_voltage(net);
                let m = &self.mcus[mi];
                let level = v > 0.45 * m.vcc;
                self.mcus[mi].core.set_input(pi, Some(level));
            }
        }
        // digital listener states
        for ni in 0..self.nets.len() {
            if matches!(self.nets[ni].kind, NetKind::Digital) {
                let v = self.nets[ni].voltage;
                for l in self.nets[ni].listeners.iter_mut() {
                    l.high = v > (l.lo + l.hi) / 2.0;
                }
            }
        }
        self.process_edges()?;
        self.integrate(t);
        Ok(())
    }

    // ------------------------------------------------------------------ public queries

    pub fn time(&self) -> SimTime {
        self.time
    }
    pub fn project(&self) -> &Project {
        &self.project
    }
    pub fn library(&self) -> &Arc<Library> {
        &self.lib
    }
    pub fn netlist(&self) -> &BuiltNetlist {
        &self.built
    }
    pub fn world(&self) -> &World {
        &self.world
    }
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }
    pub fn paused(&self) -> Option<&str> {
        self.paused.as_deref()
    }

    /// Voltage of a net now.
    pub fn net_voltage(&self, net: NetId) -> f64 {
        let n = &self.nets[net.0];
        match (&n.kind, n.node, &self.analog.session) {
            (NetKind::Analog(name), _, _) if name == "0" => 0.0,
            (NetKind::Analog(_), Some(node), Some(s)) => s.voltage_at(node),
            (NetKind::Digital, _, _) => n.voltage,
            _ => 0.0,
        }
    }

    /// Voltage of a part pin's net now (`None` if the pin is unknown).
    pub fn pin_voltage(&self, part: &str, pin: &str) -> Option<f64> {
        let n = self.built.connectivity.net_of(part, pin)?;
        if matches!(self.nets[n.0].kind, NetKind::Isolated)
            && let Some(di) = self.devices.iter().position(|d| d.spec.part == part)
            && let Some(pi) = self.devices[di].spec.pins.iter().position(|p| p.pin == pin)
        {
            // an unconnected output: its open-circuit voltage
            return Some(self.device_pin_voltage(di, pi));
        }
        Some(self.net_voltage(n))
    }

    /// Voltage of a net by name (`"A0"`, `"VCC"`).
    pub fn voltage(&self, net_name: &str) -> Option<f64> {
        let n = self.built.connectivity.net_by_name(net_name)?;
        Some(self.net_voltage(n.id))
    }

    /// Voltages of `nets` if external variable `name` were `value` — a what-if DC operating
    /// point of the circuit with every drive frozen at its present value (the bench itself is
    /// left unchanged). Used for instrument loading callouts ("meter load shifts the node by
    /// −0.4 mV"). Returns `(with the present value, with value)`.
    pub fn what_if_ext(
        &self,
        name: &str,
        value: f64,
        nets: &[NetId],
    ) -> Option<(Vec<f64>, Vec<f64>)> {
        let s = self.analog.session.as_ref()?;
        let text = s.to_spice_text();
        let param: String = std::iter::once("ext_".to_string())
            .chain(name.chars().map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase().to_string()
                } else {
                    "_".to_string()
                }
            }))
            .collect();
        let prefix = format!(".param {param}=");
        if !text.lines().any(|l| l.starts_with(&prefix)) {
            return None;
        }
        let changed: String = text
            .lines()
            .map(|l| {
                if l.starts_with(&prefix) {
                    format!("{prefix}{}", bb_spice::expr::fmt_g(value))
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let nodes: Vec<Option<String>> = nets
            .iter()
            .map(|n| match &self.nets.get(n.0)?.kind {
                NetKind::Analog(name) => Some(name.clone()),
                _ => None,
            })
            .collect();
        let op = |t: &str| -> Option<Vec<f64>> {
            let nl = bb_spice::Netlist::parse(t).ok()?;
            let mut sess = Session::new(&nl).ok()?;
            sess.op().ok()?;
            Some(
                nodes
                    .iter()
                    .map(|n| match n.as_deref() {
                        Some("0") | None => 0.0,
                        Some(n) => sess
                            .voltage(n)
                            .or_else(|| sess.voltage(&n.to_ascii_lowercase()))
                            .unwrap_or(0.0),
                    })
                    .collect(),
            )
        };
        Some((op(&text)?, op(&changed)?))
    }

    /// Current through a part's SPICE element (local spec `"D"`, `"R"`).
    pub fn element_current(&self, part: &str, spec: &str) -> Option<f64> {
        let el = self.built.element(part, spec)?;
        self.analog.session.as_ref()?.current(el)
    }

    /// Average current of a tracked element over the bench's lifetime so far.
    pub fn element_charge(&self, part: &str, spec: &str) -> Option<f64> {
        let el = self.built.element(part, spec)?;
        self.tracked_index.get(el).map(|&i| self.tracked[i].charge)
    }

    /// Visual state of a part (device visual + health + smoke).
    pub fn visual(&self, part: &str) -> Option<VisualState> {
        let d = self.devices.iter().find(|d| d.spec.part == part)?;
        let mut v = d.dev.visual();
        v.health = d.health.clone();
        v.smoke = d.smoke;
        Some(v)
    }

    /// Live readings of a part's device.
    pub fn readings(&self, part: &str) -> Vec<(String, f64)> {
        self.devices
            .iter()
            .find(|d| d.spec.part == part)
            .map(|d| d.dev.readings())
            .unwrap_or_default()
    }

    pub fn health(&self, part: &str) -> Health {
        self.devices
            .iter()
            .find(|d| d.spec.part == part)
            .map(|d| d.health.clone())
            .or_else(|| {
                self.damaged.get(part).map(|_| Health::Damaged {
                    reason: "damaged".into(),
                })
            })
            .unwrap_or(Health::Ok)
    }

    /// Events since the last call.
    pub fn take_events(&mut self) -> Vec<SimEvent> {
        self.routed = 0;
        std::mem::take(&mut self.events)
    }

    /// Serial monitor of an MCU part.
    pub fn serial(&self, part: &str) -> Option<&SerialMonitor> {
        self.mcus
            .iter()
            .find(|m| m.spec.part == part)
            .map(|m| &m.serial)
    }
    pub fn serial_mut(&mut self, part: &str) -> Option<&mut SerialMonitor> {
        self.mcus
            .iter_mut()
            .find(|m| m.spec.part == part)
            .map(|m| &mut m.serial)
    }

    /// The MCU core of a part (registers, debugger, …).
    pub fn mcu(&mut self, part: &str) -> Option<&mut dyn McuCore> {
        self.mcus
            .iter_mut()
            .find(|m| m.spec.part == part)
            .map(|m| m.core.as_mut() as &mut dyn McuCore)
    }
    pub fn mcu_parts(&self) -> Vec<String> {
        self.mcus.iter().map(|m| m.spec.part.clone()).collect()
    }

    pub fn instruments(&self) -> &Instruments {
        &self.instruments
    }
    pub fn instruments_mut(&mut self) -> &mut Instruments {
        &mut self.instruments
    }

    /// Registers a service (radio space, plugin host) stepped with the bench.
    pub fn add_service(&mut self, s: Box<dyn Service>) {
        self.services.push(s);
    }
    pub fn service_mut(&mut self, name: &str) -> Option<&mut dyn Any> {
        self.services
            .iter_mut()
            .find(|s| s.name() == name)
            .map(|s| s.as_any())
    }

    /// The bench's radio space (RF-*), if it has one.
    pub fn radio(&mut self) -> Option<&mut crate::radio::RadioService> {
        self.service_mut("radio")?
            .downcast_mut::<crate::radio::RadioService>()
    }

    /// Applies a Radios-panel / phone command now.
    pub fn radio_command(&mut self, cmd: crate::radio::RadioCommand) -> Result<(), SimError> {
        let t = self.time;
        let r = match self.radio() {
            Some(svc) => svc
                .command(cmd, t)
                .map_err(|e| SimError::Build(format!("radio: {e}"))),
            None => Err(SimError::NotFound("radio space".into())),
        };
        self.service_outputs();
        r
    }

    pub(crate) fn log(&mut self, level: LogLevel, source: &str, text: &str) {
        self.events.push(SimEvent {
            time: self.time,
            source: source.into(),
            event: DeviceEvent::Log {
                level,
                text: text.into(),
            },
        });
    }

    // ------------------------------------------------------------------ wakes

    pub(crate) fn push_wake(&mut self, time: SimTime, kind: WakeKind) {
        self.seq += 1;
        self.heap.push(Reverse(Wake {
            time,
            seq: self.seq,
            kind,
        }));
    }

    pub(crate) fn next_wake_time(&mut self) -> Option<SimTime> {
        while let Some(Reverse(w)) = self.heap.peek() {
            let stale = match w.kind {
                WakeKind::Device {
                    dev,
                    token,
                    generation,
                } => self
                    .devices
                    .get(dev)
                    .is_none_or(|d| d.token_gen.get(&token).copied().unwrap_or(0) != generation),
                WakeKind::Tick { dev, generation } => self
                    .devices
                    .get(dev)
                    .is_none_or(|d| d.tick_gen != generation || d.tick == SimTime::ZERO),
                WakeKind::World => false,
            };
            if stale {
                self.heap.pop();
            } else {
                return Some(w.time);
            }
        }
        None
    }

    // ------------------------------------------------------------------ drives & nets

    pub(crate) fn apply_mcu_drive_now(&mut self, mi: usize, pi: usize, d: LogicDrive, t: SimTime) {
        let m = &mut self.mcus[mi];
        m.pins[pi].drive = d;
        if m.led_pin == Some(pi) {
            let on = matches!(d, LogicDrive::Output { high: true });
            match (on, m.led_on_since) {
                (true, None) => m.led_on_since = Some(t),
                (false, Some(s)) => {
                    m.led_accum += (t - s).as_secs_f64();
                    m.led_on_since = None;
                }
                _ => {}
            }
        }
        let p = m.pins[pi].clone();
        let d = if p.damaged {
            LogicDrive::Input { pullup: false }
        } else {
            d
        };
        let Some(net) = p.net else { return };
        match self.nets[net.0].kind {
            NetKind::Analog(_) => {
                let (hi, lo) = self.mcus[mi].model.port_drives(d);
                if let Some(s) = self.analog.session.as_mut() {
                    if let Some(h) = p.hi {
                        s.set_port(h, hi);
                    }
                    if let Some(l) = p.lo {
                        s.set_port(l, lo);
                    }
                }
            }
            NetKind::Digital => {
                let drive = self.mcus[mi].model.net_drive(d, self.mcus[mi].vcc);
                self.set_net_drive(net, Driver::Mcu { mcu: mi, pin: pi }, drive, t);
            }
            NetKind::Isolated => {}
        }
    }

    pub(crate) fn set_net_drive(&mut self, net: NetId, who: Driver, drive: PinDrive, t: SimTime) {
        let n = &mut self.nets[net.0];
        match n.drives.iter_mut().find(|(w, _)| *w == who) {
            Some(slot) => {
                if slot.1 == drive {
                    return;
                }
                slot.1 = drive;
            }
            None => n.drives.push((who, drive)),
        }
        self.recompute_net(net, t);
    }

    pub(crate) fn recompute_net(&mut self, net: NetId, t: SimTime) {
        let n = &mut self.nets[net.0];
        let v = bb_core::resolve_drives(n.drives.iter().map(|(_, d)| d), 1e-12);
        n.voltage = v;

        for l in n.listeners.iter_mut() {
            let now_high = net_target_level(v, l.lo, l.hi, l.high);
            if now_high != l.high {
                l.high = now_high;
                self.edges.push_back((t, l.target, now_high));
            }
        }
        self.instruments.on_net_change(net, t, v);
        self.record_captures(t, Some((net, v)));
    }

    pub(crate) fn set_device_drive(&mut self, di: usize, pi: usize, drive: PinDrive, t: SimTime) {
        self.devices[di].drives[pi] = drive;
        if let Some(port) = self.devices[di].ports[pi] {
            if let Some(s) = self.analog.session.as_mut() {
                s.set_port(port, drive);
            }
        } else if let Some(net) = self.devices[di].nets[pi]
            && matches!(self.nets[net.0].kind, NetKind::Digital)
        {
            self.set_net_drive(net, Driver::Device { dev: di, pin: pi }, drive, t);
        }
    }

    pub(crate) fn add_device_watch(&mut self, di: usize, pi: usize, lo: f64, hi: f64) {
        self.devices[di].watches[pi] = Some((lo, hi));
        let Some(net) = self.devices[di].nets[pi] else {
            return;
        };
        let target = EdgeTarget::DevicePin { dev: di, pin: pi };
        match self.nets[net.0].kind.clone() {
            NetKind::Analog(name) => {
                if let Some(s) = self.analog.session.as_mut() {
                    if let Some(old) = self.devices[di].watch_ids[pi].take() {
                        s.unwatch(old);
                    }
                    if let Ok(w) = s.watch(&name, lo, hi) {
                        if w.0 >= self.analog.watch_targets.len() {
                            self.analog.watch_targets.resize(w.0 + 1, target);
                        }
                        self.analog.watch_targets[w.0] = target;
                        self.devices[di].watch_ids[pi] = Some(w);
                    }
                }
            }
            NetKind::Digital => {
                let n = &mut self.nets[net.0];
                n.listeners.retain(|l| l.target != target);
                let v = n.voltage;
                n.listeners.push(Listener {
                    target,
                    lo,
                    hi,
                    high: v > (lo + hi) / 2.0,
                });
            }
            NetKind::Isolated => {}
        }
    }

    /// Voltage a device sees on its pin.
    pub(crate) fn device_pin_voltage(&self, di: usize, pi: usize) -> f64 {
        let d = &self.devices[di];
        match d.nets.get(pi).copied().flatten() {
            Some(net) if !matches!(self.nets[net.0].kind, NetKind::Isolated) => {
                self.net_voltage(net)
            }
            _ => match d.drives.get(pi) {
                Some(PinDrive::Thevenin { v, .. }) | Some(PinDrive::Pull { v, .. }) => *v,
                _ => 0.0,
            },
        }
    }

    /// Current into a device through its pin.
    pub(crate) fn device_pin_current(&self, di: usize, pi: usize) -> f64 {
        let d = &self.devices[di];
        if let (Some(port), Some(s)) = (d.ports.get(pi).copied().flatten(), &self.analog.session) {
            return -s.port_current(port).unwrap_or(0.0);
        }
        if let Some(net) = d.nets.get(pi).copied().flatten()
            && matches!(self.nets[net.0].kind, NetKind::Digital)
        {
            let v = self.nets[net.0].voltage;
            return -d.drives[pi].current_into(v);
        }
        0.0
    }

    pub(crate) fn update_vcc(&mut self, mi: usize) {
        let v = match (self.mcus[mi].vcc_node, &self.analog.session) {
            (Some(n), Some(s)) => s.voltage_at(n),
            _ => return,
        };
        let m = &mut self.mcus[mi];
        if (m.vcc - v).abs() > 1e-4 {
            m.vcc = v;
            m.core.set_vcc(v);
        }
    }

    // ------------------------------------------------------------------ devices

    /// Runs `f` on device `di` with a context at time `t`, then applies what it did.
    pub(crate) fn with_device<R>(
        &mut self,
        di: usize,
        t: SimTime,
        f: impl FnOnce(&mut dyn Device, &mut crate::ctx::DevCtx) -> R,
    ) -> R {
        self.stats.device_calls += 1;
        let mut dev = std::mem::replace(&mut self.devices[di].dev, Box::new(NullDevice));
        let r = {
            let mut ctx = crate::ctx::DevCtx {
                bench: self,
                dev: di,
                now: t,
            };
            f(dev.as_mut(), &mut ctx)
        };
        self.devices[di].dev = dev;
        r
    }

    /// User interaction with a part (button, knob, switch).
    pub fn input(&mut self, part: &str, input: &UserInput) -> Result<(), SimError> {
        let di = self
            .devices
            .iter()
            .position(|d| d.spec.part == part)
            .ok_or_else(|| SimError::NotFound(part.into()))?;
        let t = self.time;
        self.with_device(di, t, |d, ctx| d.on_input(ctx, input));
        self.commit_analog(t)?;
        self.process_edges()?;
        Ok(())
    }

    /// Changes a part property while running; rebuilds if the device cannot apply it live.
    pub fn set_part_param(&mut self, part: &str, key: &str, value: f64) -> Result<(), SimError> {
        if let Some(p) = self.project.part_mut(part) {
            p.props
                .insert(key.into(), bb_circuit::PropValue::Num(value));
        }
        let t = self.time;
        let live = match self.devices.iter().position(|d| d.spec.part == part) {
            Some(di) => {
                self.devices[di]
                    .spec
                    .props
                    .insert(key.into(), bb_circuit::PropValue::Num(value));
                self.with_device(di, t, |d, ctx| d.set_param(ctx, key, value))
            }
            None => false,
        };
        // plain SPICE values (capacitance, inductance) go straight into the session
        let mut patched = live;
        if !live {
            let spec_el = match key {
                "capacitance" => Some(("C", "c")),
                "inductance" => Some(("L", "l")),
                _ => None,
            };
            if let Some((el, param)) = spec_el
                && let Some(name) = self.built.element(part, el).map(String::from)
                && let Some(s) = self.analog.session.as_mut()
                && s.set_param(&name, param, value).is_ok()
            {
                patched = true;
            }
        }
        if !patched {
            self.rebuild()?;
        }
        self.commit_analog(t)?;
        self.process_edges()
    }

    /// Replaces the project (an edit from the UI) and rebuilds the bench, keeping MCU, device,
    /// world and analog state where parts are unchanged.
    pub fn update_project(&mut self, project: Project) -> Result<(), SimError> {
        self.project = project;
        self.rebuild()
    }

    /// Rebuilds netlist + session from the project and damage state (SIM-7: MCU/world/devices
    /// keep their state; capacitor voltages carry over as node sets).
    pub fn rebuild(&mut self) -> Result<(), SimError> {
        self.stats.rebuilds += 1;
        self.erc_cache = None;
        let mut opts = NetlistOptions::bench();
        opts.damaged = self.damaged.clone();
        opts.loads = self.loads.clone();
        let built = netlist::build(&self.project, &self.lib, &opts)
            .map_err(|e| SimError::Build(e.to_string()))?;
        // node voltages by net name
        let mut ic = HashMap::new();
        if let Some(s) = &self.analog.session {
            for n in &self.nets {
                if let (NetKind::Analog(name), Some(node)) = (&n.kind, n.node)
                    && name != "0"
                {
                    ic.insert(name.clone(), s.voltage_at(node));
                }
            }
        }
        for (k, v) in &built.ext {
            self.ext_values.entry(k.clone()).or_insert(*v);
        }
        // devices: keep matching ones, add new, drop removed
        let old_parts: Vec<String> = self.devices.iter().map(|d| d.spec.part.clone()).collect();
        let mut old: Vec<DevRt> = std::mem::take(&mut self.devices);
        let mut new_devs = Vec::new();
        let mut fresh = Vec::new();
        for spec in &built.devices {
            match old
                .iter()
                .position(|d| d.spec.part == spec.part && d.spec.behaviour == spec.behaviour)
            {
                Some(i) => {
                    let mut d = old.remove(i);
                    d.spec = spec.clone();
                    new_devs.push(d);
                }
                None => {
                    fresh.push(new_devs.len());
                    new_devs.push(DevRt {
                        spec: spec.clone(),
                        dev: Box::new(NullDevice),
                        rng: Rng::new(0),
                        ports: vec![],
                        nets: vec![],
                        drives: vec![],
                        watches: vec![],
                        watch_ids: vec![],
                        tick: SimTime::ZERO,
                        tick_gen: 0,
                        token_gen: HashMap::new(),
                        health: Health::Ok,
                        smoke: 0.0,
                        smoke_at: SimTime::ZERO,
                    });
                }
            }
        }
        // keep damaged parts' health for the UI
        for d in old {
            if self.damaged.contains_key(&d.spec.part) {
                self.damage.remember(&d.spec.part, d.health.clone());
            }
        }
        for &i in &fresh {
            let spec = new_devs[i].spec.clone();
            new_devs[i] = self.make_device(&spec);
        }
        // health of damaged parts survives as a placeholder device
        for d in new_devs.iter_mut() {
            if let Some(h) = self.damage.remembered(&d.spec.part) {
                d.health = h;
            }
        }
        self.devices = new_devs;
        // MCUs: keep matching by part id, add new ones (fresh cores, erased flash)
        let specs = built.mcus.clone();
        self.mcus
            .retain(|m| specs.iter().any(|s| s.part == m.spec.part));
        for (i, spec) in specs.iter().enumerate() {
            if !self.mcus.iter().any(|m| m.spec.part == spec.part) {
                let m = self.make_mcu(spec, i, None)?;
                self.mcus.push(m);
            }
        }
        self.built = built;
        // wakes refer to device indices: rebuild the heap
        let wakes: Vec<Wake> = std::mem::take(&mut self.heap)
            .into_iter()
            .map(|r| r.0)
            .collect();
        self.wire_up(Some(ic))?;
        self.heap.clear();
        self.push_wake(self.time + self.world_dt, WakeKind::World);
        for d in 0..self.devices.len() {
            if self.devices[d].tick > SimTime::ZERO {
                self.devices[d].tick_gen += 1;
                let (t, g) = (self.time + self.devices[d].tick, self.devices[d].tick_gen);
                self.push_wake(
                    t,
                    WakeKind::Tick {
                        dev: d,
                        generation: g,
                    },
                );
            }
        }
        // pending device wake-ups (protocol timers, conversions) follow their device
        for w in wakes {
            if let WakeKind::Device {
                dev,
                token,
                generation,
            } = w.kind
                && let Some(part) = old_parts.get(dev)
                && let Some(ni) = self.devices.iter().position(|d| &d.spec.part == part)
                && !fresh.contains(&ni)
            {
                self.push_wake(
                    w.time.max(self.time),
                    WakeKind::Device {
                        dev: ni,
                        token,
                        generation,
                    },
                );
            }
        }
        let t = self.time;
        for &i in &fresh {
            self.with_device(i, t, |d, ctx| d.on_start(ctx));
        }
        self.settle(t)?;
        Ok(())
    }

    /// "Repair all" (WB-13): every damaged part is replaced.
    pub fn repair_all(&mut self) -> Result<(), SimError> {
        if self.damaged.is_empty() && self.devices.iter().all(|d| d.health == Health::Ok) {
            return Ok(());
        }
        self.damaged.clear();
        self.damage.repair_all();
        for d in self.devices.iter_mut() {
            d.health = Health::Ok;
            d.smoke = 0.0;
        }
        for m in self.mcus.iter_mut() {
            for p in m.pins.iter_mut() {
                p.damaged = false;
            }
        }
        self.rebuild()
    }

    // ------------------------------------------------------------------ world

    /// Sets a world variable (World panel slider).
    pub fn set_world(&mut self, key: &str, value: f64) {
        self.world.set(key, value);
    }
    pub fn water(&mut self, pot: &str, ml: f64) {
        WorldMut::water(&mut self.world, pot, ml);
    }

    pub(crate) fn world_view(&self) -> &dyn WorldView {
        &self.world
    }
}
