//! [`DeviceCtx`] implementation: what a behavioural device sees while one of its hooks runs.

use crate::bench::{Bench, WakeKind};
use bb_core::{DeviceCtx, DeviceEvent, PinDrive, Rng, SimTime, WorldMut, WorldView};

pub struct DevCtx<'a> {
    pub(crate) bench: &'a mut Bench,
    pub(crate) dev: usize,
    pub(crate) now: SimTime,
}

impl DevCtx<'_> {
    fn element_name(&self, local: &str) -> Option<String> {
        self.bench.devices[self.dev]
            .spec
            .elements
            .get(local)
            .cloned()
    }
    /// Fresh analog solution (pending drive changes applied) for reads inside a hook.
    fn fresh(&mut self) {
        if let Some(s) = self.bench.analog.session.as_mut()
            && let Ok(cr) = s.op()
        {
            for c in cr {
                if let Some(&tg) = self.bench.analog.watch_targets.get(c.watch.0) {
                    self.bench.edges.push_back((self.now, tg, c.rising));
                }
            }
        }
    }
}

impl DeviceCtx for DevCtx<'_> {
    fn now(&self) -> SimTime {
        self.now
    }
    fn pin_voltage(&self, pin: usize) -> f64 {
        self.bench.device_pin_voltage(self.dev, pin)
    }
    fn pin_current(&self, pin: usize) -> f64 {
        self.bench.device_pin_current(self.dev, pin)
    }
    fn element_current(&self, element: &str) -> Option<f64> {
        let name = self.element_name(element)?;
        self.bench.analog.session.as_ref()?.current(&name)
    }
    fn element_charge(&self, element: &str) -> Option<f64> {
        let name = self.element_name(element)?;
        let &i = self.bench.tracked_index.get(&name)?;
        let tr = &self.bench.tracked[i];
        // include the part since the last integration point
        let dt = (self.now - tr.last).as_secs_f64().max(0.0);
        Some(tr.charge + tr.i * dt)
    }
    fn element_energy(&self, element: &str) -> Option<f64> {
        let name = self.element_name(element)?;
        let &i = self.bench.tracked_index.get(&name)?;
        let tr = &self.bench.tracked[i];
        let dt = (self.now - tr.last).as_secs_f64().max(0.0);
        Some(tr.energy + tr.p * dt)
    }
    fn track_element(&mut self, element: &str) {
        if let Some(name) = self.element_name(element) {
            self.bench.track(&name);
        }
    }
    fn set_drive(&mut self, pin: usize, drive: PinDrive) {
        if pin < self.bench.devices[self.dev].drives.len() {
            let t = self.now;
            self.bench.set_device_drive(self.dev, pin, drive, t);
        }
    }
    fn set_ext(&mut self, name: &str, value: f64) {
        let full = match self.bench.devices[self.dev].spec.ext.get(name) {
            Some((full, _)) => full.clone(),
            None => format!("{}.{}", self.bench.devices[self.dev].spec.reference, name),
        };
        self.bench.ext_values.insert(full.clone(), value);
        if let Some(s) = self.bench.analog.session.as_mut() {
            s.set_ext(&full, value);
        }
    }
    fn wake_at(&mut self, at: SimTime, token: u64) {
        let generation = *self.bench.devices[self.dev]
            .token_gen
            .entry(token)
            .or_insert(0);
        let at = at.max(self.now);
        self.bench.push_wake(
            at,
            WakeKind::Device {
                dev: self.dev,
                token,
                generation,
            },
        );
    }
    fn cancel_wake(&mut self, token: u64) {
        *self.bench.devices[self.dev]
            .token_gen
            .entry(token)
            .or_insert(0) += 1;
    }
    fn watch(&mut self, pin: usize, lo: f64, hi: f64) {
        if pin < self.bench.devices[self.dev].watches.len() {
            self.fresh();
            self.bench.add_device_watch(self.dev, pin, lo, hi);
        }
    }
    fn tick_every(&mut self, period: SimTime) {
        let d = &mut self.bench.devices[self.dev];
        d.tick = period;
        d.tick_gen += 1;
        let generation = d.tick_gen;
        if period > SimTime::ZERO {
            let t = self.now + period;
            self.bench.push_wake(
                t,
                WakeKind::Tick {
                    dev: self.dev,
                    generation,
                },
            );
        }
    }
    fn world(&self) -> &dyn WorldView {
        self.bench.world_view()
    }
    fn world_mut(&mut self) -> Option<&mut dyn WorldMut> {
        Some(&mut self.bench.world)
    }
    fn rng(&mut self) -> &mut Rng {
        &mut self.bench.devices[self.dev].rng
    }
    fn service(&mut self, name: &str) -> Option<&mut dyn std::any::Any> {
        self.bench.service_mut(name)
    }
    fn emit(&mut self, ev: DeviceEvent) {
        let source = self.bench.devices[self.dev].spec.reference.clone();
        match &ev {
            DeviceEvent::Damage { reason } => {
                let part = self.bench.devices[self.dev].spec.part.clone();
                self.bench.devices[self.dev].health = bb_core::Health::Damaged {
                    reason: reason.clone(),
                };
                self.bench.damage.remember(
                    &part,
                    bb_core::Health::Damaged {
                        reason: reason.clone(),
                    },
                );
            }
            DeviceEvent::Stress { reason }
                if self.bench.devices[self.dev].health == bb_core::Health::Ok =>
            {
                self.bench.devices[self.dev].health = bb_core::Health::Stressed {
                    reason: reason.clone(),
                };
            }
            _ => {}
        }
        self.bench.events.push(crate::SimEvent {
            time: self.now,
            source,
            event: ev,
        });
    }
}
