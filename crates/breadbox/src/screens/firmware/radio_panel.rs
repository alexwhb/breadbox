//! The Radios panel (`RF-7`, design screen 2, right): radio cards, the floor plan with the
//! bench's placement (`RF-1`), packet log with pcap-ng / CSV export (`RF-4`), network
//! conditions (`RF-3`), Bluetooth LE card with "Add BLE hat". A pure view over
//! [`RadioPanelModel`] that returns [`RadioPanelOutput`] — the Workbench can reuse it.

use crate::bench::{
    NetConditionsView, PlacementView, RadioCommand, RadioKind, RadioPanelModel, StatusTone,
};
use crate::circuit::CircuitDoc;
use crate::theme::{Palette, font, pal};
use crate::widgets::{self, Btn, Icon, StopSlider, Tone, checkbox, section_header_row};
use bb_circuit::edit::{Op, Transaction};
use bb_circuit::project::{Endpoint, PartInstance, Wire};
use eframe::egui::{self, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use std::time::{Duration, Instant};

/// Local UI state (slider values while dragging, bridge address field, floor-plan drag).
#[derive(Debug, Clone)]
pub struct RadioPanelState {
    pub latency: f64,
    /// Latency jitter (± ms).
    pub jitter: f64,
    pub loss: f64,
    pub drop_ap: bool,
    /// "Drop the AP at t = N s".
    pub drop_ap_s: f64,
    drop_ap_text: String,
    pub bridge: bool,
    pub bridge_addr: String,
    /// Virtual-phone terminal input.
    pub phone_input: String,
    synced: bool,
    /// Bench position while it is being dragged (metres).
    bench_drag: Option<(f64, f64)>,
    /// Position set here that the bench has not reported back yet.
    bench_pending: Option<((f64, f64), Instant)>,
    /// Floor-plan view, frozen while dragging.
    plan_view: Option<PlanView>,
}

impl Default for RadioPanelState {
    fn default() -> Self {
        RadioPanelState {
            phone_input: String::new(),
            latency: 40.0,
            jitter: 10.0,
            loss: 2.0,
            drop_ap: false,
            drop_ap_s: 60.0,
            drop_ap_text: "60".into(),
            bridge: false,
            bridge_addr: "localhost:1883".into(),
            synced: false,
            bench_drag: None,
            bench_pending: None,
            plan_view: None,
        }
    }
}

impl RadioPanelState {
    fn conditions(&self) -> NetConditionsView {
        NetConditionsView {
            latency_ms: self.latency,
            jitter_ms: self.jitter,
            loss_pct: self.loss,
            drop_ap_at_s: self.drop_ap.then_some(self.drop_ap_s),
            bridge: None,
            bridge_status: None,
        }
    }
}

/// Packet-log export formats (`RF-4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// pcap-ng with synthetic IP frames (Wireshark).
    Pcapng,
    Csv,
}

impl LogFormat {
    /// File extension.
    pub fn ext(self) -> &'static str {
        match self {
            LogFormat::Pcapng => "pcapng",
            LogFormat::Csv => "csv",
        }
    }
    /// Format name for `SimBench::export_packet_log`.
    pub fn engine_name(self) -> &'static str {
        self.ext()
    }
    /// Save-dialog filter name.
    pub fn filter_name(self) -> &'static str {
        match self {
            LogFormat::Pcapng => "pcap-ng capture",
            LogFormat::Csv => "CSV",
        }
    }
}

/// What the panel wants done.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RadioPanelOutput {
    /// Commands for the bench (`AddBleHat` is the caller's: it edits the circuit).
    pub commands: Vec<RadioCommand>,
    /// "Export pcap-ng…" / "Export CSV…" was clicked.
    pub export: Option<LogFormat>,
    /// The bench was dropped at a new floor-plan position: store it with the project.
    pub bench_pos: Option<(f64, f64)>,
}

/// Draws the panel; returns commands for the bench and actions for the screen.
pub fn radio_panel(
    ui: &mut Ui,
    model: Option<&RadioPanelModel>,
    st: &mut RadioPanelState,
) -> RadioPanelOutput {
    let p = pal(ui);
    let mut out = RadioPanelOutput::default();
    let mut cmds = Vec::new();
    ui.spacing_mut().item_spacing.y = 16.0;
    section_header_row(ui, "Radios", "virtual RF space");
    crate::widgets::radio_tools::buttons(ui);

    // push the panel's conditions to a fresh space once
    if let Some(m) = model
        && !st.synced
    {
        st.synced = true;
        if m.conditions != st.conditions() {
            cmds.push(RadioCommand::SetConditions(st.conditions()));
        }
    }

    let cards = model.map(|m| m.cards.as_slice()).unwrap_or(&[]);
    if cards.is_empty() {
        empty_card(
            ui,
            p,
            Icon::Wifi,
            "No radio on this bench",
            "Add an ESP-01 Wi-Fi module (or an ESP32) on the Workbench and it joins SIM-AP here: DHCP, DNS, MQTT on the built-in broker.",
            None,
        );
    }
    for c in cards.iter().filter(|c| c.kind != RadioKind::Ble) {
        widgets::inner_card_frame(p, false)
            .fill(p.bg_header)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let icon = match c.kind {
                        RadioKind::Lora => Icon::Lora,
                        _ => Icon::Wifi,
                    };
                    icon.show(ui, 18.0, p.info);
                    ui.label(
                        egui::RichText::new(format!("{} · {}", c.label, c.title))
                            .font(font::heading(14.0))
                            .color(p.text),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let tone = match c.tone {
                            StatusTone::Ok => Tone::Ok,
                            StatusTone::Busy => Tone::Warn,
                            StatusTone::Error => Tone::Danger,
                            StatusTone::Off => Tone::Neutral,
                        };
                        widgets::tag(ui, &c.status, tone);
                    });
                });
                widgets::kv_grid(ui, &format!("radio-{}", c.label), &c.rows, 120.0);
            });
    }

    // floor plan: where the bench stands (RF-1)
    if let Some(pl) = model.and_then(|m| m.placement.as_ref())
        && (!cards.is_empty() || !pl.aps.is_empty())
    {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Placement")
                        .font(font::semibold(12.0))
                        .color(p.text_muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("drag the bench · arrows nudge 0.5 m")
                            .font(font::ui(11.0))
                            .color(p.text_faint),
                    );
                });
            });
            floor_plan(ui, p, st, pl, &mut cmds, &mut out.bench_pos);
        });
    }

    // packet log
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        let has_rows = model.is_some_and(|m| !m.log.is_empty());
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Packet log")
                    .font(font::semibold(12.0))
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if Btn::ghost("Export CSV…")
                    .height(26.0)
                    .font_size(12.0)
                    .enabled(has_rows)
                    .tooltip("Save the packet log as CSV")
                    .show(ui)
                    .clicked()
                {
                    out.export = Some(LogFormat::Csv);
                }
                if Btn::ghost("Export pcap-ng…")
                    .height(26.0)
                    .font_size(12.0)
                    .enabled(has_rows)
                    .tooltip("Save the packet log for Wireshark (synthetic IP frames)")
                    .show(ui)
                    .clicked()
                {
                    out.export = Some(LogFormat::Pcapng);
                }
            });
        });
        packet_log(ui, p, model);
    });

    // network conditions
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        ui.label(
            egui::RichText::new("Network conditions")
                .font(font::semibold(12.0))
                .color(p.text_muted),
        );
        let jitter = st.jitter;
        let lat_fmt = move |v: f64| format!("{v:.0} ms ± {jitter:.0}");
        let jit_fmt = |v: f64| format!("± {v:.0} ms");
        let loss_fmt = |v: f64| format!("{v:.0} %");
        let a = StopSlider::new("Latency", 0.0..=500.0, &lat_fmt)
            .step(1.0)
            .show(ui, &mut st.latency);
        let j = StopSlider::new("Jitter", 0.0..=200.0, &jit_fmt)
            .step(1.0)
            .show(ui, &mut st.jitter);
        let b = StopSlider::new("Packet loss", 0.0..=50.0, &loss_fmt)
            .step(1.0)
            .show(ui, &mut st.loss);
        let mut drop_changed = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            drop_changed |= checkbox(ui, &mut st.drop_ap, "Drop the AP at t =").changed();
            let te = ui.add(
                egui::TextEdit::singleline(&mut st.drop_ap_text)
                    .font(font::mono(12.0))
                    .desired_width(40.0),
            );
            te.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Drop the AP at (s)")
            });
            if te.lost_focus() {
                match st.drop_ap_text.trim().parse::<f64>() {
                    Ok(v) if v.is_finite() && v >= 0.0 => {
                        let v = v.min(86_400.0);
                        if (v - st.drop_ap_s).abs() > f64::EPSILON {
                            st.drop_ap_s = v;
                            drop_changed |= st.drop_ap;
                        }
                        st.drop_ap_text = format!("{v}");
                    }
                    _ => st.drop_ap_text = format!("{}", st.drop_ap_s),
                }
            }
            ui.label(
                egui::RichText::new("s (test reconnect)")
                    .font(font::ui(13.0))
                    .color(p.text),
            );
        });
        if a.changed() || j.changed() || b.changed() || drop_changed {
            cmds.push(RadioCommand::SetConditions(st.conditions()));
        }
        let c2 = checkbox(ui, &mut st.bridge, "Bridge to my real LAN broker");
        if st.bridge {
            ui.horizontal(|ui| {
                ui.add_space(26.0);
                let r = ui.add(
                    egui::TextEdit::singleline(&mut st.bridge_addr)
                        .font(font::mono(12.0))
                        .desired_width(170.0)
                        .hint_text("host:port"),
                );
                let status = model
                    .and_then(|m| m.conditions.bridge_status.clone())
                    .unwrap_or_else(|| "not connected".into());
                ui.label(
                    egui::RichText::new(status)
                        .font(font::ui(11.0))
                        .color(p.text_muted),
                );
                if r.lost_focus()
                    && let Some(b) = parse_addr(&st.bridge_addr)
                {
                    cmds.push(RadioCommand::SetLanBridge(Some(b)));
                }
            });
        }
        if c2.changed() {
            cmds.push(RadioCommand::SetLanBridge(if st.bridge {
                parse_addr(&st.bridge_addr)
            } else {
                None
            }));
        }
    });

    // Bluetooth LE
    match model.and_then(|m| m.ble.as_ref()) {
        Some(ble) => {
            widgets::inner_card_frame(p, false).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    Icon::Bluetooth.show(ui, 18.0, p.info);
                    ui.label(
                        egui::RichText::new("Bluetooth LE")
                            .font(font::heading(14.0))
                            .color(p.text),
                    );
                });
                match &ble.phone {
                    Some(phone) => cmds.extend(phone_ui(ui, p, phone, &mut st.phone_input)),
                    None => {
                        for (name, status) in &ble.devices {
                            ui.horizontal(|ui| {
                                Icon::Phone.show(ui, 14.0, p.text_muted);
                                ui.label(
                                    egui::RichText::new(name)
                                        .font(font::mono(12.0))
                                        .color(p.text),
                                );
                                widgets::tag(ui, status, Tone::Neutral);
                            });
                        }
                    }
                }
            });
        }
        None => {
            if empty_card(
                ui,
                p,
                Icon::Bluetooth,
                "Bluetooth LE",
                "No BLE radio on this bench. Add a BLE hat and a virtual phone appears as a central — scan, connect, read GATT characteristics.",
                Some("Add BLE hat"),
            ) {
                cmds.push(RadioCommand::AddBleHat);
            }
        }
    }
    out.commands = cmds;
    out
}

/// Floor-plan view: metres at the canvas' left/top edge and pixels per metre.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PlanView {
    x0: f64,
    y0: f64,
    ppm: f64,
}

impl PlanView {
    /// Fits the access points and the bench (≥ 8 × 4 m, 1.5 m margin) into `size`.
    fn fit(pl: &PlacementView, bench: (f64, f64), size: egui::Vec2) -> PlanView {
        let pts = pl.aps.iter().map(|a| (a.x, a.y)).chain([bench]);
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (x, y) in pts {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let w = (x1 - x0 + 3.0).max(8.0);
        let h = (y1 - y0 + 3.0).max(4.0);
        let ppm = (size.x as f64 / w).min(size.y as f64 / h);
        PlanView {
            x0: cx - size.x as f64 / ppm / 2.0,
            y0: cy - size.y as f64 / ppm / 2.0,
            ppm,
        }
    }
    fn to_screen(self, r: Rect, (x, y): (f64, f64)) -> egui::Pos2 {
        pos2(
            r.left() + ((x - self.x0) * self.ppm) as f32,
            r.top() + ((y - self.y0) * self.ppm) as f32,
        )
    }
    fn to_metres(self, r: Rect, p: egui::Pos2) -> (f64, f64) {
        (
            self.x0 + (p.x - r.left()) as f64 / self.ppm,
            self.y0 + (p.y - r.top()) as f64 / self.ppm,
        )
    }
}

/// Snaps a floor-plan position to 0.1 m inside ±100 m.
fn snap_pos((x, y): (f64, f64)) -> (f64, f64) {
    let s = |v: f64| ((v * 10.0).round() / 10.0).clamp(-100.0, 100.0);
    (s(x), s(y))
}

/// `SIM-AP · 3.2 m · 1 wall · -54 dBm`.
pub fn link_summary(pl: &PlacementView) -> String {
    match pl.best_ap() {
        Some(a) => format!(
            "{} · {:.1} m · {} · {:.0} dBm",
            a.ssid,
            a.distance_m,
            match a.walls {
                1 => "1 wall".to_string(),
                n => format!("{n} walls"),
            },
            a.rssi_dbm
        ),
        None => "no access point".into(),
    }
}

/// The floor plan (`RF-1`): access points, walls, the draggable bench and its link readout.
fn floor_plan(
    ui: &mut Ui,
    p: &Palette,
    st: &mut RadioPanelState,
    pl: &PlacementView,
    cmds: &mut Vec<RadioCommand>,
    done: &mut Option<(f64, f64)>,
) {
    // where the bench is drawn: the drag, then a position not reported back yet, then the model
    if let Some((pos, at)) = st.bench_pending
        && (at.elapsed() > Duration::from_secs(3)
            || ((pos.0 - pl.bench.0).abs() < 0.05 && (pos.1 - pl.bench.1).abs() < 0.05))
    {
        st.bench_pending = None;
    }
    let bench = st
        .bench_drag
        .or(st.bench_pending.map(|b| b.0))
        .unwrap_or(pl.bench);
    let w = ui.available_width();
    let (rect, canvas) = ui.allocate_exact_size(vec2(w, 150.0), Sense::click_and_drag());
    canvas.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Floor plan"));
    let dragging = st.bench_drag.is_some();
    let view = match st.plan_view {
        Some(v) if dragging => v,
        _ => {
            let v = PlanView::fit(pl, bench, rect.size());
            st.plan_view = Some(v);
            v
        }
    };
    let bp = view.to_screen(rect, bench);
    let marker_rect = Rect::from_center_size(bp, vec2(18.0, 18.0));
    let marker = ui.interact(
        marker_rect,
        ui.scope_id().with("bench-marker"),
        Sense::click_and_drag(),
    );
    let label = format!("Bench position {:.1} m, {:.1} m", bench.0, bench.1);
    marker.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    // drag (marker or anywhere on the plan) / click to place / arrow keys
    let mut new_pos = None;
    let mut release = false;
    for r in [&marker, &canvas] {
        if (r.dragged() || r.is_pointer_button_down_on())
            && let Some(pt) = r.interact_pointer_pos()
        {
            new_pos = Some(snap_pos(view.to_metres(rect, pt)));
        }
        if r.drag_stopped() || r.clicked() {
            release = true;
            if let Some(pt) = r.interact_pointer_pos() {
                new_pos = Some(snap_pos(view.to_metres(rect, pt)));
            }
        }
    }
    if marker.clicked() {
        marker.request_focus();
    }
    if marker.has_focus() {
        let (l, r, u, d) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
            )
        });
        let dx = (r as i32 - l as i32) as f64 * 0.5;
        let dy = (d as i32 - u as i32) as f64 * 0.5;
        if dx != 0.0 || dy != 0.0 {
            new_pos = Some(snap_pos((bench.0 + dx, bench.1 + dy)));
            release = true;
        }
    }
    if let Some(np) = new_pos {
        if np != bench {
            cmds.push(RadioCommand::SetBenchPos { x: np.0, y: np.1 });
        }
        st.bench_drag = Some(np);
    }
    if release && let Some(np) = st.bench_drag.take() {
        st.bench_pending = Some((np, Instant::now()));
        *done = Some(np);
    }
    let bench = st
        .bench_drag
        .or(st.bench_pending.map(|b| b.0))
        .unwrap_or(pl.bench);
    // draw
    let painter = ui.painter_at(rect);
    painter.rect(
        rect,
        CornerRadius::same(8),
        p.bg_canvas,
        Stroke::new(1.0, p.border),
        StrokeKind::Inside,
    );
    let step = if view.ppm >= 12.0 { 1.0 } else { 2.0 };
    let (mx0, my0) = view.to_metres(rect, rect.left_top());
    let (mx1, my1) = view.to_metres(rect, rect.right_bottom());
    let mut gx = (mx0 / step).ceil() * step;
    while gx <= mx1 {
        let mut gy = (my0 / step).ceil() * step;
        while gy <= my1 {
            painter.circle_filled(view.to_screen(rect, (gx, gy)), 1.0, p.border_strong);
            gy += step;
        }
        gx += step;
    }
    for wall in &pl.walls {
        let width = if wall.loss_db >= 20.0 { 3.5 } else { 2.5 };
        painter.line_segment(
            [view.to_screen(rect, wall.a), view.to_screen(rect, wall.b)],
            Stroke::new(width, p.text_ghost),
        );
    }
    let bp = view.to_screen(rect, bench);
    if let Some(a) = pl.best_ap() {
        let ap = view.to_screen(rect, (a.x, a.y));
        painter.add(egui::Shape::dashed_line(
            &[ap, bp],
            Stroke::new(1.0, p.info.gamma_multiply(0.6)),
            4.0,
            3.0,
        ));
    }
    for a in &pl.aps {
        let c = view.to_screen(rect, (a.x, a.y));
        painter.circle(c, 10.0, p.bg_panel, Stroke::new(1.2, p.info));
        Icon::Wifi.paint(
            &painter,
            Rect::from_center_size(c, vec2(12.0, 12.0)),
            p.info,
        );
        painter.text(
            c + vec2(0.0, 13.0),
            egui::Align2::CENTER_TOP,
            &a.ssid,
            font::ui(10.5),
            p.text_muted,
        );
    }
    let hot = marker.hovered() || marker.dragged() || marker.has_focus() || dragging;
    let mr = Rect::from_center_size(bp, vec2(14.0, 14.0)).expand(if hot { 1.5 } else { 0.0 });
    painter.rect(
        mr,
        CornerRadius::same(3),
        p.accent_bg,
        Stroke::new(1.5, p.accent),
        StrokeKind::Inside,
    );
    painter.text(
        mr.right_center() + vec2(5.0, 0.0),
        egui::Align2::LEFT_CENTER,
        "bench",
        font::ui(10.5),
        p.accent_soft,
    );
    painter.text(
        rect.right_bottom() + vec2(-8.0, -6.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("x {:.1} m · y {:.1} m", bench.0, bench.1),
        font::mono(10.5),
        p.text_faint,
    );
    if marker.hovered() || canvas.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    // readout (the engine's numbers)
    ui.label(
        egui::RichText::new(link_summary(pl))
            .font(font::mono(12.0))
            .color(p.text),
    );
}

/// What [`add_ble_hat`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct BleHatReport {
    /// `"U2"`.
    pub reference: String,
    /// The board it was wired to (`"U1"`).
    pub board: String,
    /// `"VCC→5V, GND→GND, TXD→D10, RXD→D11"`.
    pub wiring: String,
}

/// "Add BLE hat" (`RF-7`): an HM-10 next to the first board, wired VCC→5V (3V3 on host
/// boards), GND→GND, TXD→D10 / RXD→D11 (ESP32: RX2 / TX2, Pico: GP1 / GP0), as one
/// undoable edit. The module advertises as `HMSoft` as soon as the bench runs.
pub fn add_ble_hat(doc: &mut CircuitDoc) -> Result<BleHatReport, String> {
    let lib = doc.lib.clone();
    let project = doc.project().clone();
    let def = lib
        .get("hm10")
        .ok_or("the HM-10 module is not in the parts library")?
        .clone();
    let board = project
        .parts
        .iter()
        .find(|p| {
            crate::project::board_kind(&p.part).is_some()
                && lib.get(&p.part).is_some_and(|d| d.is_mcu())
        })
        .ok_or("Place a board (UNO, Nano, Mega, ESP32 or Pico) on the bench first.")?
        .clone();
    let bdef = lib
        .get(&board.part)
        .ok_or_else(|| format!("unknown board `{}`", board.part))?;
    let has = |pin: &str| bdef.pin(pin).is_some();
    let host = crate::project::board_kind(&board.part).is_some_and(|k| k.fqbn.is_none());
    let vcc = if host || !has("5V") { "3V3" } else { "5V" };
    let data = [("D10", "D11"), ("RX2", "TX2"), ("GP1", "GP0")]
        .into_iter()
        .find(|(rx, tx)| has(rx) && has(tx));
    // to the right of the board, below anything already there
    let (bx, by) = board
        .bench
        .as_ref()
        .map(|b| (b.at.x, b.at.y))
        .unwrap_or((0.0, 4.0));
    let x = (bx + bdef.bench.size.0 + 4.0).round();
    let (w, h) = def.bench.size;
    let overlaps = |x: f64, y: f64| {
        project.parts.iter().any(|q| {
            let (Some(b), Some(d)) = (&q.bench, lib.get(&q.part)) else {
                return false;
            };
            x < b.at.x + d.bench.size.0
                && b.at.x < x + w
                && y < b.at.y + d.bench.size.1
                && b.at.y < y + h
        })
    };
    let mut y = by.round();
    for _ in 0..12 {
        if !overlaps(x, y) {
            break;
        }
        y += h + 2.0;
    }
    let reference = project.next_ref(&def.ref_prefix);
    let mut id = reference.to_ascii_lowercase();
    if project.part(&id).is_some() || project.wire(&id).is_some() {
        id = project.next_id(&format!("{id}_"));
    }
    let mut tx = Transaction::new("Add BLE hat");
    tx.ops.push(Op::Part {
        before: None,
        after: Some(PartInstance::new(&id, &reference, "hm10").at(x, y)),
        index: None,
    });
    let mut links: Vec<(&str, &str, &str)> = vec![("VCC", vcc, "red"), ("GND", "GND", "black")];
    if let Some((rx, txp)) = data {
        links.push(("TXD", rx, "blue"));
        links.push(("RXD", txp, "yellow"));
    }
    let mut n = 0;
    let mut wiring = Vec::new();
    for (pin, board_pin, color) in &links {
        let wid = loop {
            n += 1;
            let w = format!("{id}_w{n}");
            if project.wire(&w).is_none() && project.part(&w).is_none() {
                break w;
            }
        };
        tx.ops.push(Op::Wire {
            before: None,
            after: Some(Wire {
                id: wid,
                a: Endpoint::pin(&id, pin),
                b: Endpoint::pin(&board.id, board_pin),
                color: color.to_string(),
                bends: Vec::new(),
            }),
            index: None,
        });
        wiring.push(format!("{pin}→{board_pin}"));
    }
    doc.editor.commit(tx);
    doc.touched();
    Ok(BleHatReport {
        reference,
        board: board.reference.clone(),
        wiring: wiring.join(", "),
    })
}

/// Stores the bench's floor-plan position with the project's radio settings
/// (`project.json` → `radios.bench_pos`), so it survives a reload.
pub fn store_bench_pos(doc: &mut CircuitDoc, (x, y): (f64, f64)) {
    let mut r = bb_sim::radio::ProjectRadios::from_project(doc.project());
    let pos = bb_radio::geo::Pos::new(x, y);
    if r.bench_pos == pos {
        return;
    }
    r.bench_pos = pos;
    r.store(&mut doc.editor.project);
    // not an undoable circuit edit (like the network conditions); dirty → autosave
    doc.editor.generation += 1;
    doc.touched();
}

/// The virtual phone: scan list, the connected device's GATT table, the BLE serial terminal.
fn phone_ui(
    ui: &mut Ui,
    p: &Palette,
    phone: &crate::bench::PhoneModel,
    input: &mut String,
) -> Vec<RadioCommand> {
    use crate::bench::PhoneAction;
    let mut cmds = Vec::new();
    ui.horizontal(|ui| {
        Icon::Phone.show(ui, 14.0, p.text_muted);
        let title = match &phone.connected_name {
            Some(n) => format!("Virtual phone · connected to {n}"),
            None => "Virtual phone · scanning".to_string(),
        };
        ui.label(
            egui::RichText::new(title)
                .font(font::ui(12.0))
                .color(p.text_muted),
        );
    });
    if phone.connected.is_none() {
        if phone.scan.is_empty() {
            ui.label(
                egui::RichText::new("No advertisers in range.")
                    .font(font::ui(12.0))
                    .color(p.text_faint),
            );
        }
        for d in &phone.scan {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} · {} dBm", d.name, d.rssi))
                        .font(font::mono(12.0))
                        .color(p.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if Btn::mini(&format!("Connect {}", d.name)).show(ui).clicked() {
                        cmds.push(RadioCommand::Phone(PhoneAction::Connect(d.id)));
                    }
                });
            });
        }
        return cmds;
    }
    ui.horizontal(|ui| {
        if Btn::mini("Disconnect").show(ui).clicked() {
            cmds.push(RadioCommand::Phone(PhoneAction::Disconnect));
        }
        if Btn::mini("Open terminal").show(ui).clicked() {
            cmds.push(RadioCommand::Phone(PhoneAction::TerminalOpen));
        }
    });
    for c in &phone.chars {
        let short = c.uuid.chars().take(8).collect::<String>();
        ui.label(
            egui::RichText::new(format!("{short} · {}", c.props.join(" / ")))
                .font(font::mono(11.0))
                .color(p.text_muted),
        );
        ui.horizontal(|ui| {
            let v = if c.value.is_empty() {
                "—"
            } else {
                c.value.as_str()
            };
            ui.add(
                egui::Label::new(
                    egui::RichText::new(v)
                        .font(font::mono(12.0))
                        .color(p.accent_soft),
                )
                .truncate(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if c.props.iter().any(|x| x.contains("notify"))
                    && Btn::mini(if c.notifying { "Stop" } else { "Notify" })
                        .show(ui)
                        .clicked()
                {
                    cmds.push(RadioCommand::Phone(PhoneAction::Subscribe(
                        c.handle,
                        !c.notifying,
                    )));
                }
                if c.props.iter().any(|x| x.contains("read"))
                    && Btn::mini("Read").show(ui).clicked()
                {
                    cmds.push(RadioCommand::Phone(PhoneAction::Read(c.handle)));
                }
            });
        });
    }
    if !phone.terminal.is_empty() {
        let tail: String = phone
            .terminal
            .chars()
            .rev()
            .take(400)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        ui.label(
            egui::RichText::new(tail)
                .font(font::mono(11.5))
                .color(p.text_dim),
        );
    }
    ui.horizontal(|ui| {
        let te = ui.add(
            egui::TextEdit::singleline(input)
                .hint_text("Send over BLE…")
                .desired_width(ui.available_width() - 70.0)
                .font(font::mono(12.0)),
        );
        te.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Phone terminal"));
        let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (Btn::mini("Send").show(ui).clicked() || enter) && !input.is_empty() {
            let mut b = std::mem::take(input).into_bytes();
            b.extend_from_slice(b"\r\n");
            cmds.push(RadioCommand::Phone(PhoneAction::TerminalSend(b)));
        }
    });
    cmds
}

fn parse_addr(s: &str) -> Option<(String, u16)> {
    let (h, port) = s.trim().rsplit_once(':')?;
    Some((h.to_string(), port.parse().ok()?))
}

fn packet_log(ui: &mut Ui, p: &Palette, model: Option<&RadioPanelModel>) {
    let rows = model.map(|m| m.log.as_slice()).unwrap_or(&[]);
    let w = ui.available_width();
    let font = font::mono(11.5);
    let (outer_h, list): (f32, Vec<_>) = {
        let start = rows.len().saturating_sub(40);
        (0.0, rows[start..].iter().collect())
    };
    let _ = outer_h;
    egui::Frame::new()
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .show(ui, |ui| {
            ui.set_width(w - 2.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            let header = |ui: &mut Ui| {
                let (r, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
                ui.painter().rect_filled(
                    r,
                    CornerRadius {
                        nw: 7,
                        ne: 7,
                        sw: 0,
                        se: 0,
                    },
                    p.bg_raised,
                );
                for (x, t) in [(10.0, "time"), (82.0, "dir"), (116.0, "frame")] {
                    ui.painter().text(
                        pos2(r.left() + x, r.center().y),
                        egui::Align2::LEFT_CENTER,
                        t,
                        font.clone(),
                        p.text_muted,
                    );
                }
            };
            header(ui);
            if list.is_empty() {
                let (r, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
                ui.painter().text(
                    pos2(r.left() + 10.0, r.center().y),
                    egui::Align2::LEFT_CENTER,
                    "no frames yet",
                    font.clone(),
                    p.text_faint,
                );
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("packet-log")
                .max_height(220.0)
                .stick_to_bottom(true)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let fw = ui.available_width() - 126.0;
                    for row in list {
                        let color = if row.dropped { p.accent } else { p.text };
                        let g = ui
                            .painter()
                            .layout(row.text.clone(), font.clone(), color, fw);
                        let h = g.size().y + 10.0;
                        let (r, resp) =
                            ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
                        let label = format!("{} {} {}", row.time, row.dir, row.text);
                        resp.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Label, true, &label)
                        });
                        ui.painter().line_segment(
                            [r.left_top(), r.right_top()],
                            Stroke::new(1.0, p.border_faint),
                        );
                        let tcol = if row.dropped { p.accent } else { p.text_faint };
                        let dcol = match row.dir.as_str() {
                            "tx" => p.accent_soft,
                            "rx" => p.info,
                            _ => p.accent,
                        };
                        ui.painter().text(
                            pos2(r.left() + 10.0, r.top() + 5.0),
                            egui::Align2::LEFT_TOP,
                            &row.time,
                            font.clone(),
                            tcol,
                        );
                        ui.painter().text(
                            pos2(r.left() + 82.0, r.top() + 5.0),
                            egui::Align2::LEFT_TOP,
                            &row.dir,
                            font.clone(),
                            dcol,
                        );
                        ui.painter()
                            .galley(pos2(r.left() + 116.0, r.top() + 5.0), g, color);
                    }
                });
        });
}

/// Dashed empty-state card; returns true if its button was clicked.
fn empty_card(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    title: &str,
    text: &str,
    button: Option<&str>,
) -> bool {
    let w = ui.available_width();
    let mut clicked = false;
    let resp = ui.allocate_ui(vec2(w, 0.0), |ui| {
        egui::Frame::new()
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(w - 28.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    icon.show(ui, 18.0, p.text_muted);
                    ui.label(
                        egui::RichText::new(title)
                            .font(font::heading(14.0))
                            .color(p.text),
                    );
                });
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(text)
                            .font(font::ui(12.0))
                            .color(p.text_muted),
                    )
                    .wrap(),
                );
                if let Some(b) = button
                    && Btn::ghost(b).show(ui).clicked()
                {
                    clicked = true;
                }
            });
    });
    let r: Rect = resp.response.rect;
    widgets::controls::dashed_rect(ui.painter(), r, 8.0, p.border_strong);
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use bb_circuit::project::Mode;

    fn doc_with(board: &str) -> CircuitDoc {
        let mut p = bb_circuit::Project::new("hat", Mode::Playground);
        p.parts
            .push(PartInstance::new("u1", "U1", board).at(0.0, 4.0));
        CircuitDoc::new(p, std::path::Path::new("/nonexistent"), true)
    }

    fn wire_to<'a>(doc: &'a CircuitDoc, hat: &str, pin: &str) -> Option<&'a Endpoint> {
        doc.project().wires.iter().find_map(|w| {
            if w.a == Endpoint::pin(hat, pin) {
                Some(&w.b)
            } else if w.b == Endpoint::pin(hat, pin) {
                Some(&w.a)
            } else {
                None
            }
        })
    }

    #[test]
    fn ble_hat_on_an_uno_is_one_undoable_edit() {
        let mut doc = doc_with("uno");
        let r = add_ble_hat(&mut doc).unwrap();
        assert_eq!(r.reference, "U2");
        assert_eq!(r.board, "U1");
        assert_eq!(r.wiring, "VCC→5V, GND→GND, TXD→D10, RXD→D11");
        let hat = doc
            .project()
            .parts
            .iter()
            .find(|p| p.part == "hm10")
            .unwrap()
            .clone();
        assert_eq!(
            wire_to(&doc, &hat.id, "VCC"),
            Some(&Endpoint::pin("u1", "5V"))
        );
        assert_eq!(
            wire_to(&doc, &hat.id, "GND"),
            Some(&Endpoint::pin("u1", "GND"))
        );
        assert_eq!(
            wire_to(&doc, &hat.id, "TXD"),
            Some(&Endpoint::pin("u1", "D10"))
        );
        assert_eq!(
            wire_to(&doc, &hat.id, "RXD"),
            Some(&Endpoint::pin("u1", "D11"))
        );
        // next to the board, not on top of it
        let b = hat.bench.as_ref().unwrap();
        assert!(b.at.x > 0.0);
        // ERC stays quiet about the hat
        let checks = doc.checks();
        assert!(
            !checks.erc.iter().any(|i| i.parts.contains(&hat.id)),
            "{:?}",
            checks.erc
        );
        // one undo removes module and wires
        assert_eq!(doc.editor.undo_label(), Some("Add BLE hat"));
        assert!(doc.undo());
        assert!(doc.project().parts.iter().all(|p| p.part != "hm10"));
        assert!(doc.project().wires.is_empty());
    }

    #[test]
    fn ble_hat_on_host_boards_uses_3v3_and_their_uart() {
        let mut esp = doc_with("esp32_devkit");
        assert_eq!(
            add_ble_hat(&mut esp).unwrap().wiring,
            "VCC→3V3, GND→GND, TXD→RX2, RXD→TX2"
        );
        let mut pico = doc_with("pico");
        assert_eq!(
            add_ble_hat(&mut pico).unwrap().wiring,
            "VCC→3V3, GND→GND, TXD→GP1, RXD→GP0"
        );
        let mut empty = CircuitDoc::new(
            bb_circuit::Project::new("e", Mode::Playground),
            std::path::Path::new("/nonexistent"),
            true,
        );
        assert!(add_ble_hat(&mut empty).is_err());
    }

    #[test]
    fn bench_position_is_stored_with_the_radio_settings() {
        let mut doc = doc_with("uno");
        let g = doc.generation();
        store_bench_pos(&mut doc, (7.5, -1.0));
        assert!(doc.generation() > g, "dirty → autosave");
        let r = bb_sim::radio::ProjectRadios::from_project(doc.project());
        assert_eq!(r.bench_pos, bb_radio::geo::Pos::new(7.5, -1.0));
        assert_eq!(r.settings.walls.len(), 1, "the rest of the defaults stay");
        let g = doc.generation();
        store_bench_pos(&mut doc, (7.5, -1.0));
        assert_eq!(doc.generation(), g, "no change, no save");
    }

    #[test]
    fn plan_view_fits_and_round_trips() {
        let pl = PlacementView {
            bench: (3.2, 0.0),
            aps: vec![crate::bench::ApPlacement {
                ssid: "SIM-AP".into(),
                channel: 6,
                x: 0.0,
                y: 0.0,
                distance_m: 3.2,
                walls: 1,
                rssi_dbm: -54.2,
            }],
            walls: vec![],
        };
        let r = Rect::from_min_size(pos2(100.0, 50.0), vec2(300.0, 150.0));
        let v = PlanView::fit(&pl, pl.bench, r.size());
        for pt in [(0.0, 0.0), (3.2, 0.0)] {
            let s = v.to_screen(r, pt);
            assert!(r.contains(s), "{pt:?} → {s:?}");
            let back = v.to_metres(r, s);
            assert!((back.0 - pt.0).abs() < 1e-4 && (back.1 - pt.1).abs() < 1e-4);
        }
        assert_eq!(snap_pos((3.24, -0.06)), (3.2, -0.1));
        assert_eq!(link_summary(&pl), "SIM-AP · 3.2 m · 1 wall · -54 dBm");
    }
}
