//! Gummel-Poon BJT (ngspice `bjt` level 1): IS, BF, BR, NF, NR, VAF, VAR, IKF, IKR, ISE, NE,
//! ISC, NC, RB, IRB, RBM, RE, RC, CJE, VJE, MJE, CJC, VJC, MJC, XCJC, CJS, VJS, MJS, TF, XTF,
//! VTF, ITF, PTF, TR, XTB, EG, XTI, FC, SUBS — with temperature scaling, excess phase,
//! current-dependent base resistance and shot/thermal noise.

use super::semi::{junction_temp, pnjlim, vcrit};
use super::*;

const KNOWN: &[&str] = &[
    "is", "bf", "nf", "vaf", "va", "ikf", "ik", "ise", "c2", "ne", "br", "nr", "var", "vb", "ikr",
    "isc", "c4", "nc", "rb", "irb", "rbm", "re", "rc", "cje", "vje", "pe", "mje", "me", "tf",
    "xtf", "vtf", "itf", "ptf", "cjc", "vjc", "pc", "mjc", "mc", "xcjc", "cdis", "tr", "cjs",
    "ccs", "csub", "vjs", "ps", "mjs", "ms", "xtb", "eg", "xti", "fc", "subs", "tnom", "tref",
    "iss", "ns", "nk",
];

#[derive(Debug, Clone, PartialEq)]
pub struct BjtModel {
    pub is: f64,
    pub bf: f64,
    pub nf: f64,
    pub inv_vaf: f64,
    pub ikf: f64,
    pub ise: f64,
    pub ne: f64,
    pub br: f64,
    pub nr: f64,
    pub inv_var: f64,
    pub ikr: f64,
    pub isc: f64,
    pub nc: f64,
    pub rb: f64,
    pub irb: f64,
    pub rbm: f64,
    pub re: f64,
    pub rc: f64,
    pub cje: f64,
    pub vje: f64,
    pub mje: f64,
    pub tf: f64,
    pub xtf: f64,
    pub vtf: f64,
    pub itf: f64,
    pub ptf: f64,
    pub cjc: f64,
    pub vjc: f64,
    pub mjc: f64,
    pub xcjc: f64,
    pub tr: f64,
    pub cjs: f64,
    pub vjs: f64,
    pub mjs: f64,
    pub xtb: f64,
    pub eg: f64,
    pub xti: f64,
    pub fc: f64,
    pub subs: f64,
    pub tnom: Option<f64>,
}

impl BjtModel {
    pub fn from_params(p: &ParamMap, pnp: bool) -> (BjtModel, Vec<String>) {
        let inv = |v: Option<f64>| match v {
            Some(x) if x != 0.0 => 1.0 / x,
            _ => 0.0,
        };
        let rb = p.or(&["rb"], 0.0);
        let m = BjtModel {
            is: p.or(&["is"], 1e-16),
            bf: p.or(&["bf"], 100.0),
            nf: p.or(&["nf"], 1.0),
            inv_vaf: inv(p.get(&["vaf", "va"])),
            ikf: p.or(&["ikf", "ik"], 0.0),
            ise: p.or(&["ise", "c2"], 0.0),
            ne: p.or(&["ne"], 1.5),
            br: p.or(&["br"], 1.0),
            nr: p.or(&["nr"], 1.0),
            inv_var: inv(p.get(&["var", "vb"])),
            ikr: p.or(&["ikr"], 0.0),
            isc: p.or(&["isc", "c4"], 0.0),
            nc: p.or(&["nc"], 2.0),
            rb,
            irb: p.or(&["irb"], 0.0),
            rbm: p.or(&["rbm"], rb),
            re: p.or(&["re"], 0.0),
            rc: p.or(&["rc"], 0.0),
            cje: p.or(&["cje"], 0.0),
            vje: p.or(&["vje", "pe"], 0.75),
            mje: p.or(&["mje", "me"], 0.33),
            tf: p.or(&["tf"], 0.0),
            xtf: p.or(&["xtf"], 0.0),
            vtf: p.or(&["vtf"], 0.0),
            itf: p.or(&["itf"], 0.0),
            ptf: p.or(&["ptf"], 0.0),
            cjc: p.or(&["cjc"], 0.0),
            vjc: p.or(&["vjc", "pc"], 0.75),
            mjc: p.or(&["mjc", "mc"], 0.33),
            xcjc: p.or(&["xcjc", "cdis"], 1.0),
            tr: p.or(&["tr"], 0.0),
            cjs: p.or(&["cjs", "ccs", "csub"], 0.0),
            vjs: p.or(&["vjs", "ps"], 0.75),
            mjs: p.or(&["mjs", "ms"], 0.0),
            xtb: p.or(&["xtb"], 0.0),
            eg: p.or(&["eg"], 1.11),
            xti: p.or(&["xti"], 3.0),
            fc: p.or(&["fc"], 0.5),
            subs: p.or(&["subs"], if pnp { -1.0 } else { 1.0 }),
            tnom: p.get(&["tnom", "tref"]),
        };
        (m, p.unknown(KNOWN))
    }
}

// state offsets
const VBE: usize = 0;
const VBC: usize = 1;
const CC: usize = 2;
const CB: usize = 3;
const GPI: usize = 4;
const GMU: usize = 5;
const GM: usize = 6;
const GO: usize = 7;
const QBE: usize = 8; // + CQBE = 9
const QBC: usize = 10; // + CQBC = 11
const QSUB: usize = 12; // + 13
const QBX: usize = 14; // + 15
const GX: usize = 16;
const CEXBC: usize = 17;
const GEQCB: usize = 18;
const GCSUB: usize = 19;
const GEQBX: usize = 20;
const VSUB: usize = 21;
const CAPBE: usize = 22;
const CAPBC: usize = 23;
const CAPSUB: usize = 24;
const CAPBX: usize = 25;
const NSTATES: usize = 26;

#[derive(Debug, Clone, Default)]
struct Slots {
    cc: Slot,
    bb: Slot,
    ee: Slot,
    cpcp: Slot,
    bpbp: Slot,
    epep: Slot,
    ccp: Slot,
    bbp: Slot,
    eep: Slot,
    cpc: Slot,
    cpbp: Slot,
    cpep: Slot,
    bpb: Slot,
    bpcp: Slot,
    bpep: Slot,
    epe: Slot,
    epcp: Slot,
    epbp: Slot,
    ss: Slot,
    scs: Slot,
    sss: Slot,
    scsc: Slot,
    bcp: Slot,
    cpb: Slot,
}

#[derive(Debug, Clone)]
pub struct Bjt {
    pub name: String,
    /// c, b, e, substrate
    pub nodes: [usize; 4],
    cp: usize,
    bp: usize,
    ep: usize,
    pub sign: f64,
    pub model: BjtModel,
    pub params: ParamMap,
    pub area: f64,
    pub off: bool,
    pub ic: (Option<f64>, Option<f64>),
    pub temp: Option<f64>,
    pub dtemp: f64,
    // temperature-adjusted
    t_is: f64,
    t_bf: f64,
    t_br: f64,
    t_ise: f64,
    t_isc: f64,
    t_vje: f64,
    t_cje: f64,
    t_vjc: f64,
    t_cjc: f64,
    t_vjs: f64,
    t_cjs: f64,
    t_vcrit: f64,
    vt: f64,
    temp_k: f64,
    circuit_k: f64,
    opt_tnom: f64,
    st: usize,
    s: Slots,
}

impl Bjt {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        nodes: [usize; 4],
        pnp: bool,
        params: ParamMap,
        area: f64,
        off: bool,
        ic: (Option<f64>, Option<f64>),
        temp: Option<f64>,
        dtemp: f64,
    ) -> (Self, Vec<String>) {
        let (model, warn) = BjtModel::from_params(&params, pnp);
        (
            Bjt {
                name,
                nodes,
                cp: nodes[0],
                bp: nodes[1],
                ep: nodes[2],
                sign: if pnp { -1.0 } else { 1.0 },
                model,
                params,
                area,
                off,
                ic,
                temp,
                dtemp,
                t_is: 0.0,
                t_bf: 0.0,
                t_br: 0.0,
                t_ise: 0.0,
                t_isc: 0.0,
                t_vje: 0.0,
                t_cje: 0.0,
                t_vjc: 0.0,
                t_cjc: 0.0,
                t_vjs: 0.0,
                t_cjs: 0.0,
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
        let t = self.temp.map(|t| t + KELVIN).unwrap_or(circuit_k) + self.dtemp;
        self.temp_k = t;
        let vt = KB * t / Q;
        self.vt = vt;
        let ratlog = (t / tnom).ln();
        let ratio1 = t / tnom - 1.0;
        let factlog = ratio1 * m.eg / vt + m.xti * ratlog;
        let factor = factlog.exp();
        self.t_is = m.is * factor;
        let bfactor = (ratlog * m.xtb).exp();
        self.t_bf = m.bf * bfactor;
        self.t_br = m.br * bfactor;
        self.t_ise = m.ise * (factlog / m.ne).exp() / bfactor;
        self.t_isc = m.isc * (factlog / m.nc).exp() / bfactor;
        let (vje, cje) = junction_temp(m.vje, m.cje, m.mje, tnom, t);
        let (vjc, cjc) = junction_temp(m.vjc, m.cjc, m.mjc, tnom, t);
        let (vjs, cjs) = junction_temp(m.vjs, m.cjs, m.mjs, tnom, t);
        self.t_vje = vje;
        self.t_cje = cje;
        self.t_vjc = vjc;
        self.t_cjc = cjc;
        self.t_vjs = vjs;
        self.t_cjs = cjs;
        self.t_vcrit = vcrit(vt, self.t_is * self.area);
    }

    fn has_sub(&self) -> bool {
        self.model.cjs > 0.0
    }

    /// Substrate connection node (collector' for vertical, base' for lateral).
    fn subcon(&self) -> usize {
        if self.model.subs > 0.0 {
            self.cp
        } else {
            self.bp
        }
    }

    fn reactive(&self) -> bool {
        let m = &self.model;
        m.cje > 0.0 || m.cjc > 0.0 || m.cjs > 0.0 || m.tf > 0.0 || m.tr > 0.0
    }
}

fn junction_current(v: f64, is: f64, vtn: f64) -> (f64, f64) {
    if v >= -3.0 * vtn {
        let e = (v / vtn).exp();
        (is * (e - 1.0), is * e / vtn)
    } else {
        let arg = 3.0 * vtn / (v * std::f64::consts::E);
        let arg = arg * arg * arg;
        (-is * (1.0 + arg), is * 3.0 * arg / v)
    }
}

fn dep_q(v: f64, cz: f64, pb: f64, mj: f64, fc: f64) -> (f64, f64) {
    super::semi::depletion(v, cz, pb, mj, fc)
}

impl Dev for Bjt {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_unknowns(&mut self, s: &mut Setup) -> Result<(), SimError> {
        let [c, b, e, _] = self.nodes;
        self.cp = if self.model.rc > 0.0 {
            s.alloc(format!("{}#collector", self.name), UKind::Node)
        } else {
            c
        };
        self.bp = if self.model.rb > 0.0 {
            s.alloc(format!("{}#base", self.name), UKind::Node)
        } else {
            b
        };
        self.ep = if self.model.re > 0.0 {
            s.alloc(format!("{}#emitter", self.name), UKind::Node)
        } else {
            e
        };
        self.st = s.states(NSTATES);
        Ok(())
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        let [c, b, e, sub] = self.nodes;
        let (cp, bp, ep) = (self.cp, self.bp, self.ep);
        let sc = self.subcon();
        self.s = Slots {
            cc: s.slot(c, c),
            bb: s.slot(b, b),
            ee: s.slot(e, e),
            cpcp: s.slot(cp, cp),
            bpbp: s.slot(bp, bp),
            epep: s.slot(ep, ep),
            ccp: s.slot(c, cp),
            bbp: s.slot(b, bp),
            eep: s.slot(e, ep),
            cpc: s.slot(cp, c),
            cpbp: s.slot(cp, bp),
            cpep: s.slot(cp, ep),
            bpb: s.slot(bp, b),
            bpcp: s.slot(bp, cp),
            bpep: s.slot(bp, ep),
            epe: s.slot(ep, e),
            epcp: s.slot(ep, cp),
            epbp: s.slot(ep, bp),
            ss: s.slot(sub, sub),
            scs: s.slot(sc, sub),
            sss: s.slot(sub, sc),
            scsc: s.slot(sc, sc),
            bcp: s.slot(b, cp),
            cpb: s.slot(cp, b),
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
        self.reactive()
    }
    fn load(&mut self, ck: &mut Ckt) {
        let m = self.model.clone();
        let s0 = self.st;
        let ty = self.sign;
        let area = self.area;
        let vt = self.vt;
        let [_c, b, _e, sub] = self.nodes;
        let (cp, bp, ep) = (self.cp, self.bp, self.ep);
        let sc = self.subcon();
        let csat = self.t_is * area;
        let rbpr = if m.rbm > 0.0 { m.rbm / area } else { 0.0 };
        let rbpi = m.rb / area - rbpr;
        let gcpr = if m.rc > 0.0 { area / m.rc } else { 0.0 };
        let gepr = if m.re > 0.0 { area / m.re } else { 0.0 };
        let oik = if m.ikf > 0.0 {
            1.0 / (m.ikf * area)
        } else {
            0.0
        };
        let oikr = if m.ikr > 0.0 {
            1.0 / (m.ikr * area)
        } else {
            0.0
        };
        let c2 = self.t_ise * area;
        let vte = m.ne * vt;
        let c4 = self.t_isc * area;
        let vtc = m.nc * vt;
        let xjrb = m.irb * area;
        let td = m.ptf.to_radians() * m.tf;
        let subs_sign = if m.subs > 0.0 { 1.0 } else { -1.0 };

        let mut icheck = false;
        let (vbe, vbc, vbx, vsub);
        match ck.init {
            Init::Tran => {
                vbe = ck.st[1][s0 + VBE];
                vbc = ck.st[1][s0 + VBC];
                vbx = ty * (ck.x[b] - ck.x[cp]);
                vsub = ty * subs_sign * (ck.x[sub] - ck.x[sc]);
            }
            Init::Jct if ck.tran_op && ck.uic => {
                vbe = ty * self.ic.0.unwrap_or(0.0);
                let vce = ty * self.ic.1.unwrap_or(0.0);
                vbc = vbe - vce;
                vbx = vbc;
                vsub = 0.0;
            }
            Init::Jct if !self.off => {
                vbe = self.t_vcrit;
                vbc = 0.0;
                vbx = 0.0;
                vsub = 0.0;
            }
            Init::Jct | Init::Fix if self.off || ck.init == Init::Jct => {
                vbe = 0.0;
                vbc = 0.0;
                vbx = 0.0;
                vsub = 0.0;
            }
            _ => {
                let (mut vbe_n, mut vbc_n) = if ck.init == Init::Pred {
                    for k in [VBE, VBC, CC, CB, GPI, GMU, GM, GO, GX, VSUB] {
                        ck.st[0][s0 + k] = ck.st[1][s0 + k];
                    }
                    (ck.predict(s0 + VBE), ck.predict(s0 + VBC))
                } else {
                    (ty * (ck.x[bp] - ck.x[ep]), ty * (ck.x[bp] - ck.x[cp]))
                };
                vbx = ty * (ck.x[b] - ck.x[cp]);
                vsub = ty * subs_sign * (ck.x[sub] - ck.x[sc]);
                let (v1, l1) = pnjlim(vbe_n, ck.st[0][s0 + VBE], vt, self.t_vcrit);
                let (v2, l2) = pnjlim(vbc_n, ck.st[0][s0 + VBC], vt, self.t_vcrit);
                vbe_n = v1;
                vbc_n = v2;
                icheck = l1 || l2;
                vbe = vbe_n;
                vbc = vbc_n;
            }
        }

        // DC currents
        let (cbe0, gbe0) = junction_current(vbe, csat, vt * m.nf);
        let (mut cben, mut gben) = if c2 == 0.0 {
            (0.0, 0.0)
        } else {
            junction_current(vbe, c2, vte)
        };
        gben += ck.gmin;
        cben += ck.gmin * vbe;
        let (cbc, gbc) = junction_current(vbc, csat, vt * m.nr);
        let (mut cbcn, mut gbcn) = if c4 == 0.0 {
            (0.0, 0.0)
        } else {
            junction_current(vbc, c4, vtc)
        };
        gbcn += ck.gmin;
        cbcn += ck.gmin * vbc;
        let (mut cbe, mut gbe) = (cbe0, gbe0);

        // base charge
        let q1 = 1.0 / (1.0 - m.inv_vaf * vbc - m.inv_var * vbe);
        let (qb, dqbdve, dqbdvc);
        if oik == 0.0 && oikr == 0.0 {
            qb = q1;
            dqbdve = q1 * qb * m.inv_var;
            dqbdvc = q1 * qb * m.inv_vaf;
        } else {
            let q2 = oik * cbe + oikr * cbc;
            let arg = (1.0 + 4.0 * q2).max(0.0);
            let sqarg = if arg != 0.0 { arg.sqrt() } else { 1.0 };
            qb = q1 * (1.0 + sqarg) / 2.0;
            dqbdve = q1 * (qb * m.inv_var + oik * gbe / sqarg);
            dqbdvc = q1 * (qb * m.inv_vaf + oikr * gbc / sqarg);
        }

        // excess phase
        let (mut cc, cex, gex);
        if td != 0.0 && ck.mode == Mode::Tran {
            let arg1 = ck.delta / td;
            let arg2 = 3.0 * arg1;
            let arg1 = arg2 * arg1;
            let denom = 1.0 + arg1 + arg2;
            let arg3 = arg1 / denom;
            if ck.init == Init::Tran {
                ck.st[1][s0 + CEXBC] = cbe / qb;
                ck.st[2][s0 + CEXBC] = ck.st[1][s0 + CEXBC];
            }
            let r = ck.delta / ck.delta_old[1];
            cc = (ck.st[1][s0 + CEXBC] * (1.0 + r + arg2) - ck.st[2][s0 + CEXBC] * r) / denom;
            cex = cbe * arg3;
            gex = gbe * arg3;
            ck.st[0][s0 + CEXBC] = cc + cex / qb;
        } else {
            cc = 0.0;
            cex = cbe;
            gex = gbe;
        }

        cc = cc + (cex - cbc) / qb - cbc / self.t_br - cbcn;
        let mut cb = cbe / self.t_bf + cben + cbc / self.t_br + cbcn;
        let mut gx = rbpr + rbpi / qb;
        if xjrb != 0.0 {
            let arg1 = (cb / xjrb).max(1e-9);
            let arg2 = (-1.0 + (1.0 + 14.59025 * arg1).sqrt()) / 2.4317 / arg1.sqrt();
            let t1 = arg2.tan();
            gx = rbpr + 3.0 * rbpi * (t1 - arg2) / arg2 / t1 / t1;
        }
        if gx != 0.0 {
            gx = 1.0 / gx;
        }
        let mut gpi = gbe / self.t_bf + gben;
        let mut gmu = gbc / self.t_br + gbcn;
        let go = (gbc + (cex - cbc) * dqbdvc / qb) / qb;
        let gm = (gex - (cex - cbc) * dqbdve / qb) / qb - go;

        let mut geqcb = 0.0;
        let mut gcsub = 0.0;
        let mut geqbx = 0.0;
        if self.reactive() {
            let tf = m.tf;
            let tr = m.tr;
            let czbe = self.t_cje * area;
            let ctot = self.t_cjc * area;
            let czbc = ctot * m.xcjc;
            let czbx = ctot - czbc;
            let czsub = self.t_cjs * area;
            if tf != 0.0 && vbe > 0.0 {
                let mut argtf = 0.0;
                let mut arg2 = 0.0;
                let mut arg3 = 0.0;
                let ovtf = if m.vtf != 0.0 { 1.0 / m.vtf } else { 0.0 };
                if m.xtf != 0.0 {
                    argtf = m.xtf;
                    if ovtf != 0.0 {
                        argtf *= (vbc * ovtf).exp();
                    }
                    arg2 = argtf;
                    let xjtf = m.itf * area;
                    if xjtf != 0.0 {
                        let temp = cbe / (cbe + xjtf);
                        argtf = argtf * temp * temp;
                        arg2 = argtf * (3.0 - temp - temp);
                    }
                    arg3 = cbe * argtf * ovtf;
                }
                cbe = cbe * (1.0 + argtf) / qb;
                gbe = (gbe * (1.0 + arg2) - cbe * dqbdve) / qb;
                geqcb = tf * (arg3 - cbe * dqbdvc) / qb;
            }
            let (qdbe, cdbe) = dep_q(vbe, czbe, self.t_vje, m.mje, m.fc);
            let qbe = tf * cbe + qdbe;
            let capbe = tf * gbe + cdbe;
            let (qdbc, cdbc) = dep_q(vbc, czbc, self.t_vjc, m.mjc, m.fc);
            let qbc = tr * cbc + qdbc;
            let capbc = tr * gbc + cdbc;
            let (qbx, capbx) = dep_q(vbx, czbx, self.t_vjc, m.mjc, m.fc);
            let (qsub, capsub) = if czsub == 0.0 {
                (0.0, 0.0)
            } else if vsub < 0.0 {
                let arg = 1.0 - vsub / self.t_vjs;
                let sarg = (-m.mjs * arg.ln()).exp();
                (
                    self.t_vjs * czsub * (1.0 - arg * sarg) / (1.0 - m.mjs),
                    czsub * sarg,
                )
            } else {
                (
                    vsub * czsub * (1.0 + m.mjs * vsub / (2.0 * self.t_vjs)),
                    czsub * (1.0 + m.mjs * vsub / self.t_vjs),
                )
            };
            ck.st[0][s0 + QBE] = qbe;
            ck.st[0][s0 + QBC] = qbc;
            ck.st[0][s0 + QBX] = qbx;
            ck.st[0][s0 + QSUB] = qsub;
            ck.st[0][s0 + CAPBE] = capbe;
            ck.st[0][s0 + CAPBC] = capbc;
            ck.st[0][s0 + CAPBX] = capbx;
            ck.st[0][s0 + CAPSUB] = capsub;
            if ck.mode == Mode::Tran {
                if ck.init == Init::Tran {
                    for k in [QBE, QBC, QBX, QSUB] {
                        ck.st[1][s0 + k] = ck.st[0][s0 + k];
                    }
                }
                let (geq, ccap) = ck.integrate(s0 + QBE, capbe);
                geqcb *= ck.ag[0];
                gpi += geq;
                cb += ccap;
                let (geq, ccap) = ck.integrate(s0 + QBC, capbc);
                gmu += geq;
                cb += ccap;
                cc -= ccap;
                let (g, _) = ck.integrate(s0 + QSUB, capsub);
                gcsub = g;
                let (g, _) = ck.integrate(s0 + QBX, capbx);
                geqbx = g;
                if ck.init == Init::Tran {
                    for k in [QBE, QBC, QBX, QSUB] {
                        ck.st[1][s0 + k + 1] = ck.st[0][s0 + k + 1];
                    }
                }
            }
        }
        if icheck && (ck.init != Init::Fix || !self.off) {
            ck.nonconverged();
        }
        let st = &mut ck.st[0];
        st[s0 + VBE] = vbe;
        st[s0 + VBC] = vbc;
        st[s0 + CC] = cc;
        st[s0 + CB] = cb;
        st[s0 + GPI] = gpi;
        st[s0 + GMU] = gmu;
        st[s0 + GM] = gm;
        st[s0 + GO] = go;
        st[s0 + GX] = gx;
        st[s0 + GEQCB] = geqcb;
        st[s0 + GCSUB] = gcsub;
        st[s0 + GEQBX] = geqbx;
        st[s0 + VSUB] = vsub;

        // current vector
        let (cqsub, cqbx) = if ck.mode == Mode::Tran {
            (st[s0 + QSUB + 1], st[s0 + QBX + 1])
        } else {
            (0.0, 0.0)
        };
        let ceqsub = ty * subs_sign * (cqsub - vsub * gcsub);
        let ceqbx = ty * (cqbx - vbx * geqbx);
        let ceqbe = ty * (cc + cb - vbe * (gm + go + gpi) + vbc * (go - geqcb));
        let ceqbc = ty * (-cc + vbe * (gm + go) - vbc * (gmu + go));
        ck.add_rhs(b, -ceqbx);
        ck.add_rhs(cp, ceqbx + ceqbc);
        ck.add_rhs(bp, -ceqbe - ceqbc);
        ck.add_rhs(ep, ceqbe);
        ck.add_rhs(sub, -ceqsub);
        ck.add_rhs(sc, ceqsub);

        let s = &self.s;
        ck.add(s.cc, gcpr);
        ck.add(s.bb, gx + geqbx);
        ck.add(s.ee, gepr);
        ck.add(s.cpcp, gmu + go + gcpr + geqbx);
        ck.add(s.bpbp, gx + gpi + gmu + geqcb);
        ck.add(s.epep, gpi + gepr + gm + go);
        ck.add(s.ccp, -gcpr);
        ck.add(s.bbp, -gx);
        ck.add(s.eep, -gepr);
        ck.add(s.cpc, -gcpr);
        ck.add(s.cpbp, -gmu + gm);
        ck.add(s.cpep, -gm - go);
        ck.add(s.bpb, -gx);
        ck.add(s.bpcp, -gmu - geqcb);
        ck.add(s.bpep, -gpi);
        ck.add(s.epe, -gepr);
        ck.add(s.epcp, -go + geqcb);
        ck.add(s.epbp, -gpi - gm - geqcb);
        if self.has_sub() {
            ck.add(s.ss, gcsub);
            ck.add(s.scs, -gcsub);
            ck.add(s.sss, -gcsub);
            ck.add(s.scsc, gcsub);
        }
        ck.add(s.bcp, -geqbx);
        ck.add(s.cpb, -geqbx);
    }
    fn conv_ok(&self, ck: &Ckt) -> bool {
        let s0 = self.st;
        let st = &ck.st[0];
        let vbe = self.sign * (ck.x[self.bp] - ck.x[self.ep]);
        let vbc = self.sign * (ck.x[self.bp] - ck.x[self.cp]);
        let delvbe = vbe - st[s0 + VBE];
        let delvbc = vbc - st[s0 + VBC];
        let cchat = st[s0 + CC] + (st[s0 + GM] + st[s0 + GO]) * delvbe
            - (st[s0 + GO] + st[s0 + GMU]) * delvbc;
        let cbhat = st[s0 + CB] + st[s0 + GPI] * delvbe + st[s0 + GMU] * delvbc;
        let (cc, cb) = (st[s0 + CC], st[s0 + CB]);
        let o = &ck.opts;
        (cchat - cc).abs() <= o.reltol * cchat.abs().max(cc.abs()) + o.abstol
            && (cbhat - cb).abs() <= o.reltol * cbhat.abs().max(cb.abs()) + o.abstol
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        let s0 = self.st;
        let st = ac.st;
        let m = &self.model;
        let w = ac.omega;
        let gcpr = if m.rc > 0.0 { self.area / m.rc } else { 0.0 };
        let gepr = if m.re > 0.0 { self.area / m.re } else { 0.0 };
        let gpi = st[s0 + GPI];
        let gmu = st[s0 + GMU];
        let mut gm = st[s0 + GM];
        let go = st[s0 + GO];
        let mut xgm = 0.0;
        let td = m.ptf.to_radians() * m.tf;
        if td != 0.0 {
            let arg = td * w;
            gm += go;
            xgm = -gm * arg.sin();
            gm = gm * arg.cos() - go;
        }
        let gx = st[s0 + GX];
        let xcpi = st[s0 + CAPBE] * w;
        let xcmu = st[s0 + CAPBC] * w;
        let xcbx = st[s0 + CAPBX] * w;
        let xcsub = st[s0 + CAPSUB] * w;
        let xcmcb = st[s0 + GEQCB] * w;
        let s = &self.s;
        let c = |re: f64, im: f64| C64::new(re, im);
        ac.add(s.cc, c(gcpr, 0.0));
        ac.add(s.bb, c(gx, xcbx));
        ac.add(s.ee, c(gepr, 0.0));
        ac.add(s.cpcp, c(gmu + go + gcpr, xcmu + xcbx));
        ac.add(s.bpbp, c(gx + gpi + gmu, xcpi + xcmu + xcmcb));
        ac.add(s.epep, c(gpi + gepr + gm + go, xcpi + xgm));
        ac.add(s.ccp, c(-gcpr, 0.0));
        ac.add(s.bbp, c(-gx, 0.0));
        ac.add(s.eep, c(-gepr, 0.0));
        ac.add(s.cpc, c(-gcpr, 0.0));
        ac.add(s.cpbp, c(-gmu + gm, -xcmu + xgm));
        ac.add(s.cpep, c(-gm - go, -xgm));
        ac.add(s.bpb, c(-gx, 0.0));
        ac.add(s.bpcp, c(-gmu, -xcmu - xcmcb));
        ac.add(s.bpep, c(-gpi, -xcpi));
        ac.add(s.epe, c(-gepr, 0.0));
        ac.add(s.epcp, c(-go, xcmcb));
        ac.add(s.epbp, c(-gpi - gm, -xcpi - xgm - xcmcb));
        if self.has_sub() {
            ac.add(s.ss, c(0.0, xcsub));
            ac.add(s.scs, c(0.0, -xcsub));
            ac.add(s.sss, c(0.0, -xcsub));
            ac.add(s.scsc, c(0.0, xcsub));
        }
        ac.add(s.bcp, c(0.0, -xcbx));
        ac.add(s.cpb, c(0.0, -xcbx));
    }
    fn trunc(&self, ck: &Ckt) -> f64 {
        if !self.reactive() {
            return f64::INFINITY;
        }
        let mut t = ck.trunc_q(self.st + QBE).min(ck.trunc_q(self.st + QBC));
        if self.has_sub() {
            t = t.min(ck.trunc_q(self.st + QSUB));
        }
        t
    }
    /// Collector current (into the collector terminal).
    fn current(&self, ck: &Ckt) -> Option<f64> {
        let m = &self.model;
        if m.rc > 0.0 {
            Some((ck.x[self.nodes[0]] - ck.x[self.cp]) * self.area / m.rc)
        } else {
            let st = &ck.st[0];
            let s0 = self.st;
            let mut cc = st[s0 + CC];
            if ck.mode == Mode::Tran && self.reactive() {
                cc -= st[s0 + QBX + 1];
            }
            Some(self.sign * cc)
        }
    }
    fn noise(&self, ck: &Ckt, _f: f64, out: &mut Vec<NoiseSrc>) {
        let m = &self.model;
        let st = &ck.st[0];
        let s0 = self.st;
        let [c, b, e, _] = self.nodes;
        let kt4 = 4.0 * KB * self.temp_k;
        if m.rc > 0.0 {
            out.push(NoiseSrc {
                name: format!("{}#rc", self.name),
                p: c,
                n: self.cp,
                psd: kt4 * self.area / m.rc,
            });
        }
        if m.rb > 0.0 {
            out.push(NoiseSrc {
                name: format!("{}#rb", self.name),
                p: b,
                n: self.bp,
                psd: kt4 * st[s0 + GX],
            });
        }
        if m.re > 0.0 {
            out.push(NoiseSrc {
                name: format!("{}#re", self.name),
                p: e,
                n: self.ep,
                psd: kt4 * self.area / m.re,
            });
        }
        out.push(NoiseSrc {
            name: format!("{}#ic", self.name),
            p: self.cp,
            n: self.ep,
            psd: 2.0 * Q * st[s0 + CC].abs(),
        });
        out.push(NoiseSrc {
            name: format!("{}#ib", self.name),
            p: self.bp,
            n: self.ep,
            psd: 2.0 * Q * st[s0 + CB].abs(),
        });
    }
    fn set_param(&mut self, name: &str, v: f64) -> bool {
        match name {
            "area" => self.area = v,
            "temp" => self.temp = Some(v),
            _ => {
                self.params.set(name, v);
                self.model = BjtModel::from_params(&self.params, self.sign < 0.0).0;
            }
        }
        self.do_temp(self.circuit_k, self.opt_tnom);
        true
    }
}
