//! Live debugging UI (`MCU-5`): the debug toolbar and the Debug tab (call stack, locals,
//! globals, registers + SREG). Pure views over [`DebugViewModel`].

use crate::bench::{DebugCommand, DebugState, DebugViewModel};
use crate::theme::{Palette, font, pal};
use crate::widgets::{Btn, Cell, Col, Icon, Row, TableStyle, data_table, section_header};
use eframe::egui::{self, Ui, vec2};

/// Toolbar: Continue / Pause / Step over / Step into / Step out + state text.
pub fn debug_toolbar(ui: &mut Ui, vm: Option<&DebugViewModel>) -> Option<DebugCommand> {
    let p = pal(ui);
    let mut cmd = None;
    let state = vm.map(|v| v.state.clone()).unwrap_or_default();
    let paused = matches!(state, DebugState::Paused { .. });
    let running = matches!(state, DebugState::Running);
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let b = |ui: &mut Ui, label: &str, icon: Icon, enabled: bool| {
            Btn::plain(label)
                .icon_only(icon)
                .height(26.0)
                .enabled(enabled)
                .show(ui)
                .clicked()
        };
        if b(ui, "Continue", Icon::Continue, paused) {
            cmd = Some(DebugCommand::Continue);
        }
        if b(ui, "Pause", Icon::Pause, running) {
            cmd = Some(DebugCommand::Pause);
        }
        if b(ui, "Step over", Icon::StepOver, paused) {
            cmd = Some(DebugCommand::StepOver);
        }
        if b(ui, "Step into", Icon::StepInto, paused) {
            cmd = Some(DebugCommand::StepInto);
        }
        if b(ui, "Step out", Icon::StepOut, paused) {
            cmd = Some(DebugCommand::StepOut);
        }
        ui.add_space(8.0);
        let (text, color) = match (&state, vm) {
            (DebugState::Paused { reason }, Some(v)) => {
                let loc = v
                    .location
                    .as_ref()
                    .map(|l| format!("{}:{}", l.file, l.line))
                    .unwrap_or_else(|| format!("pc 0x{:04x}", v.registers.pc));
                let f = v
                    .function
                    .as_deref()
                    .map(|f| format!(" · {f}()"))
                    .unwrap_or_default();
                (format!("paused at {loc}{f} · {reason}"), p.accent_soft)
            }
            (DebugState::Running, _) => ("running".to_string(), p.ok),
            (DebugState::Halted(e), _) => (format!("halted: {e}"), p.danger_soft),
            _ => (
                "no firmware on the bench yet — Upload to sim".to_string(),
                p.text_muted,
            ),
        };
        ui.label(
            egui::RichText::new(text)
                .font(font::mono(12.0))
                .color(color),
        );
    });
    cmd
}

/// The Debug tab body.
pub fn debug_view(ui: &mut Ui, vm: Option<&DebugViewModel>) {
    let p = pal(ui);
    let Some(vm) = vm else {
        empty(ui, p, "Upload firmware to start a debugging session.");
        return;
    };
    if !vm.is_paused() {
        let msg = match &vm.state {
            DebugState::Running => {
                "Running — set a breakpoint (click the gutter with Step · breakpoints on) or press Pause to inspect variables."
            }
            DebugState::NoFirmware => "Upload firmware to start a debugging session.",
            _ => "Not paused.",
        };
        empty(ui, p, msg);
        return;
    }
    let w = ui.available_width();
    let col = (w - 32.0) / 3.0;
    egui::ScrollArea::vertical()
        .id_salt("debug-view")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                ui.vertical(|ui| {
                    ui.set_width(col * 0.8);
                    section_header(ui, "Call stack");
                    let rows: Vec<Row> = vm
                        .call_stack
                        .iter()
                        .enumerate()
                        .map(|(i, f)| {
                            let loc = f
                                .location
                                .as_ref()
                                .map(|l| format!("{}:{}", l.file, l.line))
                                .unwrap_or_else(|| format!("0x{:04x}", f.pc));
                            let name = if f.function.starts_with("0x") {
                                f.function.clone()
                            } else {
                                format!("{}()", f.function)
                            };
                            let mut r =
                                Row::new(vec![Cell::new(name), Cell::new(loc).color(p.text_muted)]);
                            if i == 0 {
                                r = r.color(p.accent_soft);
                            }
                            r
                        })
                        .collect();
                    data_table(
                        ui,
                        "callstack",
                        &[Col::Fr("function", 1.0), Col::Fr("at", 1.0)],
                        &rows,
                        &compact(),
                    );
                });
                ui.vertical(|ui| {
                    ui.set_width(col * 1.3);
                    section_header(
                        ui,
                        if vm.debug_build {
                            "Variables"
                        } else {
                            "Variables (release build: few locals)"
                        },
                    );
                    let mut rows: Vec<Row> = vm
                        .locals
                        .iter()
                        .map(|v| {
                            Row::new(vec![
                                Cell::new(&v.name),
                                Cell::new(&v.type_name).color(p.code_ty),
                                Cell::new(&v.value).color(p.code_num),
                            ])
                        })
                        .collect();
                    rows.extend(
                        vm.globals
                            .iter()
                            .filter(|v| v.location != "const")
                            .take(24)
                            .map(|v| {
                                Row::new(vec![
                                    Cell::new(format!("::{}", v.name)).color(p.text_dim),
                                    Cell::new(&v.type_name).color(p.code_ty),
                                    Cell::new(&v.value).color(p.code_num),
                                ])
                            }),
                    );
                    data_table(
                        ui,
                        "vars",
                        &[
                            Col::Fr("name", 1.0),
                            Col::Fr("type", 1.0),
                            Col::Fr("value", 1.2),
                        ],
                        &rows,
                        &compact(),
                    );
                });
                ui.vertical(|ui| {
                    ui.set_width(col * 0.9);
                    section_header(ui, "Registers");
                    let r = &vm.registers;
                    ui.label(
                        egui::RichText::new(format!(
                            "PC 0x{:04x}  SP 0x{:04x}  SREG {}",
                            r.pc,
                            r.sp,
                            r.sreg_flags()
                        ))
                        .font(font::mono(12.0))
                        .color(p.text),
                    );
                    ui.label(
                        egui::RichText::new(format!("cycle {}", r.cycle))
                            .font(font::mono(12.0))
                            .color(p.text_muted),
                    );
                    egui::Grid::new("regs")
                        .spacing(vec2(10.0, 2.0))
                        .show(ui, |ui| {
                            for (i, v) in r.r.iter().enumerate() {
                                ui.label(
                                    egui::RichText::new(format!("r{i}"))
                                        .font(font::mono(11.0))
                                        .color(p.text_faint),
                                );
                                ui.label(
                                    egui::RichText::new(format!("{v:02x}"))
                                        .font(font::mono(11.0))
                                        .color(p.text_dim),
                                );
                                if i % 4 == 3 {
                                    ui.end_row();
                                }
                            }
                        });
                });
            })
        });
}

fn compact() -> TableStyle {
    TableStyle {
        font: font::mono(11.5),
        header_font: font::mono(11.5),
        pad_x: 8.0,
        pad_y: 3.0,
        ..Default::default()
    }
}

fn empty(ui: &mut Ui, p: &Palette, msg: &str) {
    ui.label(
        egui::RichText::new(msg)
            .font(font::ui(12.5))
            .color(p.text_muted),
    );
}
