//! `plugin.json` (PLUG-1): the manifest every plugin folder / `.bbplugin` package carries.
//!
//! ```json
//! {
//!   "id": "community.dmm-handheld",
//!   "version": "1.4.0",
//!   "name": "Handheld DMM",
//!   "kind": ["instrument"],
//!   "provides": {
//!     "instrument": "Handheld DMM",
//!     "probes": 2,
//!     "model": "./dmm.wasm",
//!     "view3d": "./dmm.bench.svg",
//!     "view2d": "./dmm.symbol.svg",
//!     "parts": "./parts.json",
//!     "panel": "./panel.json"
//!   },
//!   "spice": { "load": "R 10Meg ∥ C 100p" },
//!   "specs": { "counts": 6000, "dcv_ranges": ["600m", "6", "60", "600"],
//!              "dcv_accuracy": "0.05% + 1", "autorange_ms": 350 },
//!   "permissions": ["read:nets", "inject:load"]
//! }
//! ```
//!
//! The JSON Schema is `docs/plugin.schema.json`; [`Manifest::validate`] enforces the same rules
//! plus the ones a schema cannot express (files exist inside the plugin folder, permissions
//! cover what the plugin provides, versions are compatible).

use crate::PluginError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Plugin ABI version this build hosts (`bb_abi_version()` must return it).
pub const API_VERSION: u32 = 1;

/// The Breadbox version plugins are checked against (`min_breadbox`).
pub const BREADBOX_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a plugin adds (PLUG-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Instrument,
    Part,
    Board,
    Radio,
    WorldModel,
    Generator,
    Analysis,
    Theme,
    LessonPack,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Instrument,
        Kind::Part,
        Kind::Board,
        Kind::Radio,
        Kind::WorldModel,
        Kind::Generator,
        Kind::Analysis,
        Kind::Theme,
        Kind::LessonPack,
    ];
    /// `instrument`, `world-model`, …
    pub fn id(self) -> &'static str {
        match self {
            Kind::Instrument => "instrument",
            Kind::Part => "part",
            Kind::Board => "board",
            Kind::Radio => "radio",
            Kind::WorldModel => "world-model",
            Kind::Generator => "generator",
            Kind::Analysis => "analysis",
            Kind::Theme => "theme",
            Kind::LessonPack => "lesson-pack",
        }
    }
    /// Chip text on cards (`INSTRUMENT`, `WORLD MODEL`).
    pub fn chip(self) -> &'static str {
        match self {
            Kind::Instrument => "INSTRUMENT",
            Kind::Part => "PART",
            Kind::Board => "BOARD",
            Kind::Radio => "RADIO",
            Kind::WorldModel => "WORLD MODEL",
            Kind::Generator => "GENERATOR",
            Kind::Analysis => "ANALYSIS",
            Kind::Theme => "THEME",
            Kind::LessonPack => "LESSON PACK",
        }
    }
    /// Plugins-screen navigation category.
    pub fn category(self) -> Category {
        match self {
            Kind::Instrument => Category::Instruments,
            Kind::Part | Kind::Board => Category::PartsBoards,
            Kind::Radio => Category::RadiosNetworks,
            Kind::WorldModel => Category::WorldModels,
            Kind::Generator | Kind::Analysis => Category::SolversAnalyses,
            Kind::Theme => Category::ViewsThemes,
            Kind::LessonPack => Category::Lessons,
        }
    }
}

/// Plugins-screen navigation (PLUG-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    Instruments,
    PartsBoards,
    RadiosNetworks,
    WorldModels,
    SolversAnalyses,
    ViewsThemes,
    Lessons,
}

impl Category {
    /// Nav order of the design ("Lessons" packs live on the Lessons screen).
    pub const NAV: [Category; 6] = [
        Category::Instruments,
        Category::PartsBoards,
        Category::RadiosNetworks,
        Category::WorldModels,
        Category::SolversAnalyses,
        Category::ViewsThemes,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Category::Instruments => "Instruments",
            Category::PartsBoards => "Parts & boards",
            Category::RadiosNetworks => "Radios & networks",
            Category::WorldModels => "World models",
            Category::SolversAnalyses => "Solvers & analyses",
            Category::ViewsThemes => "Views & themes",
            Category::Lessons => "Lessons",
        }
    }
}

/// Capabilities a plugin must declare (PLUG-2). Every host import checks one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Permission {
    /// Read node voltages / pin currents / element integrals, watch edges, capture waveforms.
    ReadNets,
    /// Put an impedance into the circuit (an instrument's input load, `spice.load`).
    InjectLoad,
    /// Drive pins / sources (generators, supplies, behavioural parts).
    DrivePins,
    WorldRead,
    WorldWrite,
    /// The virtual RF space (MQTT broker, radio modules).
    RadioSpace,
    /// Draw on instrument screens (scope traces) beyond the declarative display.
    UiPanel,
    /// WASI file access, preopened to the project folder.
    FsProject,
    /// The real LAN (the MQTT LAN bridge).
    NetLan,
}

impl Permission {
    pub const ALL: [Permission; 9] = [
        Permission::ReadNets,
        Permission::InjectLoad,
        Permission::DrivePins,
        Permission::WorldRead,
        Permission::WorldWrite,
        Permission::RadioSpace,
        Permission::UiPanel,
        Permission::FsProject,
        Permission::NetLan,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Permission::ReadNets => "read:nets",
            Permission::InjectLoad => "inject:load",
            Permission::DrivePins => "drive:pins",
            Permission::WorldRead => "world:read",
            Permission::WorldWrite => "world:write",
            Permission::RadioSpace => "radio:space",
            Permission::UiPanel => "ui:panel",
            Permission::FsProject => "fs:project",
            Permission::NetLan => "net:lan",
        }
    }
    /// What the install prompt says.
    pub fn describe(self) -> &'static str {
        match self {
            Permission::ReadNets => "Read voltages and currents anywhere on the bench",
            Permission::InjectLoad => "Load the circuit with its own impedance",
            Permission::DrivePins => "Drive pins and inject signals into the circuit",
            Permission::WorldRead => "Read the world (soil, light, air, time)",
            Permission::WorldWrite => "Change the world (water pots, weather, light)",
            Permission::RadioSpace => "Use the virtual radio space and MQTT broker",
            Permission::UiPanel => "Draw on its instrument screen",
            Permission::FsProject => "Read and write files in the project folder",
            Permission::NetLan => "Connect to your real local network",
        }
    }
    pub fn parse(s: &str) -> Option<Permission> {
        Permission::ALL.into_iter().find(|p| p.id() == s)
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl Serialize for Permission {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for Permission {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Permission::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "unknown permission `{s}` (known: {})",
                Permission::ALL
                    .iter()
                    .map(|p| p.id())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
    }
}

/// A set of permissions.
pub type PermSet = BTreeSet<Permission>;

/// Probe leads: a count (`2`) or names (`["VΩ", "COM"]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Probes {
    Count(u32),
    Named(Vec<String>),
}

impl Probes {
    pub fn count(&self) -> usize {
        match self {
            Probes::Count(n) => *n as usize,
            Probes::Named(v) => v.len(),
        }
    }
}

/// A declarative AVR board variant (`kind: board`), e.g. an Arduino Pro Mini.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardVariant {
    /// Part id (`pro_mini`).
    pub id: String,
    pub name: String,
    /// Built-in board whose core and pin map it reuses: `uno`, `nano`, `mega`.
    pub base: String,
    #[serde(default)]
    pub description: String,
    /// MCU label (`ATmega328P @ 8 MHz`).
    #[serde(default)]
    pub label: Option<String>,
    /// Header pins in order (`["RAW", "GND", "RST", "VCC", "A3", …]`); every I/O name must
    /// exist on the base board.
    pub pins: Vec<String>,
    /// Supply pin the MCU runs from (`VCC`).
    #[serde(default = "default_vcc_pin")]
    pub vcc_pin: String,
    /// Header rows: `[[pin ids…], …]` laid out 0.1" apart; defaults to two rows split evenly.
    #[serde(default)]
    pub rows: Vec<Vec<String>>,
    /// Bench art (svg/png inside the plugin).
    #[serde(default)]
    pub art: Option<String>,
}

fn default_vcc_pin() -> String {
    "VCC".into()
}

/// `provides.world_model`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WorldModelInfo {
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Built-in behaviours the model takes over (`evaporation` → pots' built-in evaporation is
    /// switched off while the plugin is enabled).
    #[serde(default)]
    pub replaces: Vec<String>,
    /// World variables it introduces (shown in the World panel), e.g. `weather.rain_mm_h`.
    #[serde(default)]
    pub vars: Vec<String>,
}

/// `provides.generator`: a custom source waveform (`bb_generator_eval(t)`), tabulated over one
/// period into the part's behavioural source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratorInfo {
    /// Part id of the generated source part (`<plugin>.source` by default).
    #[serde(default)]
    pub part: Option<String>,
    pub name: String,
    /// Waveform period (s).
    pub period: f64,
    /// Samples per period (2…4096).
    #[serde(default = "default_points")]
    pub points: usize,
    /// Output impedance (Ω).
    #[serde(default = "default_rout")]
    pub r_out: f64,
}

fn default_points() -> usize {
    256
}
fn default_rout() -> f64 {
    50.0
}

/// `provides.analysis`: a Pro-mode analysis card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AnalysisInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Settings the card shows: key → default value.
    #[serde(default)]
    pub options: BTreeMap<String, serde_json::Value>,
}

/// `provides.radio`: what a radio pack adds to the RF space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RadioInfo {
    /// Enables the LoRa medium + virtual gateway (bb-radio).
    #[serde(default)]
    pub lora: bool,
    /// Gateway settings (`bb_radio::lora::Gateway` JSON) applied to the bench's radio space.
    #[serde(default)]
    pub gateway: Option<serde_json::Value>,
    /// The wasm model runs as a radio extension (`bb_radio_step`), e.g. an uplink decoder.
    #[serde(default)]
    pub extension: bool,
}

/// `provides`: the plugin's contributions. Paths are relative to the plugin folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Provides {
    /// Instrument display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instrument: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probes: Option<Probes>,
    /// WebAssembly behaviour module (`./dmm.wasm`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Bench art (2.5D, svg/png).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view3d: Option<String>,
    /// Schematic symbol (svg).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view2d: Option<String>,
    /// Instrument panel description (`panel.json`, see [`crate::panel::Panel`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<String>,
    /// Part definitions in the bb-circuit schema (ARCHITECTURE §3b).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boards: Vec<BoardVariant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_model: Option<WorldModelInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<GeneratorInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis: Option<AnalysisInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radio: Option<RadioInfo>,
    /// Theme token overrides (`theme.json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Folder of lesson TOML files (`lessons/`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lessons: Option<String>,
}

/// `spice`: circuit-facing summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SpiceInfo {
    /// The load the plugin puts on the circuit, for people (`R 10Meg ∥ C 100p`). A plugin with
    /// a load must declare `inject:load`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<String>,
    /// Extra `.model` / `.subckt` cards its parts use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library: Vec<String>,
}

/// One cell of the detail page's specs grid (`Display` / `6000 counts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Highlight {
    pub label: String,
    pub value: String,
}

/// Sandbox budget overrides (clamped to the host's caps).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct LimitsInfo {
    /// Instructions (fuel) per call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuel_per_call: Option<u64>,
    /// Linear memory cap (MiB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u32>,
}

/// The manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Reverse-DNS-style id: lowercase letters, digits, `.`, `-`, `_`; at least one `.`.
    pub id: String,
    /// Semantic version (`1.4.0`).
    pub version: String,
    /// Display name (defaults to `provides.instrument` or the id).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub kind: Vec<Kind>,
    /// Plugin ABI version (default 1).
    #[serde(default = "default_api")]
    pub api: u32,
    /// One line for catalog cards.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Detail-page paragraph.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub license: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
    #[serde(default)]
    pub provides: Provides,
    #[serde(default)]
    pub spice: SpiceInfo,
    /// Machine-readable specs (passed to the model in its config).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub specs: BTreeMap<String, serde_json::Value>,
    /// Specs grid of the detail page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub highlights: Vec<Highlight>,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    /// Wasm entry module (alias of `provides.model`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// Other files shipped with the plugin (art, docs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<String>,
    /// Oldest Breadbox version it runs on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_breadbox: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<LimitsInfo>,
}

fn default_api() -> u32 {
    API_VERSION
}

/// A non-fatal finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning(pub String);

impl Manifest {
    /// Parses `plugin.json` text.
    pub fn parse(text: &str) -> Result<Manifest, PluginError> {
        serde_json::from_str(text).map_err(|e| PluginError::Manifest {
            plugin: String::new(),
            msg: format!("plugin.json: {e}"),
        })
    }

    /// Reads and validates `<dir>/plugin.json`.
    pub fn load(dir: &Path) -> Result<Manifest, PluginError> {
        let p = dir.join("plugin.json");
        let text = std::fs::read_to_string(&p).map_err(|e| PluginError::Io {
            path: p.clone(),
            msg: e.to_string(),
        })?;
        let m = Manifest::parse(&text).map_err(|e| e.with_plugin(&dir.display().to_string()))?;
        m.validate(Some(dir))?;
        Ok(m)
    }

    /// Display name.
    pub fn display_name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        self.provides
            .instrument
            .clone()
            .unwrap_or_else(|| self.id.clone())
    }

    /// Nav category (explicit, else from the first kind).
    pub fn category(&self) -> Category {
        self.category
            .or_else(|| self.kind.first().map(|k| k.category()))
            .unwrap_or(Category::PartsBoards)
    }

    /// Wasm module path (`provides.model`, else `entry`).
    pub fn model_path(&self) -> Option<&str> {
        self.provides
            .model
            .as_deref()
            .or(self.entry.as_deref())
            .filter(|p| p.ends_with(".wasm"))
    }

    pub fn has(&self, k: Kind) -> bool {
        self.kind.contains(&k)
    }

    pub fn perms(&self) -> PermSet {
        self.permissions.iter().copied().collect()
    }

    pub fn semver(&self) -> Result<semver::Version, PluginError> {
        semver::Version::parse(&self.version)
            .map_err(|e| self.err(format!("version `{}`: {e}", self.version)))
    }

    fn err(&self, msg: String) -> PluginError {
        PluginError::Manifest {
            plugin: self.id.clone(),
            msg,
        }
    }

    /// Every file path the manifest references (relative to the folder).
    pub fn referenced_files(&self) -> Vec<String> {
        let p = &self.provides;
        let mut v: Vec<String> = [
            &p.model,
            &p.view3d,
            &p.view2d,
            &p.panel,
            &p.parts,
            &p.theme,
            &self.entry,
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
        v.extend(p.boards.iter().filter_map(|b| b.art.clone()));
        v.extend(self.assets.iter().cloned());
        v.sort();
        v.dedup();
        v
    }

    /// Checks the manifest; with `root`, also that referenced files exist inside the folder.
    pub fn validate(&self, root: Option<&Path>) -> Result<Vec<Warning>, PluginError> {
        let mut warn = Vec::new();
        if !valid_id(&self.id) {
            return Err(self.err(format!(
                "id `{}` must be lowercase letters, digits, `.`, `-` or `_` with at least one `.` (e.g. `community.dmm-handheld`)",
                self.id
            )));
        }
        self.semver()?;
        if self.api != API_VERSION {
            return Err(self.err(format!(
                "plugin ABI {} is not supported by this Breadbox (ABI {API_VERSION})",
                self.api
            )));
        }
        if let Some(min) = &self.min_breadbox {
            let want = semver::Version::parse(min)
                .map_err(|e| self.err(format!("min_breadbox `{min}`: {e}")))?;
            let have =
                semver::Version::parse(BREADBOX_VERSION).unwrap_or(semver::Version::new(0, 1, 0));
            if want > have {
                return Err(PluginError::Incompatible {
                    plugin: self.id.clone(),
                    msg: format!("needs Breadbox {want} or newer (this is {have})"),
                });
            }
        }
        if self.kind.is_empty() {
            return Err(self.err("`kind` must list at least one kind".into()));
        }
        let mut seen = BTreeSet::new();
        for p in &self.permissions {
            if !seen.insert(*p) {
                warn.push(Warning(format!("permission `{p}` listed twice")));
            }
        }
        let perms = self.perms();
        let p = &self.provides;
        // paths: relative, inside the folder
        for f in self.referenced_files() {
            if !safe_rel_path(&f) {
                return Err(self.err(format!(
                    "path `{f}` must be relative and stay inside the plugin folder"
                )));
            }
            if let Some(r) = root
                && !r.join(clean(&f)).exists()
            {
                return Err(self.err(format!("file `{f}` is missing")));
            }
        }
        if let Some(l) = &p.lessons {
            if !safe_rel_path(l) {
                return Err(self.err(format!("path `{l}` must stay inside the plugin folder")));
            }
            if let Some(r) = root
                && !r.join(clean(l)).is_dir()
            {
                return Err(self.err(format!("lesson folder `{l}` is missing")));
            }
        }
        if let Some(m) = p.model.as_deref().or(self.entry.as_deref())
            && !m.ends_with(".wasm")
        {
            return Err(self.err(format!(
                "model `{m}` must be a WebAssembly module (.wasm); compile the sources first"
            )));
        }
        // kind-specific contents
        for k in &self.kind {
            let ok = match k {
                Kind::Instrument => self.model_path().is_some() && p.parts.is_some(),
                Kind::Part => p.parts.is_some(),
                Kind::Board => !p.boards.is_empty(),
                Kind::Radio => p.parts.is_some() || p.radio.is_some(),
                Kind::WorldModel => self.model_path().is_some(),
                Kind::Generator => self.model_path().is_some() || p.parts.is_some(),
                Kind::Analysis => self.model_path().is_some() && p.analysis.is_some(),
                Kind::Theme => p.theme.is_some(),
                Kind::LessonPack => p.lessons.is_some(),
            };
            if !ok {
                let need = match k {
                    Kind::Instrument => "`provides.model` (.wasm) and `provides.parts`",
                    Kind::Part => "`provides.parts`",
                    Kind::Board => "`provides.boards`",
                    Kind::Radio => "`provides.parts` or `provides.radio`",
                    Kind::WorldModel => "`provides.model` (.wasm)",
                    Kind::Generator => "`provides.model` or `provides.parts`",
                    Kind::Analysis => "`provides.model` and `provides.analysis`",
                    Kind::Theme => "`provides.theme`",
                    Kind::LessonPack => "`provides.lessons`",
                };
                return Err(self.err(format!("kind `{}` needs {need}", k.id())));
            }
        }
        if p.world_model.is_some() && !self.has(Kind::WorldModel) {
            warn.push(Warning(
                "`provides.world_model` without kind `world-model`".into(),
            ));
        }
        // permissions that what it provides implies
        if self.spice.load.is_some() && !perms.contains(&Permission::InjectLoad) {
            return Err(PluginError::PermissionDenied {
                plugin: self.id.clone(),
                permission: Permission::InjectLoad,
                action: "declare a circuit load (`spice.load`)".into(),
            });
        }
        if self.has(Kind::WorldModel)
            && !perms.contains(&Permission::WorldRead)
            && !perms.contains(&Permission::WorldWrite)
        {
            return Err(PluginError::PermissionDenied {
                plugin: self.id.clone(),
                permission: Permission::WorldRead,
                action: "be a world model".into(),
            });
        }
        if p.radio.as_ref().is_some_and(|r| r.extension || r.lora)
            && !perms.contains(&Permission::RadioSpace)
        {
            return Err(PluginError::PermissionDenied {
                plugin: self.id.clone(),
                permission: Permission::RadioSpace,
                action: "extend the radio space".into(),
            });
        }
        if let Some(g) = &p.generator {
            if !(g.period.is_finite() && g.period > 0.0) {
                return Err(self.err("generator period must be > 0".into()));
            }
            if !(2..=4096).contains(&g.points) {
                return Err(self.err("generator points must be 2…4096".into()));
            }
            if !perms.contains(&Permission::DrivePins) {
                return Err(PluginError::PermissionDenied {
                    plugin: self.id.clone(),
                    permission: Permission::DrivePins,
                    action: "drive a source into the circuit".into(),
                });
            }
        }
        for b in &p.boards {
            if !["uno", "nano", "mega"].contains(&b.base.as_str()) {
                return Err(self.err(format!(
                    "board `{}`: base must be uno, nano or mega (got `{}`)",
                    b.id, b.base
                )));
            }
        }
        if self.summary.is_empty() {
            warn.push(Warning("no `summary` for the catalog card".into()));
        }
        Ok(warn)
    }
}

/// `community.dmm-handheld`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id.contains('.')
        && !id.starts_with('.')
        && !id.ends_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
}

/// Relative, no `..`, no root/prefix.
pub fn safe_rel_path(p: &str) -> bool {
    let path = Path::new(p);
    !p.is_empty()
        && !p.contains('\\')
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// `./a/b` → `a/b`.
pub fn clean(p: &str) -> PathBuf {
    Path::new(p)
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dmm() -> serde_json::Value {
        serde_json::json!({
            "id": "community.dmm-handheld",
            "version": "1.4.0",
            "kind": ["instrument"],
            "summary": "A handheld meter",
            "provides": {
                "instrument": "Handheld DMM",
                "probes": 2,
                "model": "./dmm.wasm",
                "view3d": "./dmm.bench.svg",
                "view2d": "./dmm.symbol.svg",
                "parts": "./parts.json"
            },
            "spice": {"load": "R 10Meg ∥ C 100p"},
            "specs": {"counts": 6000, "dcv_ranges": ["600m", "6", "60", "600"],
                      "dcv_accuracy": "0.05% + 1", "autorange_ms": 350},
            "permissions": ["read:nets", "inject:load"]
        })
    }

    fn parse(v: serde_json::Value) -> Result<Manifest, PluginError> {
        Manifest::parse(&v.to_string())
    }

    #[test]
    fn design_manifest_parses_and_validates() {
        let m = parse(dmm()).unwrap();
        assert_eq!(m.display_name(), "Handheld DMM");
        assert_eq!(m.category(), Category::Instruments);
        assert_eq!(m.model_path(), Some("./dmm.wasm"));
        assert!(m.validate(None).unwrap().is_empty());
        assert_eq!(m.provides.probes.as_ref().unwrap().count(), 2);
        // round trip keeps the shape recognisable
        let back = serde_json::to_value(&m).unwrap();
        assert_eq!(back["provides"]["instrument"], "Handheld DMM");
        assert_eq!(back["permissions"][1], "inject:load");
    }

    #[test]
    fn bad_ids_versions_kinds_and_permissions_are_rejected() {
        let mut v = dmm();
        v["id"] = "DMM".into();
        assert!(parse(v).unwrap().validate(None).is_err());
        let mut v = dmm();
        v["version"] = "one".into();
        assert!(parse(v).unwrap().validate(None).is_err());
        let mut v = dmm();
        v["kind"] = serde_json::json!(["toaster"]);
        assert!(parse(v).is_err());
        let mut v = dmm();
        v["permissions"] = serde_json::json!(["read:nets", "root:everything"]);
        let e = parse(v).unwrap_err().to_string();
        assert!(e.contains("unknown permission `root:everything`"), "{e}");
        let mut v = dmm();
        v["api"] = 7.into();
        assert!(parse(v).unwrap().validate(None).is_err());
        let mut v = dmm();
        v["min_breadbox"] = "99.0.0".into();
        assert!(matches!(
            parse(v).unwrap().validate(None),
            Err(PluginError::Incompatible { .. })
        ));
    }

    #[test]
    fn a_load_needs_inject_load() {
        let mut v = dmm();
        v["permissions"] = serde_json::json!(["read:nets"]);
        let e = parse(v).unwrap().validate(None).unwrap_err();
        assert!(matches!(
            e,
            PluginError::PermissionDenied {
                permission: Permission::InjectLoad,
                ..
            }
        ));
    }

    #[test]
    fn paths_stay_inside_and_models_are_wasm() {
        let mut v = dmm();
        v["provides"]["view3d"] = "../../etc/passwd".into();
        assert!(parse(v).unwrap().validate(None).is_err());
        let mut v = dmm();
        v["provides"]["model"] = "./dmm.model.ts".into();
        let e = parse(v).unwrap().validate(None).unwrap_err().to_string();
        assert!(e.contains("WebAssembly"), "{e}");
        assert!(safe_rel_path("./a/b.svg"));
        assert!(!safe_rel_path("/etc/x"));
        assert!(!safe_rel_path("a/../../b"));
    }

    #[test]
    fn kinds_need_their_contents() {
        let mut v = dmm();
        v["kind"] = serde_json::json!(["instrument", "theme"]);
        let e = parse(v).unwrap().validate(None).unwrap_err().to_string();
        assert!(e.contains("theme"), "{e}");
    }
}
