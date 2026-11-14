//! SPICE junction diode (ngspice `dio` level 1): IS, N, RS, CJO, VJ, M, TT, FC, BV, IBV, NBV,
//! EG, XTI with temperature scaling, breakdown, depletion + diffusion charge, shot/thermal noise.

use super::semi::{depletion, egap, pnjlim, vcrit};
use super::*;

const KNOWN: &[&str] = &[
    "is", "js", "n", "rs", "cjo", "cj0", "cj", "vj", "pb", "m", "mj", "tt", "fc", "bv", "vb",
    "ibv", "ib", "nbv", "eg", "xti", "tnom", "tref", "area",
];

/// Model parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct DiodeModel {
    pub is: f64,
    pub n: f64,
    pub rs: f64,
    pub cjo: f64,
    pub vj: f64,
    pub m: f64,
    pub tt: f64,
    pub fc: f64,
    pub bv: Option<f64>,
    pub ibv: f64,
    pub nbv: f64,
    pub eg: f64,
    pub xti: f64,
    pub tnom: Option<f64>,
}

impl DiodeModel {
    pub fn from_params(p: &ParamMap) -> (DiodeModel, Vec<String>) {
        let n = p.or(&["n"], 1.0);
        let m = DiodeModel {
            is: p.or(&["is", "js"], 1e-14),
            n,
            rs: p.or(&["rs"], 0.0),
            cjo: p.or(&["cjo", "cj0", "cj"], 0.0),
            vj: p.or(&["vj", "pb"], 1.0),
            m: p.or(&["m", "mj"], 0.5),
            tt: p.or(&["tt"], 0.0),
            fc: p.or(&["fc"], 0.5),
            bv: p.get(&["bv", "vb"]).filter(|v| *v != 0.0),
            ibv: p.or(&["ibv", "ib"], 1e-3),
            nbv: p.or(&["nbv"], n),
            eg: p.or(&["eg"], 1.11),
            xti: p.or(&["xti"], 3.0),
            tnom: p.get(&["tnom", "tref"]),
        };
        (m, p.unknown(KNOWN))
    }
}

#[derive(Debug, Clone)]
pub struct Diode {
    pub name: String,
    pub a: usize,
    pub k: usize,
    ap: usize,
    pub model: DiodeModel,
    pub params: ParamMap,
    pub area: f64,
    pub off: bool,
    pub ic: Option<f64>,
    pub temp: Option<f64>,
    pub dtemp: f64,
    // temperature-adjusted
    t_is: f64,
    t_vj: f64,
    t_cj: f64,
    t_bv: f64,
    t_vcrit: f64,
    vte: f64,
    vtebrk: f64,
    gspr: f64,
    temp_k: f64,
    circuit_k: f64,
    opt_tnom: f64,
    st: usize,
    s_rs: G4,
    s_j: G4,
    // last small-signal values
    gd: f64,
    capd: f64,
}

// state offsets
const VD: usize = 0;
const CD: usize = 1;
const GD: usize = 2;
const QC: usize = 3;

impl Diode {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        a: usize,
        k: usize,
        params: ParamMap,
        area: f64,
        off: bool,
        ic: Option<f64>,
        temp: Option<f64>,
        dtemp: f64,
    ) -> (Self, Vec<String>) {
        let (model, warn) = DiodeModel::from_params(&params);
        let d = Diode {
            name,
            a,
            k,
            ap: a,
            model,
            params,
            area,
            off,
            ic,
            temp,
            dtemp,
            t_is: 0.0,
            t_vj: 0.0,
            t_cj: 0.0,
            t_bv: 0.0,
            t_vcrit: 0.0,
            vte: 0.0,
            vtebrk: 0.0,
            gspr: 0.0,
            temp_k: 300.15,
            circuit_k: 300.15,
            opt_tnom: 27.0,
            st: 0,
            s_rs: G4::default(),
            s_j: G4::default(),
            gd: 0.0,
            capd: 0.0,
        };
        (d, warn)
    }

    fn do_temp(&mut self, circuit_k: f64, opts_tnom: f64) {
        self.circuit_k = circuit_k;
        self.opt_tnom = opts_tnom;
        let md = &self.model;
        let tnom = md.tnom.unwrap_or(opts_tnom) + KELVIN;
        let t = self.temp.map(|t| t + KELVIN).unwrap_or(circuit_k) + self.dtemp;
        self.temp_k = t;
        let vt = KB * t / Q;
        self.vte = md.n * vt;
        self.vtebrk = md.nbv * vt;
        // saturation current
        self.t_is = md.is
            * self.area
            * (((t / tnom) - 1.0) * md.eg / (md.n * vt) + md.xti / md.n * (t / tnom).ln()).exp();
        // junction potential and capacitance (ngspice DIOtemp)
        let (vj, cj) = super::semi::junction_temp(md.vj, md.cjo, md.m, tnom, t);
        let _ = egap(t);
        self.t_vj = vj;
        self.t_cj = cj * self.area;
        self.t_vcrit = vcrit(self.vte, self.t_is);
        self.gspr = if md.rs > 0.0 { self.area / md.rs } else { 0.0 };
        // breakdown voltage matched to IBV
        self.t_bv = 0.0;
        if let Some(bv) = md.bv {
            let bv = bv.abs();
            let mut cbv = md.ibv * self.area;

            let xbv = if cbv < self.t_is * bv / vt {
                cbv = self.t_is * bv / vt;
                let _ = cbv;
                bv
            } else {
                let tol = 1e-3 * cbv;
                let mut x = bv - self.vtebrk * (1.0 + cbv / self.t_is).ln();
                for _ in 0..25 {
                    x = bv - self.vtebrk * (cbv / self.t_is + 1.0 - x / vt).ln();
                    let xcbv = self.t_is * (((bv - x) / self.vtebrk).exp() - 1.0 + x / vt);
                    if (xcbv - cbv).abs() <= tol {
                        break;
                    }
                }
                x
            };
            self.t_bv = xbv;
        }
    }

    /// Junction current/conductance at `vd` (without gmin).
    fn iv(&self, vd: f64) -> (f64, f64) {
        let csat = self.t_is;
        let vte = self.vte;
        if vd >= -3.0 * vte {
            let evd = (vd / vte).exp();
            (csat * (evd - 1.0), csat * evd / vte)
        } else if self.model.bv.is_none() || vd >= -self.t_bv {
            let arg = 3.0 * vte / (vd * std::f64::consts::E);
            let arg = arg * arg * arg;
            (-csat * (1.0 + arg), csat * 3.0 * arg / vd)
        } else {
            let evrev = (-(self.t_bv + vd) / self.vtebrk).exp();
            (-csat * evrev, csat * evrev / self.vtebrk)
        }
    }

    fn charge(&self, vd: f64, cd: f64, gd: f64) -> (f64, f64) {
        let (qd, cdep) = depletion(vd, self.t_cj, self.t_vj, self.model.m, self.model.fc);
        (qd + self.model.tt * cd, cdep + self.model.tt * gd)
    }
}

impl Dev for Diode {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_unknowns(&mut self, s: &mut Setup) -> Result<(), SimError> {
        if self.model.rs > 0.0 {
            self.ap = s.alloc(format!("{}#anode", self.name), UKind::Node);
        }
        self.st = s.states(5);
        Ok(())
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.s_rs = s.g4(self.a, self.ap);
        self.s_j = s.g4(self.ap, self.k);
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
        self.t_cj > 0.0 || self.model.tt > 0.0
    }
    fn load(&mut self, ck: &mut Ckt) {
        let s = self.st;
        let mut check = false;
        let vd = match ck.init {
            Init::Tran => ck.st[1][s + VD],
            Init::Jct if ck.tran_op && ck.uic => self.ic.unwrap_or(0.0),
            Init::Jct if self.off => 0.0,
            Init::Jct => self.t_vcrit,
            Init::Fix if self.off => 0.0,
            _ => {
                let vd = if ck.init == Init::Pred {
                    ck.st[0][s + VD] = ck.st[1][s + VD];
                    ck.st[0][s + CD] = ck.st[1][s + CD];
                    ck.st[0][s + GD] = ck.st[1][s + GD];
                    ck.predict(s + VD)
                } else {
                    ck.x[self.ap] - ck.x[self.k]
                };
                let old = ck.st[0][s + VD];
                let (vnew, lim) =
                    if self.model.bv.is_some() && vd < (-self.t_bv + 10.0 * self.vtebrk).min(0.0) {
                        let vdtemp = -(vd + self.t_bv);
                        let (v, l) = pnjlim(vdtemp, -(old + self.t_bv), self.vtebrk, self.t_vcrit);
                        (-(v + self.t_bv), l)
                    } else {
                        pnjlim(vd, old, self.vte, self.t_vcrit)
                    };
                check = lim;
                vnew
            }
        };
        let (mut cd, mut gd) = self.iv(vd);
        let (cdb, gdb) = (cd, gd);
        cd += ck.gmin * vd;
        gd += ck.gmin;
        let reactive = self.is_reactive();
        if reactive {
            let (q, capd) = self.charge(vd, cdb, gdb);
            self.capd = capd;
            ck.st[0][s + QC] = q;
            if ck.mode == Mode::Tran {
                if ck.init == Init::Tran {
                    ck.st[1][s + QC] = ck.st[0][s + QC];
                }
                let (geq, ccap) = ck.integrate(s + QC, capd);
                gd += geq;
                cd += ccap;
                if ck.init == Init::Tran {
                    ck.st[1][s + QC + 1] = ck.st[0][s + QC + 1];
                }
            }
        }
        if check && (ck.init != Init::Fix || !self.off) {
            ck.nonconverged();
        }
        ck.st[0][s + VD] = vd;
        ck.st[0][s + CD] = cd;
        ck.st[0][s + GD] = gd;
        self.gd = gdb + ck.gmin;
        let cdeq = cd - gd * vd;
        ck.add_rhs(self.k, cdeq);
        ck.add_rhs(self.ap, -cdeq);
        self.s_j.stamp(ck, gd);
        if self.gspr > 0.0 {
            self.s_rs.stamp(ck, self.gspr);
        }
    }
    fn conv_ok(&self, ck: &Ckt) -> bool {
        let s = self.st;
        let vd = ck.x[self.ap] - ck.x[self.k];
        let delvd = vd - ck.st[0][s + VD];
        let cd = ck.st[0][s + CD];
        let cdhat = cd + ck.st[0][s + GD] * delvd;
        let tol = ck.opts.reltol * cdhat.abs().max(cd.abs()) + ck.opts.abstol;
        (cdhat - cd).abs() <= tol
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        self.s_j
            .stamp_ac(ac, C64::new(self.gd, ac.omega * self.capd));
        if self.gspr > 0.0 {
            self.s_rs.stamp_ac(ac, C64::new(self.gspr, 0.0));
        }
    }
    fn trunc(&self, ck: &Ckt) -> f64 {
        if self.is_reactive() {
            ck.trunc_q(self.st + QC)
        } else {
            f64::INFINITY
        }
    }
    fn current(&self, ck: &Ckt) -> Option<f64> {
        Some(ck.st[0][self.st + CD])
    }
    fn noise(&self, ck: &Ckt, _f: f64, out: &mut Vec<NoiseSrc>) {
        let id = self.iv(ck.st[0][self.st + VD]).0;
        out.push(NoiseSrc {
            name: format!("{}#shot", self.name),
            p: self.ap,
            n: self.k,
            psd: 2.0 * Q * id.abs(),
        });
        if self.gspr > 0.0 {
            out.push(NoiseSrc {
                name: format!("{}#rs", self.name),
                p: self.a,
                n: self.ap,
                psd: 4.0 * KB * self.temp_k * self.gspr,
            });
        }
    }
    fn set_param(&mut self, name: &str, v: f64) -> bool {
        match name {
            "area" => self.area = v,
            "temp" => self.temp = Some(v),
            _ => {
                self.params.set(name, v);
                self.model = DiodeModel::from_params(&self.params).0;
            }
        }
        self.do_temp(self.circuit_k, self.opt_tnom);
        true
    }
}
