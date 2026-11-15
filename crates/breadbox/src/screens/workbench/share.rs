//! The header's Share menu (`PRJ-5`): `.breadbox.zip` bundle, `.cir`, BOM CSV, schematic SVG
//! / PNG and a PNG of the bench. `BREADBOX_EXPORT_DIR` skips the save dialog (tests, scripts).

use crate::app::AppCtx;
use crate::export;
use crate::platform;
use crate::widgets::{self, Icon, plot};
use eframe::egui::{self, Rect, Response};
use std::path::PathBuf;

#[derive(Default)]
pub struct ShareState {
    /// Screen rect of the bench canvas (for the PNG).
    pub bench_rect: Option<Rect>,
    want_png: Option<PathBuf>,
    png: Option<plot::PngResult>,
    /// Last export written (tests).
    pub last: Option<PathBuf>,
}

/// What to export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Export {
    Bundle,
    Cir,
    Bom,
    SchematicSvg,
    SchematicPng,
    BenchPng,
    /// PCB fabrication files (`PRJ-5`, `PRO-7`).
    Gerbers,
}

impl Export {
    pub const ALL: [Export; 7] = [
        Export::Bundle,
        Export::Cir,
        Export::Bom,
        Export::SchematicSvg,
        Export::SchematicPng,
        Export::BenchPng,
        Export::Gerbers,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Export::Bundle => "Project bundle (.breadbox.zip)",
            Export::Cir => "SPICE netlist (.cir)",
            Export::Bom => "Bill of materials (.csv)",
            Export::SchematicSvg => "Schematic (.svg)",
            Export::SchematicPng => "Schematic (.png)",
            Export::BenchPng => "Bench picture (.png)",
            Export::Gerbers => "PCB Gerbers + drill (.zip)",
        }
    }
    fn icon(self) -> Icon {
        match self {
            Export::Bundle => Icon::Folder,
            Export::Cir => Icon::File,
            Export::Bom => Icon::Table,
            Export::SchematicSvg | Export::SchematicPng | Export::BenchPng => Icon::Image,
            Export::Gerbers => Icon::Chip,
        }
    }
    fn file(self, name: &str) -> (String, &'static str, &'static str) {
        match self {
            Export::Bundle => (format!("{name}.breadbox.zip"), "Breadbox bundle", "zip"),
            Export::Cir => (format!("{name}.cir"), "SPICE netlist", "cir"),
            Export::Bom => (format!("{name}-bom.csv"), "CSV", "csv"),
            Export::SchematicSvg => (format!("{name}.svg"), "SVG", "svg"),
            Export::SchematicPng => (format!("{name}-schematic.png"), "PNG", "png"),
            Export::BenchPng => (format!("{name}-bench.png"), "PNG", "png"),
            Export::Gerbers => (format!("{name}-gerbers.zip"), "Gerber zip", "zip"),
        }
    }
}

impl ShareState {
    pub fn menu(&mut self, resp: &Response, app: &mut AppCtx) {
        let mut pick = None;
        egui::Popup::menu(resp).width(280.0).show(|ui| {
            ui.set_max_width(280.0);
            for e in Export::ALL {
                if widgets::Btn::plain(e.label())
                    .icon(e.icon())
                    .full_width()
                    .show(ui)
                    .clicked()
                {
                    pick = Some(e);
                }
            }
        });
        if let Some(e) = pick {
            self.export(app, e);
        }
    }

    /// Runs an export (a dialog asks where unless `BREADBOX_EXPORT_DIR` is set).
    pub fn export(&mut self, app: &mut AppCtx, e: Export) {
        let Some(doc) = app.circuit.as_mut() else {
            return;
        };
        let name = doc.project().name.clone();
        let (file, ext_name, ext) = e.file(&name);
        let path = match std::env::var_os("BREADBOX_EXPORT_DIR") {
            Some(d) => Some(PathBuf::from(d).join(&file)),
            None => platform::save_file("Export", &file, ext_name, ext),
        };
        let Some(path) = path else { return };
        let conn = doc.connectivity();
        let checks = doc.checks();
        let tel = app.bench.sim().and_then(|s| s.telemetry());
        let Some(doc) = app.circuit.as_mut() else {
            return;
        };
        let values = crate::canvas::schematic::net_values(doc.project(), &conn, tel.as_deref());
        let erc = checks.erc.first().map(|i| i.message.clone());
        let r: anyhow::Result<()> = match e {
            Export::Bundle => export::bundle_zip(doc, &path),
            Export::Gerbers => crate::screens::pro::pcb::gerber_zip(doc.project(), &doc.lib, &name)
                .and_then(|z| std::fs::write(&path, z).map_err(Into::into)),
            Export::Cir => export::cir(doc.project(), &doc.lib)
                .and_then(|t| std::fs::write(&path, t).map_err(Into::into)),
            Export::Bom => {
                std::fs::write(&path, export::bom_csv(doc.project(), &doc.lib)).map_err(Into::into)
            }
            Export::SchematicSvg => {
                let svg = crate::canvas::schematic::render_svg(
                    doc.project(),
                    &doc.lib,
                    &conn,
                    &values,
                    erc.as_deref(),
                );
                std::fs::write(&path, svg).map_err(Into::into)
            }
            Export::SchematicPng => {
                let svg = crate::canvas::schematic::render_svg(
                    doc.project(),
                    &doc.lib,
                    &conn,
                    &values,
                    erc.as_deref(),
                );
                export::svg_to_png(&svg, 2.0)
                    .and_then(|png| std::fs::write(&path, png).map_err(Into::into))
            }
            Export::BenchPng => {
                self.want_png = Some(path.clone());
                return;
            }
        };
        match r {
            Ok(()) => {
                app.toasts.success(format!("Exported {}", path.display()));
                self.last = Some(path);
            }
            Err(err) => app.toasts.error(format!("Export failed: {err:#}")),
        }
    }

    /// Takes the pending bench screenshot / reports a finished one.
    pub fn poll(&mut self, app: &mut AppCtx) {
        if let (Some(path), Some(rect)) = (self.want_png.take(), self.bench_rect) {
            self.png = Some(plot::export_png(&app.egui, rect, path));
        }
        if let Some(slot) = &self.png {
            let done = slot.lock().ok().and_then(|mut g| g.take());
            if let Some(r) = done {
                match r {
                    Ok(p) => {
                        app.toasts.success(format!("Exported {}", p.display()));
                        self.last = Some(p);
                    }
                    Err(e) => app.toasts.error(format!("Export failed: {e}")),
                }
                self.png = None;
            }
        }
    }
}
