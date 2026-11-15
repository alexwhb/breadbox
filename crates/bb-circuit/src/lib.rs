//! Breadbox document model (`bb-circuit`): projects, the parts library, breadboard geometry,
//! connectivity, the netlister to `bb-spice`, `.cir` import/export, ERC/DRC, undo/redo.
//!
//! * [`part`] / [`library`] — declarative part definitions (PART-9) and the built-in library
//!   (PART-1…PART-8) with derived geometry.
//! * [`project`] — the `project.json` document (PRJ-1): parts, wires, schematic, probes,
//!   instruments, world, settings; load/save + migrations.
//! * [`edit`] — undoable editing transactions and copy/paste.
//! * [`connectivity`] — nets from breadboard strips, leads in holes, wires, schematic wires,
//!   labels and part-internal bonds; ratsnest (WB-7).
//! * [`netlist`] — project → `bb_spice::Netlist` (bench co-simulation or Pro export).
//! * [`spice_io`] — `.cir` export and import with auto-placed schematic (PRJ-4).
//! * [`erc`] — electrical / design rule checks (WB-12).

pub mod breadboard;
pub mod builder;
pub mod connectivity;
pub mod edit;
pub mod erc;
pub mod eseries;
pub mod geometry;
pub mod library;
pub mod netlist;
pub mod part;
pub mod pcb;
pub mod project;
pub mod spice_io;
pub mod symbols;
pub mod template;

pub use connectivity::{Connectivity, Net, NetId, RatsLine, TermRef};
pub use edit::{Clipboard, Editor, Op, Transaction};
pub use erc::{ErcItem, ErcSeverity};
pub use library::{Library, LibraryError};
pub use netlist::{BuiltNetlist, NetKind, NetlistMode};
pub use part::{Category, PartDef, PinDef, PinRole, PropValue};
pub use project::{
    Endpoint, FORMAT, InstrumentDef, PartInstance, ProbeDef, Project, ProjectError, Wire,
};
