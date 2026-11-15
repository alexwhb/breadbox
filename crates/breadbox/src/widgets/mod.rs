//! The widget kit shared by every screen (see `docs/ARCHITECTURE.md` §8).

pub mod controls;
pub mod icons;
pub mod overlay;
pub mod table;

pub use controls::*;
pub use icons::{Icon, paint_logo};
pub use overlay::{ModalResult, PanelMode, Side, Toast, ToastKind, Toasts, collapsible_side, modal, panel_title_with_collapse};
pub use table::{Cell, Col, Row, TableStyle, data_table};
