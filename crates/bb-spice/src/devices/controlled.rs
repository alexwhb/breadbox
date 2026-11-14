//! Linear controlled sources E, G, F, H (POLY / VALUE forms become B sources).

use super::*;

fn ctrl_branch(s: &Setup, owner: &str, ctrl: &str) -> Result<usize, SimError> {
    s.branch_of(ctrl).ok_or_else(|| {
        SimError::invalid(format!(
            "`{owner}`: controlling element `{ctrl}` does not exist or carries no branch current (use a V source, inductor, E or H)"
        ))
    })
}

/// E: `v(p) - v(n) = gain·(v(cp) - v(cn))`.
#[derive(Debug, Clone)]
pub struct Vcvs {
    pub name: String,
    pub nodes: [usize; 4],
    pub gain: f64,
    pub br: usize,
    s: [Slot; 6],
}

impl Vcvs {
    pub fn new(name: String, nodes: [usize; 4], gain: f64) -> Self {
        Vcvs {
            name,
            nodes,
            gain,
            br: 0,
            s: [Slot::GROUND; 6],
        }
    }
}

impl Dev for Vcvs {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_unknowns(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.br = s.alloc(format!("{}#branch", self.name), UKind::Branch);
        Ok(())
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        let [p, n, cp, cn] = self.nodes;
        let b = self.br;
        self.s = [
            s.slot(p, b),
            s.slot(n, b),
            s.slot(b, p),
            s.slot(b, n),
            s.slot(b, cp),
            s.slot(b, cn),
        ];
        Ok(())
    }
    fn branch(&self) -> Option<usize> {
        Some(self.br)
    }
    fn load(&mut self, ck: &mut Ckt) {
        ck.add(self.s[0], 1.0);
        ck.add(self.s[1], -1.0);
        ck.add(self.s[2], 1.0);
        ck.add(self.s[3], -1.0);
        ck.add(self.s[4], -self.gain);
        ck.add(self.s[5], self.gain);
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        ac.add_re(self.s[0], 1.0);
        ac.add_re(self.s[1], -1.0);
        ac.add_re(self.s[2], 1.0);
        ac.add_re(self.s[3], -1.0);
        ac.add_re(self.s[4], -self.gain);
        ac.add_re(self.s[5], self.gain);
    }
    fn current(&self, ck: &Ckt) -> Option<f64> {
        Some(ck.x[self.br])
    }
    fn set_param(&mut self, name: &str, v: f64) -> bool {
        if matches!(name, "gain" | "value") {
            self.gain = v;
            true
        } else {
            false
        }
    }
}

/// G: current `gm·(v(cp) - v(cn))` from `p` through the source to `n`.
#[derive(Debug, Clone)]
pub struct Vccs {
    pub name: String,
    pub nodes: [usize; 4],
    pub gm: f64,
    s: Gm4,
}

impl Vccs {
    pub fn new(name: String, nodes: [usize; 4], gm: f64) -> Self {
        Vccs {
            name,
            nodes,
            gm,
            s: Gm4::default(),
        }
    }
}

impl Dev for Vccs {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        let [p, n, cp, cn] = self.nodes;
        self.s = Gm4::new(s, p, n, cp, cn);
        Ok(())
    }
    fn load(&mut self, ck: &mut Ckt) {
        self.s.stamp(ck, self.gm);
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        self.s.stamp_ac(ac, C64::new(self.gm, 0.0));
    }
    fn current(&self, ck: &Ckt) -> Option<f64> {
        Some(self.gm * (ck.x[self.nodes[2]] - ck.x[self.nodes[3]]))
    }
    fn set_param(&mut self, name: &str, v: f64) -> bool {
        if matches!(name, "gain" | "value" | "gm") {
            self.gm = v;
            true
        } else {
            false
        }
    }
}

/// F: current `gain·i(ctrl)` from `p` through the source to `n`.
#[derive(Debug, Clone)]
pub struct Cccs {
    pub name: String,
    pub p: usize,
    pub n: usize,
    pub ctrl: String,
    pub gain: f64,
    cb: usize,
    s: [Slot; 2],
}

impl Cccs {
    pub fn new(name: String, p: usize, n: usize, ctrl: String, gain: f64) -> Self {
        Cccs {
            name,
            p,
            n,
            ctrl,
            gain,
            cb: 0,
            s: [Slot::GROUND; 2],
        }
    }
}

impl Dev for Cccs {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.cb = ctrl_branch(s, &self.name, &self.ctrl)?;
        self.s = [s.slot(self.p, self.cb), s.slot(self.n, self.cb)];
        Ok(())
    }
    fn load(&mut self, ck: &mut Ckt) {
        ck.add(self.s[0], self.gain);
        ck.add(self.s[1], -self.gain);
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        ac.add_re(self.s[0], self.gain);
        ac.add_re(self.s[1], -self.gain);
    }
    fn current(&self, ck: &Ckt) -> Option<f64> {
        Some(self.gain * ck.x[self.cb])
    }
    fn set_param(&mut self, name: &str, v: f64) -> bool {
        if matches!(name, "gain" | "value") {
            self.gain = v;
            true
        } else {
            false
        }
    }
}

/// H: `v(p) - v(n) = r·i(ctrl)`.
#[derive(Debug, Clone)]
pub struct Ccvs {
    pub name: String,
    pub p: usize,
    pub n: usize,
    pub ctrl: String,
    pub r: f64,
    pub br: usize,
    s: [Slot; 5],
}

impl Ccvs {
    pub fn new(name: String, p: usize, n: usize, ctrl: String, r: f64) -> Self {
        Ccvs {
            name,
            p,
            n,
            ctrl,
            r,
            br: 0,
            s: [Slot::GROUND; 5],
        }
    }
}

impl Dev for Ccvs {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_unknowns(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.br = s.alloc(format!("{}#branch", self.name), UKind::Branch);
        Ok(())
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        let cb = ctrl_branch(s, &self.name, &self.ctrl)?;
        let b = self.br;
        self.s = [
            s.slot(self.p, b),
            s.slot(self.n, b),
            s.slot(b, self.p),
            s.slot(b, self.n),
            s.slot(b, cb),
        ];
        Ok(())
    }
    fn branch(&self) -> Option<usize> {
        Some(self.br)
    }
    fn load(&mut self, ck: &mut Ckt) {
        ck.add(self.s[0], 1.0);
        ck.add(self.s[1], -1.0);
        ck.add(self.s[2], 1.0);
        ck.add(self.s[3], -1.0);
        ck.add(self.s[4], -self.r);
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        ac.add_re(self.s[0], 1.0);
        ac.add_re(self.s[1], -1.0);
        ac.add_re(self.s[2], 1.0);
        ac.add_re(self.s[3], -1.0);
        ac.add_re(self.s[4], -self.r);
    }
    fn current(&self, ck: &Ckt) -> Option<f64> {
        Some(ck.x[self.br])
    }
    fn set_param(&mut self, name: &str, v: f64) -> bool {
        if matches!(name, "gain" | "value") {
            self.r = v;
            true
        } else {
            false
        }
    }
}
