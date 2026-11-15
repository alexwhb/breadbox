//! The co-simulation scheduler (ARCHITECTURE §6, SIM-1).
//!
//! Each step:
//!
//! 1. pick a target: `min(now + quantum, next wake, limit)` — the quantum is the configured
//!    sync quantum (1 µs) while the analog circuit is moving, and up to `max_quantum` while it
//!    is quiescent (nothing can cross a threshold until an input changes);
//! 2. run every MCU to the target; each stops early (cycle-exact) on a pin change or an ADC
//!    sample it cannot be answered inline; the earliest stop is the sync time;
//! 3. advance the analog session to the sync time, applying every MCU pin event at its exact
//!    time and delivering threshold crossings (interpolated times) to MCU inputs and devices;
//! 4. answer ADC samples, run due device wake-ups / ticks, step the world, integrate element
//!    currents (damage, power, LEDs), update probes/scope, check damage.

use crate::SimError;
use crate::bench::{Bench, EdgeTarget, WakeKind};
use crate::mcu::McuStop;
use bb_circuit::netlist::NetKind;
use bb_core::{LogLevel, SimTime};

impl Bench {
    fn analog_quiescent(&self) -> bool {
        match &self.analog.session {
            None => true,
            Some(s) => s.is_quiescent(),
        }
    }

    /// Runs the bench until simulated time `t` (or a debugger pause).
    pub fn run_until(&mut self, t: SimTime) -> Result<(), SimError> {
        while self.time < t && self.paused.is_none() {
            self.step(t)?;
        }
        Ok(())
    }

    /// Runs for `dt` of simulated time.
    pub fn run_for(&mut self, dt: SimTime) -> Result<(), SimError> {
        let t = self.time + dt;
        self.run_until(t)
    }

    /// Continues after a debugger pause.
    pub fn resume(&mut self) {
        self.paused = None;
        for m in self.mcus.iter_mut() {
            m.paused = None;
        }
    }

    /// One scheduler step towards `limit`.
    pub fn step(&mut self, limit: SimTime) -> Result<(), SimError> {
        self.stats.sync_points += 1;
        let t0 = self.time;
        let quiet = self.analog_quiescent() && self.edges.is_empty();
        let single_mcu = self.mcus.len() <= 1;
        let settle = std::mem::take(&mut self.settle_next);
        let q = if settle {
            // an MCU output just switched an analog node: look again right after it settles
            // (capacitive nodes can't jump at the re-solve), so samples and integrals see it
            self.quantum
        } else if self.mcus.is_empty() || (quiet && single_mcu) {
            // device watches are exact (the solver stops at every crossing), and nothing else
            // needs a short quantum
            self.max_quantum
        } else if single_mcu {
            self.adaptive_quantum(t0)
        } else {
            self.quantum
        };
        let mut target = (t0 + q).min(limit);
        if let Some(w) = self.next_wake_time() {
            target = target.min(w.max(t0));
        }
        for s in &self.services {
            if let Some(w) = s.next_wake() {
                target = target.min(w.max(t0));
            }
        }
        if let Some(c) = self.next_capture() {
            target = target.min(c.max(t0 + SimTime::from_ns(10)));
        }
        if target <= t0 {
            // something is due right now
            self.process_due(t0)?;
            // services due now step before the MCUs run ahead: what they produce (radio frames
            // to send, device wakes) must start at its real time, not in an MCU's past (the
            // first bits of a UART frame would collapse into one late edge)
            let mut stepped = false;
            for i in 0..self.services.len() {
                if self.services[i].next_wake().is_some_and(|w| w <= t0) {
                    self.services[i].step(t0);
                    stepped = true;
                }
            }
            if stepped {
                self.service_outputs();
            }
            if self.next_wake_time().is_some_and(|w| w <= t0) {
                return Ok(());
            }
            target = (t0 + q).min(limit);
            // what was just processed may have scheduled wakes inside this quantum (a module's
            // first UART edge 0.8 ms from now): the MCUs must stop there, not run past it
            if let Some(w) = self.next_wake_time().filter(|w| *w > t0) {
                target = target.min(w);
            }
            for s in &self.services {
                if let Some(w) = s.next_wake().filter(|w| *w > t0) {
                    target = target.min(w);
                }
            }
            if let Some(c) = self.next_capture() {
                target = target.min(c.max(t0 + SimTime::from_ns(10)));
            }
            if target <= t0 {
                return Ok(());
            }
        }

        // ---- MCUs
        let mut t_sync = target;
        let inline_ok = quiet && single_mcu;
        for mi in 0..self.mcus.len() {
            if self.mcus[mi].paused.is_some() {
                continue;
            }
            if self.mcus[mi].core.now() >= target {
                continue;
            }
            // API-level boards cannot stop for a sample: answer from the current solution; they
            // also reach bench devices transaction by transaction (I²C, SPI)
            let api_level = self.mcus[mi].core.engine() == "API-level";
            let stop = if api_level {
                let mut core: Box<dyn crate::mcu::McuCore> =
                    std::mem::replace(&mut self.mcus[mi].core, Box::new(crate::mcu::Detached));
                let mut io = BenchIo {
                    bench: self,
                    mi,
                    answered: 0,
                };
                let stop = core.run_until(target, &mut io);
                let answered = io.answered;
                self.mcus[mi].core = core;
                self.stats.adc_answered_inline += answered;
                stop
            } else {
                let m = &mut self.mcus[mi];
                let mut io = AdcIoBorrow {
                    inline: inline_ok && m.pending.is_empty(),
                    nets: &self.nets,
                    session: self.analog.session.as_ref(),
                    pins: &m.pins,
                    answered: 0,
                    services: &mut self.services,
                    part: &m.spec.part,
                };
                let stop = m.core.run_until(target, &mut io);
                self.stats.adc_answered_inline += io.answered;
                stop
            };
            let m = &mut self.mcus[mi];
            match stop {
                McuStop::Reached => {}
                McuStop::PinChange => {
                    self.stats.mcu_stops += 1;
                    t_sync = t_sync.min(m.core.now().max(t0));
                }
                McuStop::AdcSample { pin, time } => {
                    self.stats.adc_stops += 1;
                    m.adc_request = Some((pin, time));
                    t_sync = t_sync.min(time.max(t0));
                }
                McuStop::Paused(why) => {
                    let now = m.core.now();
                    m.paused = Some(why.clone());
                    t_sync = t_sync.min(now.max(t0));
                    self.paused = Some(format!("{}: {why}", self.mcus[mi].spec.reference));
                }
                McuStop::Fault(f) => {
                    let r = m.spec.reference.clone();
                    self.log(LogLevel::Error, &r, &format!("firmware fault: {f}"));
                }
            }
            let evs = self.mcus[mi].core.take_pin_events();
            self.mcus[mi].pending.extend(evs);
        }

        // ---- events up to t_sync, in time order
        let mut evs: Vec<(SimTime, usize, crate::mcu::McuPinEvent)> = Vec::new();
        for (mi, m) in self.mcus.iter_mut().enumerate() {
            while let Some(e) = m.pending.front() {
                if e.time <= t_sync {
                    let e = m.pending.pop_front().expect("front");
                    evs.push((e.time, mi, e));
                } else {
                    break;
                }
            }
        }
        evs.sort_by_key(|(t, mi, _)| (*t, *mi));
        for (t, mi, e) in evs {
            let t = t.max(self.time);
            self.advance_analog(t)?;
            self.time = t;
            if let Some(net) = self.mcus[mi].pins.get(e.pin).and_then(|p| p.net)
                && matches!(self.nets[net.0].kind, NetKind::Analog(_))
            {
                self.settle_next = true;
            }
            self.apply_mcu_drive_now(mi, e.pin, e.drive, t);
            self.process_edges()?;
        }
        self.advance_analog(t_sync)?;
        self.time = t_sync;
        self.commit_analog(t_sync)?;

        // ---- ADC samples at the sync time
        for mi in 0..self.mcus.len() {
            if let Some((pin, at)) = self.mcus[mi].adc_request
                && at <= t_sync
            {
                let v = self.mcus[mi].pins[pin]
                    .net
                    .map(|n| self.net_voltage(n))
                    .unwrap_or(0.0);
                self.mcus[mi].core.supply_adc(v);
                self.mcus[mi].adc_request = None;
            }
        }

        // ---- due wakes, services, bookkeeping
        self.process_due(t_sync)?;
        for i in 0..self.services.len() {
            if self.services[i].next_wake().is_some_and(|w| w <= t_sync) {
                self.services[i].step(t_sync);
            }
        }
        self.service_outputs();
        if self.next_wake_time().is_some_and(|w| w <= t_sync) {
            // a service woke a device right now (radio frames to start sending)
            self.process_due(t_sync)?;
        }
        self.after_sync(t_sync)?;
        Ok(())
    }

    /// Longest quantum that keeps MCU inputs on moving analog nets within one sync quantum
    /// of their threshold crossings: half the predicted time for the fastest-approaching
    /// watched input to reach its threshold (from its slope since the last sync point).
    fn adaptive_quantum(&mut self, now: SimTime) -> SimTime {
        let mut q = self.max_quantum.as_secs_f64();
        let session = self.analog.session.as_ref();
        for m in self.mcus.iter_mut() {
            let (lo, hi) = (m.model.vil * m.vcc, m.model.vih * m.vcc);
            for p in m.pins.iter_mut() {
                if !p.watched || !matches!(p.drive, crate::mcu::LogicDrive::Input { .. }) {
                    continue;
                }
                let Some(net) = p.net else { continue };
                let n = &self.nets[net.0];
                let (Some(node), Some(s)) = (n.node, session) else {
                    continue;
                };
                let v = s.voltage_at(node);
                let dt = (now - p.last_t).as_secs_f64();
                if dt > 0.0 {
                    let slope = (v - p.last_v) / dt;
                    let mid = 0.5 * (lo + hi);
                    // distance to the threshold it is heading for
                    let dist = if slope > 0.0 && v < hi {
                        Some(hi - v)
                    } else if slope < 0.0 && v > lo {
                        Some(v - lo)
                    } else if (v - mid).abs() < 0.5 * (hi - lo) {
                        Some(0.0)
                    } else {
                        None
                    };
                    if let Some(d) = dist {
                        let t_cross = d / slope.abs().max(1e-12);
                        q = q.min(0.5 * t_cross);
                    }
                } else {
                    q = q.min(self.quantum.as_secs_f64());
                }
                p.last_v = v;
                p.last_t = now;
            }
        }
        SimTime::from_secs_f64(q).clamp(self.quantum, self.max_quantum)
    }

    /// Advances the analog session to `t`, stopping at every threshold crossing to deliver it.
    pub(crate) fn advance_analog(&mut self, t: SimTime) -> Result<(), SimError> {
        let t0 = self.analog.t0;
        let Some(s) = self.analog.session.as_mut() else {
            return Ok(());
        };
        let ta = (t - t0).as_secs_f64();
        if ta <= s.time() {
            return Ok(());
        }
        loop {
            let s = self.analog.session.as_mut().expect("session");
            let cr = s
                .advance_until_crossing(ta)
                .map_err(|e| SimError::Analog(e.to_string()))?;
            if cr.is_empty() {
                break;
            }
            let tc = t0 + SimTime::from_secs_f64(cr[0].time);
            let tc = tc.max(self.time).min(t);
            self.integrate(tc);
            for c in cr {
                if let Some(&target) = self.analog.watch_targets.get(c.watch.0) {
                    self.edges.push_back((tc, target, c.rising));
                }
            }
            self.process_edges()?;
            let s = self.analog.session.as_ref().expect("session");
            if s.time() >= ta {
                break;
            }
        }
        self.integrate(t);
        Ok(())
    }

    /// Resolves pending input changes at the current analog time and delivers jumps.
    pub(crate) fn commit_analog(&mut self, t: SimTime) -> Result<(), SimError> {
        let Some(s) = self.analog.session.as_mut() else {
            return self.process_edges();
        };
        let cr = s.op().map_err(|e| SimError::Analog(e.to_string()))?;
        for c in cr {
            if let Some(&target) = self.analog.watch_targets.get(c.watch.0) {
                self.edges.push_back((t, target, c.rising));
            }
        }
        self.process_edges()
    }

    /// Delivers queued edges (devices may cause more: processed in FIFO order).
    pub(crate) fn process_edges(&mut self) -> Result<(), SimError> {
        if self.in_edges {
            return Ok(());
        }
        self.in_edges = true;
        let mut guard = 0u32;
        while let Some((t, target, rising)) = self.edges.pop_front() {
            guard += 1;
            if guard > 100_000 {
                self.in_edges = false;
                return Err(SimError::Analog(
                    "edge storm: a circuit oscillates without delay".into(),
                ));
            }
            self.stats.edges += 1;
            match target {
                EdgeTarget::McuPin { mcu, pin } => {
                    let m = &mut self.mcus[mcu];
                    let now = m.core.now();
                    if t > now {
                        m.core.schedule_input(pin, Some(rising), t);
                    } else {
                        if t < now {
                            self.stats.late_edges += 1;
                        }
                        m.core.set_input(pin, Some(rising));
                    }
                    if let Some(net) = m.pins.get(pin).and_then(|p| p.net) {
                        // comparator inputs (AIN0/AIN1) follow the voltage too
                        let v = self.net_voltage(net);
                        self.mcus[mcu].core.set_analog(pin, v);
                    }
                }
                EdgeTarget::DevicePin { dev, pin } => {
                    if dev < self.devices.len() {
                        self.with_device(dev, t, |d, ctx| d.on_edge(ctx, pin, rising));
                        // drives set by the device take effect now
                        if let Some(s) = self.analog.session.as_mut() {
                            let cr = s.op().map_err(|e| SimError::Analog(e.to_string()))?;
                            for c in cr {
                                if let Some(&tg) = self.analog.watch_targets.get(c.watch.0) {
                                    self.edges.push_back((t, tg, c.rising));
                                }
                            }
                        }
                    }
                }
                EdgeTarget::McuReset { mcu } => {
                    self.mcus[mcu].core.set_reset_line(!rising);
                    if !rising {
                        let r = self.mcus[mcu].spec.reference.clone();
                        self.mcus[mcu]
                            .serial
                            .system(t, &format!("{r} reset (RESET pin pulled low)"));
                    }
                }
            }
        }
        self.in_edges = false;
        Ok(())
    }

    /// Runs device wakes, ticks and world steps due at or before `t`.
    pub(crate) fn process_due(&mut self, t: SimTime) -> Result<(), SimError> {
        let mut guard = 0u32;
        while let Some(w) = self.next_wake_time() {
            if w > t {
                break;
            }
            guard += 1;
            if guard > 200_000 {
                return Err(SimError::Analog("wake-up storm".into()));
            }
            let std::cmp::Reverse(wake) = self.heap.pop().expect("peeked");
            match wake.kind {
                WakeKind::Device { dev, token, .. } => {
                    self.with_device(dev, wake.time, |d, ctx| d.on_wake(ctx, token));
                }
                WakeKind::Tick { dev, generation } => {
                    let period = self.devices[dev].tick;
                    self.with_device(dev, wake.time, |d, ctx| d.on_tick(ctx));
                    if self.devices[dev].tick_gen == generation && period > SimTime::ZERO {
                        self.push_wake(wake.time + period, WakeKind::Tick { dev, generation });
                    }
                }
                WakeKind::World => {
                    self.world.step(wake.time);
                    for e in self.world.take_events() {
                        self.events.push(crate::SimEvent {
                            time: e.time,
                            source: "world".into(),
                            event: bb_core::DeviceEvent::Log {
                                level: LogLevel::Info,
                                text: e.text,
                            },
                        });
                    }
                    // die temperature follows the air around the board
                    let tc = bb_core::WorldView::var(&self.world, "air.temp_c").unwrap_or(21.0);
                    for m in self.mcus.iter_mut() {
                        m.core.set_temperature(tc);
                    }
                    self.push_wake(wake.time + self.world_dt, WakeKind::World);
                }
            }
            self.commit_analog(wake.time)?;
        }
        Ok(())
    }

    /// Bookkeeping after each sync point: integrals, supplies, serial, damage, instruments.
    fn after_sync(&mut self, t: SimTime) -> Result<(), SimError> {
        self.integrate(t);
        self.record_captures(t, None);
        for mi in 0..self.mcus.len() {
            self.update_vcc(mi);
            let bytes = self.mcus[mi].core.take_serial();
            if !bytes.is_empty() {
                self.mcus[mi].tx_until = t + SimTime::from_ms(30);
            }
            for (port, at, b) in bytes {
                if port == 0 {
                    self.mcus[mi].serial.byte(at, b);
                }
            }
            for (at, text) in self.mcus[mi].core.take_log() {
                self.mcus[mi].serial.system(at.min(t), &text);
            }
            let resets = self.mcus[mi].core.reset_count();
            if self.mcus[mi].serial.seen_resets < resets {
                self.mcus[mi].serial.seen_resets = resets;
                let label = self.mcus[mi].core.label();
                let cause = self.mcus[mi].core.last_reset();
                self.mcus[mi]
                    .serial
                    .system(t, &format!("Breadbox sim · {label} · reset cause: {cause}"));
            }
        }
        // device events → serial annotations / log
        self.route_events();
        // damage
        if self
            .damage
            .check(t, &self.tracked, &mut self.events, &self.built)
        {
            self.apply_damage(t)?;
        }
        self.check_pin_damage(t);
        // instruments: analog nets changed?
        if self.analog.session.is_some() {
            for ni in self.instruments.watched_nets() {
                if matches!(self.nets[ni.0].kind, NetKind::Analog(_)) {
                    let v = self.net_voltage(ni);
                    self.instruments.on_net_change(ni, t, v);
                }
            }
        }
        Ok(())
    }

    fn route_events(&mut self) {
        let start = self.routed.min(self.events.len());
        for ev in &self.events[start..] {
            if let bb_core::DeviceEvent::Annotation { channel, text } = &ev.event {
                // annotations go to the serial monitor of the MCU wired to the source (RF-8)
                match self.routes.get(&ev.source) {
                    Some(mcus) => {
                        for &mi in mcus {
                            if let Some(m) = self.mcus.get_mut(mi) {
                                m.serial.annotation(ev.time, channel, text);
                            }
                        }
                    }
                    None => {
                        for m in self.mcus.iter_mut() {
                            m.serial.annotation(ev.time, channel, text);
                        }
                    }
                }
            }
        }
        self.routed = self.events.len();
        // keep the event log bounded when nobody drains it
        if self.events.len() > 50_000 {
            self.events.drain(..25_000);
            self.routed = self.events.len();
        }
    }

    /// Integrates tracked element currents/powers up to `t` and samples new values.
    pub(crate) fn integrate(&mut self, t: SimTime) {
        let Some(s) = self.analog.session.as_ref() else {
            return;
        };
        for tr in self.tracked.iter_mut() {
            // the value at the end of the interval stands for it: inputs change at sync points
            // (the start of an interval), and a node with capacitance only settles to its new
            // value after the re-solve there
            let i = s.current(&tr.element).unwrap_or(0.0);
            let va = tr.a.map(|n| s.voltage_at(n)).unwrap_or(0.0);
            let vb = tr.b.map(|n| s.voltage_at(n)).unwrap_or(0.0);
            tr.i = i;
            tr.v = va - vb;
            tr.p = i * (va - vb);
            let dt = (t - tr.last).as_secs_f64();
            if dt > 0.0 {
                tr.charge += tr.i * dt;
                tr.energy += tr.p * dt;
                self.damage.integrate_one(&tr.element, tr.i, tr.p, tr.v, dt);
                tr.last = t;
            }
        }
        self.damage.pin_integrate(t, &self.mcus, s, &self.nets);
    }

    /// Starts integrating an element (by SPICE name).
    pub(crate) fn track(&mut self, element: &str) {
        if self.tracked_index.contains_key(element) {
            return;
        }
        let Some(s) = self.analog.session.as_ref() else {
            return;
        };
        // element nodes from the netlist card
        let card = s.netlist().top.elements.iter().find(|e| e.name == element);
        let (a, b) = match card {
            Some(c) if c.nodes.len() >= 2 => (s.node(&c.nodes[0]), s.node(&c.nodes[1])),
            _ => (None, None),
        };
        let i = s.current(element).unwrap_or(0.0);
        self.tracked_index
            .insert(element.to_string(), self.tracked.len());
        self.tracked.push(crate::bench::Tracked {
            element: element.into(),
            a,
            b,
            i,
            p: 0.0,
            v: 0.0,
            charge: 0.0,
            energy: 0.0,
            last: self.time,
        });
    }

    fn apply_damage(&mut self, t: SimTime) -> Result<(), SimError> {
        let newly = self.damage.take_new();
        if newly.is_empty() {
            return Ok(());
        }
        for d in &newly {
            self.damaged.insert(d.part.clone(), d.effect);
            if let Some(dev) = self.devices.iter_mut().find(|x| x.spec.part == d.part) {
                dev.health = bb_core::Health::Damaged {
                    reason: d.message.clone(),
                };
                dev.smoke = if d.smoke { 1.0 } else { 0.3 };
                dev.smoke_at = t;
            }
            self.damage.remember(
                &d.part,
                bb_core::Health::Damaged {
                    reason: d.message.clone(),
                },
            );
        }
        self.rebuild()
    }

    fn check_pin_damage(&mut self, t: SimTime) {
        for (mi, pi, msg) in self.damage.take_pin_damage() {
            if let Some(p) = self.mcus.get_mut(mi).and_then(|m| m.pins.get_mut(pi)) {
                if p.damaged {
                    continue;
                }
                p.damaged = true;
                let d = p.drive;
                self.apply_mcu_drive_now(mi, pi, d, t);
                let r = self.mcus[mi].spec.reference.clone();
                let part = self.mcus[mi].spec.part.clone();
                self.events.push(crate::SimEvent {
                    time: t,
                    source: r.clone(),
                    event: bb_core::DeviceEvent::Damage {
                        reason: msg.clone(),
                    },
                });
                self.mcus[mi].serial.warn(t, &format!("{r}: {msg}"));
                let _ = part;
            }
        }
    }
}

struct AdcIoBorrow<'a> {
    inline: bool,
    nets: &'a [crate::bench::NetRt],
    session: Option<&'a bb_spice::Session>,
    pins: &'a [crate::bench::McuPinRt],
    answered: u64,
    services: &'a mut [Box<dyn crate::bench::Service>],
    part: &'a str,
}

impl crate::mcu::McuIo for AdcIoBorrow<'_> {
    fn adc_sample(&mut self, pin: usize, _time: SimTime) -> Option<f64> {
        if !self.inline {
            return None;
        }
        let net = self.pins.get(pin)?.net?;
        let n = &self.nets[net.0];
        let v = match (&n.kind, n.node, self.session) {
            (NetKind::Analog(name), _, _) if name == "0" => 0.0,
            (NetKind::Analog(_), Some(node), Some(s)) => s.voltage_at(node),
            (NetKind::Digital, _, _) => n.voltage,
            _ => return None,
        };
        self.answered += 1;
        Some(v)
    }
    fn service(&mut self, channel: u8, request: &[u8], at: SimTime) -> Option<Vec<u8>> {
        self.services
            .iter_mut()
            .find_map(|s| s.host_service(self.part, channel, request, at))
    }
}

/// I/O for an API-level board with the whole bench at hand (its core is detached meanwhile).
struct BenchIo<'a> {
    bench: &'a mut Bench,
    mi: usize,
    answered: u64,
}

impl BenchIo<'_> {
    fn net_of(&self, pin: usize) -> Option<bb_circuit::NetId> {
        self.bench.mcus[self.mi].pins.get(pin)?.net
    }
    /// Devices with a pin on `net`.
    fn devices_on(&self, net: bb_circuit::NetId) -> Vec<usize> {
        (0..self.bench.devices.len())
            .filter(|&d| self.bench.devices[d].nets.contains(&Some(net)))
            .collect()
    }
}

impl crate::mcu::McuIo for BenchIo<'_> {
    fn adc_sample(&mut self, pin: usize, _time: SimTime) -> Option<f64> {
        let net = self.net_of(pin)?;
        if matches!(self.bench.nets[net.0].kind, NetKind::Isolated) {
            return None;
        }
        self.answered += 1;
        Some(self.bench.net_voltage(net))
    }
    fn service(&mut self, channel: u8, request: &[u8], at: SimTime) -> Option<Vec<u8>> {
        let part = self.bench.mcus[self.mi].spec.part.clone();
        self.bench
            .services
            .iter_mut()
            .find_map(|s| s.host_service(&part, channel, request, at))
    }
    fn i2c(
        &mut self,
        sda: usize,
        addr: u8,
        write: &[u8],
        read: usize,
        at: SimTime,
    ) -> Option<Vec<u8>> {
        let net = self.net_of(sda)?;
        let at = at.max(self.bench.time);
        for d in self.devices_on(net) {
            let r = self
                .bench
                .with_device(d, at, |dev, ctx| dev.i2c_transfer(ctx, addr, write, read));
            if r.is_some() {
                return r;
            }
        }
        None
    }
    fn spi(&mut self, sck: usize, mosi: &[u8], at: SimTime) -> Option<Vec<u8>> {
        let net = self.net_of(sck)?;
        let at = at.max(self.bench.time);
        let mut out = None;
        for d in self.devices_on(net) {
            let r = self
                .bench
                .with_device(d, at, |dev, ctx| dev.spi_transfer(ctx, mosi));
            if out.is_none() {
                out = r;
            }
        }
        out
    }
}
