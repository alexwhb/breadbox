//! MOSFET level 1 (Shichman-Hodges, ngspice `mos1`): VTO, KP, GAMMA, PHI, LAMBDA, RD, RS, RSH,
//! CBD, CBS, IS, JS, PB, CJ, MJ, CJSW, MJSW, CGSO, CGDO, CGBO, TOX, NSUB, UO, LD, FC — with body
//! effect, channel-length modulation, temperature scaling, bulk junction diodes/capacitances,
//! Meyer gate capacitances and channel/resistor thermal noise. Other levels fall back to this
//! model (with a warning issued during elaboration).

use super::semi::{fetlim, limvds, pnjlim, vcrit};
use super::*;

const KNOWN: &[&str] = &[
    "vto", "vt0", "kp", "gamma", "phi", "lambda", "rd", "rs", "cbd", "cbs", "is", "pb", "cgso",
    "cgdo", "cgbo", "rsh", "cj", "mj", "cjsw", "mjsw", "js", "tox", "nsub", "nss", "tpg", "ld",
    "uo", "u0", "fc", "tnom", "tref", "level", "xj", "neff", "ucrit", "uexp", "utra", "vmax",
    "delta", "theta", "eta", "kappa", "nfs", "xqc",
];

#[derive(Debug, Clone, PartialEq)]
pub struct MosModel {
    pub vto: f64,
    pub kp: f64,
    pub gamma: f64,
    pub phi: f64,
    pub lambda: f64,
    pub rd: Option<f64>,
    pub rs: Option<f64>,
    pub rsh: Option<f64>,
    pub cbd: Option<f64>,
    pub cbs: Option<f64>,
    pub is: f64,
    pub js: f64,
    pub pb: f64,
    pub cj: Option<f64>,
    pub mj: f64,
    pub cjsw: Option<f64>,
    pub mjsw: f64,
    pub cgso: f64,
    pub cgdo: f64,
    pub cgbo: f64,
    pub ld: f64,
    pub fc: f64,
    pub cox: f64,
    pub tnom: Option<f64>,
}

impl MosModel {
    pub fn from_params(p: &ParamMap, pmos: bool) -> (MosModel, Vec<String>) {
        let ty = if pmos { -1.0 } else { 1.0 };
        let tox = p.get(&["tox"]).filter(|t| *t > 0.0);
        let cox = tox.map(|t| 3.9 * 8.854214871e-12 / t).unwrap_or(0.0);
        let mut kp = p.get(&["kp"]);
        let mut phi = p.get(&["phi"]);
        let mut gamma = p.get(&["gamma"]);
        let mut vto = p.get(&["vto", "vt0"]);
        if cox > 0.0 {
            if kp.is_none() {
                let uo = p.or(&["uo", "u0"], 600.0);
                kp = Some(uo * cox * 1e-4);
            }
            if let Some(nsub) = p.get(&["nsub"]) {
                // derive phi, gamma, vto from doping (SPICE MOS1temp)
                let ni = 1.45e16;
                let eps_si = 11.7 * 8.854214871e-12;
                let vtnom = KB * (p.or(&["tnom"], 27.0) + KELVIN) / Q;
                let nsub_m3 = nsub * 1e6;
                if nsub_m3 > ni {
                    if phi.is_none() {
                        phi = Some((2.0 * vtnom * (nsub_m3 / ni).ln()).max(0.1));
                    }
                    let fermis = ty * 0.5 * phi.unwrap();
                    let mut wkfng = 3.2;
                    let tpg = p.or(&["tpg"], 1.0);
                    if tpg != 0.0 {
                        let fermig = ty * tpg * 0.5 * super::semi::egap(300.15);
                        wkfng = 3.25 + 0.5 * super::semi::egap(300.15) - fermig;
                    }
                    let wkfngs = wkfng - (3.25 + 0.5 * super::semi::egap(300.15) + fermis);
                    if gamma.is_none() {
                        gamma = Some((2.0 * eps_si * Q * nsub_m3).sqrt() / cox);
                    }
                    if vto.is_none() {
                        let nss = p.or(&["nss"], 0.0) * 1e4;
                        let vfb = wkfngs - nss * Q / cox;
                        vto =
                            Some(vfb + ty * (gamma.unwrap() * phi.unwrap().sqrt() + phi.unwrap()));
                    }
                }
            }
        }
        let m = MosModel {
            vto: vto.unwrap_or(0.0),
            kp: kp.unwrap_or(2e-5),
            gamma: gamma.unwrap_or(0.0),
            phi: phi.unwrap_or(0.6),
            lambda: p.or(&["lambda"], 0.0),
            rd: p.get(&["rd"]),
            rs: p.get(&["rs"]),
            rsh: p.get(&["rsh"]),
            cbd: p.get(&["cbd"]),
            cbs: p.get(&["cbs"]),
            is: p.or(&["is"], 1e-14),
            js: p.or(&["js"], 0.0),
            pb: p.or(&["pb"], 0.8),
            cj: p.get(&["cj"]),
            mj: p.or(&["mj"], 0.5),
            cjsw: p.get(&["cjsw"]),
            mjsw: p.or(&["mjsw"], 0.5),
            cgso: p.or(&["cgso"], 0.0),
            cgdo: p.or(&["cgdo"], 0.0),
            cgbo: p.or(&["cgbo"], 0.0),
            ld: p.or(&["ld"], 0.0),
            fc: p.or(&["fc"], 0.5),
            cox,
            tnom: p.get(&["tnom", "tref"]),
        };
        (m, p.unknown(KNOWN))
    }
}

/// Meyer gate capacitances (halved, SPICE `DEVqmeyer`): returns (capgs, capgd, capgb).
pub fn qmeyer(
    vgs: f64,
    vgd: f64,
    _vgb: f64,
    von: f64,
    vdsat: f64,
    phi: f64,
    cox: f64,
) -> (f64, f64, f64) {
    let vgst = vgs - von;
    let vdsat = vdsat.max(1e-28);
    if vgst <= -phi {
        (0.0, 0.0, cox / 2.0)
    } else if vgst <= -phi / 2.0 {
        (0.0, 0.0, -vgst * cox / (2.0 * phi))
    } else if vgst <= 0.0 {
        (
            vgst * cox / (1.5 * phi) + cox / 3.0,
            0.0,
            -vgst * cox / (2.0 * phi),
        )
    } else {
        let vds = vgs - vgd;
        if vdsat <= vds {
            (cox / 3.0, 0.0, 0.0)
        } else {
            let vddif = 2.0 * vdsat - vds;
            let vddif1 = vdsat - vds;
            let vddif2 = vddif * vddif;
            (
                cox * (1.0 - vddif1 * vddif1 / vddif2) / 3.0,
                cox * (1.0 - vdsat * vdsat / vddif2) / 3.0,
                0.0,
            )
        }
    }
}

// state offsets
const VBD: usize = 0;
const VBS: usize = 1;
const VGS: usize = 2;
const VDS: usize = 3;
const CAPGS: usize = 4;
const QGS: usize = 5; // +1 current
const CAPGD: usize = 7;
const QGD: usize = 8;
const CAPGB: usize = 10;
const QGB: usize = 11;
const QBD: usize = 13;
const QBS: usize = 15;
const NSTATES: usize = 17;

#[derive(Debug, Clone, Default)]
struct Slots {
    dd: Slot,
    gg: Slot,
    ss: Slot,
    bb: Slot,
    dpdp: Slot,
    spsp: Slot,
    ddp: Slot,
    gb: Slot,
    gdp: Slot,
    gsp: Slot,
    ssp: Slot,
    bdp: Slot,
    bsp: Slot,
    dpsp: Slot,
    dpd: Slot,
    bg: Slot,
    dpg: Slot,
    spg: Slot,
    sps: Slot,
    dpb: Slot,
    spb: Slot,
    spdp: Slot,
}

#[derive(Debug, Clone, Default)]
struct OpVals {
    mode: f64,
    cd: f64,
    cbs: f64,
    cbd: f64,
    gm: f64,
    gds: f64,
    gmbs: f64,
    gbd: f64,
    gbs: f64,
    capbd: f64,
    capbs: f64,
    von: f64,
    vdsat: f64,
    // totals including capacitor companions (for convergence test)
    t_cd: f64,
    t_cbs: f64,
    t_cbd: f64,
    t_gbd: f64,
    t_gbs: f64,
}

#[derive(Debug, Clone)]
pub struct Mos1 {
    pub name: String,
    /// d, g, s, b
    pub nodes: [usize; 4],
    dp: usize,
    sp: usize,
    pub sign: f64,
    pub model: MosModel,
    pub params: ParamMap,
    pub off: bool,
    l: f64,
    w: f64,
    ad: f64,
    as_: f64,
    pd: f64,
    ps: f64,
    nrd: f64,
    nrs: f64,
    m: f64,
    ic: [f64; 3],
    temp: Option<f64>,
    dtemp: f64,
    // temperature-adjusted
    t_kp: f64,
    t_phi: f64,
    t_vbi: f64,
    t_vto: f64,
    t_is: f64,
    t_js: f64,
    t_pb: f64,
    t_cbd: f64,
    t_cbs: f64,
    t_cj: f64,
    t_cjsw: f64,
    t_depcap: f64,
    czbd: f64,
    czbdsw: f64,
    czbs: f64,
    czbssw: f64,
    f2d: f64,
    f3d: f64,
    f4d: f64,
    f2s: f64,
    f3s: f64,
    f4s: f64,
    gdpr: f64,
    gspr: f64,
    vt: f64,
    temp_k: f64,
    circuit_k: f64,
    opt_tnom: f64,
    st: usize,
    s: Slots,
    op: OpVals,
}

impl Mos1 {
    pub fn new(
        name: String,
        nodes: [usize; 4],
        pmos: bool,
        params: ParamMap,
        inst: ParamMap,
        off: bool,
        o: &SimOptions,
    ) -> (Self, Vec<String>) {
        let (model, mut warn) = MosModel::from_params(&params, pmos);
        let known_inst = [
            "l", "w", "ad", "as", "pd", "ps", "nrd", "nrs", "m", "ic", "ic.1", "ic.2", "temp",
            "dtemp", "off",
        ];
        for (k, _) in &inst.vals {
            if !known_inst.contains(&k.as_str()) {
                warn.push(format!("instance {k}"));
            }
        }
        let g = |k: &str, d: f64| inst.get(&[k]).unwrap_or(d);
        let mut d = Mos1 {
            name,
            nodes,
            dp: nodes[0],
            sp: nodes[2],
            sign: if pmos { -1.0 } else { 1.0 },
            model,
            params,
            off,
            l: g("l", o.defl),
            w: g("w", o.defw),
            ad: g("ad", 0.0),
            as_: g("as", 0.0),
            pd: g("pd", 0.0),
            ps: g("ps", 0.0),
            nrd: g("nrd", 1.0),
            nrs: g("nrs", 1.0),
            m: g("m", 1.0),
            ic: [g("ic", 0.0), g("ic.1", 0.0), g("ic.2", 0.0)],
            temp: inst.get(&["temp"]),
            dtemp: g("dtemp", 0.0),
            t_kp: 0.0,
            t_phi: 0.0,
            t_vbi: 0.0,
            t_vto: 0.0,
            t_is: 0.0,
            t_js: 0.0,
            t_pb: 0.0,
            t_cbd: 0.0,
            t_cbs: 0.0,
            t_cj: 0.0,
            t_cjsw: 0.0,
            t_depcap: 0.0,
            czbd: 0.0,
            czbdsw: 0.0,
            czbs: 0.0,
            czbssw: 0.0,
            f2d: 0.0,
            f3d: 0.0,
            f4d: 0.0,
            f2s: 0.0,
            f3s: 0.0,
            f4s: 0.0,
            gdpr: 0.0,
            gspr: 0.0,
            vt: 0.0,
            temp_k: 300.15,
            circuit_k: 300.15,
            opt_tnom: 27.0,
            st: 0,
            s: Slots::default(),
            op: OpVals::default(),
        };
        d.resistances();
        (d, warn)
    }

    fn resistances(&mut self) {
        let md = &self.model;
        self.gdpr = match (md.rd, md.rsh) {
            (Some(rd), _) if rd != 0.0 => self.m / rd,
            (None, Some(rsh)) if rsh != 0.0 && self.nrd != 0.0 => self.m / (rsh * self.nrd),
            _ => 0.0,
        };
        self.gspr = match (md.rs, md.rsh) {
            (Some(rs), _) if rs != 0.0 => self.m / rs,
            (None, Some(rsh)) if rsh != 0.0 && self.nrs != 0.0 => self.m / (rsh * self.nrs),
            _ => 0.0,
        };
    }

    fn do_temp(&mut self, circuit_k: f64, opt_tnom: f64) {
        self.circuit_k = circuit_k;
        self.opt_tnom = opt_tnom;
        let md = &self.model;
        let ty = self.sign;
        let tnom = md.tnom.unwrap_or(opt_tnom) + KELVIN;
        let t = self.temp.map(|t| t + KELVIN).unwrap_or(circuit_k) + self.dtemp;
        self.temp_k = t;
        const REFT: f64 = 300.15;
        let vt = KB * t / Q;
        self.vt = vt;
        let ratio = t / tnom;
        let fact2 = t / REFT;
        let kt = KB * t;
        let egfet = 1.16 - (7.02e-4 * t * t) / (t + 1108.0);
        let arg = -egfet / (kt + kt) + 1.1150877 / (KB * (REFT + REFT));
        let pbfact = -2.0 * vt * (1.5 * fact2.ln() + Q * arg);
        let fact1 = tnom / REFT;
        let vtnom = tnom * KB / Q;
        let kt1 = KB * tnom;
        let egfet1 = 1.16 - (7.02e-4 * tnom * tnom) / (tnom + 1108.0);
        let arg1 = -egfet1 / (kt1 + kt1) + 1.1150877 / (KB * (REFT + REFT));
        let pbfact1 = -2.0 * vtnom * (1.5 * fact1.ln() + Q * arg1);
        let ratio4 = ratio * ratio.sqrt();
        self.t_kp = md.kp / ratio4;
        let phio = (md.phi - pbfact1) / fact1;
        self.t_phi = fact2 * phio + pbfact;
        self.t_vbi = md.vto - ty * (md.gamma * md.phi.sqrt())
            + 0.5 * (egfet1 - egfet)
            + ty * 0.5 * (self.t_phi - md.phi);
        self.t_vto = self.t_vbi + ty * md.gamma * self.t_phi.sqrt();
        let isfact = (-egfet / vt + egfet1 / vtnom).exp();
        self.t_is = md.is * isfact;
        self.t_js = md.js * isfact;
        let pbo = (md.pb - pbfact1) / fact1;
        let gmaold = (md.pb - pbo) / pbo;
        let mut capfact = 1.0 / (1.0 + md.mj * (4e-4 * (tnom - REFT) - gmaold));
        self.t_cbd = md.cbd.unwrap_or(0.0) * capfact;
        self.t_cbs = md.cbs.unwrap_or(0.0) * capfact;
        self.t_cj = md.cj.unwrap_or(0.0) * capfact;
        capfact = 1.0 / (1.0 + md.mjsw * (4e-4 * (tnom - REFT) - gmaold));
        self.t_cjsw = md.cjsw.unwrap_or(0.0) * capfact;
        self.t_pb = fact2 * pbo + pbfact;
        let gmanew = (self.t_pb - pbo) / pbo;
        capfact = 1.0 + md.mj * (4e-4 * (t - REFT) - gmanew);
        self.t_cbd *= capfact;
        self.t_cbs *= capfact;
        self.t_cj *= capfact;
        capfact = 1.0 + md.mjsw * (4e-4 * (t - REFT) - gmanew);
        self.t_cjsw *= capfact;
        self.t_depcap = md.fc * self.t_pb;
        // junction capacitance coefficients
        let m = self.m;
        self.czbd = if md.cbd.is_some() {
            self.t_cbd * m
        } else if md.cj.is_some() {
            self.t_cj * m * self.ad
        } else {
            0.0
        };
        self.czbdsw = if md.cjsw.is_some() {
            self.t_cjsw * self.pd * m
        } else {
            0.0
        };
        self.czbs = if md.cbs.is_some() {
            self.t_cbs * m
        } else if md.cj.is_some() {
            self.t_cj * m * self.as_
        } else {
            0.0
        };
        self.czbssw = if md.cjsw.is_some() {
            self.t_cjsw * self.ps * m
        } else {
            0.0
        };
        let arg = 1.0 - md.fc;
        let sarg = (-md.mj * arg.ln()).exp();
        let sargsw = (-md.mjsw * arg.ln()).exp();
        let pb = self.t_pb;
        let dep = self.t_depcap;
        let coeffs = |cz: f64, czsw: f64| {
            let f2 = cz * (1.0 - md.fc * (1.0 + md.mj)) * sarg / arg
                + czsw * (1.0 - md.fc * (1.0 + md.mjsw)) * sargsw / arg;
            let f3 = cz * md.mj * sarg / arg / pb + czsw * md.mjsw * sargsw / arg / pb;
            let f4 = cz * pb * (1.0 - arg * sarg) / (1.0 - md.mj)
                + czsw * pb * (1.0 - arg * sargsw) / (1.0 - md.mjsw)
                - f3 / 2.0 * (dep * dep)
                - dep * f2;
            (f2, f3, f4)
        };
        (self.f2d, self.f3d, self.f4d) = coeffs(self.czbd, self.czbdsw);
        (self.f2s, self.f3s, self.f4s) = coeffs(self.czbs, self.czbssw);
    }

    fn sat_currents(&self) -> (f64, f64) {
        if self.t_js == 0.0 || self.ad == 0.0 || self.as_ == 0.0 {
            (self.m * self.t_is, self.m * self.t_is)
        } else {
            (self.m * self.t_js * self.ad, self.m * self.t_js * self.as_)
        }
    }

    fn jcharge(&self, v: f64, cz: f64, czsw: f64, f: (f64, f64, f64)) -> (f64, f64) {
        if cz == 0.0 && czsw == 0.0 {
            return (0.0, 0.0);
        }
        let md = &self.model;
        if v < self.t_depcap {
            let arg = 1.0 - v / self.t_pb;
            let sarg = (-md.mj * arg.ln()).exp();
            let sargsw = (-md.mjsw * arg.ln()).exp();
            let q = self.t_pb
                * (cz * (1.0 - arg * sarg) / (1.0 - md.mj)
                    + czsw * (1.0 - arg * sargsw) / (1.0 - md.mjsw));
            (q, cz * sarg + czsw * sargsw)
        } else {
            (f.2 + v * (f.0 + v * (f.1 / 2.0)), f.0 + f.1 * v)
        }
    }

    fn eff_len(&self) -> f64 {
        self.l - 2.0 * self.model.ld
    }

    fn overlaps(&self) -> (f64, f64, f64) {
        let md = &self.model;
        (
            md.cgso * self.m * self.w,
            md.cgdo * self.m * self.w,
            md.cgbo * self.m * self.eff_len(),
        )
    }

    fn has_caps(&self) -> bool {
        let md = &self.model;
        md.cox > 0.0
            || md.cgso > 0.0
            || md.cgdo > 0.0
            || md.cgbo > 0.0
            || self.czbd > 0.0
            || self.czbs > 0.0
            || self.czbdsw > 0.0
            || self.czbssw > 0.0
    }
}

fn bulk_diode(v: f64, isat: f64, vt: f64, gmin: f64) -> (f64, f64) {
    if v <= -3.0 * vt {
        let arg = 3.0 * vt / (v * std::f64::consts::E);
        let arg = arg * arg * arg;
        (-isat * (1.0 + arg) + gmin * v, isat * 3.0 * arg / v + gmin)
    } else {
        let e = (v / vt).min(709.0).exp();
        (isat * (e - 1.0) + gmin * v, isat * e / vt + gmin)
    }
}

impl Dev for Mos1 {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_unknowns(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.dp = if self.gdpr > 0.0 {
            s.alloc(format!("{}#drain", self.name), UKind::Node)
        } else {
            self.nodes[0]
        };
        self.sp = if self.gspr > 0.0 {
            s.alloc(format!("{}#source", self.name), UKind::Node)
        } else {
            self.nodes[2]
        };
        self.st = s.states(NSTATES);
        Ok(())
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        let [d, g, so, b] = self.nodes;
        let (dp, sp) = (self.dp, self.sp);
        self.s = Slots {
            dd: s.slot(d, d),
            gg: s.slot(g, g),
            ss: s.slot(so, so),
            bb: s.slot(b, b),
            dpdp: s.slot(dp, dp),
            spsp: s.slot(sp, sp),
            ddp: s.slot(d, dp),
            gb: s.slot(g, b),
            gdp: s.slot(g, dp),
            gsp: s.slot(g, sp),
            ssp: s.slot(so, sp),
            bdp: s.slot(b, dp),
            bsp: s.slot(b, sp),
            dpsp: s.slot(dp, sp),
            dpd: s.slot(dp, d),
            bg: s.slot(b, g),
            dpg: s.slot(dp, g),
            spg: s.slot(sp, g),
            sps: s.slot(sp, so),
            dpb: s.slot(dp, b),
            spb: s.slot(sp, b),
            spdp: s.slot(sp, dp),
        };
        if self.eff_len() <= 0.0 {
            return Err(SimError::invalid(format!(
                "`{}`: effective channel length L-2·LD is not positive",
                self.name
            )));
        }
        self.do_temp(s.opts.temp + KELVIN, s.opts.tnom);
        Ok(())
    }
    fn temperature(&mut self, temp_k: f64, o: &SimOptions) {
        self.do_temp(temp_k, o.tnom);
    }
    fn is_nonlinear(&self) -> bool {
        true
    }
    fn is_reactive(&self) -> bool {
        self.has_caps()
    }
    fn load(&mut self, ck: &mut Ckt) {
        let md = self.model.clone();
        let ty = self.sign;
        let s0 = self.st;
        let vt = self.vt;
        let [_d, g, _so, b] = self.nodes;
        let (dp, sp) = (self.dp, self.sp);
        let leff = self.eff_len();
        let (drain_isat, source_isat) = self.sat_currents();
        let (cgso, cgdo, cgbo) = self.overlaps();
        let beta = self.t_kp * self.m * self.w / leff;
        let oxide_cap = md.cox * leff * self.m * self.w;
        let mut check = false;
        let (mut vbs, mut vgs, mut vds);
        let normal = matches!(ck.init, Init::Float | Init::Pred | Init::Tran)
            || (ck.init == Init::Fix && !self.off);
        if normal {
            if ck.init == Init::Pred || ck.init == Init::Tran {
                if ck.init == Init::Pred {
                    for k in [VBS, VGS, VDS] {
                        ck.st[0][s0 + k] = ck.st[1][s0 + k];
                    }
                    vbs = ck.predict(s0 + VBS);
                    vgs = ck.predict(s0 + VGS);
                    vds = ck.predict(s0 + VDS);
                    ck.st[0][s0 + VBD] = ck.st[0][s0 + VBS] - ck.st[0][s0 + VDS];
                } else {
                    vbs = ck.st[1][s0 + VBS];
                    vgs = ck.st[1][s0 + VGS];
                    vds = ck.st[1][s0 + VDS];
                }
            } else {
                vbs = ty * (ck.x[b] - ck.x[sp]);
                vgs = ty * (ck.x[g] - ck.x[sp]);
                vds = ty * (ck.x[dp] - ck.x[sp]);
            }
            let mut vgd = vgs - vds;
            let vgdo = ck.st[0][s0 + VGS] - ck.st[0][s0 + VDS];
            let von = ty * self.op.von;
            if ck.st[0][s0 + VDS] >= 0.0 {
                vgs = fetlim(vgs, ck.st[0][s0 + VGS], von);
                vds = vgs - vgd;
                vds = limvds(vds, ck.st[0][s0 + VDS]);
                vgd = vgs - vds;
            } else {
                vgd = fetlim(vgd, vgdo, von);
                vds = vgs - vgd;
                vds = -limvds(-vds, -ck.st[0][s0 + VDS]);
                vgs = vgd + vds;
            }
            let _ = vgd;
            if vds >= 0.0 {
                let (v, l) = pnjlim(
                    vbs,
                    ck.st[0][s0 + VBS],
                    vt,
                    vcrit(vt, source_isat.max(1e-300)),
                );
                vbs = v;
                check = l;
            } else {
                let vbd0 = vbs - vds;
                let (v, l) = pnjlim(
                    vbd0,
                    ck.st[0][s0 + VBD],
                    vt,
                    vcrit(vt, drain_isat.max(1e-300)),
                );
                vbs = v + vds;
                check = l;
            }
        } else if ck.init == Init::Jct && !self.off {
            vds = ty * self.ic[0];
            vgs = ty * self.ic[1];
            vbs = ty * self.ic[2];
            if vds == 0.0 && vgs == 0.0 && vbs == 0.0 && !(ck.tran_op && ck.uic) {
                vbs = -1.0;
                vgs = ty * self.t_vto;
                vds = 0.0;
            }
        } else {
            vbs = 0.0;
            vgs = 0.0;
            vds = 0.0;
        }
        let vbd = vbs - vds;
        let vgd = vgs - vds;
        let vgb = vgs - vbs;

        let (mut cbs, mut gbs) = bulk_diode(vbs, source_isat, vt, ck.gmin);
        let (mut cbd, mut gbd) = bulk_diode(vbd, drain_isat, vt, ck.gmin);
        let mode = if vds >= 0.0 { 1.0 } else { -1.0 };

        // drain current
        let vbsvbd = if mode > 0.0 { vbs } else { vbd };
        let sarg = if vbsvbd <= 0.0 {
            (self.t_phi - vbsvbd).sqrt()
        } else {
            let s = self.t_phi.sqrt();
            (s - vbsvbd / (s + s)).max(0.0)
        };
        let von = self.t_vbi * ty + md.gamma * sarg;
        let vgst = (if mode > 0.0 { vgs } else { vgd }) - von;
        let vdsat = vgst.max(0.0);
        let arg = if sarg <= 0.0 {
            0.0
        } else {
            md.gamma / (sarg + sarg)
        };
        let (cdrain, gm, gds, gmbs);
        if vgst <= 0.0 {
            cdrain = 0.0;
            gm = 0.0;
            gds = 0.0;
            gmbs = 0.0;
        } else {
            let vdsm = vds * mode;
            let betap = beta * (1.0 + md.lambda * vdsm);
            if vgst <= vdsm {
                cdrain = betap * vgst * vgst * 0.5;
                gm = betap * vgst;
                gds = md.lambda * beta * vgst * vgst * 0.5;
                gmbs = gm * arg;
            } else {
                cdrain = betap * vdsm * (vgst - 0.5 * vdsm);
                gm = betap * vdsm;
                gds = betap * (vgst - vdsm) + md.lambda * beta * vdsm * (vgst - 0.5 * vdsm);
                gmbs = gm * arg;
            }
        }
        self.op.von = ty * von;
        self.op.vdsat = ty * vdsat;
        let mut cd = mode * cdrain - cbd;
        self.op.mode = mode;
        self.op.cd = cd;
        self.op.cbs = cbs;
        self.op.cbd = cbd;
        self.op.gm = gm;
        self.op.gds = gds;
        self.op.gmbs = gmbs;
        self.op.gbd = gbd;
        self.op.gbs = gbs;

        let caps = self.has_caps();
        if caps {
            let (qbs, capbs) =
                self.jcharge(vbs, self.czbs, self.czbssw, (self.f2s, self.f3s, self.f4s));
            let (qbd, capbd) =
                self.jcharge(vbd, self.czbd, self.czbdsw, (self.f2d, self.f3d, self.f4d));
            ck.st[0][s0 + QBS] = qbs;
            ck.st[0][s0 + QBD] = qbd;
            self.op.capbs = capbs;
            self.op.capbd = capbd;
            if ck.mode == Mode::Tran {
                if ck.init == Init::Tran {
                    ck.st[1][s0 + QBS] = qbs;
                    ck.st[1][s0 + QBD] = qbd;
                }
                let (geq, ccap) = ck.integrate(s0 + QBD, capbd);
                gbd += geq;
                cbd += ccap;
                cd -= ccap;
                let (geq, ccap) = ck.integrate(s0 + QBS, capbs);
                gbs += geq;
                cbs += ccap;
                if ck.init == Init::Tran {
                    ck.st[1][s0 + QBS + 1] = ck.st[0][s0 + QBS + 1];
                    ck.st[1][s0 + QBD + 1] = ck.st[0][s0 + QBD + 1];
                }
            }
        }
        if check && (!self.off || ck.init != Init::Fix) {
            ck.nonconverged();
        }
        ck.st[0][s0 + VBS] = vbs;
        ck.st[0][s0 + VBD] = vbd;
        ck.st[0][s0 + VGS] = vgs;
        ck.st[0][s0 + VDS] = vds;

        // Meyer gate capacitances
        let (mut gcgs, mut ceqgs, mut gcgd, mut ceqgd, mut gcgb, mut ceqgb) =
            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        if caps {
            let (cgs_h, cgd_h, cgb_h) = if mode > 0.0 {
                qmeyer(vgs, vgd, vgb, von, vdsat, self.t_phi, oxide_cap)
            } else {
                let (a, b2, c) = qmeyer(vgd, vgs, vgb, von, vdsat, self.t_phi, oxide_cap);
                (b2, a, c)
            };
            ck.st[0][s0 + CAPGS] = cgs_h;
            ck.st[0][s0 + CAPGD] = cgd_h;
            ck.st[0][s0 + CAPGB] = cgb_h;
            let vgs1 = ck.st[1][s0 + VGS];
            let vgd1 = vgs1 - ck.st[1][s0 + VDS];
            let vgb1 = vgs1 - ck.st[1][s0 + VBS];
            let (capgs, capgd, capgb) = if ck.mode == Mode::Dc {
                (2.0 * cgs_h + cgso, 2.0 * cgd_h + cgdo, 2.0 * cgb_h + cgbo)
            } else {
                (
                    cgs_h + ck.st[1][s0 + CAPGS] + cgso,
                    cgd_h + ck.st[1][s0 + CAPGD] + cgdo,
                    cgb_h + ck.st[1][s0 + CAPGB] + cgbo,
                )
            };
            if ck.init == Init::Pred || ck.init == Init::Tran {
                let xfact = ck.delta / ck.delta_old[1];
                for q in [QGS, QGD, QGB] {
                    ck.st[0][s0 + q] = (1.0 + xfact) * ck.st[1][s0 + q] - xfact * ck.st[2][s0 + q];
                }
            } else if ck.mode == Mode::Tran {
                ck.st[0][s0 + QGS] = (vgs - vgs1) * capgs + ck.st[1][s0 + QGS];
                ck.st[0][s0 + QGD] = (vgd - vgd1) * capgd + ck.st[1][s0 + QGD];
                ck.st[0][s0 + QGB] = (vgb - vgb1) * capgb + ck.st[1][s0 + QGB];
            } else {
                ck.st[0][s0 + QGS] = vgs * capgs;
                ck.st[0][s0 + QGD] = vgd * capgd;
                ck.st[0][s0 + QGB] = vgb * capgb;
            }
            if ck.init != Init::Tran && ck.mode == Mode::Tran {
                if capgs == 0.0 {
                    ck.st[0][s0 + QGS + 1] = 0.0;
                }
                if capgd == 0.0 {
                    ck.st[0][s0 + QGD + 1] = 0.0;
                }
                if capgb == 0.0 {
                    ck.st[0][s0 + QGB + 1] = 0.0;
                }
                let (g1, c1) = ck.integrate(s0 + QGS, capgs);
                let (g2, c2) = ck.integrate(s0 + QGD, capgd);
                let (g3, c3) = ck.integrate(s0 + QGB, capgb);
                gcgs = g1;
                gcgd = g2;
                gcgb = g3;
                ceqgs = c1 - g1 * vgs;
                ceqgd = c2 - g2 * vgd;
                ceqgb = c3 - g3 * vgb;
            }
        }
        self.op.t_cd = cd;
        self.op.t_cbs = cbs;
        self.op.t_cbd = cbd;
        self.op.t_gbd = gbd;
        self.op.t_gbs = gbs;

        // right-hand side
        let ceqbs = ty * (cbs - gbs * vbs);
        let ceqbd = ty * (cbd - gbd * vbd);
        let (xnrm, xrev, cdreq) = if mode >= 0.0 {
            (1.0, 0.0, ty * (cdrain - gds * vds - gm * vgs - gmbs * vbs))
        } else {
            (
                0.0,
                1.0,
                -ty * (cdrain - gds * (-vds) - gm * vgd - gmbs * vbd),
            )
        };
        ck.add_rhs(g, -ty * (ceqgs + ceqgb + ceqgd));
        ck.add_rhs(b, -(ceqbs + ceqbd - ty * ceqgb));
        ck.add_rhs(dp, ceqbd - cdreq + ty * ceqgd);
        ck.add_rhs(sp, cdreq + ceqbs + ty * ceqgs);
        let s = &self.s;
        let (gdpr, gspr) = (self.gdpr, self.gspr);
        ck.add(s.dd, gdpr);
        ck.add(s.gg, gcgd + gcgs + gcgb);
        ck.add(s.ss, gspr);
        ck.add(s.bb, gbd + gbs + gcgb);
        ck.add(s.dpdp, gdpr + gds + gbd + xrev * (gm + gmbs) + gcgd);
        ck.add(s.spsp, gspr + gds + gbs + xnrm * (gm + gmbs) + gcgs);
        ck.add(s.ddp, -gdpr);
        ck.add(s.gb, -gcgb);
        ck.add(s.gdp, -gcgd);
        ck.add(s.gsp, -gcgs);
        ck.add(s.ssp, -gspr);
        ck.add(s.bg, -gcgb);
        ck.add(s.bdp, -gbd);
        ck.add(s.bsp, -gbs);
        ck.add(s.dpd, -gdpr);
        ck.add(s.dpg, (xnrm - xrev) * gm - gcgd);
        ck.add(s.dpb, -gbd + (xnrm - xrev) * gmbs);
        ck.add(s.dpsp, -gds - xnrm * (gm + gmbs));
        ck.add(s.spg, -(xnrm - xrev) * gm - gcgs);
        ck.add(s.sps, -gspr);
        ck.add(s.spb, -gbs - (xnrm - xrev) * gmbs);
        ck.add(s.spdp, -gds - xrev * (gm + gmbs));
    }
    fn conv_ok(&self, ck: &Ckt) -> bool {
        let ty = self.sign;
        let s0 = self.st;
        let [_d, g, _so, b] = self.nodes;
        let vbs = ty * (ck.x[b] - ck.x[self.sp]);
        let vgs = ty * (ck.x[g] - ck.x[self.sp]);
        let vds = ty * (ck.x[self.dp] - ck.x[self.sp]);
        let vbd = vbs - vds;
        let vgd = vgs - vds;
        let vgdo = ck.st[0][s0 + VGS] - ck.st[0][s0 + VDS];
        let delvbs = vbs - ck.st[0][s0 + VBS];
        let delvbd = vbd - ck.st[0][s0 + VBD];
        let delvgs = vgs - ck.st[0][s0 + VGS];
        let delvds = vds - ck.st[0][s0 + VDS];
        let delvgd = vgd - vgdo;
        let o = &self.op;
        let cdhat = if o.mode >= 0.0 {
            o.t_cd - o.t_gbd * delvbd + o.gmbs * delvbs + o.gm * delvgs + o.gds * delvds
        } else {
            o.t_cd - (o.t_gbd - o.gmbs) * delvbd - o.gm * delvgd + o.gds * delvds
        };
        let cbhat = o.t_cbs + o.t_cbd + o.t_gbd * delvbd + o.t_gbs * delvbs;
        let opts = &ck.opts;
        let tol = opts.reltol * cdhat.abs().max(o.t_cd.abs()) + opts.abstol;
        if (cdhat - o.t_cd).abs() >= tol {
            return false;
        }
        let cb = o.t_cbs + o.t_cbd;
        let tol = opts.reltol * cbhat.abs().max(cb.abs()) + opts.abstol;
        (cbhat - cb).abs() <= tol
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        let o = &self.op;
        let st = ac.st;
        let s0 = self.st;
        let (xnrm, xrev) = if o.mode < 0.0 { (0.0, 1.0) } else { (1.0, 0.0) };
        let (cgso, cgdo, cgbo) = self.overlaps();
        let w = ac.omega;
        let (capgs, capgd, capgb) = if self.has_caps() {
            (
                2.0 * st[s0 + CAPGS] + cgso,
                2.0 * st[s0 + CAPGD] + cgdo,
                2.0 * st[s0 + CAPGB] + cgbo,
            )
        } else {
            (0.0, 0.0, 0.0)
        };
        let (xgs, xgd, xgb) = (capgs * w, capgd * w, capgb * w);
        let (xbd, xbs) = (o.capbd * w, o.capbs * w);
        let (gm, gds, gmbs, gbd, gbs) = (o.gm, o.gds, o.gmbs, o.gbd, o.gbs);
        let s = &self.s;
        let c = C64::new;
        ac.add(s.gg, c(0.0, xgd + xgs + xgb));
        ac.add(s.bb, c(gbd + gbs, xgb + xbd + xbs));
        ac.add(
            s.dpdp,
            c(self.gdpr + gds + gbd + xrev * (gm + gmbs), xgd + xbd),
        );
        ac.add(
            s.spsp,
            c(self.gspr + gds + gbs + xnrm * (gm + gmbs), xgs + xbs),
        );
        ac.add(s.gb, c(0.0, -xgb));
        ac.add(s.gdp, c(0.0, -xgd));
        ac.add(s.gsp, c(0.0, -xgs));
        ac.add(s.bg, c(0.0, -xgb));
        ac.add(s.bdp, c(-gbd, -xbd));
        ac.add(s.bsp, c(-gbs, -xbs));
        ac.add(s.dpg, c((xnrm - xrev) * gm, -xgd));
        ac.add(s.dpb, c(-gbd + (xnrm - xrev) * gmbs, -xbd));
        ac.add(s.spg, c(-(xnrm - xrev) * gm, -xgs));
        ac.add(s.spb, c(-gbs - (xnrm - xrev) * gmbs, -xbs));
        ac.add(s.dd, c(self.gdpr, 0.0));
        ac.add(s.ss, c(self.gspr, 0.0));
        ac.add(s.ddp, c(-self.gdpr, 0.0));
        ac.add(s.ssp, c(-self.gspr, 0.0));
        ac.add(s.dpd, c(-self.gdpr, 0.0));
        ac.add(s.sps, c(-self.gspr, 0.0));
        ac.add(s.dpsp, c(-gds - xnrm * (gm + gmbs), 0.0));
        ac.add(s.spdp, c(-gds - xrev * (gm + gmbs), 0.0));
    }
    fn trunc(&self, ck: &Ckt) -> f64 {
        if !self.has_caps()
            || self.model.cox == 0.0
                && self.model.cgso == 0.0
                && self.model.cgdo == 0.0
                && self.model.cgbo == 0.0
        {
            return f64::INFINITY;
        }
        ck.trunc_q(self.st + QGS)
            .min(ck.trunc_q(self.st + QGD))
            .min(ck.trunc_q(self.st + QGB))
    }
    /// Drain current (into the drain terminal).
    fn current(&self, _ck: &Ckt) -> Option<f64> {
        Some(self.sign * self.op.t_cd)
    }
    fn noise(&self, _ck: &Ckt, _f: f64, out: &mut Vec<NoiseSrc>) {
        let kt4 = 4.0 * KB * self.temp_k;
        if self.gdpr > 0.0 {
            out.push(NoiseSrc {
                name: format!("{}#rd", self.name),
                p: self.nodes[0],
                n: self.dp,
                psd: kt4 * self.gdpr,
            });
        }
        if self.gspr > 0.0 {
            out.push(NoiseSrc {
                name: format!("{}#rs", self.name),
                p: self.nodes[2],
                n: self.sp,
                psd: kt4 * self.gspr,
            });
        }
        out.push(NoiseSrc {
            name: format!("{}#id", self.name),
            p: self.dp,
            n: self.sp,
            psd: kt4 * 2.0 / 3.0 * self.op.gm.abs(),
        });
    }
    fn set_param(&mut self, name: &str, v: f64) -> bool {
        match name {
            "l" => self.l = v,
            "w" => self.w = v,
            "m" => self.m = v,
            _ => {
                self.params.set(name, v);
                self.model = MosModel::from_params(&self.params, self.sign < 0.0).0;
            }
        }
        self.resistances();
        self.do_temp(self.circuit_k, self.opt_tnom);
        true
    }
}
