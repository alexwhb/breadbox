//! Firmware screen (stub, replaced below).
use crate::app::AppCtx;
use crate::router::Screen;
use eframe::egui::Ui;
#[derive(Default)]
pub struct FirmwareScreen;
impl Screen for FirmwareScreen {
    fn ui(&mut self, _ui: &mut Ui, _app: &mut AppCtx) {}
}
