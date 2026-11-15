//! The telemetry snapshot the UI renders (SIM-5): time and run state, part visuals, probes,
//! scope frames, meters, world values, supply rails / dissipation / USB budget, MCU status,
//! ERC/DRC.

use crate::bench::Bench;
use crate::instruments::{MeterReading, ProbeReading, ScopeFrame};
use bb_circuit::erc::{self, ErcItem};
use bb_circuit::netlist::NetKind;
use bb_core::{Health, Look, SimTime, VisualState};
use bb_world::WorldSnapshot;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum RunStatus {
    Running,
    #[default]
    Paused,
    Stopped,
    Error(String),
}

impl RunStatus {
    /// Header label (`running` / `paused` / `stopped` / `error`).
    pub fn label(&self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Paused => "paused",
            RunStatus::Stopped => "stopped",
            RunStatus::Error(_) => "error",
        }
    }
}

/// One part's live state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartTelemetry {
    pub part: String,
    pub reference: String,
    pub visual: VisualState,
    pub readings: Vec<(String, f64)>,
}

/// One MCU's status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McuTelemetry {
    pub part: String,
    pub reference: String,
    pub label: String,
    /// `cycle-accurate AVR core` / `API-level`.
    pub engine: String,
    pub vcc: f64,
    pub in_reset: bool,
    pub reset_count: u64,
    pub last_reset: String,
    pub paused: Option<String>,
    /// Serial-monitor header: `U1 · SoftwareSerial + USB · 115200 baud · NL`.
    pub serial_label: String,
    pub firmware: Option<String>,
    pub pc: Option<u32>,
}

/// A supply rail in "Probes & power".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rail {
    pub name: String,
    pub source: String,
    pub voltage: f64,
    pub current: f64,
}

/// "Probes & power" (WB-11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PowerReport {
    pub rails: Vec<Rail>,
    /// Power delivered by all sources (W).
    pub total_power: f64,
    /// (designator, watts) for parts dissipating more than 1 mW.
    pub dissipation: Vec<(String, f64)>,
    /// Current drawn from USB (A) against the 500 mA budget.
    pub usb_current: f64,
    pub usb_budget: f64,
}

/// Everything the UI needs for one frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Telemetry {
    pub time: SimTime,
    pub status: RunStatus,
    /// Requested speed (0 = Max).
    pub speed: f64,
    /// Measured simulated seconds per wall second.
    pub realtime_factor: f64,
    pub parts: Vec<PartTelemetry>,
    pub probes: Vec<ProbeReading>,
    pub scopes: Vec<ScopeFrame>,
    pub meters: Vec<MeterReading>,
    /// Every net's voltage (hover tooltips): (name, volts).
    pub nets: Vec<(String, f64)>,
    pub world: WorldSnapshot,
    pub power: PowerReport,
    pub mcus: Vec<McuTelemetry>,
    pub erc: Vec<ErcItem>,
    pub drc: Vec<ErcItem>,
    /// `ERC 1 warning · DRC clean`.
    pub erc_summary: String,
    pub stats: crate::bench::BenchStats,
    /// Radios panel + virtual phone (RF-6, RF-7); `None` without radios on the bench.
    #[serde(default)]
    pub radio: Option<crate::radio::RadioTelemetry>,
}

impl Bench {
    /// Builds a telemetry snapshot of the current state.
    pub fn telemetry(&mut self) -> Telemetry {
        let now = self.time;
        let mcu_parts: Vec<String> = self.mcus.iter().map(|m| m.spec.part.clone()).collect();
        let board_extra: Vec<(String, Vec<(String, f64)>)> = self
            .devices
            .iter()
            .filter(|d| mcu_parts.contains(&d.spec.part))
            .map(|d| (d.spec.part.clone(), d.dev.readings()))
            .collect();
        let mut parts: Vec<PartTelemetry> = self
            .devices
            .iter()
            .filter(|d| !mcu_parts.contains(&d.spec.part))
            .map(|d| {
                let mut v = d.dev.visual();
                v.health = d.health.clone();
                let age = (now - d.smoke_at).as_secs_f64();
                v.smoke = if d.smoke > 0.0 {
                    (d.smoke * (-age / 3.0).exp() as f32).max(if v.health.is_damaged() {
                        0.05
                    } else {
                        0.0
                    })
                } else {
                    0.0
                };
                PartTelemetry {
                    part: d.spec.part.clone(),
                    reference: d.spec.reference.clone(),
                    visual: v,
                    readings: d.dev.readings(),
                }
            })
            .chain(self.mcus.iter_mut().map(|m| {
                // board LEDs: power, L (D13 duty over the last window), TX/RX activity
                let w = (now - m.led_window_start).as_secs_f64();
                if w > 0.05 {
                    let mut on = m.led_accum;
                    if let Some(s) = m.led_on_since {
                        on += (now - s.max(m.led_window_start)).as_secs_f64();
                        m.led_on_since = Some(now);
                    }
                    m.led_level = (on / w).clamp(0.0, 1.0) as f32;
                    m.led_accum = 0.0;
                    m.led_window_start = now;
                }
                PartTelemetry {
                    part: m.spec.part.clone(),
                    reference: m.spec.reference.clone(),
                    visual: VisualState {
                        look: Look::Board {
                            power: m.vcc > 2.0,
                            l: m.led_level,
                            tx: now < m.tx_until,
                            rx: now < m.rx_until,
                        },
                        health: if m.pins.iter().any(|p| p.damaged) {
                            Health::Damaged {
                                reason: "a pin was destroyed by over-current".into(),
                            }
                        } else {
                            Health::Ok
                        },
                        smoke: 0.0,
                    },
                    readings: vec![("vcc".into(), m.vcc)],
                }
            }))
            .collect();
        for (part, extra) in board_extra {
            if let Some(p) = parts.iter_mut().find(|p| p.part == part) {
                p.readings.extend(extra);
            }
        }
        let probes = self.instruments.probe_readings(now, &self.nets);
        let scopes = self.instruments.scope_frames(now, &self.nets);
        let meters = {
            let f = |n: bb_circuit::NetId| self.net_voltage(n);
            self.instruments.meter_readings(&f)
        };
        let nets = self
            .nets
            .iter()
            .enumerate()
            .filter(|(_, n)| !matches!(n.kind, NetKind::Isolated))
            .map(|(i, n)| {
                let name = if n.name == "0" {
                    "GND".to_string()
                } else {
                    n.name.clone()
                };
                (name, self.net_voltage(bb_circuit::NetId(i)))
            })
            .collect();
        let mcus = self
            .mcus
            .iter()
            .map(|m| {
                let baud = m.core.serial_baud(0);
                McuTelemetry {
                    part: m.spec.part.clone(),
                    reference: m.spec.reference.clone(),
                    label: m.core.label(),
                    engine: m.core.engine().into(),
                    vcc: m.vcc,
                    in_reset: m.core.in_reset(),
                    reset_count: m.core.reset_count(),
                    last_reset: m.core.last_reset(),
                    paused: m.paused.clone(),
                    serial_label: format!(
                        "{} · USB · {} baud · NL",
                        m.spec.reference,
                        baud.round() as u64
                    ),
                    firmware: m.firmware_name.clone(),
                    pc: m.core.as_avr_ref().map(|a| a.pc()),
                }
            })
            .collect();
        // ERC/DRC only change with the circuit: computed once per (re)build
        if self.erc_cache.is_none() {
            let conn = &self.built.connectivity;
            let e = erc::erc(&self.project, &self.lib, conn);
            let d = erc::drc(&self.project, &self.lib, conn);
            self.erc_cache = Some((e, d));
        }
        let (erc_items, drc_items) = self.erc_cache.clone().unwrap_or_default();
        let erc_summary = format!(
            "{} · {}",
            erc::summary("ERC", &erc_items),
            erc::summary("DRC", &drc_items)
        );
        Telemetry {
            time: now,
            status: if let Some(p) = &self.paused {
                RunStatus::Error(p.clone()).into_paused()
            } else {
                RunStatus::Paused
            },
            speed: 1.0,
            realtime_factor: 0.0,
            parts,
            probes,
            scopes,
            meters,
            nets,
            world: self.world.snapshot(),
            power: self.power_report(),
            mcus,
            erc: erc_items,
            drc: drc_items,
            erc_summary,
            stats: self.stats,
            radio: self
                .radio()
                .filter(|r| r.is_active())
                .map(|r| r.telemetry()),
        }
    }

    /// Supply rails, dissipation and the USB budget.
    pub fn power_report(&self) -> PowerReport {
        let mut rails = Vec::new();
        let mut total = 0.0;
        let mut usb = 0.0;
        let current = |part: &str, spec: &str| -> f64 {
            self.built
                .element(part, spec)
                .and_then(|e| {
                    let i = *self.tracked_index.get(e)?;
                    Some(self.tracked[i].i.abs())
                })
                .unwrap_or(0.0)
        };
        for p in &self.project.parts {
            let Some(def) = self.lib.get(&p.part) else {
                continue;
            };
            let supply_pins: Vec<&str> = match def.id.as_str() {
                "uno" | "nano" | "mega" => vec!["5V"],
                "esp32_devkit" => vec!["VIN"],
                "pico" => vec!["VBUS"],
                "usb_power" => vec!["VBUS"],
                "battery_pack" | "barrel_jack" => vec!["plus"],
                _ => vec![],
            };
            for pin in supply_pins {
                let v = self.pin_voltage(&p.id, pin).unwrap_or(0.0);
                let (i, is_usb) = match def.id.as_str() {
                    "battery_pack" => (current(&p.id, "V"), false),
                    "barrel_jack" => (current(&p.id, "R"), false),
                    _ => (current(&p.id, "R:fuse"), true),
                };
                if is_usb {
                    usb += i;
                }
                total += v * i;
                let net = self
                    .built
                    .connectivity
                    .net_of(&p.id, pin)
                    .map(|n| self.nets[n.0].name.clone())
                    .unwrap_or_default();
                rails.push(Rail {
                    name: if net == "0" { "GND".into() } else { net },
                    source: p.reference.clone(),
                    voltage: v,
                    current: i,
                });
            }
        }
        let mut diss: std::collections::BTreeMap<String, f64> = Default::default();
        for (part, _, el) in &self.built.elements {
            if let Some(&i) = self.tracked_index.get(el) {
                let p = self.tracked[i].p.max(0.0);
                let r = self
                    .project
                    .part(part)
                    .map(|x| x.reference.clone())
                    .unwrap_or_else(|| part.clone());
                *diss.entry(r).or_default() += p;
            }
        }
        PowerReport {
            rails,
            total_power: total,
            dissipation: diss.into_iter().filter(|(_, w)| *w > 1e-3).collect(),
            usb_current: usb,
            usb_budget: 0.5,
        }
    }
}

impl RunStatus {
    fn into_paused(self) -> RunStatus {
        match self {
            RunStatus::Error(m) if m.contains("fault") => RunStatus::Error(m),
            _ => RunStatus::Paused,
        }
    }
}
