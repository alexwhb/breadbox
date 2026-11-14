//! R, C, L (with mutual coupling).

use super::*;
use crate::elab::SimOptions;

/// Temperature factor `1 + tc1·ΔT + tc2·ΔT²`.
fn tc_factor(tc1: f64, tc2: f64, temp_k: f64, tnom_c: f64) -> f64 {
    let dt = temp_k - (tnom_c + KELVIN);
    1.0 + tc1 * dt + tc2 * dt * dt
}

/// Instance temperature: `temp=` (°C) overrides, `dtemp=` adds.
pub(crate) fn inst_temp(circuit_k: f64, temp: Option<f64>, dtemp: f64) -> f64 {
    temp.map(|t| t + KELVIN).unwrap_or(circuit_k) + dtemp
}

#[derive(Debug, Clone)]
pub struct Resistor {
    pub name: String,
    pub a: usize,
    pub b: usize,
    /// Nominal resistance (at tnom), before `m`.
    pub r: f64,
    pub tc1: f64,
    pub tc2: f64,
    pub temp: Option<f64>,
    pub dtemp: f64,
    pub m: f64,
    pub noisy: bool,
    g: f64,
    temp_k: f64,
    tnom: f64,
    s: G4,
}

impl Resistor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        a: usize,
        b: usize,
        r: f64,
        tc1: f64,
        tc2: f64,
        temp: Option<f64>,
        dtemp: f64,
        m: f64,
        noisy: bool,
    ) -> Self {
        Resistor {
            name,
            a,
            b,
            r,
            tc1,
            tc2,
            temp,
            dtemp,
            m,
            noisy,
            g: 0.0,
            temp_k: 300.15,
            tnom: 27.0,
            s: G4::default(),
        }
    }
    fn update(&mut self) {
        let mut r =
            self.r * tc_factor(self.tc1, self.tc2, self.temp_k, self.tnom) / self.m.max(1e-30);
        if r.abs() < 1e-6 {
            r = if r < 0.0 { -1e-6 } else { 1e-6 };
        }
        self.g = 1.0 / r;
    }
    pub fn conductance(&self) -> f64 {
        self.g
    }
}

impl Dev for Resistor {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.s = s.g4(self.a, self.b);
        self.tnom = s.opts.tnom;
        self.temp_k = inst_temp(s.opts.temp + KELVIN, self.temp, self.dtemp);
        self.update();
        Ok(())
    }
    fn temperature(&mut self, temp_k: f64, o: &SimOptions) {
        self.tnom = o.tnom;
        self.temp_k = inst_temp(temp_k, self.temp, self.dtemp);
        self.update();
    }
    fn load(&mut self, ck: &mut Ckt) {
        self.s.stamp(ck, self.g);
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        self.s.stamp_ac(ac, C64::new(self.g, 0.0));
    }
    fn current(&self, ck: &Ckt) -> Option<f64> {
        Some((ck.x[self.a] - ck.x[self.b]) * self.g)
    }
    fn noise(&self, _ck: &Ckt, _f: f64, out: &mut Vec<NoiseSrc>) {
        if self.noisy {
            out.push(NoiseSrc {
                name: self.name.clone(),
                p: self.a,
                n: self.b,
                psd: 4.0 * KB * self.temp_k * self.g.abs(),
            });
        }
    }
    fn set_param(&mut self, name: &str, v: f64) -> bool {
        match name {
            "r" | "value" | "resistance" => self.r = v,
            "tc1" => self.tc1 = v,
            "tc2" => self.tc2 = v,
            "m" => self.m = v,
            _ => return false,
        }
        self.update();
        true
    }
}

#[derive(Debug, Clone)]
pub struct Capacitor {
    pub name: String,
    pub a: usize,
    pub b: usize,
    pub c_nom: f64,
    pub tc1: f64,
    pub tc2: f64,
    pub temp: Option<f64>,
    pub dtemp: f64,
    pub m: f64,
    pub ic: Option<f64>,
    c: f64,
    uic_v: f64,
    q: usize,
    s: G4,
    tnom: f64,
    temp_k: f64,
}

impl Capacitor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        a: usize,
        b: usize,
        c: f64,
        tc1: f64,
        tc2: f64,
        temp: Option<f64>,
        dtemp: f64,
        m: f64,
        ic: Option<f64>,
    ) -> Self {
        Capacitor {
            name,
            a,
            b,
            c_nom: c,
            tc1,
            tc2,
            temp,
            dtemp,
            m,
            ic,
            c: c * m,
            uic_v: 0.0,
            q: 0,
            s: G4::default(),
            tnom: 27.0,
            temp_k: 300.15,
        }
    }
    fn update(&mut self) {
        self.c = self.c_nom * self.m * tc_factor(self.tc1, self.tc2, self.temp_k, self.tnom);
    }
}

impl Dev for Capacitor {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_unknowns(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.q = s.states(2);
        Ok(())
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.s = s.g4(self.a, self.b);
        self.tnom = s.opts.tnom;
        Ok(())
    }
    fn temperature(&mut self, temp_k: f64, o: &SimOptions) {
        self.tnom = o.tnom;
        self.temp_k = inst_temp(temp_k, self.temp, self.dtemp);
        self.update();
    }
    fn load(&mut self, ck: &mut Ckt) {
        let v = if ck.uic && ck.init == Init::Tran {
            self.uic_v
        } else {
            ck.x[self.a] - ck.x[self.b]
        };
        ck.st[0][self.q] = self.c * v;
        if ck.mode != Mode::Tran {
            return;
        }
        if ck.init == Init::Tran {
            ck.st[1][self.q] = ck.st[0][self.q];
        }
        let (geq, ccap) = ck.integrate(self.q, self.c);
        if ck.init == Init::Tran {
            ck.st[1][self.q + 1] = ck.st[0][self.q + 1];
        }
        let ceq = ccap - geq * v;
        self.s.stamp(ck, geq);
        ck.add_rhs(self.a, -ceq);
        ck.add_rhs(self.b, ceq);
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        self.s.stamp_ac(ac, C64::new(0.0, ac.omega * self.c));
    }
    fn trunc(&self, ck: &Ckt) -> f64 {
        ck.trunc_q(self.q)
    }
    fn is_reactive(&self) -> bool {
        true
    }
    fn current(&self, ck: &Ckt) -> Option<f64> {
        Some(if ck.mode == Mode::Tran {
            ck.st[0][self.q + 1]
        } else {
            0.0
        })
    }
    fn init_uic(&mut self, ck: &mut Ckt) {
        self.uic_v = self.ic.unwrap_or(ck.x[self.a] - ck.x[self.b]);
        ck.st[0][self.q] = self.c * self.uic_v;
        ck.st[0][self.q + 1] = 0.0;
    }
    fn set_param(&mut self, name: &str, v: f64) -> bool {
        match name {
            "c" | "value" | "capacitance" => self.c_nom = v,
            "m" => self.m = v,
            _ => return false,
        }
        self.update();
        true
    }
}

#[derive(Debug, Clone)]
pub struct Inductor {
    pub name: String,
    pub a: usize,
    pub b: usize,
    pub l_nom: f64,
    pub tc1: f64,
    pub tc2: f64,
    pub temp: Option<f64>,
    pub dtemp: f64,
    pub m: f64,
    pub ic: Option<f64>,
    l: f64,
    uic_flux: f64,
    pub br: usize,
    flux: usize,
    s_ab: Slot,
    s_bb: Slot,
    s_ba: Slot,
    s_bbn: Slot,
    s_brbr: Slot,
    /// Mutual couplings: (other inductor branch, mutual inductance M, slot (br, other))
    pub mutuals: Vec<(String, f64, usize, Slot)>,
    tnom: f64,
    temp_k: f64,
}

impl Inductor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        a: usize,
        b: usize,
        l: f64,
        tc1: f64,
        tc2: f64,
        temp: Option<f64>,
        dtemp: f64,
        m: f64,
        ic: Option<f64>,
    ) -> Self {
        Inductor {
            name,
            a,
            b,
            l_nom: l,
            tc1,
            tc2,
            temp,
            dtemp,
            m,
            ic,
            l: l / m,
            uic_flux: 0.0,
            br: 0,
            flux: 0,
            s_ab: Slot::GROUND,
            s_bb: Slot::GROUND,
            s_ba: Slot::GROUND,
            s_bbn: Slot::GROUND,
            s_brbr: Slot::GROUND,
            mutuals: Vec::new(),
            tnom: 27.0,
            temp_k: 300.15,
        }
    }
    /// Inductance after temperature and multiplier.
    pub fn inductance(&self) -> f64 {
        self.l
    }
    fn update(&mut self) {
        self.l =
            self.l_nom / self.m.max(1e-30) * tc_factor(self.tc1, self.tc2, self.temp_k, self.tnom);
    }
    fn flux_now(&self, ck: &Ckt) -> f64 {
        let mut f = self.l * ck.x[self.br];
        for (_, m, br2, _) in &self.mutuals {
            f += m * ck.x[*br2];
        }
        f
    }
}

impl Dev for Inductor {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_unknowns(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.br = s.alloc(format!("{}#branch", self.name), UKind::Branch);
        self.flux = s.states(2);
        Ok(())
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.s_ab = s.slot(self.a, self.br);
        self.s_bb = s.slot(self.b, self.br);
        self.s_ba = s.slot(self.br, self.a);
        self.s_bbn = s.slot(self.br, self.b);
        self.s_brbr = s.slot(self.br, self.br);
        for m in self.mutuals.iter_mut() {
            m.3 = s.slot(self.br, m.2);
        }
        self.tnom = s.opts.tnom;
        Ok(())
    }
    fn temperature(&mut self, temp_k: f64, o: &SimOptions) {
        self.tnom = o.tnom;
        self.temp_k = inst_temp(temp_k, self.temp, self.dtemp);
        self.update();
    }
    fn branch(&self) -> Option<usize> {
        Some(self.br)
    }
    fn load(&mut self, ck: &mut Ckt) {
        ck.add(self.s_ab, 1.0);
        ck.add(self.s_bb, -1.0);
        ck.add(self.s_ba, 1.0);
        ck.add(self.s_bbn, -1.0);
        ck.st[0][self.flux] = if ck.uic && ck.init == Init::Tran {
            self.uic_flux
        } else {
            self.flux_now(ck)
        };
        if ck.mode != Mode::Tran {
            return;
        }
        if ck.init == Init::Tran {
            ck.st[1][self.flux] = ck.st[0][self.flux];
        }
        let (req, ccap) = ck.integrate(self.flux, self.l);
        if ck.init == Init::Tran {
            ck.st[1][self.flux + 1] = ck.st[0][self.flux + 1];
        }
        let ag0 = ck.ag[0];
        let veq = ccap - ag0 * ck.st[0][self.flux];
        ck.add(self.s_brbr, -req);
        for (_, m, _, slot) in &self.mutuals {
            ck.add(*slot, -ag0 * m);
        }
        ck.add_rhs(self.br, veq);
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        ac.add_re(self.s_ab, 1.0);
        ac.add_re(self.s_bb, -1.0);
        ac.add_re(self.s_ba, 1.0);
        ac.add_re(self.s_bbn, -1.0);
        ac.add_im(self.s_brbr, -ac.omega * self.l);
        for (_, m, _, slot) in &self.mutuals {
            ac.add_im(*slot, -ac.omega * m);
        }
    }
    fn trunc(&self, ck: &Ckt) -> f64 {
        ck.trunc_q(self.flux)
    }
    fn is_reactive(&self) -> bool {
        true
    }
    fn current(&self, ck: &Ckt) -> Option<f64> {
        Some(ck.x[self.br])
    }
    fn init_uic(&mut self, ck: &mut Ckt) {
        if let Some(ic) = self.ic {
            ck.x[self.br] = ic;
        }
        self.uic_flux = self.flux_now(ck);
        ck.st[0][self.flux] = self.uic_flux;
        ck.st[0][self.flux + 1] = 0.0;
    }
    fn set_param(&mut self, name: &str, v: f64) -> bool {
        match name {
            "l" | "value" | "inductance" => self.l_nom = v,
            "m" => self.m = v,
            _ => return false,
        }
        self.update();
        true
    }
}
