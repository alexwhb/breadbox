//! Live debugging UI (`MCU-5`): the debug toolbar and the Debug tab — Variables (call stack,
//! locals, globals, registers + SREG), Memory (SRAM / flash / EEPROM hex views with the SP /
//! X / Y / Z pointers and the bytes changed since the previous pause) and I/O registers
//! (grouped by peripheral). Pure views over [`DebugViewModel`].

use crate::bench::{BenchTarget, DebugCommand, DebugState, DebugViewModel, IoRegView, MemoryView};
use crate::theme::{Palette, font, pal};
use crate::widgets::{
    Btn, Cell, Col, Icon, Row, Seg, TableStyle, data_table, mini_segmented, section_header,
};
use eframe::egui::{
    self, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use serde::{Deserialize, Serialize};

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

/// Which view the Debug tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DebugView {
    #[default]
    Variables,
    Memory,
    Io,
}

/// Memory region of the hex view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MemRegion {
    #[default]
    Sram,
    Flash,
    Eeprom,
}

impl MemRegion {
    pub fn label(self) -> &'static str {
        match self {
            MemRegion::Sram => "SRAM",
            MemRegion::Flash => "Flash",
            MemRegion::Eeprom => "EEPROM",
        }
    }
}

/// Debug-tab UI state (view, region, "Go to" field, I/O filter).
#[derive(Debug, Clone, Default)]
pub struct DebugPanelState {
    pub view: DebugView,
    pub region: MemRegion,
    /// "Go to address" text.
    pub goto: String,
    /// Why the last "Go to" failed.
    pub goto_error: Option<String>,
    /// Scroll the hex view to this row next frame.
    scroll_to_row: Option<(MemRegion, usize)>,
    /// Byte the last "Go to" landed on (outlined): (region, address).
    pub mark: Option<(MemRegion, u32)>,
    /// I/O view filter (`PORT`, `TCCR`, `USART0`).
    pub io_filter: String,
}

const BYTES_PER_ROW: usize = 16;
const ROW_H: f32 = 17.0;

/// The bytes of `region` and the address of the first one.
fn region_bytes(m: &MemoryView, region: MemRegion) -> (&[u8], u32) {
    match region {
        MemRegion::Sram => (m.sram.as_slice(), m.sram_start as u32),
        MemRegion::Flash => (m.flash.as_slice(), 0),
        MemRegion::Eeprom => (m.eeprom.as_slice(), 0),
    }
}

fn prev_bytes(m: &MemoryView, region: MemRegion) -> Option<&[u8]> {
    match region {
        MemRegion::Sram => m.prev_sram.as_deref().map(Vec::as_slice),
        MemRegion::Eeprom => m.prev_eeprom.as_deref().map(Vec::as_slice),
        MemRegion::Flash => None,
    }
}

/// Parses a "Go to" entry: `0x08f0`, `8f0`, `$8f0`, or a pointer name (`SP`, `X`, `Y`, `Z`).
pub fn parse_address(text: &str, m: Option<&MemoryView>) -> Option<u32> {
    let t = text.trim();
    if let Some(m) = m
        && let Some((_, v)) = m.pointers().iter().find(|(n, _)| n.eq_ignore_ascii_case(t))
    {
        return Some(*v as u32);
    }
    let hex = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .or_else(|| t.strip_prefix('$'))
        .unwrap_or(t);
    if hex.is_empty() {
        return None;
    }
    u32::from_str_radix(&hex.replace('_', ""), 16).ok()
}

/// Address width (hex digits) for a region of `len` bytes starting at `base`.
fn addr_digits(base: u32, len: usize) -> usize {
    if base as usize + len > 0x10000 { 5 } else { 4 }
}

/// `SRAM 0x0100: 00 01 … 0f  ................` — one hex-dump row (also its accessible name).
pub fn hex_row_text(region: MemRegion, addr: u32, digits: usize, bytes: &[u8]) -> String {
    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let ascii: String = bytes
        .iter()
        .map(|&b| {
            if (0x20..0x7F).contains(&b) {
                b as char
            } else {
                '.'
            }
        })
        .collect();
    format!(
        "{} 0x{addr:0digits$x}: {}  {ascii}",
        region.label(),
        hex.join(" ")
    )
}

/// `PORTB 0x25 0x20 0010 0000` — an I/O row's accessible name.
pub fn io_row_text(r: &IoRegView) -> String {
    format!("{} 0x{:02x} 0x{:02x} {}", r.name, r.addr, r.value, r.bits())
}

/// Order of the I/O groups: ports, timers, serial, analog, buses, EEPROM, interrupts, core.
fn group_rank(name: &str) -> u8 {
    if name.starts_with("Port") {
        0
    } else if name.starts_with("Timer") {
        1
    } else if name.starts_with("USART") {
        2
    } else {
        match name {
            "ADC" => 3,
            "Analog comparator" => 4,
            "SPI" => 5,
            "TWI (I2C)" => 6,
            "EEPROM" => 7,
            "External interrupts" => 8,
            "Watchdog" => 9,
            "CPU" => 10,
            _ => 11,
        }
    }
}

/// The Debug tab: view selector + the selected view. Returns true while the Memory or I/O
/// view is on screen for a cycle-accurate target (the caller keeps the link's snapshot of it
/// fresh with `BenchLink::watch_memory`).
pub fn debug_tab(
    ui: &mut Ui,
    st: &mut DebugPanelState,
    vm: Option<&DebugViewModel>,
    target: Option<&BenchTarget>,
) -> bool {
    let p = pal(ui);
    let api_level = target.is_some_and(|t| t.fqbn.is_none());
    let mem = vm.and_then(|v| v.memory.as_ref());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        mini_segmented(
            ui,
            "Debug view",
            &mut st.view,
            &[
                Seg::new(DebugView::Variables, "Variables"),
                Seg::new(DebugView::Memory, "Memory"),
                Seg::new(DebugView::Io, "I/O registers"),
            ],
        );
        match st.view {
            DebugView::Variables => {}
            DebugView::Memory if !api_level => memory_toolbar(ui, p, st, mem),
            DebugView::Io if !api_level => {
                let te = ui.add(
                    egui::TextEdit::singleline(&mut st.io_filter)
                        .hint_text("Filter (PORT, TCCR…)")
                        .font(font::mono(12.0))
                        .desired_width(200.0),
                );
                te.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Filter registers")
                });
                if let Some(m) = mem {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        caption(
                            ui,
                            p,
                            &format!(
                                "{} · {} registers · amber = changed since the previous pause",
                                m.mcu,
                                m.io.len()
                            ),
                        );
                    });
                }
            }
            _ => {}
        }
    });
    ui.add_space(6.0);
    match st.view {
        DebugView::Variables => {
            debug_view(ui, vm);
            false
        }
        DebugView::Memory | DebugView::Io if api_level => {
            let board = target.map(|t| t.board.as_str()).unwrap_or("This board");
            empty(
                ui,
                p,
                &format!(
                    "Hex views need the cycle-accurate AVR core. {board} runs API-level (its sketch is compiled for this computer), so there is no SRAM, flash, EEPROM or I/O space to show."
                ),
            );
            false
        }
        DebugView::Memory => {
            match mem {
                Some(m) => memory_view(ui, p, st, m),
                None => empty(
                    ui,
                    p,
                    "No memory snapshot yet — Upload to sim (or run the bench) and the AVR's SRAM, flash and EEPROM appear here.",
                ),
            }
            true
        }
        DebugView::Io => {
            match mem {
                Some(m) => io_view(ui, p, st, m),
                None => empty(
                    ui,
                    p,
                    "No I/O snapshot yet — Upload to sim (or run the bench) to see the AVR's I/O registers.",
                ),
            }
            true
        }
    }
}

fn caption(ui: &mut Ui, p: &Palette, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .font(font::ui(11.5))
            .color(p.text_faint),
    );
}

/// Region selector, "Go to address" field, region caption.
fn memory_toolbar(ui: &mut Ui, p: &Palette, st: &mut DebugPanelState, mem: Option<&MemoryView>) {
    let before = st.region;
    mini_segmented(
        ui,
        "Memory region",
        &mut st.region,
        &[
            Seg::new(MemRegion::Sram, "SRAM"),
            Seg::new(MemRegion::Flash, "Flash"),
            Seg::new(MemRegion::Eeprom, "EEPROM"),
        ],
    );
    if st.region != before {
        st.goto_error = None;
    }
    let te = ui.add(
        egui::TextEdit::singleline(&mut st.goto)
            .hint_text("0x08f0, SP, Z…")
            .font(font::mono(12.0))
            .desired_width(110.0),
    );
    te.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Go to address"));
    let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if (Btn::ghost("Go")
        .height(26.0)
        .font_size(12.0)
        .show(ui)
        .clicked()
        || enter)
        && !st.goto.trim().is_empty()
    {
        go_to(st, mem);
    }
    if let Some(e) = &st.goto_error {
        ui.label(
            egui::RichText::new(e)
                .font(font::ui(11.5))
                .color(p.danger_soft),
        );
    }
    if let Some(m) = mem {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let text = match st.region {
                MemRegion::Sram => format!(
                    "{} B · 0x{:04x}–0x{:04x}",
                    m.sram.len(),
                    m.sram_start,
                    m.sram_end().saturating_sub(1)
                ),
                MemRegion::Flash => format!(
                    "{} KB · {} bytes programmed",
                    m.flash.len() / 1024,
                    group_thousands(m.flash_used)
                ),
                MemRegion::Eeprom => format!("{} B", m.eeprom.len()),
            };
            caption(ui, p, &text);
        });
    }
}

fn group_thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Applies the "Go to address" field: scrolls the hex view and outlines the byte.
fn go_to(st: &mut DebugPanelState, mem: Option<&MemoryView>) {
    let Some(m) = mem else {
        st.goto_error = Some("no snapshot yet".into());
        return;
    };
    let Some(addr) = parse_address(&st.goto, Some(m)) else {
        st.goto_error = Some(format!("“{}” is not an address", st.goto.trim()));
        return;
    };
    // a pointer name always means SRAM
    if m.pointers()
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case(st.goto.trim()))
    {
        st.region = MemRegion::Sram;
    }
    let (bytes, base) = region_bytes(m, st.region);
    let end = base + bytes.len() as u32;
    if addr < base || addr >= end {
        st.goto_error = Some(format!(
            "outside {} (0x{base:04x}–0x{:04x})",
            st.region.label(),
            end.saturating_sub(1)
        ));
        return;
    }
    st.goto_error = None;
    let row = ((addr - base) as usize) / BYTES_PER_ROW;
    st.scroll_to_row = Some((st.region, row));
    st.mark = Some((st.region, addr));
}

/// Pointer colours: SP cyan, X green, Y purple, Z blue.
fn pointer_color(p: &Palette, name: &str) -> Color32 {
    match name {
        "SP" => p.info,
        "X" => p.ok,
        "Y" => p.code_kw,
        _ => p.code_fn,
    }
}

/// The hex view (virtualised rows) + the pointer / legend column.
fn memory_view(ui: &mut Ui, p: &Palette, st: &mut DebugPanelState, m: &MemoryView) {
    let region = st.region;
    let (bytes, base) = region_bytes(m, region);
    if bytes.is_empty() {
        empty(ui, p, "Reading flash…");
        return;
    }
    let prev = prev_bytes(m, region).filter(|b| b.len() == bytes.len());
    let digits = addr_digits(base, bytes.len());
    let mono = font::mono(11.5);
    let cw = ui.fonts_mut(|f| f.glyph_width(&mono, '0'));
    let addr_w = (digits as f32 + 4.0) * cw;
    let cell_w = 3.0 * cw;
    let group_gap = cw;
    let ascii_x = addr_w + BYTES_PER_ROW as f32 * cell_w + group_gap + 2.0 * cw;
    let dump_w = ascii_x + BYTES_PER_ROW as f32 * cw + 8.0;
    let rows = bytes.len().div_ceil(BYTES_PER_ROW);
    let side_w = 190.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        let avail_h = ui.available_height();
        ui.vertical(|ui| {
            ui.set_width(dump_w.min(ui.available_width() - side_w));
            ui.spacing_mut().item_spacing.y = 0.0;
            // column header
            let (hr, _) = ui.allocate_exact_size(vec2(dump_w, 18.0), Sense::hover());
            let painter = ui.painter();
            painter.line_segment(
                [hr.left_bottom(), hr.right_bottom()],
                Stroke::new(1.0, p.border_faint),
            );
            painter.text(
                pos2(hr.left() + 4.0, hr.center().y - 1.0),
                egui::Align2::LEFT_CENTER,
                "address",
                mono.clone(),
                p.text_faint,
            );
            for i in 0..BYTES_PER_ROW {
                let x =
                    hr.left() + addr_w + i as f32 * cell_w + if i >= 8 { group_gap } else { 0.0 };
                painter.text(
                    pos2(x, hr.center().y),
                    egui::Align2::LEFT_CENTER,
                    format!("{i:02x}"),
                    mono.clone(),
                    p.text_ghost,
                );
            }
            painter.text(
                pos2(hr.left() + ascii_x, hr.center().y),
                egui::Align2::LEFT_CENTER,
                "ascii",
                mono.clone(),
                p.text_faint,
            );
            let mut area = egui::ScrollArea::vertical()
                .id_salt(("hex-view", region as u8))
                .auto_shrink([false, false])
                .max_height((avail_h - 20.0).max(ROW_H * 3.0));
            if let Some((r, row)) = st.scroll_to_row
                && r == region
            {
                area = area.vertical_scroll_offset(row as f32 * ROW_H);
                st.scroll_to_row = None;
            }
            area.show_rows(ui, ROW_H, rows, |ui, range| {
                for row in range {
                    let start = row * BYTES_PER_ROW;
                    let end = (start + BYTES_PER_ROW).min(bytes.len());
                    let addr = base + start as u32;
                    hex_row(
                        ui,
                        p,
                        HexRow {
                            region,
                            addr,
                            digits,
                            bytes: &bytes[start..end],
                            prev: prev.map(|b| &b[start..end]),
                            mem: m,
                            mark: st.mark,
                            used: (region == MemRegion::Flash).then_some(m.flash_used as u32),
                        },
                        &mono,
                        (addr_w, cell_w, group_gap, ascii_x, cw, dump_w),
                    );
                }
            });
        });
        ui.vertical(|ui| {
            ui.set_width(side_w);
            ui.spacing_mut().item_spacing.y = 6.0;
            if region == MemRegion::Sram {
                section_header(ui, "Pointers");
                for (name, v) in m.pointers() {
                    let c = pointer_color(p, name);
                    let label = format!("{name} 0x{v:04x}");
                    let resp = ui
                        .horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                            ui.painter().rect_stroke(
                                r,
                                CornerRadius::same(2),
                                Stroke::new(1.5, c),
                                StrokeKind::Inside,
                            );
                            Btn::plain(&label)
                                .height(20.0)
                                .font_size(12.0)
                                .tooltip("Show in the hex view")
                                .show(ui)
                        })
                        .inner;
                    if resp.clicked() {
                        st.goto = name.to_string();
                        go_to(st, Some(m));
                    }
                }
            }
            if prev.is_some() {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    ui.painter()
                        .rect_filled(r, CornerRadius::same(2), p.accent_bg);
                    ui.painter().rect_stroke(
                        r,
                        CornerRadius::same(2),
                        Stroke::new(1.0, p.accent),
                        StrokeKind::Inside,
                    );
                    caption(ui, p, "changed since the previous pause");
                });
            }
            caption(
                ui,
                p,
                &format!("{} · cycle {}", m.mcu, group_thousands(m.cycle as usize)),
            );
        });
    });
}

struct HexRow<'a> {
    region: MemRegion,
    addr: u32,
    digits: usize,
    bytes: &'a [u8],
    prev: Option<&'a [u8]>,
    mem: &'a MemoryView,
    mark: Option<(MemRegion, u32)>,
    /// Flash: programmed length (bytes past it are dimmed).
    used: Option<u32>,
}

fn hex_row(
    ui: &mut Ui,
    p: &Palette,
    row: HexRow<'_>,
    mono: &FontId,
    (addr_w, cell_w, group_gap, ascii_x, cw, w): (f32, f32, f32, f32, f32, f32),
) {
    let (r, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::hover());
    let text = hex_row_text(row.region, row.addr, row.digits, row.bytes);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
    if !ui.is_rect_visible(r) {
        return;
    }
    let painter = ui.painter();
    let cy = r.center().y;
    painter.text(
        pos2(r.left() + 4.0, cy),
        egui::Align2::LEFT_CENTER,
        format!("0x{:0w$x}", row.addr, w = row.digits),
        mono.clone(),
        p.text_faint,
    );
    for (i, &b) in row.bytes.iter().enumerate() {
        let a = row.addr + i as u32;
        let x = r.left() + addr_w + i as f32 * cell_w + if i >= 8 { group_gap } else { 0.0 };
        let cell = Rect::from_min_size(
            pos2(x - 2.0, r.top() + 1.0),
            vec2(2.0 * cw + 4.0, ROW_H - 2.0),
        );
        let changed = row.prev.is_some_and(|pv| pv.get(i) != Some(&b));
        if changed {
            painter.rect_filled(cell, CornerRadius::same(3), p.accent_bg);
        }
        if row.region == MemRegion::Sram {
            let mut inset = 0.0;
            for (name, v) in row.mem.pointers() {
                if v as u32 == a {
                    painter.rect_stroke(
                        cell.shrink(inset),
                        CornerRadius::same(3),
                        Stroke::new(1.5, pointer_color(p, name)),
                        StrokeKind::Inside,
                    );
                    inset += 2.0;
                }
            }
        }
        if row.mark == Some((row.region, a)) {
            painter.rect_stroke(
                cell.expand(1.0),
                CornerRadius::same(3),
                Stroke::new(1.5, p.accent),
                StrokeKind::Outside,
            );
        }
        let unused = row.used.is_some_and(|u| a >= u);
        let color = if changed {
            p.accent
        } else if unused {
            p.text_ghost
        } else if b == 0 {
            p.text_faint
        } else {
            p.text_dim
        };
        painter.text(
            pos2(x, cy),
            egui::Align2::LEFT_CENTER,
            format!("{b:02x}"),
            mono.clone(),
            color,
        );
        let ch = if (0x20..0x7F).contains(&b) {
            b as char
        } else {
            '.'
        };
        painter.text(
            pos2(r.left() + ascii_x + i as f32 * cw, cy),
            egui::Align2::LEFT_CENTER,
            ch,
            mono.clone(),
            if changed { p.accent } else { p.text_muted },
        );
    }
}

/// I/O registers grouped by peripheral, in balanced columns.
fn io_view(ui: &mut Ui, p: &Palette, st: &DebugPanelState, m: &MemoryView) {
    let filter = st.io_filter.trim().to_ascii_lowercase();
    let mut groups: Vec<(&'static str, Vec<&IoRegView>)> = Vec::new();
    for r in &m.io {
        if !filter.is_empty()
            && !r.name.to_ascii_lowercase().contains(&filter)
            && !r.peripheral.to_ascii_lowercase().contains(&filter)
        {
            continue;
        }
        match groups.iter_mut().find(|g| g.0 == r.peripheral) {
            Some(g) => g.1.push(r),
            None => groups.push((r.peripheral, vec![r])),
        }
    }
    groups.sort_by(|a, b| {
        group_rank(a.0)
            .cmp(&group_rank(b.0))
            .then_with(|| a.0.cmp(b.0))
    });
    if groups.is_empty() {
        empty(ui, p, "No register matches the filter.");
        return;
    }
    const COL_W: f32 = 236.0;
    const IO_ROW_H: f32 = 16.0;
    const HEAD_H: f32 = 22.0;
    let n_cols = ((ui.available_width() + 16.0) / (COL_W + 16.0))
        .floor()
        .max(1.0) as usize;
    let mut cols: Vec<(f32, Vec<usize>)> = vec![(0.0, Vec::new()); n_cols];
    for (i, g) in groups.iter().enumerate() {
        let h = HEAD_H + g.1.len() as f32 * IO_ROW_H + 8.0;
        if let Some(c) = cols.iter_mut().min_by(|a, b| a.0.total_cmp(&b.0)) {
            c.0 += h;
            c.1.push(i);
        }
    }
    let mono = font::mono(11.5);
    egui::ScrollArea::vertical()
        .id_salt("io-view")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                for (_, idx) in &cols {
                    ui.vertical(|ui| {
                        ui.set_width(COL_W);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for &gi in idx {
                            let (name, regs) = &groups[gi];
                            let (hr, _) =
                                ui.allocate_exact_size(vec2(COL_W, HEAD_H), Sense::hover());
                            let mut c = crate::widgets::child_ui(ui, hr);
                            c.add_space(4.0);
                            section_header(&mut c, name);
                            for r in regs {
                                io_row(ui, p, r, &mono, COL_W, IO_ROW_H);
                            }
                            ui.add_space(8.0);
                        }
                    });
                }
            });
        });
}

fn io_row(ui: &mut Ui, p: &Palette, r: &IoRegView, mono: &FontId, w: f32, h: f32) {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let text = io_row_text(r);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
    if !ui.is_rect_visible(rect) {
        return;
    }
    let changed = r.changed();
    let painter = ui.painter();
    if changed {
        painter.rect_filled(rect, CornerRadius::same(3), p.accent_bg2);
    }
    let cy = rect.center().y;
    let t = |x: f32, s: String, c: Color32| {
        painter.text(
            pos2(rect.left() + x, cy),
            egui::Align2::LEFT_CENTER,
            s,
            mono.clone(),
            c,
        );
    };
    t(4.0, r.name.to_string(), p.text);
    t(82.0, format!("0x{:02x}", r.addr), p.text_faint);
    t(
        126.0,
        format!("0x{:02x}", r.value),
        if changed { p.accent } else { p.text_dim },
    );
    // bits, MSB first; set bits bright, changed bits amber
    let cw = ui.fonts_mut(|f| f.glyph_width(mono, '0'));
    let mut x = rect.left() + 166.0;
    for bit in (0..8).rev() {
        let on = r.value & (1 << bit) != 0;
        let flipped = r.prev.is_some_and(|pv| (pv ^ r.value) & (1 << bit) != 0);
        let c = if flipped {
            p.accent
        } else if on {
            p.text
        } else {
            p.text_ghost
        };
        painter.text(
            pos2(x, cy),
            egui::Align2::LEFT_CENTER,
            if on { "1" } else { "0" },
            mono.clone(),
            c,
        );
        x += cw;
        if bit == 4 {
            x += cw;
        }
    }
}

/// The Variables view (call stack, locals + globals, registers).
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn mem() -> MemoryView {
        MemoryView {
            mcu: "ATmega328P".into(),
            sram_start: 0x100,
            sram: Arc::new(vec![0; 2048]),
            eeprom: Arc::new(vec![0xFF; 1024]),
            flash: Arc::new(vec![0xFF; 32 * 1024]),
            sp: 0x08F5,
            x: 0x0123,
            y: 0x08FB,
            z: 0x0200,
            ..Default::default()
        }
    }

    #[test]
    fn go_to_parses_hex_and_pointer_names() {
        let m = mem();
        assert_eq!(parse_address("0x08f0", None), Some(0x8F0));
        assert_eq!(parse_address("8F0", None), Some(0x8F0));
        assert_eq!(parse_address("$1_00", None), Some(0x100));
        assert_eq!(parse_address("sp", Some(&m)), Some(0x8F5));
        assert_eq!(parse_address("Z", Some(&m)), Some(0x200));
        assert_eq!(parse_address("nope", Some(&m)), None);
        assert_eq!(parse_address("", None), None);
        let mut st = DebugPanelState {
            goto: "0x08f0".into(),
            ..Default::default()
        };
        go_to(&mut st, Some(&m));
        assert_eq!(st.goto_error, None);
        assert_eq!(
            st.scroll_to_row,
            Some((MemRegion::Sram, (0x8F0 - 0x100) / 16))
        );
        assert_eq!(st.mark, Some((MemRegion::Sram, 0x8F0)));
        st.goto = "0x50".into();
        go_to(&mut st, Some(&m));
        assert!(
            st.goto_error
                .as_deref()
                .unwrap()
                .starts_with("outside SRAM")
        );
        // pointer names jump to SRAM from any region
        st.region = MemRegion::Eeprom;
        st.goto = "Y".into();
        go_to(&mut st, Some(&m));
        assert_eq!(st.region, MemRegion::Sram);
        assert_eq!(st.mark, Some((MemRegion::Sram, 0x8FB)));
    }

    #[test]
    fn row_labels() {
        let bytes: Vec<u8> = (0x41..0x51).collect();
        assert_eq!(
            hex_row_text(MemRegion::Sram, 0x100, 4, &bytes),
            "SRAM 0x0100: 41 42 43 44 45 46 47 48 49 4a 4b 4c 4d 4e 4f 50  ABCDEFGHIJKLMNOP"
        );
        assert!(
            hex_row_text(MemRegion::Flash, 0x3fff0, 5, &[0, 0xff])
                .starts_with("Flash 0x3fff0: 00 ff  ..")
        );
        let r = IoRegView {
            name: "PORTB",
            addr: 0x25,
            peripheral: "Port B",
            value: 0x20,
            prev: Some(0x00),
        };
        assert_eq!(io_row_text(&r), "PORTB 0x25 0x20 0010 0000");
        assert!(r.changed());
        assert_eq!(addr_digits(0, 256 * 1024), 5);
        assert_eq!(addr_digits(0x100, 2048), 4);
        assert_eq!(group_thousands(9842), "9,842");
        assert!(group_rank("Port B") < group_rank("Timer/Counter0"));
        assert!(group_rank("USART0") < group_rank("CPU"));
    }
}
