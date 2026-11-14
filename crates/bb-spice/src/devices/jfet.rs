//! JFET (ngspice `jfet` level 1, Shichman-Hodges with the Sydney University `B` doping-tail
//! parameter): VTO, BETA, LAMBDA, RD, RS, CGS, CGD, PB, IS, N, B, FC, EG, XTI, VTOTC, BETATCE.

use super::semi::{fetlim, junction_temp, pnjlim, vcrit};
use super::*;

const KNOWN: &[&str] = &[
    "vto", "vt0", "beta", "lambda", "rd", "rs", "cgs", "cgd", "pb", "is", "n", "b", "fc", "eg",
    "xti", "vtotc", "betatce", "tnom", "tref",
];

#[derive(Debug, Clone, PartialEq)]
pub struct JfetModel {
    pub vto: f64,
    pub beta: f64,
    pub lambda: f64,
    pub rd: f64,
    pub rs: f64,
    pub cgs: f64,
    pub cgd: f64,
    pub pb: f64,
    pub is: f64,
    pub n: f64,
    pub b: f64,
    pub fc: f64,
    pub eg: f64,
    pub xti: f64,
    pub vtotc: f64,
    pub betatce: f64,
    pub tnom: Option<f64>,
}

impl JfetModel {
    pub fn from_params(p: &ParamMap) -> (JfetModel, Vec<String>) {
        (
            JfetModel {
                vto: p.or(&["vto", "vt0"], -2.0),
                beta: p.or(&["beta"], 1e-4),
                lambda: p.or(&["lambda"], 0.0),
                rd: p.or(&["rd"], 0.0),
                rs: p.or(&["rs"], 0.0),
                cgs: p.or(&["cgs"], 0.0),
                cgd: p.or(&["cgd"], 0.0),
                pb: p.or(&["pb"], 1.0),
                is: p.or(&["is"], 1e-14),
                n: p.or(&["n"], 1.0),
                b: p.or(&["b"], 1.0),
                fc: p.or(&["fc"], 0.5),
                eg: p.or(&["eg"], 1.11),
                xti: p.or(&["xti"], 3.0),
                vtotc: p.or(&["vtotc"], 0.0),
                betatce: p.or(&["betatce"], 0.0),
                tnom: p.get(&["tnom", "tref"]),
            },
            p.unknown(KNOWN),
        )
    }
}

const VGS: usize = 0;
const VGD: usize = 1;
const CG: usize = 2;
const CD: usize = 3;
const CGD: usize = 4;
const GM: usize = 5;
const GDS: usize = 6;
const GGS: usize = 7;
const GGD: usize = 8;
const QGS: usize = 9;
const QGD: usize = 11;
const CAPGS: usize = 13;
const CAPGD: usize = 14;
const NSTATES: usize = 15;

#[derive(Debug, Clone, Default)]
struct Slots {
    dd: Slot,
    gg: Slot,
    ss: Slot,
    dpdp: Slot,
    spsp: Slot,
    ddp: Slot,
    gdp: Slot,
    gsp: Slot,
    ssp: Slot,
    dpd: Slot,
    dpg: Slot,
    dpsp: Slot,
    spg: Slot,
    sps: Slot,
    spdp: Slot,
}

#[derive(Debug, Clone)]
pub struct Jfet {
    pub name: String,
    /// d, g, s
    pub nodes: [usize; 3],
    dp: usize,
    sp: usize,
    pub sign: f64,
    pub model: JfetModel,
    pub params: ParamMap,
    pub area: f64,
    pub off: bool,
    pub ic: (Option<f64>, Option<f64>),
    pub temp: Option<f64>,
    t_is: f64,
    t_pb: f64,
    t_cgs: f64,
    t_cgd: f64,
    t_vto: f64,
    t_beta: f64,
    t_vcrit: f64,
    vt: f64,
    temp_k: f64,
    circuit_k: f64,
    opt_tnom: f64,
    st: usize,
    s: Slots,
}

impl Jfet {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        nodes: [usize; 3],
        pchan: bool,
        params: ParamMap,
        area: f64,
        off: bool,
        ic: (Option<f64>, Option<f64>),
        temp: Option<f64>,
    ) -> (Self, Vec<String>) {
        let (model, warn) = JfetModel::from_params(&params);
        (
            Jfet {
                name,
                nodes,
                dp: nodes[0],
                sp: nodes[2],
                sign: if pchan { -1.0 } else { 1.0 },
                model,
                params,
                area,
                off,
                ic,
                temp,
                t_is: 0.0,
                t_pb: 0.0,
                t_cgs: 0.0,
                t_cgd: 0.0,
                t_vto: 0.0,
                t_beta: 0.0,
                t_vcrit: 0.0,
                vt: 0.0,
                temp_k: 300.15,
                circuit_k: 300.15,
                opt_tnom: 27.0,
                st: 0,
                s: Slots::default(),
            },
            warn,
        )
    }

    fn do_temp(&mut self, circuit_k: f64, opt_tnom: f64) {
        self.circuit_k = circuit_k;
        self.opt_tnom = opt_tnom;
        let m = &self.model;
        let tnom = m.tnom.unwrap_or(opt_tnom) + KELVIN;
        let t = self.temp.map(|t| t + KELVIN).unwrap_or(circuit_k);
        self.temp_k = t;
        let vt = KB * t / Q;
        self.vt = vt;
        let ratio = t / tnom;
        self.t_is = m.is * ((ratio - 1.0) * m.eg / (m.n * vt)).exp() * ratio.powf(m.xti / m.n);
        let (pb, cgs) = junction_temp(m.pb, m.cgs, 0.5, tnom, t);
        let (_, cgd) = junction_temp(m.pb, m.cgd, 0.5, tnom, t);
        self.t_pb = pb;
        self.t_cgs = cgs;
        self.t_cgd = cgd;
        self.t_vto = m.vto - m.vtotc * (t - tnom);
        self.t_beta = m.beta * 1.01f64.powf(m.betatce * (t - tnom));
        self.t_vcrit = vcrit(vt, self.t_is * self.area);
    }

    fn gate_charge(&self, v: f64, cz: f64) -> (f64, f64) {
        if cz == 0.0 {
            return (0.0, 0.0);
        }
        let fc = self.model.fc;
        let pb = self.t_pb;
        let dep = fc * pb;
        let twop = pb + pb;
        if v < dep {
            let sarg = (1.0 - v / pb).sqrt();
            (twop * cz * (1.0 - sarg), cz / sarg)
        } else {
            let xfc = (1.0 - fc).ln();
            let f1 = pb * (1.0 - (0.5 * xfc).exp()) / 0.5;
            let f2 = (1.5 * xfc).exp();
            let f3 = 1.0 - fc * 1.5;
            let czf2 = cz / f2;
            (
                cz * f1 + czf2 * (f3 * (v - dep) + (v * v - dep * dep) / (twop + twop)),
                czf2 * (f3 + v / twop),
            )
        }
    }
}

fn gate_diode(v: f64, csat: f64, vt: f64, gmin: f64) -> (f64, f64) {
    if v <= -3.0 * vt {
        let arg = 3.0 * vt / (v * std::f64::consts::E);
        let arg = arg * arg * arg;
        (-csat * (1.0 + arg) + gmin * v, csat * 3.0 * arg / v + gmin)
    } else {
        let e = (v / vt).exp();
        (csat * (e - 1.0) + gmin * v, csat * e / vt + gmin)
    }
}

impl Dev for Jfet {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_unknowns(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.dp = if self.model.rd > 0.0 {
            s.alloc(format!("{}#drain", self.name), UKind::Node)
        } else {
            self.nodes[0]
        };
        self.sp = if self.model.rs > 0.0 {
            s.alloc(format!("{}#source", self.name), UKind::Node)
        } else {
            self.nodes[2]
        };
        self.st = s.states(NSTATES);
        Ok(())
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        let [d, g, so] = self.nodes;
        let (dp, sp) = (self.dp, self.sp);
        self.s = Slots {
            dd: s.slot(d, d),
            gg: s.slot(g, g),
            ss: s.slot(so, so),
            dpdp: s.slot(dp, dp),
            spsp: s.slot(sp, sp),
            ddp: s.slot(d, dp),
            gdp: s.slot(g, dp),
            gsp: s.slot(g, sp),
            ssp: s.slot(so, sp),
            dpd: s.slot(dp, d),
            dpg: s.slot(dp, g),
            dpsp: s.slot(dp, sp),
            spg: s.slot(sp, g),
            sps: s.slot(sp, so),
            spdp: s.slot(sp, dp),
        };
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
        self.model.cgs > 0.0 || self.model.cgd > 0.0
    }
    fn load(&mut self, ck: &mut Ckt) {
        let m = self.model.clone();
        let ty = self.sign;
        let s0 = self.st;
        let vt = self.vt;
        let beta = self.t_beta * self.area;
        let gdpr = if m.rd > 0.0 { self.area / m.rd } else { 0.0 };
        let gspr = if m.rs > 0.0 { self.area / m.rs } else { 0.0 };
        let csat = self.t_is * self.area;
        let [_d, g, _so] = self.nodes;
        let (dp, sp) = (self.dp, self.sp);
        let mut icheck = false;
        let (vgs, vgd) = match ck.init {
            Init::Tran => (ck.st[1][s0 + VGS], ck.st[1][s0 + VGD]),
            Init::Jct if ck.tran_op && ck.uic => {
                let vds = ty * self.ic.0.unwrap_or(0.0);
                let vgs = ty * self.ic.1.unwrap_or(0.0);
                (vgs, vgs - vds)
            }
            Init::Jct if self.off => (0.0, 0.0),
            Init::Jct => (-1.0, -1.0),
            Init::Fix if self.off => (0.0, 0.0),
            _ => {
                let (mut vgs, mut vgd) = if ck.init == Init::Pred {
                    for k in [VGS, VGD, CG, CD, CGD, GM, GDS, GGS, GGD] {
                        ck.st[0][s0 + k] = ck.st[1][s0 + k];
                    }
                    (ck.predict(s0 + VGS), ck.predict(s0 + VGD))
                } else {
                    (ty * (ck.x[g] - ck.x[sp]), ty * (ck.x[g] - ck.x[dp]))
                };
                let (a, l1) = pnjlim(vgs, ck.st[0][s0 + VGS], vt, self.t_vcrit);
                let (b, l2) = pnjlim(vgd, ck.st[0][s0 + VGD], vt, self.t_vcrit);
                icheck = l1 || l2;
                vgs = fetlim(a, ck.st[0][s0 + VGS], self.t_vto);
                vgd = fetlim(b, ck.st[0][s0 + VGD], self.t_vto);
                (vgs, vgd)
            }
        };
        let vds = vgs - vgd;
        let (cgs_dc, mut ggs) = gate_diode(vgs, csat, vt, ck.gmin);
        let (mut cgd, mut ggd) = gate_diode(vgd, csat, vt, ck.gmin);
        let mut cg = cgs_dc + cgd;
        let vto = self.t_vto;
        let bb = m.b;
        let (cdrain, gm, gds);
        if vds >= 0.0 {
            let vgst = vgs - vto;
            if vgst <= 0.0 {
                cdrain = 0.0;
                gm = 0.0;
                gds = 0.0;
            } else {
                let betap = beta * (1.0 + m.lambda * vds);
                let bfac = (1.0 - bb) / (self.t_pb - vto);
                if vgst >= vds {
                    let apart = 2.0 * bb + 3.0 * bfac * (vgst - vds);
                    let cpart = vds * (vds * (bfac * vds - bb) + vgst * apart);
                    cdrain = betap * cpart;
                    gm = betap * vds * (apart + 3.0 * bfac * vgst);
                    gds = betap * (vgst - vds) * apart + beta * m.lambda * cpart;
                } else {
                    let bfac = vgst * bfac;
                    gm = betap * vgst * (2.0 * bb + 3.0 * bfac);
                    let cpart = vgst * vgst * (bb + bfac);
                    cdrain = betap * cpart;
                    gds = m.lambda * beta * cpart;
                }
            }
        } else {
            let vgdt = vgd - vto;
            if vgdt <= 0.0 {
                cdrain = 0.0;
                gm = 0.0;
                gds = 0.0;
            } else {
                let betap = beta * (1.0 - m.lambda * vds);
                let bfac = (1.0 - bb) / (self.t_pb - vto);
                if vgdt + vds >= 0.0 {
                    let apart = 2.0 * bb + 3.0 * bfac * (vgdt + vds);
                    let cpart = vds * (-vds * (-bfac * vds - bb) + vgdt * apart);
                    cdrain = betap * cpart;
                    let gmv = betap * vds * (apart + 3.0 * bfac * vgdt);
                    gm = gmv;
                    gds = betap * (vgdt + vds) * apart - beta * m.lambda * cpart - gmv;
                } else {
                    let bfac = vgdt * bfac;
                    let gmv = -betap * vgdt * (2.0 * bb + 3.0 * bfac);
                    gm = gmv;
                    let cpart = vgdt * vgdt * (bb + bfac);
                    cdrain = -betap * cpart;
                    gds = m.lambda * beta * cpart - gmv;
                }
            }
        }
        let mut cd = cdrain - cgd;
        if self.is_reactive() {
            let czgs = self.t_cgs * self.area;
            let czgd = self.t_cgd * self.area;
            let (qgs, capgs) = self.gate_charge(vgs, czgs);
            let (qgd, capgd) = self.gate_charge(vgd, czgd);
            ck.st[0][s0 + QGS] = qgs;
            ck.st[0][s0 + QGD] = qgd;
            ck.st[0][s0 + CAPGS] = capgs;
            ck.st[0][s0 + CAPGD] = capgd;
            if ck.mode == Mode::Tran {
                if ck.init == Init::Tran {
                    ck.st[1][s0 + QGS] = qgs;
                    ck.st[1][s0 + QGD] = qgd;
                }
                let (geq, ccap) = ck.integrate(s0 + QGS, capgs);
                ggs += geq;
                cg += ccap;
                let (geq, ccap) = ck.integrate(s0 + QGD, capgd);
                ggd += geq;
                cg += ccap;
                cd -= ccap;
                cgd += ccap;
                if ck.init == Init::Tran {
                    ck.st[1][s0 + QGS + 1] = ck.st[0][s0 + QGS + 1];
                    ck.st[1][s0 + QGD + 1] = ck.st[0][s0 + QGD + 1];
                }
            }
        }
        if icheck && (ck.init != Init::Fix || !self.off) {
            ck.nonconverged();
        }
        let st = &mut ck.st[0];
        st[s0 + VGS] = vgs;
        st[s0 + VGD] = vgd;
        st[s0 + CG] = cg;
        st[s0 + CD] = cd;
        st[s0 + CGD] = cgd;
        st[s0 + GM] = gm;
        st[s0 + GDS] = gds;
        st[s0 + GGS] = ggs;
        st[s0 + GGD] = ggd;
        let ceqgd = ty * (cgd - ggd * vgd);
        let ceqgs = ty * ((cg - cgd) - ggs * vgs);
        let cdreq = ty * ((cd + cgd) - gds * vds - gm * vgs);
        ck.add_rhs(g, -ceqgs - ceqgd);
        ck.add_rhs(dp, -cdreq + ceqgd);
        ck.add_rhs(sp, cdreq + ceqgs);
        let s = &self.s;
        ck.add(s.dd, gdpr);
        ck.add(s.gg, ggd + ggs);
        ck.add(s.ss, gspr);
        ck.add(s.dpdp, gdpr + gds + ggd);
        ck.add(s.spsp, gspr + gds + gm + ggs);
        ck.add(s.ddp, -gdpr);
        ck.add(s.gdp, -ggd);
        ck.add(s.gsp, -ggs);
        ck.add(s.ssp, -gspr);
        ck.add(s.dpd, -gdpr);
        ck.add(s.dpg, gm - ggd);
        ck.add(s.dpsp, -gds - gm);
        ck.add(s.spg, -ggs - gm);
        ck.add(s.sps, -gspr);
        ck.add(s.spdp, -gds);
    }
    fn conv_ok(&self, ck: &Ckt) -> bool {
        let ty = self.sign;
        let s0 = self.st;
        let st = &ck.st[0];
        let g = self.nodes[1];
        let vgs = ty * (ck.x[g] - ck.x[self.sp]);
        let vgd = ty * (ck.x[g] - ck.x[self.dp]);
        let delvgs = vgs - st[s0 + VGS];
        let delvgd = vgd - st[s0 + VGD];
        let delvds = delvgs - delvgd;
        let cghat = st[s0 + CG] + st[s0 + GGD] * delvgd + st[s0 + GGS] * delvgs;
        let cdhat =
            st[s0 + CD] + st[s0 + GM] * delvgs + st[s0 + GDS] * delvds - st[s0 + GGD] * delvgd;
        let o = &ck.opts;
        let (cg, cd) = (st[s0 + CG], st[s0 + CD]);
        (cghat - cg).abs() <= o.reltol * cghat.abs().max(cg.abs()) + o.abstol
            && (cdhat - cd).abs() <= o.reltol * cdhat.abs().max(cd.abs()) + o.abstol
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        let st = ac.st;
        let s0 = self.st;
        let m = &self.model;
        let gdpr = if m.rd > 0.0 { self.area / m.rd } else { 0.0 };
        let gspr = if m.rs > 0.0 { self.area / m.rs } else { 0.0 };
        let (gm, gds, ggs, ggd) = (st[s0 + GM], st[s0 + GDS], st[s0 + GGS], st[s0 + GGD]);
        let (xgs, xgd) = if self.is_reactive() {
            (st[s0 + CAPGS] * ac.omega, st[s0 + CAPGD] * ac.omega)
        } else {
            (0.0, 0.0)
        };
        let s = &self.s;
        let c = C64::new;
        ac.add(s.dd, c(gdpr, 0.0));
        ac.add(s.gg, c(ggd + ggs, xgd + xgs));
        ac.add(s.ss, c(gspr, 0.0));
        ac.add(s.dpdp, c(gdpr + gds + ggd, xgd));
        ac.add(s.spsp, c(gspr + gds + gm + ggs, xgs));
        ac.add(s.ddp, c(-gdpr, 0.0));
        ac.add(s.gdp, c(-ggd, -xgd));
        ac.add(s.gsp, c(-ggs, -xgs));
        ac.add(s.ssp, c(-gspr, 0.0));
        ac.add(s.dpd, c(-gdpr, 0.0));
        ac.add(s.dpg, c(-ggd + gm, -xgd));
        ac.add(s.dpsp, c(-gds - gm, 0.0));
        ac.add(s.spg, c(-ggs - gm, -xgs));
        ac.add(s.sps, c(-gspr, 0.0));
        ac.add(s.spdp, c(-gds, 0.0));
    }
    fn trunc(&self, ck: &Ckt) -> f64 {
        if self.is_reactive() {
            ck.trunc_q(self.st + QGS).min(ck.trunc_q(self.st + QGD))
        } else {
            f64::INFINITY
        }
    }
    fn current(&self, ck: &Ckt) -> Option<f64> {
        Some(self.sign * ck.st[0][self.st + CD])
    }
    fn noise(&self, ck: &Ckt, _f: f64, out: &mut Vec<NoiseSrc>) {
        let kt4 = 4.0 * KB * self.temp_k;
        let m = &self.model;
        if m.rd > 0.0 {
            out.push(NoiseSrc {
                name: format!("{}#rd", self.name),
                p: self.nodes[0],
                n: self.dp,
                psd: kt4 * self.area / m.rd,
            });
        }
        if m.rs > 0.0 {
            out.push(NoiseSrc {
                name: format!("{}#rs", self.name),
                p: self.nodes[2],
                n: self.sp,
                psd: kt4 * self.area / m.rs,
            });
        }
        let gm = ck.st[0][self.st + GM];
        out.push(NoiseSrc {
            name: format!("{}#id", self.name),
            p: self.dp,
            n: self.sp,
            psd: kt4 * 2.0 / 3.0 * gm.abs(),
        });
    }
    fn set_param(&mut self, name: &str, v: f64) -> bool {
        match name {
            "area" => self.area = v,
            _ => {
                self.params.set(name, v);
                self.model = JfetModel::from_params(&self.params).0;
            }
        }
        self.do_temp(self.circuit_k, self.opt_tnom);
        true
    }
}
