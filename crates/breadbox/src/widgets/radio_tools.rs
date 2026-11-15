//! The virtual phone (`RF-6`) and the MQTT explorer (`RF-7`) as floating tool windows over
//! any screen, driven by the open bench's `Telemetry.radio` and sending
//! `bb_sim::radio::RadioCommand`s.
//!
//! * **Phone** (a BLE central): scan list with RSSI bars, connect / disconnect, the GATT
//!   table (services → characteristics with their properties and value as text + hex), Read,
//!   Notify on/off, Write, an activity log, and the Bluetooth serial terminal (HM-10 FFE1 /
//!   Nordic UART / HC-05 SPP).
//! * **MQTT explorer**: subscriptions with `+` / `#` wildcards, the live message list from the
//!   built-in broker, and a publish form (topic, payload, QoS, retain).
//!
//! Open them with [`open`] (the Radios panel and the Workbench rail have buttons); the app
//! draws them every frame with [`show`].

use crate::app::AppCtx;
use crate::theme::{font, pal};
use crate::widgets::{self, Btn, Icon};
use bb_sim::radio::{PhoneCommand, RadioCommand, RadioTelemetry};
use eframe::egui::{self, Color32, CornerRadius, Id, Rect, Sense, Stroke, Ui, pos2, vec2};
use std::collections::HashMap;

/// Tool window state (kept in egui memory).
#[derive(Debug, Clone, Default)]
pub struct RadioTools {
    pub phone: bool,
    pub mqtt: bool,
    pub phone_tab: PhoneTab,
    pub write: HashMap<u16, String>,
    pub terminal: String,
    pub sub: String,
    pub pub_topic: String,
    pub pub_payload: String,
    pub pub_qos: u8,
    pub pub_retain: bool,
    pub filter: String,
    /// Topics the explorer subscribed to (the bench may restart).
    pub subscribed: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PhoneTab {
    #[default]
    Gatt,
    Terminal,
    Log,
}

fn key() -> Id {
    Id::unique("radio-tools")
}

fn load(ctx: &egui::Context) -> RadioTools {
    ctx.data(|d| d.get_temp::<RadioTools>(key()))
        .unwrap_or_default()
}

fn store(ctx: &egui::Context, t: RadioTools) {
    ctx.data_mut(|d| d.insert_temp(key(), t));
}

/// Opens (or focuses) the phone and/or the MQTT explorer.
pub fn open(ctx: &egui::Context, phone: bool, mqtt: bool) {
    let mut t = load(ctx);
    t.phone |= phone;
    t.mqtt |= mqtt;
    store(ctx, t);
    ctx.request_repaint();
}

/// Whether a tool window is open.
pub fn is_open(ctx: &egui::Context) -> (bool, bool) {
    let t = load(ctx);
    (t.phone, t.mqtt)
}

/// Two buttons ("Virtual phone", "MQTT explorer") for panels.
pub fn buttons(ui: &mut Ui) {
    ui.horizontal(|ui| {
        if Btn::ghost("Virtual phone")
            .icon(Icon::Phone)
            .show(ui)
            .clicked()
        {
            open(ui.ctx(), true, false);
        }
        if Btn::ghost("MQTT explorer")
            .icon(Icon::Radio)
            .show(ui)
            .clicked()
        {
            open(ui.ctx(), false, true);
        }
    });
}

/// Rail buttons for the Workbench's right rail.
pub fn rail_buttons(ui: &mut Ui) {
    let (phone, mqtt) = is_open(ui.ctx());
    if widgets::rail_button(ui, Icon::Phone, "Virtual phone", phone).clicked() {
        let mut t = load(ui.ctx());
        t.phone = !t.phone;
        store(ui.ctx(), t);
    }
    if widgets::rail_button(ui, Icon::Radio, "MQTT explorer", mqtt).clicked() {
        let mut t = load(ui.ctx());
        t.mqtt = !t.mqtt;
        store(ui.ctx(), t);
    }
}

fn send(app: &mut AppCtx, cmd: RadioCommand) {
    if let Some(sim) = app.sim() {
        sim.send(bb_sim::Command::Radio(cmd));
    }
}

/// Draws the open tool windows (called by the app every frame).
pub fn show(ctx: &egui::Context, app: &mut AppCtx) {
    let mut t = load(ctx);
    if !t.phone && !t.mqtt {
        return;
    }
    let radio: Option<RadioTelemetry> = if app.project.is_some() {
        app.bench
            .sim()
            .and_then(|s| s.telemetry())
            .and_then(|tel| tel.radio.clone())
    } else {
        None
    };
    let mut cmds: Vec<RadioCommand> = Vec::new();
    if t.phone {
        let mut open = true;
        egui::Window::new("Virtual phone")
            .id(Id::unique("phone-window"))
            .open(&mut open)
            .default_pos(ctx.content_rect().right_top() + vec2(-720.0, 80.0))
            .resizable(false)
            .collapsible(false)
            .title_bar(false)
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| phone(ui, &mut t, radio.as_ref(), &mut cmds));
        if !open {
            t.phone = false;
        }
    }
    if t.mqtt {
        let mut open = true;
        egui::Window::new("MQTT explorer")
            .id(Id::unique("mqtt-window"))
            .open(&mut open)
            .default_pos(ctx.content_rect().right_top() + vec2(-400.0, 80.0))
            .default_size(vec2(380.0, 520.0))
            .collapsible(false)
            .show(ctx, |ui| mqtt(ui, &mut t, radio.as_ref(), &mut cmds));
        if !open {
            t.mqtt = false;
        }
    }
    // re-subscribe after a bench restart
    if let Some(r) = &radio {
        for s in &t.subscribed {
            if !r.explorer_subscriptions.contains(s) {
                cmds.push(RadioCommand::ExplorerSubscribe(s.clone()));
            }
        }
    }
    for c in cmds {
        send(app, c);
    }
    store(ctx, t);
    if app.bench.running {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

fn rssi_bars(ui: &mut Ui, rssi: i32) {
    let p = pal(ui);
    let (r, _) = ui.allocate_exact_size(vec2(18.0, 12.0), Sense::hover());
    let level = match rssi {
        x if x >= -55 => 4,
        x if x >= -67 => 3,
        x if x >= -80 => 2,
        _ => 1,
    };
    for i in 0..4 {
        let h = 3.0 + i as f32 * 3.0;
        let b = Rect::from_min_size(
            pos2(r.left() + i as f32 * 4.5, r.bottom() - h),
            vec2(3.0, h),
        );
        ui.painter()
            .rect_filled(b, 1.0, if i < level { p.ok } else { p.border_strong });
    }
}

fn hex_bytes(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The phone: a handset-shaped frame with the scan list, GATT table, terminal and log.
fn phone(
    ui: &mut Ui,
    t: &mut RadioTools,
    radio: Option<&RadioTelemetry>,
    cmds: &mut Vec<RadioCommand>,
) {
    let p = pal(ui);
    let body = egui::Frame::new()
        .fill(Color32::from_rgb(0x1A, 0x1A, 0x1C))
        .stroke(Stroke::new(2.0, Color32::from_rgb(0x3A, 0x3A, 0x3E)))
        .corner_radius(CornerRadius::same(34))
        .inner_margin(egui::Margin::same(12))
        .shadow(ui.ctx().global_style().visuals.window_shadow);
    body.show(ui, |ui| {
        let screen = egui::Frame::new()
            .fill(Color32::from_rgb(0x0B, 0x0B, 0x0D))
            .corner_radius(CornerRadius::same(24))
            .inner_margin(egui::Margin::symmetric(14, 12));
        screen.show(ui, |ui| {
            ui.set_width(300.0);
            ui.set_min_height(560.0);
            ui.spacing_mut().item_spacing.y = 8.0;
            // status bar + notch
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("9:41").font(font::semibold(12.0)).color(p.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if Btn::plain("Close phone").icon_only(Icon::Close).height(22.0).show(ui).clicked() {
                        t.phone = false;
                    }
                    Icon::Bluetooth.show(ui, 13.0, p.info);
                });
            });
            let (title, sub) = match radio.map(|r| &r.phone) {
                Some(ph) => match &ph.connected_name {
                    Some(n) => (format!("Connected to {n}"), "GATT · notifications · serial"),
                    None => ("Nearby devices".to_string(), "Bluetooth LE + classic scan"),
                },
                None => ("Virtual phone".to_string(), "no bench running"),
            };
            let tr = ui.label(egui::RichText::new(&title).font(font::heading(18.0)).color(p.text));
            let tt = format!("Phone: {title}");
            tr.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Heading, true, &tt));
            ui.label(egui::RichText::new(sub).font(font::ui(11.5)).color(p.text_muted));
            let Some(r) = radio else {
                ui.add_space(20.0);
                ui.label(egui::RichText::new("Open a project and press Run: radios on the bench (HM-10, HC-05, ESP32 BLE) show up here.").font(font::ui(12.5)).color(p.text_faint));
                return;
            };
            if !r.has_ble {
                ui.label(egui::RichText::new("No BLE radio on this bench. Add an HM-10 / HC-05 module or an ESP32 and run the sketch.").font(font::ui(12.5)).color(p.text_faint));
            }
            let ph = &r.phone;
            match ph.connected {
                None => {
                    if ph.scan.is_empty() {
                        ui.horizontal(|ui| {
                            widgets::spinner(ui, 12.0);
                            ui.label(egui::RichText::new("Scanning…").font(font::ui(12.5)).color(p.text_muted));
                        });
                    }
                    for d in &ph.scan {
                        egui::Frame::new()
                            .fill(p.bg_raised)
                            .corner_radius(CornerRadius::same(12))
                            .inner_margin(egui::Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        ui.label(egui::RichText::new(&d.name).font(font::medium(13.5)).color(p.text));
                                        ui.label(egui::RichText::new(format!("{} · {} · {}", d.kind, d.addr, d.services.join(", "))).font(font::mono(10.5)).color(p.text_muted));
                                    });
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        let b = Btn::amber("Connect").height(26.0).show(ui);
                                        let l = format!("Phone: connect {}", d.name);
                                        b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &l));
                                        if b.clicked() {
                                            cmds.push(RadioCommand::Phone(PhoneCommand::Connect { id: d.id, pin: None }));
                                        }
                                        let rl = ui.label(egui::RichText::new(format!("{} dBm", d.rssi)).font(font::mono(11.0)).color(p.text_muted));
                                        let rr = format!("{} RSSI {} dBm", d.name, d.rssi);
                                        rl.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &rr));
                                        rssi_bars(ui, d.rssi);
                                    });
                                });
                            });
                    }
                }
                Some(_) => {
                    ui.horizontal(|ui| {
                        if Btn::ghost("Disconnect").height(26.0).show(ui).clicked() {
                            cmds.push(RadioCommand::Phone(PhoneCommand::Disconnect));
                        }
                    });
                    ui.horizontal(|ui| {
                        for (tab, name) in [(PhoneTab::Gatt, "Services"), (PhoneTab::Terminal, "Terminal"), (PhoneTab::Log, "Log")] {
                            if widgets::tab(ui, name, t.phone_tab == tab).clicked() {
                                t.phone_tab = tab;
                                if tab == PhoneTab::Terminal {
                                    cmds.push(RadioCommand::Phone(PhoneCommand::TerminalOpen));
                                }
                            }
                        }
                    });
                    match t.phone_tab {
                        PhoneTab::Gatt => gatt(ui, t, r, cmds),
                        PhoneTab::Terminal => terminal(ui, t, r, cmds),
                        PhoneTab::Log => log(ui, r),
                    }
                }
            }
            if ph.connected.is_none() && !ph.log.is_empty() {
                widgets::section_header(ui, "Activity");
                log(ui, r);
            }
        });
    });
}

fn gatt(ui: &mut Ui, t: &mut RadioTools, r: &RadioTelemetry, cmds: &mut Vec<RadioCommand>) {
    let p = pal(ui);
    egui::ScrollArea::vertical()
        .id_salt("phone-gatt")
        .max_height(400.0)
        .show(ui, |ui| {
            if r.phone.services.is_empty() {
                ui.horizontal(|ui| {
                    widgets::spinner(ui, 12.0);
                    ui.label(
                        egui::RichText::new("Discovering services…")
                            .font(font::ui(12.0))
                            .color(p.text_muted),
                    );
                });
            }
            for s in &r.phone.services {
                ui.label(
                    egui::RichText::new(format!("Service {}", s.uuid))
                        .font(font::mono_medium(11.5))
                        .color(p.info),
                );
                for c in &s.characteristics {
                    egui::Frame::new()
                        .fill(p.bg_raised)
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 4.0;
                            let name = c.description.clone().unwrap_or_else(|| c.uuid.clone());
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&name)
                                        .font(font::medium(12.5))
                                        .color(p.text),
                                );
                                for pr in &c.props {
                                    widgets::tag(ui, pr, widgets::Tone::Neutral);
                                }
                            });
                            let value = if c.value.is_empty() {
                                "—".to_string()
                            } else {
                                format!("“{}” · {}", c.value_text.trim_end(), hex_bytes(&c.value))
                            };
                            let vl = ui.label(
                                egui::RichText::new(&value)
                                    .font(font::mono(11.0))
                                    .color(p.text_dim),
                            );
                            let vv = format!("{} value {}", c.uuid, c.value_text.trim_end());
                            vl.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Label, true, &vv)
                            });
                            ui.horizontal(|ui| {
                                if c.props.iter().any(|x| x == "read") {
                                    let b = Btn::mini("Read").show(ui);
                                    let l = format!("Read {}", c.uuid);
                                    b.widget_info(|| {
                                        egui::WidgetInfo::labeled(egui::Role::Button, true, &l)
                                    });
                                    if b.clicked() {
                                        cmds.push(RadioCommand::Phone(PhoneCommand::Read {
                                            handle: c.handle,
                                        }));
                                    }
                                }
                                if c.props.iter().any(|x| x == "notify" || x == "indicate") {
                                    let b =
                                        Btn::mini(if c.notifying { "Notify on" } else { "Notify" })
                                            .selected(c.notifying)
                                            .show(ui);
                                    let l = format!("Notify {}", c.uuid);
                                    b.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::Role::Button,
                                            true,
                                            c.notifying,
                                            &l,
                                        )
                                    });
                                    if b.clicked() {
                                        cmds.push(RadioCommand::Phone(PhoneCommand::Subscribe {
                                            handle: c.handle,
                                            on: !c.notifying,
                                        }));
                                    }
                                }
                            });
                            if c.props.iter().any(|x| x.starts_with("write")) {
                                ui.horizontal(|ui| {
                                    let text = t.write.entry(c.handle).or_default();
                                    let te = ui.add(
                                        egui::TextEdit::singleline(text)
                                            .desired_width(150.0)
                                            .font(font::mono(11.5))
                                            .hint_text("text to write"),
                                    );
                                    let l = format!("Write value {}", c.uuid);
                                    te.widget_info(|| {
                                        egui::WidgetInfo::labeled(egui::Role::TextInput, true, &l)
                                    });
                                    let b = Btn::mini("Write").show(ui);
                                    let l2 = format!("Write {}", c.uuid);
                                    b.widget_info(|| {
                                        egui::WidgetInfo::labeled(egui::Role::Button, true, &l2)
                                    });
                                    if b.clicked() && !text.is_empty() {
                                        cmds.push(RadioCommand::Phone(PhoneCommand::Write {
                                            handle: c.handle,
                                            data: text.as_bytes().to_vec(),
                                        }));
                                        text.clear();
                                    }
                                });
                            }
                        });
                }
            }
            if !r.phone.log.is_empty() {
                widgets::section_header(ui, "Activity");
                log(ui, r);
            }
        });
}

fn terminal(ui: &mut Ui, t: &mut RadioTools, r: &RadioTelemetry, cmds: &mut Vec<RadioCommand>) {
    let p = pal(ui);
    egui::Frame::new()
        .fill(Color32::from_rgb(0x06, 0x06, 0x06))
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .id_salt("phone-term")
                .max_height(330.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let text = if r.phone.terminal.is_empty() {
                        "(nothing received yet)"
                    } else {
                        r.phone.terminal.as_str()
                    };
                    let l = ui.label(egui::RichText::new(text).font(font::mono(11.5)).color(p.ok));
                    let tt = format!("Phone terminal: {}", r.phone.terminal.trim_end());
                    l.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &tt));
                });
        });
    ui.horizontal(|ui| {
        let te = ui.add(
            egui::TextEdit::singleline(&mut t.terminal)
                .desired_width(200.0)
                .font(font::mono(12.0))
                .hint_text("message"),
        );
        te.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Phone message"));
        let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (Btn::amber("Send").height(26.0).show(ui).clicked() || enter) && !t.terminal.is_empty() {
            let mut b = t.terminal.as_bytes().to_vec();
            b.push(b'\n');
            cmds.push(RadioCommand::Phone(PhoneCommand::TerminalSend(b)));
            t.terminal.clear();
        }
    });
}

fn log(ui: &mut Ui, r: &RadioTelemetry) {
    let p = pal(ui);
    for row in r.phone.log.iter().rev().take(14) {
        let col = match row.kind.as_str() {
            "notify" => p.info,
            "error" => p.danger_soft,
            "connected" | "disconnected" => p.accent_soft,
            _ => p.text_dim,
        };
        let text = row.text.trim_end().replace('\n', " ⏎ ");
        let s = format!("{:>8.3}  {:<9} {}", row.time.as_secs_f64(), row.kind, text);
        let l = ui.label(egui::RichText::new(&s).font(font::mono(10.5)).color(col));
        let tt = format!("Phone log {} {}", row.kind, text);
        l.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &tt));
    }
}

/// The MQTT explorer.
fn mqtt(
    ui: &mut Ui,
    t: &mut RadioTools,
    radio: Option<&RadioTelemetry>,
    cmds: &mut Vec<RadioCommand>,
) {
    let p = pal(ui);
    ui.spacing_mut().item_spacing.y = 10.0;
    ui.label(
        egui::RichText::new("Built-in broker · mqtt://10.0.0.1:1883")
            .font(font::mono(11.5))
            .color(p.text_muted),
    );
    let Some(r) = radio else {
        ui.label(
            egui::RichText::new(
                "Open a project and press Run: the bench's MQTT traffic shows up here.",
            )
            .font(font::ui(12.5))
            .color(p.text_faint),
        );
        return;
    };
    widgets::section_header(ui, "Subscriptions");
    ui.horizontal(|ui| {
        let te = ui.add(
            egui::TextEdit::singleline(&mut t.sub)
                .desired_width(220.0)
                .font(font::mono(12.0))
                .hint_text("garden/# or +/pot1"),
        );
        te.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Subscribe topic")
        });
        let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (Btn::ghost("Subscribe").height(26.0).show(ui).clicked() || enter)
            && !t.sub.trim().is_empty()
        {
            let s = t.sub.trim().to_string();
            cmds.push(RadioCommand::ExplorerSubscribe(s.clone()));
            if !t.subscribed.contains(&s) {
                t.subscribed.push(s);
            }
            t.sub.clear();
        }
    });
    let mut subs = r.explorer_subscriptions.clone();
    for s in &t.subscribed {
        if !subs.contains(s) {
            subs.push(s.clone());
        }
    }
    ui.horizontal_wrapped(|ui| {
        for s in &subs {
            let c = widgets::filter_chip(ui, &format!("{s}  ×"), true);
            let l = format!("Unsubscribe {s}");
            c.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &l));
            if c.clicked() {
                cmds.push(RadioCommand::ExplorerUnsubscribe(s.clone()));
                t.subscribed.retain(|x| x != s);
            }
        }
        if subs.is_empty() {
            ui.label(
                egui::RichText::new("No subscriptions: add `#` to see everything.")
                    .font(font::ui(12.0))
                    .color(p.text_faint),
            );
        }
    });
    widgets::section_header(ui, "Messages");
    ui.horizontal(|ui| {
        let te = ui.add(
            egui::TextEdit::singleline(&mut t.filter)
                .desired_width(220.0)
                .font(font::mono(11.5))
                .hint_text("filter"),
        );
        te.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::SearchInput, true, "Filter messages")
        });
    });
    egui::Frame::new()
        .fill(p.bg_canvas)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .id_salt("mqtt-rows")
                .max_height(240.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let rows: Vec<_> = r
                        .mqtt
                        .iter()
                        .filter(|m| {
                            t.filter.is_empty()
                                || m.topic.contains(&t.filter)
                                || m.payload.contains(&t.filter)
                        })
                        .collect();
                    if rows.is_empty() {
                        ui.label(
                            egui::RichText::new("Waiting for messages…")
                                .font(font::ui(12.0))
                                .color(p.text_faint),
                        );
                    }
                    for m in rows.iter().rev().take(80).rev() {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{:>7.3}", m.time.as_secs_f64()))
                                    .font(font::mono(10.5))
                                    .color(p.text_faint),
                            );
                            let tl = ui.label(
                                egui::RichText::new(&m.topic)
                                    .font(font::mono_medium(11.0))
                                    .color(p.info),
                            );
                            let tt = format!("MQTT {} {}", m.topic, m.payload);
                            tl.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Label, true, &tt)
                            });
                            ui.label(
                                egui::RichText::new(&m.payload)
                                    .font(font::mono(11.0))
                                    .color(p.text_dim),
                            );
                            let mut meta = format!("{} B · QoS {}", m.payload.len(), m.qos);
                            if m.retain {
                                meta.push_str(" · retained");
                            }
                            ui.label(
                                egui::RichText::new(meta)
                                    .font(font::mono(10.0))
                                    .color(p.text_faint),
                            );
                        });
                    }
                });
        });
    widgets::section_header(ui, "Publish");
    ui.horizontal(|ui| {
        let te = ui.add(
            egui::TextEdit::singleline(&mut t.pub_topic)
                .desired_width(150.0)
                .font(font::mono(12.0))
                .hint_text("topic"),
        );
        te.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Publish topic"));
        let te = ui.add(
            egui::TextEdit::singleline(&mut t.pub_payload)
                .desired_width(150.0)
                .font(font::mono(12.0))
                .hint_text("payload"),
        );
        te.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Publish payload")
        });
    });
    ui.horizontal(|ui| {
        widgets::mini_segmented(
            ui,
            "QoS",
            &mut t.pub_qos,
            &[
                widgets::Seg::new(0u8, "QoS 0"),
                widgets::Seg::new(1u8, "QoS 1"),
            ],
        );
        widgets::checkbox(ui, &mut t.pub_retain, "Retain");
        if Btn::amber("Publish").height(26.0).show(ui).clicked() && !t.pub_topic.trim().is_empty() {
            cmds.push(RadioCommand::ExplorerPublish {
                topic: t.pub_topic.trim().to_string(),
                payload: t.pub_payload.as_bytes().to_vec(),
                qos: t.pub_qos,
                retain: t.pub_retain,
            });
        }
    });
}
