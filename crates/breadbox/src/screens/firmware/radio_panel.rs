//! The Radios panel (`RF-7`, design screen 2, right): radio cards, packet log, network
//! conditions, Bluetooth LE card. A pure view over [`RadioPanelModel`] that returns
//! [`RadioCommand`]s — the Workbench reuses it unchanged.

use crate::bench::{NetConditionsView, RadioCommand, RadioKind, RadioPanelModel, StatusTone};
use crate::theme::{Palette, font, pal};
use crate::widgets::{self, Btn, Icon, StopSlider, Tone, checkbox, section_header_row};
use eframe::egui::{self, CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};

/// Local UI state (slider values while dragging, bridge address field).
#[derive(Debug, Clone)]
pub struct RadioPanelState {
    pub latency: f64,
    pub loss: f64,
    pub drop_ap: bool,
    pub bridge: bool,
    pub bridge_addr: String,
    /// Virtual-phone terminal input.
    pub phone_input: String,
    synced: bool,
}

impl Default for RadioPanelState {
    fn default() -> Self {
        RadioPanelState {
            phone_input: String::new(),
            latency: 40.0,
            loss: 2.0,
            drop_ap: false,
            bridge: false,
            bridge_addr: "localhost:1883".into(),
            synced: false,
        }
    }
}

impl RadioPanelState {
    fn conditions(&self) -> NetConditionsView {
        NetConditionsView {
            latency_ms: self.latency,
            jitter_ms: (self.latency / 4.0).round(),
            loss_pct: self.loss,
            drop_ap_at_s: self.drop_ap.then_some(60.0),
            bridge: None,
            bridge_status: None,
        }
    }
}

/// Draws the panel; returns commands for the bench.
pub fn radio_panel(
    ui: &mut Ui,
    model: Option<&RadioPanelModel>,
    st: &mut RadioPanelState,
) -> Vec<RadioCommand> {
    let p = pal(ui);
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

    // packet log
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(
            egui::RichText::new("Packet log")
                .font(font::semibold(12.0))
                .color(p.text_muted),
        );
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
        let lat_fmt = |v: f64| format!("{:.0} ms ± {:.0}", v, (v / 4.0).round());
        let loss_fmt = |v: f64| format!("{v:.0} %");
        let a = StopSlider::new("Latency", 0.0..=500.0, &lat_fmt)
            .step(1.0)
            .show(ui, &mut st.latency);
        let b = StopSlider::new("Packet loss", 0.0..=50.0, &loss_fmt)
            .step(1.0)
            .show(ui, &mut st.loss);
        let c1 = checkbox(
            ui,
            &mut st.drop_ap,
            "Drop the AP at t = 60 s (test reconnect)",
        );
        if a.changed() || b.changed() || c1.changed() {
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
    cmds
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
