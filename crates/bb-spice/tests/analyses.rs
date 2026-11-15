//! Analysis-level behaviour: convergence aids and diagnostics, Monte Carlo statistics,
//! `.step`/`.temp` loops, `.meas`, export round trips and the Pro-screen netlist.

use bb_spice::export::{ComplexCsv, plot_to_csv, read_raw, results_to_raw};
use bb_spice::{Library, Netlist, PlotKind, SimError};
use std::sync::Arc;

fn run(text: &str) -> bb_spice::Results {
    Netlist::parse(text)
        .unwrap_or_else(|e| panic!("{e}"))
        .run()
        .unwrap_or_else(|e| panic!("{e}"))
}

// ---------------------------------------------------------------------------------------------
// Convergence
// ---------------------------------------------------------------------------------------------

#[test]
fn gmin_stepping_rescues_a_hard_operating_point() {
    // A diode stack straight across a stiff source: plain Newton is not allowed enough
    // iterations, so gmin stepping must take over.
    let text = "stack\nV1 a 0 DC 50\nR1 a b 0.1\nD1 b c dd\nD2 c d dd\nD3 d 0 dd\n.model dd D(IS=1e-16 N=1)\n.options itl1=4\n.op\n";
    let r = run(text);
    let v = r.op("v(b)").unwrap();
    assert!(v > 2.0 && v < 50.0, "v(b) = {v}");
    assert!(r.stats.convergence_warnings > 0);
    assert!(
        r.warnings
            .iter()
            .any(|w| w.msg.contains("gmin stepping") || w.msg.contains("source stepping")),
        "{:?}",
        r.warnings
    );
}

#[test]
fn noopiter_goes_straight_to_gmin_stepping() {
    let r = run("t\nV1 a 0 5\nR1 a b 1k\nD1 b 0 dd\n.model dd D\n.options noopiter\n.op\n");
    assert!((r.op("v(b)").unwrap() - 0.68).abs() < 0.1);
    assert!(r.stats.convergence_warnings > 0);
}

#[test]
fn bistable_latch_settles_in_one_state_and_nodeset_picks_it() {
    let base = "latch\nVCC vcc 0 DC 5\nRC1 vcc c1 1k\nRC2 vcc c2 1k\nRB1 c2 b1 10k\nRB2 c1 b2 10k\nQ1 c1 b1 0 qn\nQ2 c2 b2 0 qn\n.model qn NPN(IS=1e-15 BF=100)\n";
    for (ns, low, high) in [
        ("v(c1)=0.1 v(c2)=5", "v(c1)", "v(c2)"),
        ("v(c1)=5 v(c2)=0.1", "v(c2)", "v(c1)"),
    ] {
        let r = run(&format!("{base}.nodeset {ns}\n.op\n"));
        assert!(
            r.op(low).unwrap() < 0.3,
            "{ns}: {low} = {}",
            r.op(low).unwrap()
        );
        assert!(
            r.op(high).unwrap() > 4.0,
            "{ns}: {high} = {}",
            r.op(high).unwrap()
        );
    }
}

#[test]
fn schmitt_trigger_transient_converges_through_regeneration() {
    let r = run(
        "schmitt\nVCC vcc 0 DC 5\nVIN in 0 PWL(0 0 1m 5 2m 0)\nRB in b1 1k\nRC1 vcc c1 2.2k\nR12 c1 b2 4.7k\nR2G b2 0 10k\nRC2 vcc out 1k\nRE e 0 470\nQ1 c1 b1 e qn\nQ2 out b2 e qn\n.model qn NPN(IS=1e-15 BF=100 VAF=50 CJE=2p CJC=1p TF=0.5n)\n.tran 2u 2m\n",
    );
    let p = r.plot(PlotKind::Tran).unwrap();
    let out = p.real("v(out)").unwrap();
    assert!(out.iter().any(|&v| v < 2.0) && out.iter().any(|&v| v > 4.5));
}

#[test]
fn floating_node_reports_the_node() {
    let e = Netlist::parse("t\nV1 a 0 1\nR1 a 0 1k\nC1 float b 1n\nC2 b 0 1n\n.op\n")
        .unwrap()
        .run()
        .unwrap_err();
    let msg = e.to_string();
    assert!(matches!(e, SimError::Singular { .. }), "{msg}");
    assert!(msg.contains("V(float)") || msg.contains("V(b)"), "{msg}");
}

#[test]
fn voltage_source_loop_is_reported() {
    let e = Netlist::parse("t\nV1 a 0 1\nV2 a 0 2\n.op\n")
        .unwrap()
        .run()
        .unwrap_err();
    assert!(matches!(e, SimError::Singular { .. }), "{e}");
}

#[test]
fn impossible_circuit_names_elements_and_nodes() {
    // a comparator in a positive-feedback loop with no stable solution in DC
    let e = Netlist::parse(
        "t\nB1 a 0 V=v(a) > 0.5 ? 0 : 1\nR1 a 0 1k\n.options gminsteps=0 srcsteps=0\n.op\n",
    )
    .unwrap()
    .run()
    .unwrap_err();
    let msg = e.to_string();
    match e {
        SimError::NoConvergence { report, .. } => {
            assert!(!report.unknowns.is_empty(), "{msg}");
            assert!(msg.contains("V(a)") || msg.contains("I(b1)"), "{msg}");
        }
        other => panic!("unexpected {other}"),
    }
}

#[test]
fn timestep_cut_recovers_and_too_small_is_diagnosed() {
    // a hard switching event: the step is cut but the run finishes
    let r = run(
        "t\nV1 in 0 PULSE(0 5 1u 1p 1p 5u 10u)\nR1 in a 10\nD1 a b dd\nC1 b 0 1n\nR2 b 0 100\n.model dd D(IS=1e-14 RS=0.1 CJO=1p TT=1n)\n.tran 10n 30u\n",
    );
    assert!(r.stats.timesteps > 50);
}

// ---------------------------------------------------------------------------------------------
// Monte Carlo
// ---------------------------------------------------------------------------------------------

#[test]
fn monte_carlo_statistics_with_fixed_seed() {
    let text = "divider\nV1 in 0 5\nR1 in out 10k\nR2 out 0 10k\n.mc 2000 tol=6% seed=7\n.op\n";
    let r = run(text);
    let mc = r.mc.as_ref().unwrap();
    assert_eq!(mc.runs.len(), 2000);
    let d = mc.distribution("v(out)").unwrap();
    // σ_R = tol/3 = 2 %; V = 5·R2/(R1+R2): σ_V ≈ 2.5 · √(0.5²+0.5²) · 2 % ≈ 35.4 mV
    assert!((d.mean - 2.5).abs() < 3e-3, "mean {}", d.mean);
    assert!((d.sigma - 0.0354).abs() < 0.0035, "sigma {}", d.sigma);
    assert!(d.min > 2.5 * (1.0 - 0.07) && d.max < 2.5 * (1.0 + 0.07));
    // deterministic for a seed, different for another
    let r2 = run(text);
    assert_eq!(
        r2.mc.unwrap().distribution("v(out)").unwrap().values,
        d.values
    );
    let r3 = run(&text.replace("seed=7", "seed=8"));
    assert_ne!(
        r3.mc.unwrap().distribution("v(out)").unwrap().values,
        d.values
    );
    let h = d.histogram(10);
    assert_eq!(h.iter().map(|b| b.2).sum::<usize>(), 2000);
    assert!(d.fraction_above(2.5) > 0.4 && d.fraction_above(2.5) < 0.6);
}

#[test]
fn monte_carlo_uniform_and_per_element_tolerance() {
    let r =
        run("t\nV1 in 0 1\nR1 in out 1k tol=10%\nR2 out 0 1k\n.mc 3000 dist=uniform seed=3\n.op\n");
    let d = r.mc.unwrap().distribution("i(v1)").unwrap();
    // only R1 varies: uniform ±10 % → I = 1/(2k·(1+δ/2)) spread ≈ ±5 %
    let nominal = -0.5e-3;
    assert!((d.mean / nominal - 1.0).abs() < 0.01);
    assert!(d.min / nominal < 1.06 && d.max / nominal > 0.94);
    let rel_sigma = d.sigma / nominal.abs();
    let expect = 0.05 / 3f64.sqrt();
    assert!(
        (rel_sigma - expect).abs() < 0.15 * expect,
        "{rel_sigma} vs {expect}"
    );
}

#[test]
fn monte_carlo_random_functions_in_params() {
    let r =
        run("t\n.param rv={agauss(1k, 100, 1)}\nV1 a 0 1\nR1 a 0 {rv}\n.mc 1000 seed=11\n.op\n");
    let d = r.mc.unwrap().distribution("i(v1)").unwrap();
    // current spread ≈ 10 % of 1 mA
    assert!((d.sigma / 1e-3 - 0.1).abs() < 0.03, "{}", d.sigma);
    // nominal run uses the nominal value
    assert!(
        (r.runs[0]
            .plot(PlotKind::Op)
            .unwrap()
            .value("i(v1)")
            .unwrap()
            + 1e-3)
            .abs()
            < 1e-12
    );
}

// ---------------------------------------------------------------------------------------------
// .step / .temp / .dc
// ---------------------------------------------------------------------------------------------

#[test]
fn step_produces_a_labelled_family() {
    let r = run(
        "t\n.param rb=1k\nV1 a 0 1\nR1 a b 1k\nR2 b 0 {rb}\n.step param rb list 1k 3k\n.step temp list 0 50\n.op\n",
    );
    assert_eq!(r.runs.len(), 4);
    assert_eq!(r.runs[0].label, "rb=1k temp=0");
    assert_eq!(r.runs[3].label, "rb=3k temp=50");
    let vb: Vec<f64> = r
        .runs
        .iter()
        .map(|run| run.plot(PlotKind::Op).unwrap().value("v(b)").unwrap())
        .collect();
    assert!((vb[0] - 0.5).abs() < 1e-9 && (vb[2] - 0.75).abs() < 1e-9);
    assert_eq!(r.family(PlotKind::Op).len(), 4);
    assert_eq!(r.summary().split(' ').next(), Some("4"));
}

#[test]
fn step_over_a_source_and_an_element() {
    let r = run("t\nV1 a 0 1\nR1 a b 1k\nR2 b 0 1k\n.step V1 1 3 1\n.step R2 list 1k 3k\n.op\n");
    assert_eq!(r.runs.len(), 6);
    let last = r.runs[5].plot(PlotKind::Op).unwrap().value("v(b)").unwrap();
    assert!((last - 3.0 * 0.75).abs() < 1e-9);
}

#[test]
fn dc_sweeps_lin_dec_list_nested_temp() {
    let r = run(
        "t\nV1 a 0 0\nR1 a b 1k\nD1 b 0 dd\n.model dd D\n.dc V1 0 2 0.5\n.dc dec V1 0.1 10 2\n.dc V1 list 1 5\n.dc temp 0 100 50\n",
    );
    let fam = r.family(PlotKind::Dc);
    assert_eq!(fam.len(), 4);
    assert_eq!(fam[0].1.len(), 5);
    assert_eq!(fam[1].1.len(), 5);
    assert_eq!(fam[2].1.scale_values(), vec![1.0, 5.0]);
    assert_eq!(fam[3].1.scale().unwrap().name, "temp-sweep");
    let r = run("t\nV1 a 0 0\nV2 b 0 0\nR1 a b 1k\n.dc V1 0 1 0.5 V2 0 2 1\n");
    let p = r.plot(PlotKind::Dc).unwrap();
    assert_eq!(p.len(), 9);
    // SPICE branch current flows into the + terminal: (V2 - V1)/R
    assert!((p.real("i(v1)").unwrap()[8] - (2.0 - 1.0) / 1e3).abs() < 1e-12);
}

// ---------------------------------------------------------------------------------------------
// Transient options
// ---------------------------------------------------------------------------------------------

#[test]
fn integration_methods_agree_on_an_rc() {
    let mut finals = Vec::new();
    for m in ["trap", "gear", "euler"] {
        let r = run(&format!(
            "t\nV1 a 0 PULSE(0 1 0 1n 1n 1 2)\nR1 a b 1k\nC1 b 0 1u\n.options method={m}\n.tran 10u 2m\n"
        ));
        let p = r.plot(PlotKind::Tran).unwrap();
        let v = *p.real("v(b)").unwrap().last().unwrap();
        finals.push(v);
        let exact = 1.0 - (-2.0f64).exp();
        assert!((v - exact).abs() < 2e-3, "{m}: {v} vs {exact}");
    }
}

#[test]
fn tstart_and_tmax_are_honoured() {
    let r = run("t\nV1 a 0 SIN(0 1 1k)\nR1 a 0 1k\n.tran 1u 2m 1m 5u\n");
    let p = r.plot(PlotKind::Tran).unwrap();
    let t = p.scale_values();
    assert!(t[0] >= 1e-3 - 1e-12);
    assert!(t.windows(2).all(|w| w[1] - w[0] <= 5e-6 * 1.0001));
}

// ---------------------------------------------------------------------------------------------
// .meas, export
// ---------------------------------------------------------------------------------------------

#[test]
fn meas_on_a_known_waveform() {
    // v(a) = t (ramp) on 0..1 ms: avg = 0.5 m, rms = 1m/√3, integ = 0.5e-6
    let r = run(
        "t\nB1 a 0 V=time\nR1 a 0 1k\n.tran 1u 1m\n.meas tran av avg v(a)\n.meas tran rm rms v(a)\n.meas tran ig integ v(a)\n.meas tran pk max v(a) from=0.2m to=0.4m\n.meas tran tw when v(a)=0.25m\n.meas tran f find v(a) at=0.6m\n.meas tran d deriv v(a) at=0.5m\n.meas tran x param='av*2'\n",
    );
    let m = |n: &str| {
        r.measure(n)
            .unwrap_or_else(|| panic!("{n}: {:?}", r.runs[0].measures))
    };
    assert!((m("av") - 0.5e-3).abs() < 1e-9);
    assert!((m("rm") - 1e-3 / 3f64.sqrt()).abs() < 1e-8);
    assert!((m("ig") - 0.5e-6).abs() < 1e-11);
    assert!((m("pk") - 0.4e-3).abs() < 1e-9);
    assert!((m("tw") - 0.25e-3).abs() < 1e-9);
    assert!((m("f") - 0.6e-3).abs() < 1e-9);
    assert!((m("d") - 1.0).abs() < 1e-6);
    assert!((m("x") - 1e-3).abs() < 1e-9);
}

#[test]
fn export_cir_round_trip() {
    let text = "round trip\n.param r1v=2k\nV1 in 0 DC 0 AC 1 PULSE(0 1 0 1n 1n 1u 2u)\nR1 in a {r1v}\nC1 a 0 1n ic=0\nL1 a b 1m\nR2 b 0 50\nE1 e 0 a 0 2\nRe e 0 1k\nB1 f 0 V=v(a)*2+1\nRf f 0 1k\nD1 b d dd\nRd d 0 1k\n.model dd D(IS=1e-14 RS=2)\n.subckt buf x y params: g=1\nE1 y 0 x 0 {g}\n.ends\nX1 a o buf g=3\nRo o 0 1k\n.op\n.ac dec 5 1k 1meg\n.tran 10n 2u\n.end\n";
    let nl = Netlist::parse(text).unwrap();
    let out = nl.to_spice_text();
    let nl2 = Netlist::parse(&out).unwrap_or_else(|e| panic!("{e}\n{out}"));
    let (r1, r2) = (nl.run().unwrap(), nl2.run().unwrap());
    for kind in [PlotKind::Op, PlotKind::Ac, PlotKind::Tran] {
        let (a, b) = (r1.plot(kind).unwrap(), r2.plot(kind).unwrap());
        assert_eq!(a.len(), b.len());
        for v in &a.vectors {
            let w = b
                .vector(&v.name)
                .unwrap_or_else(|| panic!("{} missing after round trip", v.name));
            for i in 0..v.len() {
                assert!(
                    (v.complex_at(i) - w.complex_at(i)).abs()
                        <= 1e-9 * v.complex_at(i).abs().max(1e-6)
                );
            }
        }
    }
}

#[test]
fn per_element_tolerance_survives_export_as_an_ngspice_comment() {
    // `tol=` is Breadbox-only: ngspice rejects it on an instance ("unknown parameter (tol)"),
    // so the exporter writes it after `;!bb` — a comment for ngspice, a parameter for bb-spice
    let text = "mc\nV1 in 0 DC 10\nR1 in out 1k tol=10%\nR2 out 0 1k\n.op\n.mc 200 seed=3\n.end\n";
    let nl = Netlist::parse(text).unwrap();
    let out = nl.to_spice_text();
    let r1 = out
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("r1 "))
        .unwrap();
    let (card, bb) = r1
        .split_once(';')
        .expect("tol moved into the inline comment");
    assert!(!card.to_ascii_lowercase().contains("tol"), "{r1}");
    assert!(bb.starts_with("!bb tol="), "{r1}");
    let nl2 = Netlist::parse(&out).unwrap_or_else(|e| panic!("{e}\n{out}"));
    let (a, b) = (nl.run().unwrap(), nl2.run().unwrap());
    let (da, db) = (
        a.mc.as_ref().unwrap().distribution("v(out)").unwrap(),
        b.mc.as_ref().unwrap().distribution("v(out)").unwrap(),
    );
    assert!(da.sigma > 0.05, "R1 varies: σ = {}", da.sigma);
    assert_eq!(da.values, db.values, "same spread after the round trip");
}

#[test]
fn a_switch_closing_onto_an_led_converges() {
    // the 555 output stage in miniature: a hysteretic switch flips a latch node, a linear B
    // source follows it, and the step turns an LED on through 470 Ω. Newton needs more than
    // ITL4 iterations across that discontinuity; retries resume from the last iterate.
    // ngspice-47: v(n5) = 0.0500 V off, 2.1020 V on.
    let r = run("switch onto an LED
.model led_green D(is=1e-20 n=1.9 rs=5)
.model swl SW(VT=0 VH=0.5 RON=1 ROFF=1g)
vc c 0 PULSE(-1 1 1m 1u 1u 1m 2m)
vr r1 0 1
rl r1 q 1k
sl q 0 c 0 swl OFF
bo oi 0 V={max(9.37-1.7,0)*(1-v(q)) + 0.05*v(q)}
ro oi out 10
r3 out n5 470
d1 n5 0 led_green
.tran 1u 5m
");
    let tran = r.plot(PlotKind::Tran).unwrap();
    let t = tran.vectors[0].reals();
    let v = tran.vector("v(n5)").unwrap().reals();
    let at = |x: f64| v[t.iter().position(|&ti| ti >= x).unwrap()];
    assert!((at(0.5e-3) - 0.0500).abs() < 1e-3, "off: {}", at(0.5e-3));
    assert!((at(1.5e-3) - 2.1020).abs() < 2e-3, "on: {}", at(1.5e-3));
    assert!(
        (at(2.5e-3) - 0.0500).abs() < 1e-3,
        "off again: {}",
        at(2.5e-3)
    );
    assert!(
        (at(3.5e-3) - 2.1020).abs() < 2e-3,
        "on again: {}",
        at(3.5e-3)
    );
}

#[test]
fn raw_and_csv_export() {
    let r = run(
        "t\nV1 in 0 DC 1 AC 1\nR1 in out 1k\nC1 out 0 1u\n.op\n.ac dec 5 10 10k\n.tran 10u 1m\n",
    );
    let raw = results_to_raw(&r);
    let plots = read_raw(&raw).unwrap();
    assert_eq!(plots.len(), 3);
    assert_eq!(plots[1].kind, PlotKind::Ac);
    let ac = r.plot(PlotKind::Ac).unwrap();
    let csv = plot_to_csv(ac, ComplexCsv::DbPhase);
    assert!(csv.starts_with("frequency,db(v(in)),phase(v(in)),db(v(out))"));
    assert_eq!(csv.lines().count(), ac.len() + 1);
}

// ---------------------------------------------------------------------------------------------
// The Pro-screen netlist
// ---------------------------------------------------------------------------------------------

const GARDEN: &str = "* garden-node — generated, editable
V1    VCC 0  DC 5
RLDR  VCC A1 {RL}
R1    A1  0  {R1v}
XJ1   VCC 0  A0 CAPSOIL vwc=42
R2    D9  N1 220
D1    N1  0  LED_AMBER
R3    D8  N2 330
D2    N2  0  LED_RED
XU1   VCC 0 A0 A1 D9 D8 ATMEGA328P
+     fw=garden-node.hex  sync=1u

* GL5528: R ≈ 350k · lux^-0.7, ±10%
.param lux=180 RL={350k*pwr(lux,-0.7)}
.param R1v=10k
.model LED_AMBER D(Is=1e-19 N=1.8 Rs=6)
.model LED_RED   D(Is=1e-18 N=1.9 Rs=5)

.tran 0 10m 0 1u
.dc   lux 1 10k dec 40
.step param R1v list 4.7k 10k 22k
.mc   100 tol=5%  seed=7
.op
.end
";

#[test]
fn design_netlist_parses_and_needs_breadbox_parts() {
    let nl = Netlist::parse(GARDEN).unwrap();
    // the MCU instance keeps its non-numeric parameters for bb-sim
    match &nl.element("xu1").unwrap().kind {
        bb_spice::netlist::ElemKind::Subckt { name, text_params } => {
            assert_eq!(name, "atmega328p");
            assert_eq!(
                text_params,
                &vec![("fw".to_string(), "garden-node.hex".to_string())]
            );
        }
        k => panic!("{k:?}"),
    }
    assert_eq!(nl.element("xu1").unwrap().params.len(), 1); // sync=1u
    let err = nl.run().unwrap_err().to_string();
    assert!(
        err.contains("line 5")
            && err.contains("unknown subcircuit `capsoil`")
            && err.contains("add_library"),
        "{err}"
    );
}

#[test]
fn design_netlist_runs_with_registered_parts() {
    // bb-sim registers its parts as libraries; here: a soil sensor model and the MCU's pins as
    // high-impedance inputs (in co-simulation the MCU becomes ports).
    let lib = Library::parse(
        ".subckt capsoil vcc gnd out params: vwc=30\nBs s gnd V=v(vcc,gnd)*(0.62 - {vwc}*0.0055)\nRs s out 1k\nCs out gnd 100n\n.ends\n\
         .subckt atmega328p vcc gnd a0 a1 d9 d8 params: sync=1u\nRvcc vcc gnd 1k\nRa0 a0 gnd 100meg\nRa1 a1 gnd 100meg\nR9 d9 gnd 1meg\nR8 d8 gnd 1meg\n.ends\n",
    )
    .unwrap();
    let text = GARDEN.replace("+     fw=garden-node.hex  sync=1u", "+     sync=1u");
    let mut nl = Netlist::parse(&text).unwrap();
    nl.add_library(Arc::new(lib));
    let r = nl.run().unwrap();
    assert_eq!(r.runs.len(), 3);
    assert_eq!(r.runs[0].plots.len(), 3);
    let mc = r.mc.as_ref().unwrap();
    assert_eq!(mc.runs.len(), 100);
    let d = mc.distribution("v(a1)").unwrap();
    // nominal divider: 5 · 10k / (10k + 350k·180^-0.7)
    let rl = 350e3 * 180f64.powf(-0.7);
    let nominal = 5.0 * 10e3 / (10e3 + rl);
    assert!((d.mean - nominal).abs() < 0.02, "{} vs {nominal}", d.mean);
    assert!(d.sigma > 0.005 && d.sigma < 0.1);
    let dc = r.runs[1].plot(PlotKind::Dc).unwrap();
    assert_eq!(dc.len(), 161);
    assert_eq!(dc.scale().unwrap().name, "lux");
    let summary = r.summary();
    assert!(
        summary.starts_with("3 steps · ") && summary.ends_with("0 convergence warnings"),
        "{summary}"
    );
}

#[test]
fn noise_contributions_per_source() {
    // equal resistors in a divider contribute equally; total = √(sum of squares)
    let r = run(
        "t\nVIN in 0 DC 0 AC 1\nR1 in out 10k\nR2 out 0 10k\n.noise v(out) VIN lin 10 1k 10k\n",
    );
    let p = r.plot(PlotKind::NoiseTotal).unwrap();
    let (a, b, t) = (
        p.value("onoise_r1").unwrap(),
        p.value("onoise_r2").unwrap(),
        p.value("onoise_total").unwrap(),
    );
    assert!((a - b).abs() < 1e-6 * a);
    assert!((t - (a * a + b * b).sqrt()).abs() < 1e-9 * t);
    // 4kT·(R1||R2) over 9 kHz: √(4·1.380649e-23·300.15·5k·9k)
    let expect = (4.0 * 1.380649e-23 * 300.15 * 5e3 * 9e3f64).sqrt();
    assert!((t - expect).abs() < 1e-3 * expect, "{t} vs {expect}");
}

#[test]
fn breadbox_extensions_export_as_directives() {
    let text = "ext\n.param lux=180 r1v=10k\nV1 vcc 0 5\nRLDR vcc a1 {350k*pwr(lux,-0.7)}\nR1 a1 0 {r1v}\n.dc lux 1 10k dec 10\n.dc V1 0 5 1\n.step param r1v list 4.7k 10k\n.mc 5 tol=5% seed=3\n.temp 0 50\n.op\n.end\n";
    let nl = Netlist::parse(text).unwrap();
    let out = nl.to_spice_text();
    for card in [
        "*!bb .dc lux",
        "*!bb .step param r1v",
        "*!bb .mc 5",
        "*!bb .temp 0 50",
    ] {
        assert!(out.contains(card), "missing `{card}` in\n{out}");
    }
    assert!(out.contains("\n.dc v1 0 5 1\n"), "{out}");
    // round trip keeps the extensions
    let back = Netlist::parse(&out).unwrap();
    assert_eq!(back.steps.len(), 1);
    assert!(back.mc.is_some());
    assert_eq!(back.temps, vec![0.0, 50.0]);
    assert_eq!(back.analyses.len(), 3);
}

/// SPICE-1 / SPICE-3 sweep forms the reference suite does not use: the `mil` suffix
/// (1 mil = 25.4 µ), `.dc … oct`, `.step … dec` and `.step … oct`.
#[test]
fn mil_suffix_dc_octave_and_step_dec_oct() {
    // 1mil = 25.4e-6: a 1mil Ω resistor from 1 V draws 1 / 25.4µ A
    let r = run("t\nV1 a 0 1\nR1 a 0 1mil\n.op\n");
    let i = r.op("i(v1)").unwrap();
    assert!((i + 1.0 / 25.4e-6).abs() < 1e-3 * (1.0 / 25.4e-6), "{i}");
    // .dc oct: 1 → 8 V, 2 points per octave = 7 points
    let r = run("t\nV1 a 0 0\nR1 a 0 1k\n.dc oct V1 1 8 2\n");
    let v = r.plot(PlotKind::Dc).unwrap().scale_values();
    assert_eq!(v.len(), 7, "{v:?}");
    assert!((v[2] - 2.0).abs() < 1e-9 && (v[6] - 8.0).abs() < 1e-9, "{v:?}");
    // .step dec / oct over a parameter
    let r = run("t\n.param rv=1k\nV1 a 0 1\nR1 a 0 {rv}\n.step dec param rv 1k 100k 1\n.op\n");
    let labels: Vec<String> = r.runs.iter().map(|x| x.label.clone()).collect();
    assert_eq!(r.runs.len(), 3, "{labels:?}");
    let currents: Vec<f64> = r
        .runs
        .iter()
        .map(|x| -x.plot(PlotKind::Op).unwrap().value("i(v1)").unwrap())
        .collect();
    for (c, rv) in currents.iter().zip([1e3, 1e4, 1e5]) {
        assert!((c - 1.0 / rv).abs() < 1e-9, "{currents:?}");
    }
    let r = run("t\n.param rv=1k\nV1 a 0 1\nR1 a 0 {rv}\n.step oct param rv 1k 4k 1\n.op\n");
    assert_eq!(r.runs.len(), 3, "1k, 2k, 4k");
    let last = -r.runs[2]
        .plot(PlotKind::Op)
        .unwrap()
        .value("i(v1)")
        .unwrap();
    assert!((last - 1.0 / 4e3).abs() < 1e-9, "{last}");
}
