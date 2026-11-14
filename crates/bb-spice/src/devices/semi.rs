//! Shared semiconductor helpers: junction limiting, depletion charge, temperature scaling.

use super::{KB, Q};

/// SPICE `DEVpnjlim`: limits the change of a junction voltage. Returns `(vnew, limited)`.
pub fn pnjlim(vnew: f64, vold: f64, vt: f64, vcrit: f64) -> (f64, bool) {
    if vnew > vcrit && (vnew - vold).abs() > vt + vt {
        if vold > 0.0 {
            let arg = 1.0 + (vnew - vold) / vt;
            if arg > 0.0 {
                (vold + vt * arg.ln(), true)
            } else {
                (vcrit, true)
            }
        } else {
            (vt * (vnew / vt).ln(), true)
        }
    } else if vnew < 0.0 {
        // ngspice: limit large negative steps too
        let arg = if vold > 0.0 {
            -vold - 1.0
        } else {
            2.0 * vold - 1.0
        };
        if vnew < arg {
            (arg, true)
        } else {
            (vnew, false)
        }
    } else {
        (vnew, false)
    }
}

/// SPICE `DEVfetlim`: limits gate-source voltage changes of FETs.
pub fn fetlim(vnew: f64, vold: f64, vto: f64) -> f64 {
    let vtsthi = (2.0 * (vold - vto)).abs() + 2.0;
    let vtstlo = vtsthi / 2.0 + 2.0;
    let vtox = vto + 3.5;
    let delv = vnew - vold;
    let mut v = vnew;
    if vold >= vto {
        if vold >= vtox {
            if delv <= 0.0 {
                // going off
                if vnew >= vtox {
                    if -delv > vtstlo {
                        v = vold - vtstlo;
                    }
                } else {
                    v = vnew.max(vto + 2.0);
                }
            } else if delv >= vtsthi {
                v = vold + vtsthi;
            }
        } else {
            // middle region
            if delv <= 0.0 {
                v = vnew.max(vto - 0.5);
            } else {
                v = vnew.min(vto + 4.0);
            }
        }
    } else {
        // off
        if delv <= 0.0 {
            if -delv > vtsthi {
                v = vold - vtsthi;
            }
        } else {
            let vtemp = vto + 0.5;
            if vnew <= vtemp {
                if delv > vtstlo {
                    v = vold + vtstlo;
                }
            } else {
                v = vtemp;
            }
        }
    }
    v
}

/// SPICE `DEVlimvds`: limits drain-source voltage changes.
pub fn limvds(vnew: f64, vold: f64) -> f64 {
    if vold >= 3.5 {
        if vnew > vold {
            vnew.min(3.0 * vold + 2.0)
        } else if vnew < 3.5 {
            vnew.max(2.0)
        } else {
            vnew
        }
    } else if vnew > vold {
        vnew.min(4.0)
    } else {
        vnew.max(-0.5)
    }
}

/// Critical voltage `vt·ln(vt/(√2·is))`.
pub fn vcrit(vt: f64, is: f64) -> f64 {
    vt * (vt / (std::f64::consts::SQRT_2 * is)).ln()
}

/// Depletion charge and capacitance of a junction (SPICE: linear extension above `fc·vj`).
/// `f1 = vj(1-(1-fc)^(1-m))/(1-m)`, `f2 = (1-fc)^(1+m)`, `f3 = 1-fc(1+m)`.
pub fn depletion(v: f64, cj: f64, vj: f64, m: f64, fc: f64) -> (f64, f64) {
    if cj == 0.0 {
        return (0.0, 0.0);
    }
    let fcpb = fc * vj;
    if v < fcpb {
        let arg = 1.0 - v / vj;
        let sarg = (-m * arg.ln()).exp();
        let q = vj * cj * (1.0 - arg * sarg) / (1.0 - m);
        (q, cj * sarg)
    } else {
        let f1 = vj * (1.0 - (1.0 - fc).powf(1.0 - m)) / (1.0 - m);
        let f2 = (1.0 - fc).powf(1.0 + m);
        let f3 = 1.0 - fc * (1.0 + m);
        let czof2 = cj / f2;
        let q = cj * f1 + czof2 * (f3 * (v - fcpb) + (m / (vj + vj)) * (v * v - fcpb * fcpb));
        let c = czof2 * (f3 + m * v / vj);
        (q, c)
    }
}

/// Energy gap of silicon at temperature `t` (K), SPICE formula.
pub fn egap(t: f64) -> f64 {
    1.16 - (7.02e-4 * t * t) / (t + 1108.0)
}

/// Temperature-adjusted junction potential and capacitance (SPICE `DIOtemp`/`BJTtemp` style).
/// Returns `(vj(T), cj(T))`.
pub fn junction_temp(vj: f64, cj: f64, m: f64, tnom: f64, temp: f64) -> (f64, f64) {
    let vt = KB * temp / Q;
    let fact1 = tnom / 300.15;
    let vtnom = KB * tnom / Q;
    let egfet1 = egap(tnom);
    let arg1 = -egfet1 / (2.0 * KB * tnom) + 1.1150877 / (KB * (300.15 + 300.15));
    let pbfact1 = -2.0 * vtnom * (1.5 * fact1.ln() + Q * arg1);
    let pbo = (vj - pbfact1) / fact1;
    let gmaold = (vj - pbo) / pbo;
    let fact2 = temp / 300.15;
    let egfet = egap(temp);
    let arg = -egfet / (2.0 * KB * temp) + 1.1150877 / (KB * (300.15 + 300.15));
    let pbfact = -2.0 * vt * (1.5 * fact2.ln() + Q * arg);
    let vj_t = fact2 * pbo + pbfact;
    let gmanew = (vj_t - pbo) / pbo;
    let cj_t = cj / (1.0 + m * (4e-4 * (tnom - 300.15) - gmaold))
        * (1.0 + m * (4e-4 * (temp - 300.15) - gmanew));
    (vj_t, cj_t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn junction_temp_identity_at_tnom() {
        let (vj, cj) = junction_temp(0.75, 1e-12, 0.33, 300.15, 300.15);
        assert!((vj - 0.75).abs() < 1e-12);
        assert!((cj - 1e-12).abs() < 1e-24);
    }

    #[test]
    fn depletion_continuous_at_fc() {
        let (q1, c1) = depletion(0.5 * 0.8 - 1e-9, 1e-12, 0.8, 0.5, 0.5);
        let (q2, c2) = depletion(0.5 * 0.8 + 1e-9, 1e-12, 0.8, 0.5, 0.5);
        assert!((q1 - q2).abs() < 1e-20);
        assert!((c1 - c2).abs() / c1 < 1e-6);
    }

    #[test]
    fn pnjlim_limits_big_steps() {
        let vt = 0.025852;
        let (v, lim) = pnjlim(5.0, 0.6, vt, 0.6);
        assert!(lim && v < 0.8);
        let (v, lim) = pnjlim(0.61, 0.6, vt, 0.6);
        assert!(!lim && v == 0.61);
    }
}
