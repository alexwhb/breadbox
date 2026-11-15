//! [`PluginHost`]: turns the active plugins into what the engine and the UI consume —
//!
//! * a parts [`Library`] (built-in + plugin parts, board variants, generator sources; same
//!   schema and validator, `Library::add_json`),
//! * a `DeviceRegistry` with a [`crate::device::WasmDevice`] factory per plugin behaviour,
//! * `BenchOptions` hooks: radio packs (LoRa gateway, extensions) on the radio service, world
//!   models on the world,
//! * UI data: instrument panels + bench art, themes, lesson folders, analyses, and the live
//!   "meter load shifts the node by −0.4 mV" callout.

use crate::analysis::AnalysisOutput;
use crate::device::WasmDevice;
use crate::manifest::{AnalysisInfo, Kind, Manifest, Permission};
use crate::panel::Panel;
use crate::registry::Registry;
use crate::sandbox::Compiled;
use crate::theme::Theme;
use crate::world::WasmWorldModel;
use crate::{PluginError, io};
use bb_circuit::{Library, Project};
use bb_sim::radio::RadioSetup;
use bb_sim::{Bench, BenchOptions, DeviceRegistry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Built-in behaviours a plugin part may use only with `radio:space` (radio modules).
const RADIO_BEHAVIOURS: &[&str] = &["esp01", "hm10", "hc05", "rylr896"];

/// URL scheme for plugin assets in part definitions (`plugin://<id>/<path>`).
pub const ASSET_SCHEME: &str = "plugin://";

/// An instrument a plugin adds (Plugins screen + bench).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstrumentInfo {
    pub plugin: String,
    /// Part definition placed on the bench ("Place on bench").
    pub part: String,
    pub name: String,
    pub panel: Option<Panel>,
    /// Bench art (absolute path).
    pub art: Option<PathBuf>,
    pub symbol: Option<PathBuf>,
}

/// The live loading callout of an instrument on the bench.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadingCallout {
    /// Net being probed (`A0`).
    pub net: String,
    /// What drives it (`J1 out`).
    pub source: Option<String>,
    /// Node shift caused by the meter (V, negative = pulled down).
    pub shift: f64,
    /// `Probing A0 (J1 out): meter load shifts the node by −0.4 mV — shown as it would be on a
    /// real bench.`
    pub text: String,
}

struct Hosted {
    manifest: Arc<Manifest>,
    root: PathBuf,
    compiled: Option<Arc<Compiled>>,
    parts: Vec<String>,
    panel: Option<Panel>,
    theme: Option<Theme>,
}

/// Active plugins, adapted.
pub struct PluginHost {
    hosted: Vec<Hosted>,
    library: Arc<Library>,
    devices: Arc<DeviceRegistry>,
    radio_setup: Vec<RadioSetup>,
    world_models: Vec<bb_sim::bench::WorldModelFactory>,
    /// Plugins that could not be loaded (shown on the Plugins screen).
    pub errors: Vec<PluginError>,
    project_dir: Option<PathBuf>,
}

impl PluginHost {
    /// Every active plugin of a registry.
    pub fn new(reg: &mut Registry, project_dir: Option<&Path>) -> PluginHost {
        let ids: Vec<String> = reg.active().iter().map(|p| p.id().to_string()).collect();
        let mut items = Vec::new();
        let mut errors = Vec::new();
        for id in ids {
            let compiled = match reg.compiled(&id) {
                Ok(c) => c,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };
            if let Some(p) = reg.get(&id) {
                items.push((p.manifest.clone(), p.root.clone(), compiled));
            }
        }
        let mut h = PluginHost::build(items, project_dir);
        h.errors.splice(0..0, errors);
        h
    }

    /// Loads plugin folders directly with the permissions they declare (tests, tools).
    pub fn from_dirs(dirs: &[PathBuf], project_dir: Option<&Path>) -> PluginHost {
        let mut items = Vec::new();
        let mut errors = Vec::new();
        for d in dirs {
            let m = match Manifest::load(d) {
                Ok(m) => Arc::new(m),
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };
            let compiled = match m.model_path() {
                Some(model) => {
                    let path = d.join(crate::manifest::clean(model));
                    match std::fs::read(&path)
                        .map_err(|e| io(&path, e))
                        .and_then(|b| Compiled::new(m.clone(), d.clone(), &b))
                    {
                        Ok(c) => Some(Arc::new(c)),
                        Err(e) => {
                            errors.push(e);
                            continue;
                        }
                    }
                }
                None => None,
            };
            items.push((m, d.clone(), compiled));
        }
        let mut h = PluginHost::build(items, project_dir);
        h.errors.splice(0..0, errors);
        h
    }

    fn build(
        items: Vec<(Arc<Manifest>, PathBuf, Option<Arc<Compiled>>)>,
        project_dir: Option<&Path>,
    ) -> PluginHost {
        let mut lib = Library::with_builtin();
        let builtin_ids: Vec<String> = Library::builtin().parts().map(|p| p.id.clone()).collect();
        let mut devices = DeviceRegistry::builtin();
        let mut hosted = Vec::new();
        let mut errors = Vec::new();
        let mut radio_setup: Vec<RadioSetup> = Vec::new();
        let mut world_models: Vec<bb_sim::bench::WorldModelFactory> = Vec::new();
        let project_dir = project_dir.map(Path::to_path_buf);
        for (m, root, compiled) in items {
            match PluginHost::load_one(
                &m,
                &root,
                compiled.clone(),
                &mut lib,
                &builtin_ids,
                &mut devices,
                project_dir.clone(),
            ) {
                Ok((parts, panel, theme)) => {
                    // radio packs
                    if let Some(r) = m.provides.radio.clone() {
                        let c = compiled.clone();
                        radio_setup.push(Arc::new(move |svc, project: &Project| {
                            if r.lora || r.extension {
                                svc.activate();
                            }
                            if let Some(g) = &r.gateway
                                && let Ok(gw) =
                                    serde_json::from_value::<bb_radio::lora::Gateway>(g.clone())
                            {
                                svc.space.lora.set_gateway(gw);
                            }
                            if r.extension
                                && let Some(c) = &c
                            {
                                svc.add_extension(Box::new(crate::radio::WasmRadioExtension::new(
                                    c, project,
                                )));
                            }
                        }));
                    }
                    // world models
                    if m.has(Kind::WorldModel)
                        && let Some(c) = compiled.clone()
                    {
                        world_models.push(Arc::new(move |p: &Project| {
                            Box::new(WasmWorldModel::new(&c, p)) as Box<dyn bb_world::WorldModel>
                        }));
                    }
                    hosted.push(Hosted {
                        manifest: m,
                        root,
                        compiled,
                        parts,
                        panel,
                        theme,
                    });
                }
                Err(e) => errors.push(e),
            }
        }
        PluginHost {
            hosted,
            library: Arc::new(lib),
            devices: Arc::new(devices),
            radio_setup,
            world_models,
            errors,
            project_dir,
        }
    }

    #[allow(clippy::type_complexity)]
    fn load_one(
        m: &Arc<Manifest>,
        root: &Path,
        compiled: Option<Arc<Compiled>>,
        lib: &mut Library,
        builtin_ids: &[String],
        devices: &mut DeviceRegistry,
        project_dir: Option<PathBuf>,
    ) -> Result<(Vec<String>, Option<Panel>, Option<Theme>), PluginError> {
        let perms = m.perms();
        let err = |msg: String| PluginError::Content {
            plugin: m.id.clone(),
            msg,
        };
        let mut defs: Vec<serde_json::Value> = Vec::new();
        if let Some(parts) = &m.provides.parts {
            let path = root.join(crate::manifest::clean(parts));
            let text = std::fs::read_to_string(&path).map_err(|e| io(&path, e))?;
            let v: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| err(format!("{parts}: {e}")))?;
            let list = match v {
                serde_json::Value::Array(a) => a,
                serde_json::Value::Object(mut o) if o.contains_key("parts") => {
                    match o.remove("parts") {
                        Some(serde_json::Value::Array(a)) => a,
                        _ => return Err(err(format!("{parts}: `parts` must be a list"))),
                    }
                }
                o => vec![o],
            };
            defs.extend(list);
        }
        for b in &m.provides.boards {
            let art = b.art.as_ref().map(|a| {
                format!(
                    "{ASSET_SCHEME}{}/{}",
                    m.id,
                    crate::manifest::clean(a).display()
                )
            });
            defs.push(crate::board::part_json(m, b, art)?);
        }
        if m.provides.generator.is_some()
            && let Some(c) = &compiled
        {
            defs.push(crate::generator::part_json(c)?);
        }
        let wasm_behaviour = m.id.clone();
        let mut ids = Vec::new();
        for mut d in defs {
            let id = d
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                return Err(err("a part has no id".into()));
            }
            if builtin_ids.contains(&id) {
                return Err(err(format!("part `{id}` would replace a built-in part")));
            }
            // behaviour: the plugin's wasm model, a radio module, or a built-in behaviour
            let beh = d
                .pointer("/model/behaviour")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let beh = if beh == "wasm" || beh == m.id || beh.starts_with(&format!("{}/", m.id)) {
                if compiled.is_none() {
                    return Err(err(format!(
                        "part `{id}` uses the wasm model but the plugin has none"
                    )));
                }
                if beh == "wasm" {
                    wasm_behaviour.clone()
                } else {
                    beh
                }
            } else {
                if RADIO_BEHAVIOURS.contains(&beh.as_str())
                    && !perms.contains(&Permission::RadioSpace)
                {
                    return Err(PluginError::PermissionDenied {
                        plugin: m.id.clone(),
                        permission: Permission::RadioSpace,
                        action: format!("add the radio module part `{id}`"),
                    });
                }
                if !beh.is_empty() && !devices.contains(&beh) {
                    return Err(err(format!("part `{id}`: unknown behaviour `{beh}`")));
                }
                beh
            };
            if let Some(model) = d.get_mut("model").and_then(|x| x.as_object_mut())
                && !beh.is_empty()
            {
                model.insert("behaviour".into(), serde_json::Value::String(beh.clone()));
            }
            // an instrument's elements across its probes are a circuit load
            let has_spice = d
                .pointer("/model/spice")
                .and_then(|x| x.as_array())
                .is_some_and(|a| !a.is_empty());
            if m.has(Kind::Instrument)
                && has_spice
                && !perms.contains(&Permission::InjectLoad)
                && !perms.contains(&Permission::DrivePins)
            {
                return Err(PluginError::PermissionDenied {
                    plugin: m.id.clone(),
                    permission: Permission::InjectLoad,
                    action: format!("load the circuit with part `{id}`"),
                });
            }
            // bench art inside the plugin → plugin:// URL
            if let Some(art) = d
                .pointer("/bench/art")
                .and_then(|x| x.as_str())
                .map(String::from)
                && (art.contains('.') || art.contains('/'))
            {
                if !crate::manifest::safe_rel_path(&art)
                    || !root.join(crate::manifest::clean(&art)).exists()
                {
                    return Err(err(format!(
                        "part `{id}`: art `{art}` is not a file in the plugin"
                    )));
                }
                let url = format!(
                    "{ASSET_SCHEME}{}/{}",
                    m.id,
                    crate::manifest::clean(&art).display()
                );
                if let Some(b) = d.get_mut("bench").and_then(|x| x.as_object_mut()) {
                    b.insert("art".into(), serde_json::Value::String(url));
                }
            }
            if let Some(o) = d.as_object_mut() {
                o.entry("source")
                    .or_insert(serde_json::Value::String(m.id.clone()));
            }
            let text = d.to_string();
            lib.add_json(&text, &format!("{}:{id}", m.id), &m.id)
                .map_err(|e| err(e.to_string()))?;
            if beh == wasm_behaviour || beh.starts_with(&format!("{}/", m.id)) {
                let c = compiled.clone().expect("checked above");
                let dir = project_dir.clone();
                devices.register_fn(&beh, move |spec| {
                    Box::new(WasmDevice::new(&c, spec, dir.clone())) as Box<dyn bb_core::Device>
                });
            }
            ids.push(id);
        }
        // panel
        let panel = match &m.provides.panel {
            Some(p) => {
                let path = root.join(crate::manifest::clean(p));
                let text = std::fs::read_to_string(&path).map_err(|e| io(&path, e))?;
                let panel: Panel =
                    serde_json::from_str(&text).map_err(|e| err(format!("{p}: {e}")))?;
                let pins: Vec<String> = ids
                    .iter()
                    .filter_map(|id| lib.get(id))
                    .flat_map(|d| d.pins.iter().map(|p| p.id.clone()).collect::<Vec<_>>())
                    .collect();
                panel
                    .validate(&pins)
                    .map_err(|e| err(format!("{p}: {e}")))?;
                Some(panel)
            }
            None => None,
        };
        let theme = match &m.provides.theme {
            Some(t) => {
                let path = root.join(crate::manifest::clean(t));
                let text = std::fs::read_to_string(&path).map_err(|e| io(&path, e))?;
                Some(Theme::parse(&m.id, &text)?)
            }
            None => None,
        };
        Ok((ids, panel, theme))
    }

    /// Built-in + plugin parts.
    pub fn library(&self) -> Arc<Library> {
        self.library.clone()
    }

    /// Built-in + plugin behaviours.
    pub fn device_registry(&self) -> Arc<DeviceRegistry> {
        self.devices.clone()
    }

    /// Adds the plugins to bench options (device registry, radio service hooks, world models).
    pub fn apply(&self, opts: &mut BenchOptions) {
        opts.registry = self.devices.clone();
        opts.services.retain(|_| false);
        opts.services.push(bb_sim::radio::service_factory_with(
            self.radio_setup.clone(),
        ));
        opts.world_models.extend(self.world_models.iter().cloned());
    }

    /// `BenchOptions::default()` with the plugins applied.
    pub fn bench_options(&self) -> BenchOptions {
        let mut o = BenchOptions::default();
        self.apply(&mut o);
        o
    }

    /// Ids of the hosted plugins.
    pub fn ids(&self) -> Vec<String> {
        self.hosted.iter().map(|h| h.manifest.id.clone()).collect()
    }

    pub fn manifest(&self, id: &str) -> Option<&Manifest> {
        self.hosted
            .iter()
            .find(|h| h.manifest.id == id)
            .map(|h| &*h.manifest)
    }

    /// Part ids a plugin added.
    pub fn parts_of(&self, id: &str) -> Vec<String> {
        self.hosted
            .iter()
            .find(|h| h.manifest.id == id)
            .map(|h| h.parts.clone())
            .unwrap_or_default()
    }

    /// Resolves `plugin://<id>/<path>` (or a plugin + relative path) to a file.
    pub fn asset_path(&self, url: &str) -> Option<PathBuf> {
        let rest = url.strip_prefix(ASSET_SCHEME)?;
        let (id, rel) = rest.split_once('/')?;
        let h = self.hosted.iter().find(|h| h.manifest.id == id)?;
        crate::manifest::safe_rel_path(rel).then(|| h.root.join(crate::manifest::clean(rel)))
    }

    /// Instruments with their panels and art.
    pub fn instruments(&self) -> Vec<InstrumentInfo> {
        let mut v = Vec::new();
        for h in &self.hosted {
            if !h.manifest.has(Kind::Instrument) {
                continue;
            }
            for part in &h.parts {
                let file = |p: &Option<String>| {
                    p.as_ref()
                        .filter(|x| crate::manifest::safe_rel_path(x))
                        .map(|x| h.root.join(crate::manifest::clean(x)))
                };
                v.push(InstrumentInfo {
                    plugin: h.manifest.id.clone(),
                    part: part.clone(),
                    name: h
                        .manifest
                        .provides
                        .instrument
                        .clone()
                        .unwrap_or_else(|| h.manifest.display_name()),
                    panel: h.panel.clone(),
                    art: file(&h.manifest.provides.view3d),
                    symbol: file(&h.manifest.provides.view2d),
                });
            }
        }
        v
    }

    /// The panel of an instrument part.
    pub fn panel_for_part(&self, part_def: &str) -> Option<&Panel> {
        self.hosted
            .iter()
            .find(|h| h.parts.iter().any(|p| p == part_def))
            .and_then(|h| h.panel.as_ref())
    }

    pub fn themes(&self) -> Vec<(String, Theme)> {
        self.hosted
            .iter()
            .filter_map(|h| h.theme.clone().map(|t| (h.manifest.id.clone(), t)))
            .collect()
    }

    /// Lesson folders from lesson packs: (plugin id, folder).
    pub fn lesson_dirs(&self) -> Vec<(String, PathBuf)> {
        self.hosted
            .iter()
            .filter_map(|h| {
                let l = h.manifest.provides.lessons.as_ref()?;
                Some((
                    h.manifest.id.clone(),
                    h.root.join(crate::manifest::clean(l)),
                ))
            })
            .collect()
    }

    /// Analyses for the Pro screen: (plugin id, card info).
    pub fn analyses(&self) -> Vec<(String, AnalysisInfo)> {
        self.hosted
            .iter()
            .filter_map(|h| Some((h.manifest.id.clone(), h.manifest.provides.analysis.clone()?)))
            .collect()
    }

    /// Runs an analysis plugin on a netlist (call from a background thread).
    pub fn run_analysis(
        &self,
        id: &str,
        netlist: &str,
        options: &serde_json::Value,
    ) -> Result<AnalysisOutput, PluginError> {
        let h = self
            .hosted
            .iter()
            .find(|h| h.manifest.id == id)
            .ok_or_else(|| PluginError::NotFound(id.into()))?;
        let c = h.compiled.as_ref().ok_or_else(|| PluginError::Abi {
            plugin: id.into(),
            msg: "has no wasm model".into(),
        })?;
        crate::analysis::run(c, netlist, options)
    }

    pub fn project_dir(&self) -> Option<&Path> {
        self.project_dir.as_deref()
    }

    /// Plugin specs of every hosted plugin (detail pages).
    pub fn specs(&self) -> BTreeMap<String, BTreeMap<String, serde_json::Value>> {
        self.hosted
            .iter()
            .map(|h| (h.manifest.id.clone(), h.manifest.specs.clone()))
            .collect()
    }
}

/// The design's callout for an instrument part on a running bench: how much its input
/// impedance (ext `rin` of its first two pins) shifts the probed node, from the operating
/// point with and without it.
pub fn loading_callout(bench: &mut Bench, part: &str) -> Option<LoadingCallout> {
    let spec = bench
        .netlist()
        .devices
        .iter()
        .find(|d| d.part == part)?
        .clone();
    let (ext_name, _) = spec.ext.get("rin")?.clone();
    let plus = spec.pins.first()?.net?;
    let com = spec.pins.get(1)?.net?;
    let (with, open) = bench.what_if_ext(&ext_name, 1e18, &[plus, com])?;
    let shift = (with[0] - with[1]) - (open[0] - open[1]);
    let conn = &bench.netlist().connectivity;
    let net = conn
        .nets
        .get(plus.0)
        .map(|n| n.name.clone())
        .unwrap_or_default();
    // what drives the node: a device output on it
    let lib = bench.library().clone();
    let source = conn.nets.get(plus.0).and_then(|n| {
        n.pins.iter().find_map(|t| {
            let p = bench.project().part(&t.part)?;
            let def = lib.get(&p.part)?;
            let pin = def.pins.iter().find(|x| x.id == t.pin)?;
            matches!(
                pin.role,
                bb_circuit::part::PinRole::Output | bb_circuit::part::PinRole::AnalogOut
            )
            .then(|| format!("{} out", p.reference))
        })
    });
    let mv = shift * 1e3;
    let amount = if mv.abs() >= 1000.0 {
        format!("{:+.2} V", shift)
    } else if mv.abs() >= 0.05 {
        format!("{:+.1} mV", mv)
    } else {
        format!("{:+.1} µV", mv * 1e3)
    }
    .replace('-', "−");
    let label = match &source {
        Some(s) => format!("{net} ({s})"),
        None => net.clone(),
    };
    Some(LoadingCallout {
        text: format!(
            "Probing {label}: meter load shifts the node by {amount} — shown as it would be on a real bench."
        ),
        net,
        source,
        shift,
    })
}
