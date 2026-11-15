//! The widget kit shared by every screen (see `docs/ARCHITECTURE.md` §8).

pub mod code_editor;
pub mod controls;
pub mod highlight;
pub mod icons;
pub mod instrument;
pub mod markdown;
pub mod overlay;
pub mod plot;
pub mod radio_tools;
pub mod table;

pub use code_editor::{
    CodeDoc, CodeEditor, DiagSeverity, EditorDiagnostic, EditorOutput, EditorView, editor_tabs,
};
pub use controls::*;
pub use highlight::{Lang, Tok};
pub use icons::{Icon, paint_logo};
pub use overlay::{
    ModalResult, PanelMode, Side, Toast, ToastKind, Toasts, collapsible_side, modal,
    panel_title_with_collapse,
};
pub use table::{Cell, Col, Row, TableStyle, data_table};
