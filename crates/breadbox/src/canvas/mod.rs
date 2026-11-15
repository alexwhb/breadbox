//! Canvases: the 2.5D breadboard bench ([`bench`]) and the schematic ([`schematic`]), with the
//! shared drawing toolkit ([`pen`]), per-part art ([`art`]), the scene/hit-testing model
//! ([`scene`]) and the instrument rendering hook ([`instruments`]).

pub mod art;
pub mod bench;
pub mod instruments;
pub mod pen;
pub mod scene;
pub mod schematic;
