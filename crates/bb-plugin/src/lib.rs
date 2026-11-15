//! # bb-plugin — Breadbox plugins (PLUG-1, PLUG-2, PLUG-7)
//!
//! A plugin is a folder (or a `.bbplugin` zip of one) with a `plugin.json` [`Manifest`], an
//! optional WebAssembly behaviour module and its data (part definitions, panel, art, theme,
//! lessons). This crate
//!
//! * parses and validates manifests ([`manifest`], JSON Schema in `docs/plugin.schema.json`),
//! * keeps the catalog of bundled / user / local plugins with install, uninstall, enable,
//!   permission grants, version checks and hot reload ([`registry`], [`package`]),
//! * runs plugin code in a wasmtime sandbox — fuel, epoch interruption, memory caps,
//!   permission-checked host imports, no WASI unless `fs:project` ([`sandbox`]),
//! * adapts plugins into the engine ([`host::PluginHost`]): parts and instruments become
//!   `bb_core::Device`s (registered in a `bb_sim::DeviceRegistry`, parts via
//!   `bb_circuit::Library::add_json`), world models step with the world, radio packs extend the
//!   radio space, generators become behavioural sources, analyses run on netlists, boards become
//!   AVR board parts, themes override tokens, lesson packs add lessons,
//! * describes instrument front panels declaratively ([`panel`]).
//!
//! The guest side (Rust SDK, ABI for other languages) is `bb-plugin-sdk` /
//! `docs/PLUGIN-SDK.md`.

pub mod analysis;
pub mod board;
pub mod device;
pub mod generator;
pub mod host;
pub mod manifest;
pub mod package;
pub mod panel;
pub mod radio;
pub mod registry;
pub mod sandbox;
pub mod theme;
pub mod world;

pub use host::{InstrumentInfo, PluginHost};
pub use manifest::{Category, Kind, Manifest, Permission};
pub use panel::{Panel, PanelState};
pub use registry::{CatalogEntry, CatalogStatus, InstallReport, Plugin, Registry, Source};

use std::path::PathBuf;
use thiserror::Error;

/// Everything that can go wrong with a plugin. Messages are written for the person using the
/// app (they show up in the Plugins screen and the bench log).
#[derive(Debug, Error, Clone, PartialEq)]
pub enum PluginError {
    #[error("{plugin}: {msg}")]
    Manifest { plugin: String, msg: String },
    #[error("{plugin} is not compatible: {msg}")]
    Incompatible { plugin: String, msg: String },
    #[error("{plugin} may not {action}: it does not declare the `{permission}` permission")]
    PermissionDenied {
        plugin: String,
        permission: Permission,
        action: String,
    },
    #[error("{plugin} needs your approval for: {}", .missing.iter().map(|p| p.id()).collect::<Vec<_>>().join(", "))]
    NotGranted {
        plugin: String,
        missing: Vec<Permission>,
    },
    #[error("{msg}")]
    Violation { plugin: String, msg: String },
    #[error("{plugin} does not follow the plugin ABI: {msg}")]
    Abi { plugin: String, msg: String },
    #[error("{plugin} stopped: {msg}")]
    Faulted { plugin: String, msg: String },
    #[error("{}: {msg}", .path.display())]
    Io { path: PathBuf, msg: String },
    #[error("package: {0}")]
    Package(String),
    #[error("{plugin} {have} is already installed (newer than {offered})")]
    Downgrade {
        plugin: String,
        have: String,
        offered: String,
    },
    #[error("no plugin `{0}`")]
    NotFound(String),
    #[error("{plugin}: {msg}")]
    Content { plugin: String, msg: String },
}

impl PluginError {
    pub(crate) fn with_plugin(self, id: &str) -> PluginError {
        match self {
            PluginError::Manifest { plugin, msg } if plugin.is_empty() => PluginError::Manifest {
                plugin: id.into(),
                msg,
            },
            e => e,
        }
    }
    /// The plugin the error is about, if known.
    pub fn plugin(&self) -> Option<&str> {
        match self {
            PluginError::Manifest { plugin, .. }
            | PluginError::Incompatible { plugin, .. }
            | PluginError::PermissionDenied { plugin, .. }
            | PluginError::NotGranted { plugin, .. }
            | PluginError::Violation { plugin, .. }
            | PluginError::Abi { plugin, .. }
            | PluginError::Faulted { plugin, .. }
            | PluginError::Downgrade { plugin, .. }
            | PluginError::Content { plugin, .. } => Some(plugin),
            _ => None,
        }
    }
}

pub(crate) fn io(path: &std::path::Path, e: impl std::fmt::Display) -> PluginError {
    PluginError::Io {
        path: path.to_path_buf(),
        msg: e.to_string(),
    }
}
