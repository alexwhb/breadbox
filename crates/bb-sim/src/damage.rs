//! Overstress and damage (WB-13, SIM-6): every rating of the part library is watched through a
//! first-order thermal filter of the stressed quantity; above the limit a part is *stressed*,
//! above `destroy_at × limit` it is destroyed (open / short), with magic smoke, a pop for
//! electrolytics, and an event. MCU pins over 40 mA are stressed and die at 80 mA. Damaged
//! parts stay damaged until "Repair all".

use crate::bench::{McuRt, NetRt};
use crate::mcu::LogicDrive;
use crate::SimEvent;
use bb_circuit::netlist::{BuiltNetlist, DamageEffect, NetKind, RatingSpec};
use bb_circuit::part::RatingKind;
use bb_core::{DeviceEvent, Health, SimTime, Sound};
use bb_spice::Session;
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct RatingRt {
    spec: RatingSpec,
    filt: f64,
    peak: f64,
    stressed: bool,
    destroyed: bool,
}

/// A part that was just destroyed.
#[derive(Debug, Clone, PartialEq)]
pub struct NewDamage {
    pub part: String,
    pub message: String,
    pub effect: DamageEffect,
    pub smoke: bool,
}

#[derive(Debug, Clone, Default)]
struct PinRt {
    filt: f64,
    stressed: bool,
}

/// Damage state of a bench.
#[derive(Debug, Clone, Default)]
pub struct Damage {
    ratings: Vec<RatingRt>,
    by_element: HashMap<String, Vec<usize>>,
    new_damage: Vec<NewDamage>,
    remembered: HashMap<String, Health>,
    pins: HashMap<(usize, usize), PinRt>,
    pin_last: SimTime,
    pin_damage: Vec<(usize, usize, String)>,
    pin_events: Vec<(String, String)>,
}

impl Damage {
    pub(crate) fn bind(&mut self, built: &BuiltNetlist, _mcus: &[McuRt]) {
        let old: HashMap<(String, String), RatingRt> = self
            .ratings
            .drain(..)
            .map(|r| ((r.spec.part.clone(), r.spec.element.clone()), r))
            .collect();
        self.by_element.clear();
        for r in &built.ratings {
            let rt = old
                .get(&(r.part.clone(), r.element.clone()))
                .cloned()
                .map(|mut o| {
                    o.spec = r.clone();
                    o
                })
                .unwrap_or(RatingRt {
                    spec: r.clone(),
                    filt: 0.0,
                    peak: 0.0,
                    stressed: false,
                    destroyed: false,
                });
            self.by_element
                .entry(r.element.clone())
                .or_default()
                .push(self.ratings.len());
            self.ratings.push(rt);
        }
    }

    /// Feeds one integration interval of an element (piecewise-constant values over `dt`).
    pub(crate) fn integrate_one(&mut self, element: &str, i: f64, p: f64, v: f64, dt: f64) {
        let Some(idx) = self.by_element.get(element) else {
            return;
        };
        for &k in idx {
            let r = &mut self.ratings[k];
            let x = match r.spec.kind {
                RatingKind::Current => i.abs(),
                RatingKind::Power => p.max(0.0),
                RatingKind::Voltage => v.abs(),
                RatingKind::ReverseVoltage => (-v).max(0.0),
            };
            let a = 1.0 - (-dt / r.spec.tau.max(1e-6)).exp();
            r.filt += (x - r.filt) * a;
            r.peak = r.peak.max(r.filt);
        }
    }

    /// Checks limits; returns true when a part was destroyed (the bench must rebuild).
    pub(crate) fn check(
        &mut self,
        t: SimTime,
        _tracked: &[crate::bench::Tracked],
        events: &mut Vec<SimEvent>,
        _built: &BuiltNetlist,
    ) -> bool {
        let mut destroyed_parts: Vec<String> = Vec::new();
        for r in self.ratings.iter_mut() {
            if r.destroyed || !r.spec.limit.is_finite() || r.spec.limit <= 0.0 {
                continue;
            }
            let ratio = r.filt / r.spec.limit;
            let unit = match r.spec.kind {
                RatingKind::Current => "A",
                RatingKind::Power => "W",
                _ => "V",
            };
            let value = bb_core::si::format(r.filt, unit);
            if ratio > r.spec.destroy_at && r.spec.effect != "none" {
                r.destroyed = true;
                if destroyed_parts.contains(&r.spec.part) {
                    continue;
                }
                destroyed_parts.push(r.spec.part.clone());
                let msg = format!("{} — {} ({value})", r.spec.message, r.spec.reference);
                let effect = match r.spec.effect.as_str() {
                    "short" => DamageEffect::Short,
                    _ => DamageEffect::Open,
                };
                if r.spec.effect == "pop" {
                    events.push(SimEvent {
                        time: t,
                        source: r.spec.reference.clone(),
                        event: DeviceEvent::Sound(Sound::Pop),
                    });
                }
                events.push(SimEvent {
                    time: t,
                    source: r.spec.reference.clone(),
                    event: DeviceEvent::Damage { reason: msg.clone() },
                });
                self.new_damage.push(NewDamage {
                    part: r.spec.part.clone(),
                    message: msg,
                    effect,
                    smoke: r.spec.effect == "smoke" || r.spec.effect == "pop" || r.spec.effect == "open",
                });
            } else if ratio > 1.0 && !r.stressed {
                r.stressed = true;
                events.push(SimEvent {
                    time: t,
                    source: r.spec.reference.clone(),
                    event: DeviceEvent::Stress {
                        reason: format!("{} — {} ({value})", r.spec.message, r.spec.reference),
                    },
                });
            } else if ratio < 0.8 && r.stressed {
                r.stressed = false;
            }
        }
        for (src, msg) in self.pin_events.drain(..) {
            events.push(SimEvent {
                time: t,
                source: src,
                event: DeviceEvent::Stress { reason: msg },
            });
        }
        !self.new_damage.is_empty()
    }

    pub(crate) fn take_new(&mut self) -> Vec<NewDamage> {
        std::mem::take(&mut self.new_damage)
    }

    pub(crate) fn remember(&mut self, part: &str, h: Health) {
        self.remembered.insert(part.into(), h);
    }
    pub(crate) fn remembered(&self, part: &str) -> Option<Health> {
        self.remembered.get(part).cloned()
    }

    pub(crate) fn repair_all(&mut self) {
        self.remembered.clear();
        self.pins.clear();
        for r in self.ratings.iter_mut() {
            r.destroyed = false;
            r.stressed = false;
            r.filt = 0.0;
        }
    }

    /// Stressed parts (for the inspector: OK / stressed / damaged).
    pub fn stressed_parts(&self) -> Vec<(String, String)> {
        self.ratings
            .iter()
            .filter(|r| r.stressed && !r.destroyed)
            .map(|r| (r.spec.part.clone(), r.spec.message.clone()))
            .collect()
    }

    /// MCU pin currents (> 40 mA stress, > 80 mA destroyed).
    pub(crate) fn pin_integrate(&mut self, t: SimTime, mcus: &[McuRt], s: &Session, nets: &[NetRt]) {
        let dt = (t - self.pin_last).as_secs_f64();
        if dt <= 0.0 {
            return;
        }
        self.pin_last = t;
        for (mi, m) in mcus.iter().enumerate() {
            for (pi, p) in m.pins.iter().enumerate() {
                if p.damaged || !matches!(p.drive, LogicDrive::Output { .. }) {
                    if let Some(st) = self.pins.get_mut(&(mi, pi)) {
                        st.filt *= (-dt / 0.05f64).exp();
                    }
                    continue;
                }
                let Some(net) = p.net else { continue };
                let i = match nets[net.0].kind {
                    NetKind::Analog(_) => {
                        let ih = p.hi.and_then(|h| s.port_current(h)).unwrap_or(0.0);
                        let il = p.lo.and_then(|l| s.port_current(l)).unwrap_or(0.0);
                        ih.abs() + il.abs()
                    }
                    NetKind::Digital => {
                        let d = m.model.net_drive(p.drive, m.vcc);
                        d.current_into(nets[net.0].voltage).abs()
                    }
                    NetKind::Isolated => 0.0,
                };
                if i < 1e-4 && !self.pins.contains_key(&(mi, pi)) {
                    continue;
                }
                let st = self.pins.entry((mi, pi)).or_default();
                st.filt += (i - st.filt) * (1.0 - (-dt / 0.05f64).exp());
                let max = m.model.max_current;
                if st.filt > 2.0 * max {
                    self.pin_damage.push((
                        mi,
                        pi,
                        format!(
                            "pin {} destroyed — {} through a {} rated pin",
                            p.name,
                            bb_core::si::format(st.filt, "A"),
                            bb_core::si::format(max, "A")
                        ),
                    ));
                    st.filt = 0.0;
                } else if st.filt > max && !st.stressed {
                    st.stressed = true;
                    self.pin_events.push((
                        m.spec.reference.clone(),
                        format!(
                            "pin {} over {} ({})",
                            p.name,
                            bb_core::si::format(max, "A"),
                            bb_core::si::format(st.filt, "A")
                        ),
                    ));
                } else if st.filt < 0.8 * max {
                    st.stressed = false;
                }
            }
        }
    }

    pub(crate) fn take_pin_damage(&mut self) -> Vec<(usize, usize, String)> {
        std::mem::take(&mut self.pin_damage)
    }
}
