//! Netlister: project → `bb_spice` netlist.
//!
//! **Bench mode** (co-simulation): every net touched by a SPICE element becomes a node; MCU
//! GPIO pins on such nets become two co-simulation ports (high-side to the MCU's supply net,
//! low-side to ground — so output current really comes out of the 5 V rail); behavioural
//! device pins become one port each. Nets that only connect co-simulation pins (an MCU pin
//! wired straight to a sensor's data pin) are left out of SPICE and solved in closed form by
//! bb-sim ([`NetKind::Digital`]). Nets without a DC path to ground get a 1 GΩ shunt so the
//! matrix never goes singular.
//!
//! **Pro mode** (export / analyses): `pro` templates where defined, MCUs as
//! `XU1 VCC 0 A0 … ATMEGA328P fw=garden-node.hex sync=1u` with a stub subcircuit, behavioural
//! parts as their Pro subcircuits (`XJ1 VCC 0 A0 CAPSOIL vwc=42`).

use crate::connectivity::{Connectivity, NetId};
use crate::library::Library;
use crate::part::{PartDef, PinRole, PropValue, RatingKind, ValueRef};
use crate::project::{FirmwareRef, Project};
use crate::template::{self, TemplateCtx};
use bb_core::{PinDrive, Pos};
use bb_spice::{CircuitBuilder, Netlist};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum NetlistError {
    #[error("{part}: {msg}")]
    Part { part: String, msg: String },
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetlistMode {
    Bench,
    Pro,
}

/// How a damaged part is netlisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DamageEffect {
    /// The part conducts nothing (burnt LED, blown resistor).
    Open,
    /// Its first two pins are shorted (failed capacitor, shorted diode).
    Short,
}

/// An extra load between two pins' nets (an instrument's input impedance, e.g. a DMM's
/// 10 MΩ ‖ 100 pF).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Load {
    pub id: String,
    pub a: crate::project::Endpoint,
    pub b: crate::project::Endpoint,
    /// Ohms (`f64::INFINITY` = none).
    pub r: f64,
    /// Farads (0 = none).
    pub c: f64,
}

/// Netlisting options.
#[derive(Debug, Clone)]
pub struct NetlistOptions<'a> {
    pub mode: NetlistMode,
    /// Damaged parts (part id → effect).
    pub damaged: BTreeMap<String, DamageEffect>,
    /// World values for `$(world:…)` (Pro export freezes them).
    pub world: Option<&'a bb_world::WorldConfig>,
    /// Add 1 TΩ shunts on every node (bench default).
    pub shunts: bool,
    /// Pro: write `fw=… sync=…` on MCU `X` lines (display text; not runnable/ngspice-safe).
    pub mcu_params: bool,
    /// Instrument loads injected into the circuit.
    pub loads: Vec<Load>,
}

impl NetlistOptions<'_> {
    pub fn bench() -> NetlistOptions<'static> {
        NetlistOptions {
            mode: NetlistMode::Bench,
            damaged: BTreeMap::new(),
            world: None,
            shunts: true,
            mcu_params: false,
            loads: Vec::new(),
        }
    }
    pub fn pro(world: Option<&bb_world::WorldConfig>) -> NetlistOptions<'_> {
        NetlistOptions {
            mode: NetlistMode::Pro,
            damaged: BTreeMap::new(),
            world,
            shunts: false,
            mcu_params: false,
            loads: Vec::new(),
        }
    }
}

/// How bb-sim treats a net.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetKind {
    /// Nothing to simulate (unconnected pin, empty breadboard strip).
    Isolated,
    /// Only co-simulation pins: solved in closed form by bb-sim.
    Digital,
    /// A SPICE node.
    Analog(String),
}

/// Which side of an MCU pin driver a port models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortRole {
    /// Between the pin and the MCU supply (output high, pull-up).
    McuHigh,
    /// Between the pin and ground (output low, input leakage).
    McuLow,
    /// A behavioural device pin to ground.
    Device,
}

/// A co-simulation port in the SPICE netlist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortSpec {
    /// Port element name (`pu1_d9_h`); `bb_spice::Session::port` finds it.
    pub name: String,
    pub part: String,
    pub pin: String,
    pub role: PortRole,
    pub node_p: String,
    pub node_n: String,
}

/// One MCU pin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McuPinSpec {
    /// Part pin id (`D9`).
    pub pin: String,
    /// MCU pin name for the board map (`D9`, `A0`, `GPIO4`).
    pub mcu: String,
    pub net: NetId,
    pub kind: NetKind,
    pub hi_port: Option<String>,
    pub lo_port: Option<String>,
}

/// An MCU to instantiate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McuSpec {
    pub part: String,
    pub reference: String,
    pub board: String,
    /// `"avr"` or `"host"`.
    pub core: String,
    pub label: String,
    pub vcc_net: Option<NetId>,
    pub vcc_node: Option<String>,
    pub pins: Vec<McuPinSpec>,
    /// Analog node of the RESET pin, if wired into the circuit.
    pub reset_node: Option<String>,
    pub aref_node: Option<String>,
    pub firmware: Option<FirmwareRef>,
}

/// One pin of a behavioural device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevicePinSpec {
    pub pin: String,
    pub index: usize,
    pub net: Option<NetId>,
    pub kind: NetKind,
    /// Port when the device drives this pin on an analog net.
    pub port: Option<String>,
    /// The pin is part of the device's own SPICE template (read-only for the device).
    pub spice: bool,
}

/// A behavioural device to instantiate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceSpec {
    pub part: String,
    pub reference: String,
    pub def: String,
    pub behaviour: String,
    pub pins: Vec<DevicePinSpec>,
    /// Effective properties (defaults filled, choice-implied numbers included).
    pub props: BTreeMap<String, PropValue>,
    /// External variables of this part: local name → (full `ext()` name, default).
    pub ext: BTreeMap<String, (String, f64)>,
    /// SPICE elements of this part: local spec (`D`, `R:esr`) → element name.
    pub elements: BTreeMap<String, String>,
    pub attach: BTreeMap<String, String>,
    /// Bench position (m) for the world.
    pub pos: Option<Pos>,
    pub damaged: bool,
}

/// A resolved damage rating.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RatingSpec {
    pub part: String,
    pub reference: String,
    pub element: String,
    pub kind: RatingKind,
    pub limit: f64,
    pub tau: f64,
    pub destroy_at: f64,
    pub message: String,
    pub effect: String,
}

/// Everything bb-sim needs to build a bench.
#[derive(Debug, Clone)]
pub struct BuiltNetlist {
    pub netlist: Netlist,
    pub connectivity: Connectivity,
    pub net_kind: Vec<NetKind>,
    pub ports: Vec<PortSpec>,
    pub mcus: Vec<McuSpec>,
    pub devices: Vec<DeviceSpec>,
    pub ratings: Vec<RatingSpec>,
    /// (part id, local spec, element name) for every SPICE element.
    pub elements: Vec<(String, String, String)>,
    /// Every `ext()` variable with its default.
    pub ext: Vec<(String, f64)>,
    pub warnings: Vec<String>,
}

impl BuiltNetlist {
    /// SPICE node of a part pin (analog nets only).
    pub fn node_of(&self, part: &str, pin: &str) -> Option<&str> {
        let n = self.connectivity.net_of(part, pin)?;
        match &self.net_kind[n.0] {
            NetKind::Analog(s) => Some(s),
            _ => None,
        }
    }
    /// A co-simulation session with every `ext()` variable at its default.
    pub fn session(
        &self,
        opts: bb_spice::SessionOptions,
    ) -> Result<bb_spice::Session, bb_spice::SimError> {
        bb_spice::Session::with_ext(&self.netlist, opts, &self.ext)
    }

    /// Element name of a part's local element spec.
    pub fn element(&self, part: &str, spec: &str) -> Option<&str> {
        self.elements
            .iter()
            .find(|(p, s, _)| p == part && s == spec)
            .map(|(_, _, e)| e.as_str())
    }
}

fn world_lookup(
    world: Option<&bb_world::WorldConfig>,
    attach: &BTreeMap<String, String>,
    key: &str,
) -> Option<f64> {
    let w = world?;
    let pot = attach
        .get("pot")
        .cloned()
        .or_else(|| w.pots.first().map(|p| p.id.clone()));
    match key {
        "vwc" => pot.and_then(|id| w.pots.iter().find(|p| p.id == id).map(|p| p.vwc)),
        "lux" => Some(w.light.ambient_lux),
        "air.temp_c" | "temp" => Some(w.air.temp_c),
        "air.rh" | "rh" => Some(w.air.rh),
        _ => w.vars.get(key).copied(),
    }
}

fn port_name(reference: &str, pin: &str, suffix: &str) -> String {
    let s = template::sanitize(&format!("{reference}_{pin}{suffix}")).to_ascii_lowercase();
    if s.starts_with('p') {
        format!("p_{s}")
    } else {
        format!("p{s}")
    }
}

/// Builds the netlist for a project.
pub fn build(
    project: &Project,
    lib: &Library,
    opts: &NetlistOptions,
) -> Result<BuiltNetlist, NetlistError> {
    let conn = Connectivity::compute(project, lib);
    let bench = opts.mode == NetlistMode::Bench;
    let mut warnings = Vec::new();

    // which pins are SPICE / MCU / device pins
    struct PartInfo {
        def: std::sync::Arc<PartDef>,
        lines: Vec<String>,
        spice_pins: BTreeSet<String>,
        mcu_pins: BTreeSet<String>,
        dev_pins: BTreeSet<String>,
        damaged: Option<DamageEffect>,
    }
    let mut infos: HashMap<String, PartInfo> = HashMap::new();
    for p in &project.parts {
        let Some(def) = lib.get(&p.part).cloned() else {
            warnings.push(format!(
                "{}: unknown part `{}` skipped",
                p.reference, p.part
            ));
            continue;
        };
        let damaged = opts.damaged.get(&p.id).copied();
        let lines: Vec<String> = match (opts.mode, damaged) {
            (_, Some(DamageEffect::Open)) => def.model.damaged.clone().unwrap_or_default(),
            (_, Some(DamageEffect::Short)) => {
                let pins: Vec<&str> = def.pins.iter().take(2).map(|p| p.id.as_str()).collect();
                if pins.len() == 2 {
                    vec![format!("$(el:R:dmg) $({}) $({}) 0.05", pins[0], pins[1])]
                } else {
                    Vec::new()
                }
            }
            (NetlistMode::Bench, None) => def.model.spice.clone(),
            (NetlistMode::Pro, None) => def
                .model
                .pro
                .clone()
                .unwrap_or_else(|| def.model.spice.clone()),
        };
        let mut spice_pins = BTreeSet::new();
        for l in &lines {
            for ph in template::placeholders(l) {
                if def.pin(&ph).is_some() {
                    spice_pins.insert(ph);
                }
            }
            // `$(pin:x)` form
            let mut rest = l.as_str();
            while let Some(i) = rest.find("$(pin:") {
                let after = &rest[i + 6..];
                let j = after.find(')').unwrap_or(after.len());
                spice_pins.insert(after[..j].to_string());
                rest = &after[j.min(after.len())..];
            }
        }
        // spice_card: its card text references pins indirectly
        if def.id == "spice_card"
            && let Some(card) = Project::prop(&def, p, "card")
        {
            for ph in template::placeholders(&card.as_text()) {
                if def.pin(&ph).is_some() {
                    spice_pins.insert(ph);
                }
            }
        }
        let mcu_pins: BTreeSet<String> = if def.is_mcu() {
            def.pins
                .iter()
                .filter(|x| x.mcu.is_some())
                .map(|x| x.id.clone())
                .collect()
        } else {
            BTreeSet::new()
        };
        let dev_pins: BTreeSet<String> = if bench && damaged.is_none() {
            def.device_pins().into_iter().collect()
        } else {
            BTreeSet::new()
        };
        infos.insert(
            p.id.clone(),
            PartInfo {
                def,
                lines,
                spice_pins,
                mcu_pins,
                dev_pins,
                damaged,
            },
        );
    }

    // ---- classify nets
    let endpoint_net = |e: &crate::project::Endpoint| match e {
        crate::project::Endpoint::Pin { part, pin } => conn.net_of(part, pin),
        crate::project::Endpoint::Free { .. } => None,
    };
    let load_nets: BTreeSet<usize> = opts
        .loads
        .iter()
        .flat_map(|l| [endpoint_net(&l.a), endpoint_net(&l.b)])
        .flatten()
        .map(|n| n.0)
        .collect();
    let mut net_kind = vec![NetKind::Isolated; conn.nets.len()];
    for net in &conn.nets {
        let mut spice = net.name == "0" || load_nets.contains(&net.id.0);
        let mut cosim = 0;
        let mut device_pins = 0;
        for t in &net.pins {
            let Some(info) = infos.get(&t.part) else {
                continue;
            };
            if info.spice_pins.contains(&t.pin) {
                spice = true;
            }
            if (bench || opts.mode == NetlistMode::Pro) && info.mcu_pins.contains(&t.pin) {
                cosim += 1;
            }
            if info.dev_pins.contains(&t.pin) {
                cosim += 1;
                device_pins += 1;
            }
        }
        net_kind[net.id.0] = if spice {
            NetKind::Analog(net.name.clone())
        } else if bench && (cosim >= 2 || device_pins >= 1) {
            NetKind::Digital
        } else if !bench && cosim >= 1 {
            // Pro: MCU pins appear on X lines, so the net needs a node
            NetKind::Analog(net.name.clone())
        } else {
            NetKind::Isolated
        };
    }
    // ground nets other than "0" must still exist as nodes if analog (named GND*)
    let node_of = |part: &str, pin: &str| -> Option<String> {
        let n = conn.net_of(part, pin)?;
        match &net_kind[n.0] {
            NetKind::Analog(s) => Some(s.clone()),
            _ => Some(conn.net(n).name.clone()),
        }
    };

    // ---- elements
    let mut b = CircuitBuilder::new(&project.name);
    let mut lib_cards: Vec<String> = Vec::new();
    let mut element_lines: Vec<(String, Vec<String>)> = Vec::new();
    let mut elements = Vec::new();
    let mut ext = Vec::new();
    let mut ratings = Vec::new();
    let mut devices = Vec::new();
    let mut mcus = Vec::new();
    let mut ports = Vec::new();
    for p in &project.parts {
        let Some(info) = infos.get(&p.id) else {
            continue;
        };
        let def = &info.def;
        let pin_node = |pin: &str| -> Option<String> {
            def.pin(pin)?;
            node_of(&p.id, pin)
        };
        let prop = |k: &str| Project::prop(def, p, k);
        let world = |k: &str| world_lookup(opts.world, &p.attach, k);
        let ctx = TemplateCtx {
            reference: &p.reference,
            pin_node: &pin_node,
            prop: &prop,
            world: &world,
        };
        let expand = |line: &str| -> Result<Option<String>, NetlistError> {
            let mut out = template::expand(line, &ctx).map_err(|msg| NetlistError::Part {
                part: p.reference.clone(),
                msg,
            })?;
            // a property may itself hold a template (spice_card)
            if let Some(s) = &out
                && s.contains("$(")
            {
                out = template::expand(s, &ctx).map_err(|msg| NetlistError::Part {
                    part: p.reference.clone(),
                    msg,
                })?;
            }
            Ok(out)
        };
        let mut lines = Vec::new();
        for l in &info.lines {
            if let Some(s) = expand(l)? {
                for spec in template::element_specs(l) {
                    elements.push((
                        p.id.clone(),
                        spec.clone(),
                        template::element_spec_name(&spec, &p.reference).to_ascii_lowercase(),
                    ));
                }
                lines.push(s);
            }
        }
        if info.damaged.is_none() {
            for l in &def.model.library {
                if let Some(s) = expand(l)?
                    && !lib_cards.contains(&s)
                {
                    lib_cards.push(s);
                }
            }
        }
        let ext_default = |e: &crate::part::ExtDef| match &e.default {
            ValueRef::Num(v) => *v,
            ValueRef::Expr(x) => {
                let k = x.trim().trim_start_matches("$(").trim_end_matches(')');
                Project::prop_f64(def, p, k).unwrap_or(0.0)
            }
        };
        for e in &def.model.ext {
            ext.push((format!("{}.{}", p.reference, e.name), ext_default(e)));
        }
        // MCU (Pro: an X line with a stub subcircuit)
        if let Some(m) = &def.model.mcu
            && info.damaged.is_none()
        {
            let vcc_net = conn.net_of(&p.id, &m.vcc_pin);
            let vcc_node = vcc_net.and_then(|n| match &net_kind[n.0] {
                NetKind::Analog(s) => Some(s.clone()),
                _ => None,
            });
            let mut pins = Vec::new();
            for pd in def.pins.iter().filter(|x| x.mcu.is_some()) {
                let Some(net) = conn.net_of(&p.id, &pd.id) else {
                    continue;
                };
                let kind = net_kind[net.0].clone();
                let (mut hi, mut lo) = (None, None);
                if bench && let NetKind::Analog(node) = &kind {
                    if let Some(v) = &vcc_node {
                        let hn = port_name(&p.reference, &pd.id, "_h");
                        b.port(&hn, node, v, PinDrive::HighZ);
                        ports.push(PortSpec {
                            name: hn.clone(),
                            part: p.id.clone(),
                            pin: pd.id.clone(),
                            role: PortRole::McuHigh,
                            node_p: node.clone(),
                            node_n: v.clone(),
                        });
                        hi = Some(hn);
                    }
                    let ln = port_name(&p.reference, &pd.id, "_l");
                    b.port(&ln, node, "0", PinDrive::HighZ);
                    ports.push(PortSpec {
                        name: ln.clone(),
                        part: p.id.clone(),
                        pin: pd.id.clone(),
                        role: PortRole::McuLow,
                        node_p: node.clone(),
                        node_n: "0".into(),
                    });
                    lo = Some(ln);
                }
                pins.push(McuPinSpec {
                    pin: pd.id.clone(),
                    mcu: pd.mcu.clone().unwrap_or_default(),
                    net,
                    kind,
                    hi_port: hi,
                    lo_port: lo,
                });
            }
            let analog_node = |pin: &str| -> Option<String> {
                let n = conn.net_of(&p.id, pin)?;
                match &net_kind[n.0] {
                    NetKind::Analog(s) => Some(s.clone()),
                    _ => None,
                }
            };
            if !bench {
                let mcu_name = def.part_number.to_ascii_uppercase();
                let mcu_name = if mcu_name.is_empty() {
                    def.id.to_ascii_uppercase()
                } else {
                    template::sanitize(&mcu_name)
                };
                let stub = if mcus.is_empty() {
                    mcu_name.clone()
                } else {
                    format!("{mcu_name}_{}", template::sanitize(&p.reference))
                };
                let connected: Vec<&McuPinSpec> = pins
                    .iter()
                    .filter(|x| conn.net(x.net).pins.iter().any(|t| t.part != p.id))
                    .collect();
                let vcc = vcc_node.clone().unwrap_or_else(|| "vcc".into());
                let nodes: Vec<String> = connected
                    .iter()
                    .map(|x| match &x.kind {
                        NetKind::Analog(s) => s.clone(),
                        _ => conn.net(x.net).name.clone(),
                    })
                    .collect();
                let fw = p
                    .firmware
                    .as_ref()
                    .and_then(|f| f.sketch.rsplit('/').next().map(|s| format!("{s}.hex")))
                    .unwrap_or_else(|| "firmware.hex".into());
                let params = format!(
                    "fw={} sync={}u",
                    fw,
                    bb_spice::expr::fmt_g(project.settings.sync_quantum_us)
                );
                let xname = template::element_name('X', &p.reference, None);
                if opts.mcu_params {
                    lines.push(format!(
                        "{xname} {vcc} 0 {} {stub} {params}",
                        nodes.join(" ")
                    ));
                } else {
                    warnings.push(format!("{xname}: {params}"));
                    lines.push(format!("{xname} {vcc} 0 {} {stub}", nodes.join(" ")));
                }
                let mut sub = format!(".subckt {stub} vcc vss");
                for (i, _) in connected.iter().enumerate() {
                    sub.push_str(&format!(" p{}", i + 1));
                }
                if opts.mcu_params {
                    // the X line passes `sync=…`: declare it (an unknown parameter warns)
                    sub.push_str(" params: sync=1u");
                }
                sub.push('\n');
                for (i, _) in connected.iter().enumerate() {
                    sub.push_str(&format!("R{} p{} vss 1G\n", i + 1, i + 1));
                }
                sub.push_str(&format!(".ends {stub}"));
                lib_cards.push(sub);
            }
            mcus.push(McuSpec {
                part: p.id.clone(),
                reference: p.reference.clone(),
                board: m.board.clone(),
                core: m.core.clone(),
                label: m.label.clone(),
                vcc_net,
                vcc_node,
                pins,
                reset_node: analog_node("RESET")
                    .or_else(|| analog_node("EN"))
                    .or_else(|| analog_node("RUN")),
                aref_node: analog_node("AREF"),
                firmware: p.firmware.clone(),
            });
        }
        // behavioural device
        if let Some(beh) = &def.model.behaviour
            && bench
            && (def.model.mcu.is_none() || beh == "board_power")
        {
            let mut dpins = Vec::new();
            for (index, pd) in def.pins.iter().enumerate() {
                let net = conn.net_of(&p.id, &pd.id);
                let kind = net
                    .map(|n| net_kind[n.0].clone())
                    .unwrap_or(NetKind::Isolated);
                let is_dev = info.dev_pins.contains(&pd.id);
                let mut port = None;
                if is_dev
                    && let NetKind::Analog(node) = &kind
                    && node != "0"
                    && pd.role != PinRole::Ground
                {
                    let pn = port_name(&p.reference, &pd.id, "");
                    b.port(&pn, node, "0", PinDrive::HighZ);
                    ports.push(PortSpec {
                        name: pn.clone(),
                        part: p.id.clone(),
                        pin: pd.id.clone(),
                        role: PortRole::Device,
                        node_p: node.clone(),
                        node_n: "0".into(),
                    });
                    port = Some(pn);
                }
                dpins.push(DevicePinSpec {
                    pin: pd.id.clone(),
                    index,
                    net,
                    kind,
                    port,
                    spice: info.spice_pins.contains(&pd.id),
                });
            }
            let mut props = BTreeMap::new();
            for pd in &def.props {
                if let Some(v) = Project::prop(def, p, &pd.key) {
                    props.insert(pd.key.clone(), v);
                }
                if let crate::part::PropKind::Choice { options, .. } = &pd.kind {
                    let sel = Project::prop(def, p, &pd.key)
                        .map(|v| v.as_text())
                        .unwrap_or_default();
                    if let Some(o) = options.iter().find(|o| o.value == sel) {
                        for (k, v) in &o.sets {
                            props.insert(k.clone(), PropValue::Num(*v));
                        }
                    }
                }
            }
            devices.push(DeviceSpec {
                part: p.id.clone(),
                reference: p.reference.clone(),
                def: def.id.clone(),
                behaviour: beh.clone(),
                pins: dpins,
                props,
                ext: def
                    .model
                    .ext
                    .iter()
                    .map(|e| {
                        (
                            e.name.clone(),
                            (format!("{}.{}", p.reference, e.name), ext_default(e)),
                        )
                    })
                    .collect(),
                elements: elements
                    .iter()
                    .filter(|(pid, _, _)| pid == &p.id)
                    .map(|(_, s, e)| (s.clone(), e.clone()))
                    .collect(),
                attach: p.attach.clone(),
                pos: Project::world_pos(p),
                damaged: info.damaged.is_some(),
            });
        }
        // ratings
        if info.damaged.is_none() {
            for r in &def.ratings {
                let limit = match &r.limit {
                    ValueRef::Num(v) => *v,
                    ValueRef::Expr(e) => {
                        let k = e.trim().trim_start_matches("$(").trim_end_matches(')');
                        Project::prop_f64(def, p, k).unwrap_or(f64::INFINITY)
                    }
                };
                ratings.push(RatingSpec {
                    part: p.id.clone(),
                    reference: p.reference.clone(),
                    element: template::element_spec_name(&r.element, &p.reference)
                        .to_ascii_lowercase(),
                    kind: r.kind,
                    limit,
                    tau: r.tau,
                    destroy_at: r.destroy_at,
                    message: r.message.clone(),
                    effect: if r.effect.is_empty() {
                        "open".into()
                    } else {
                        r.effect.clone()
                    },
                });
            }
        }
        if !lines.is_empty() {
            element_lines.push((p.reference.clone(), lines));
        }
    }

    // instrument loads
    let mut load_lines = Vec::new();
    for l in &opts.loads {
        let node = |e: &crate::project::Endpoint| {
            endpoint_net(e).and_then(|n| match &net_kind[n.0] {
                NetKind::Analog(s) => Some(s.clone()),
                _ => None,
            })
        };
        let (Some(a), Some(b)) = (node(&l.a), node(&l.b)) else {
            warnings.push(format!("load {}: endpoints not on the circuit", l.id));
            continue;
        };
        let id = template::sanitize(&l.id).to_ascii_lowercase();
        if l.r.is_finite() && l.r > 0.0 {
            load_lines.push(format!("rinst_{id} {a} {b} {}", bb_spice::expr::fmt_g(l.r)));
        }
        if l.c > 0.0 {
            load_lines.push(format!("cinst_{id} {a} {b} {}", bb_spice::expr::fmt_g(l.c)));
        }
    }
    if !load_lines.is_empty() {
        element_lines.push(("instruments".into(), load_lines));
    }

    // ---- assemble
    for card in &lib_cards {
        b.cards(card)
            .map_err(|e| NetlistError::Other(format!("model library: {e}")))?;
    }
    for (reference, lines) in &element_lines {
        b.cards(&lines.join("\n")).map_err(|e| NetlistError::Part {
            part: reference.clone(),
            msg: e.to_string(),
        })?;
    }
    if opts.shunts {
        // 1 pS from every node to ground (like ngspice's rshunt): keeps nodes that only see
        // capacitors, controlled-source inputs or open switches solvable; negligible load.
        let mut shunt_text = String::new();
        let mut seen = BTreeSet::new();
        for k in &net_kind {
            if let NetKind::Analog(name) = k {
                let l = name.to_ascii_lowercase();
                if l != "0" && seen.insert(l.clone()) {
                    shunt_text.push_str(&format!("rshunt_{l} {l} 0 1T\n"));
                }
            }
        }
        if !shunt_text.is_empty() {
            b.cards(&shunt_text)
                .map_err(|e| NetlistError::Other(format!("shunts: {e}")))?;
        }
    }
    let mut netlist = b.build();
    if !bench {
        // keep the title readable
        netlist.title = format!("{} — generated, editable", project.name);
    }
    Ok(BuiltNetlist {
        netlist,
        connectivity: conn,
        net_kind,
        ports,
        mcus,
        devices,
        ratings,
        elements,
        ext,
        warnings,
    })
}

#[cfg(test)]
mod tests;
