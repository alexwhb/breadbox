//! Model stamp tests: for every device, the DC matrix stamped by `load` must equal the
//! finite-difference Jacobian of the currents it stamps (`F(x) = A(x)·x − b(x)`), and the AC
//! susceptances must equal the transient charge Jacobian (`C = (A_tran − A_dc)·h` for a
//! backward-Euler step `h`).

use crate::analysis::AcSystem;
use crate::devices::{Dev, Init, Mode};
use crate::elab::Method;
use crate::engine::Engine;
use crate::netlist::Netlist;

fn engine(text: &str) -> Engine {
    let nl = Netlist::parse(text).unwrap_or_else(|e| panic!("{e}"));
    let c = nl.elaborate().unwrap_or_else(|e| panic!("{e}"));
    Engine::new(&c).unwrap_or_else(|e| panic!("{e}"))
}

fn dense(e: &Engine) -> Vec<Vec<f64>> {
    let n = e.n();
    let mut a = vec![vec![0.0; n + 1]; n + 1];
    for s in 1..e.pattern.len() {
        let (r, c) = e.pattern.position(crate::matrix::Slot(s as u32));
        a[r][c] += e.ck.mat.vals[s];
    }
    a
}

/// F(x) = A(x) x − b(x) after a load at x (states reset to `st0` first so limiting is idle).
fn residual(e: &mut Engine, x: &[f64], st0: &[f64]) -> (Vec<f64>, Vec<Vec<f64>>) {
    e.ck.x.copy_from_slice(x);
    e.ck.st[0].copy_from_slice(st0);
    e.ck.init = Init::Float;
    e.load();
    let a = dense(e);
    let n = e.n();
    let mut f = vec![0.0; n + 1];
    for i in 1..=n {
        let mut s = -e.ck.rhs[i];
        for j in 1..=n {
            s += a[i][j] * x[j];
        }
        f[i] = s;
    }
    (f, a)
}

/// Solves the operating point, optionally shifts some unknowns, and compares Jacobians.
fn check_dc_jacobian(text: &str, shift: &[(&str, f64)]) {
    let mut e = engine(text);
    e.solve_op("test op")
        .unwrap_or_else(|err| panic!("{text}\n{err}"));
    let mut x = e.ck.x.clone();
    for (name, dv) in shift {
        let i = e
            .names
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("no unknown {name}: {:?}", e.names));
        x[i] += dv;
    }
    // base load establishes state[0] (old junction voltages) at x
    e.ck.x.copy_from_slice(&x);
    e.ck.init = Init::Float;
    e.load();
    let st0 = e.ck.st[0].clone();
    let (_, a) = residual(&mut e, &x, &st0);
    let n = e.n();
    for j in 1..=n {
        let h = 1e-6 * x[j].abs().max(1e-3);
        let mut xp = x.clone();
        xp[j] += h;
        let mut xm = x.clone();
        xm[j] -= h;
        let (fp, _) = residual(&mut e, &xp, &st0);
        let (fm, _) = residual(&mut e, &xm, &st0);
        for i in 1..=n {
            let fd = (fp[i] - fm[i]) / (2.0 * h);
            let an = a[i][j];
            let scale = a[i].iter().map(|v| v.abs()).fold(0.0, f64::max).max(1e-9);
            assert!(
                (fd - an).abs() <= 2e-4 * scale + 1e-12,
                "{}\nd F[{}] / d {} : analytic {an:.9e} vs finite-difference {fd:.9e}",
                text.lines().next().unwrap_or(""),
                e.names[i],
                e.names[j]
            );
        }
    }
}

/// AC imaginary part / ω must match the transient charge Jacobian, and the AC real part the
/// DC Jacobian.
fn check_ac_vs_tran(text: &str) {
    let mut e = engine(text);
    e.ck.method = Method::BackwardEuler;
    e.solve_op("test op").unwrap();
    e.ck.init = Init::Float;
    e.load();
    let a_dc = dense(&e);
    let st0 = e.ck.st[0].clone();
    // AC at a low frequency
    let omega = 1.0;
    let mut sys = AcSystem::new(&e);
    let _ = sys.load(&e, omega);
    let n = e.n();
    let mut y = vec![vec![crate::matrix::C64::default(); n + 1]; n + 1];
    for s in 1..e.pattern.len() {
        let (r, c) = e.pattern.position(crate::matrix::Slot(s as u32));
        y[r][c] += sys.mat.vals[s];
    }
    // transient BE step of size h from the operating point
    let h = 1e-12;
    e.ck.mode = Mode::Tran;
    e.ck.delta = h;
    e.ck.delta_old = [h; 4];
    e.ck.order = 1;
    e.ck.compute_coeffs();
    for k in 1..4 {
        e.ck.st[k].copy_from_slice(&st0);
    }
    e.ck.st[0].copy_from_slice(&st0);
    e.ck.init = Init::Float;
    e.load();
    let a_tr = dense(&e);
    for i in 1..=n {
        for j in 1..=n {
            let c_tr = (a_tr[i][j] - a_dc[i][j]) * h;
            let c_ac = y[i][j].im / omega;
            let cscale = y[i]
                .iter()
                .map(|v| v.im.abs())
                .fold(0.0, f64::max)
                .max(1e-18);
            assert!(
                (c_tr - c_ac).abs() <= 1e-3 * cscale + 1e-20,
                "{}\nC[{}][{}]: transient {c_tr:.6e} vs AC {c_ac:.6e}",
                text.lines().next().unwrap_or(""),
                e.names[i],
                e.names[j]
            );
            let g_scale = a_dc[i]
                .iter()
                .map(|v| v.abs())
                .fold(0.0, f64::max)
                .max(1e-12);
            assert!(
                (y[i][j].re - a_dc[i][j]).abs() <= 1e-6 * g_scale + 1e-15,
                "{}\nG[{}][{}]: AC {:.6e} vs DC {:.6e}",
                text.lines().next().unwrap_or(""),
                e.names[i],
                e.names[j],
                y[i][j].re,
                a_dc[i][j]
            );
        }
    }
}

const DIODE: &str = ".model dd D(IS=1e-14 N=1.3 RS=5 CJO=2p VJ=0.7 M=0.4 TT=5n BV=6 IBV=1m)\n";
// RB = RBM: SPICE (like ngspice) treats the base resistance as constant in the Jacobian, so
// the bias-dependent forms (IRB, RB > RBM with qb) are exercised by the reference suite only.
const NPN: &str = ".model qn NPN(IS=1e-15 BF=120 NF=1.02 VAF=60 IKF=0.05 ISE=1e-14 NE=1.6 BR=3 NR=1.01 VAR=25 IKR=0.02 ISC=1e-13 NC=1.8 RB=40 RBM=40 RE=0.6 RC=3 CJE=8p VJE=0.8 MJE=0.4 CJC=4p VJC=0.6 MJC=0.35 XCJC=0.7 TF=0.4n XTF=2 VTF=4 ITF=0.1 TR=20n CJS=2p)\n";
const NMOS: &str = ".model nm NMOS(VTO=0.8 KP=100u GAMMA=0.5 PHI=0.65 LAMBDA=0.05 RD=5 RS=5 TOX=20n CGSO=0.3n CGDO=0.3n CGBO=0.1n CJ=0.3m CJSW=0.3n MJ=0.5 MJSW=0.33)\n";
const PMOS: &str =
    ".model pm PMOS(VTO=-0.9 KP=40u GAMMA=0.4 LAMBDA=0.04 TOX=20n CBD=10f CBS=10f)\n";
const NJF: &str = ".model jn NJF(VTO=-2 BETA=1m LAMBDA=0.02 RD=4 RS=4 CGS=3p CGD=2p B=0.8)\n";

#[test]
fn diode_forward_reverse_breakdown() {
    for (v, label) in [(0.9, "forward"), (-3.0, "reverse"), (-8.0, "breakdown")] {
        let t = format!("diode {label}\nV1 a 0 DC {v}\nR1 a k 50\nD1 k 0 dd\n{DIODE}");
        check_dc_jacobian(&t, &[("v(k)", 0.01)]);
    }
    check_ac_vs_tran(&format!(
        "diode ac\nV1 a 0 DC 0.75\nR1 a k 50\nD1 k 0 dd\n{DIODE}"
    ));
}

#[test]
fn bjt_all_regions() {
    for (vb, vc, label) in [
        (0.75, 5.0, "active"),
        (0.85, 0.1, "saturation"),
        (-1.0, 3.0, "cutoff"),
        (0.7, -1.0, "reverse"),
    ] {
        let t = format!(
            "bjt {label}\nVB b0 0 DC {vb}\nRB b0 b 100\nVC c0 0 DC {vc}\nRC c0 c 100\nQ1 c b 0 s qn\nVS s 0 DC -2\n{NPN}"
        );
        check_dc_jacobian(&t, &[("v(b)", 0.003), ("v(q1#base)", 0.002)]);
    }
    check_ac_vs_tran(&format!(
        "bjt ac\nVB b0 0 DC 0.75\nRB b0 b 1k\nVC c0 0 DC 3\nRC c0 c 1k\nQ1 c b 0 s qn\nVS s 0 DC -2\n{NPN}"
    ));
    // PNP mirror
    let t = format!(
        "pnp\nVE e 0 DC 5\nVB b 0 DC 4.2\nVC c 0 DC 1\nQ1 c b e qp\n.model qp PNP(IS=1e-15 BF=80 VAF=40 RB=10 CJE=1p CJC=1p TF=1n)\n"
    );
    check_dc_jacobian(&t, &[]);
    check_ac_vs_tran(&t);
}

#[test]
fn mosfet_regions_and_reverse_mode() {
    for (vg, vd, vb, label) in [
        (2.0, 3.0, 0.0, "saturation"),
        (3.0, 0.3, -0.5, "linear"),
        (0.3, 2.0, 0.0, "cutoff"),
        (2.5, -0.8, -1.0, "reverse"),
    ] {
        let t = format!(
            "nmos {label}\nVG g 0 DC {vg}\nVD d0 0 DC {vd}\nRD d0 d 100\nVB b 0 DC {vb}\nM1 d g 0 b nm L=1u W=10u AD=10p AS=10p PD=12u PS=12u\n{NMOS}"
        );
        check_dc_jacobian(&t, &[("v(d)", 0.01)]);
        check_ac_vs_tran(&t);
    }
    let t = format!(
        "pmos\nVS s 0 DC 3.3\nVG g 0 DC 1.5\nVD d0 0 DC 1\nRD d0 d 100\nM1 d g s s pm L=1u W=20u\n{PMOS}"
    );
    check_dc_jacobian(&t, &[("v(d)", 0.02)]);
    check_ac_vs_tran(&t);
}

#[test]
fn jfet_regions() {
    for (vg, vd, label) in [
        (-0.5, 5.0, "saturation"),
        (-0.5, 0.4, "linear"),
        (-3.0, 2.0, "cutoff"),
        (-0.5, -0.6, "reverse"),
    ] {
        let t = format!(
            "jfet {label}\nVG g 0 DC {vg}\nVD d0 0 DC {vd}\nRD d0 d 100\nJ1 d g 0 jn\n{NJF}"
        );
        check_dc_jacobian(&t, &[("v(d)", 0.01)]);
    }
    check_ac_vs_tran(&format!(
        "jfet ac\nVG g 0 DC -0.5\nVD d0 0 DC 5\nRD d0 d 1k\nJ1 d g 0 jn\n{NJF}"
    ));
}

#[test]
fn behavioural_and_controlled_sources() {
    let t = "b sources\nV1 x 0 DC 0.4\nR1 x 0 1k\nB1 a 0 V=exp(v(x))*sin(v(x)*3)+pwr(v(x)+1,1.5)\nR2 a 0 1k\nB2 0 b I=1m*tanh(v(a)-v(x))+i(V2)*0.1\nV2 b c 0\nR3 c 0 1k\nE1 e 0 a x 2\nRe e 0 1k\nG1 0 g a 0 1m\nRg g 0 1k\nF1 0 f V2 3\nRf f 0 1k\nH1 h 0 V2 500\nRh h 0 1k\n";
    check_dc_jacobian(t, &[("v(a)", 0.01)]);
    check_ac_vs_tran(t);
}

#[test]
fn passives_switches_tline_coupled() {
    let t = "passives\nV1 a 0 DC 1\nR1 a b 1k\nC1 b 0 1n\nL1 b c 1m\nL2 d 0 2m\nK1 L1 L2 0.9\nR2 c 0 50\nR3 d 0 50\nS1 b e a 0 sw\nRe e 0 1k\n.model sw SW(VT=0.5 VH=0.1 RON=1 ROFF=1meg)\n";
    check_dc_jacobian(t, &[]);
    check_ac_vs_tran(t);
    // the line's AC model is the exact delay e^(−jωTD), not a capacitance: DC only here
    check_dc_jacobian(
        "tline\nV1 a 0 DC 1\nR1 a b 50\nT1 b 0 t 0 Z0=50 TD=1n\nRt t 0 75\n",
        &[],
    );
}

#[test]
fn temperature_scaling_matches_spice_formulas() {
    // diode saturation current at 127 °C: IS·exp((T/Tnom − 1)·EG/(N·Vt))·(T/Tnom)^(XTI/N)
    let mut e =
        engine("d\nI1 0 a DC 1m\nD1 a 0 dt\n.model dt D(IS=1e-15 N=1 EG=1.11 XTI=3)\n.temp 127\n");
    e.solve_op("op").unwrap();
    let t = 400.15f64;
    let tn = 300.15f64;
    let vt = super::KB * t / super::Q;
    let is = 1e-15 * ((t / tn - 1.0) * 1.11 / vt).exp() * (t / tn).powi(3);
    let vd = vt * (1e-3 / is + 1.0).ln();
    let a = e.node("a").unwrap();
    assert!((e.ck.x[a] - vd).abs() < 1e-6, "{} vs {vd}", e.ck.x[a]);
    // resistor tempco
    let mut e = engine("r\nI1 0 a DC 1m\nR1 a 0 1k tc1=3m tc2=1e-5\n.temp 77\n");
    e.solve_op("op").unwrap();
    let dt = 50.0;
    let r = 1e3 * (1.0 + 3e-3 * dt + 1e-5 * dt * dt);
    assert!((e.ck.x[e.node("a").unwrap()] - 1e-3 * r).abs() < 1e-9);
}

#[test]
fn level3_mosfet_falls_back_with_warning() {
    let nl = Netlist::parse("t\nVD d 0 1\nM1 d d 0 0 m3 L=1u W=1u\n.model m3 NMOS(LEVEL=3 VTO=0.7 KP=50u THETA=0.1)\n.op\n").unwrap();
    let r = nl.run().unwrap();
    assert!(
        r.warnings
            .iter()
            .any(|w| w.msg.contains("level 3") && w.msg.contains("level-1")),
        "{:?}",
        r.warnings
    );
    assert!(r.op("i(vd)").unwrap() < 0.0);
}
