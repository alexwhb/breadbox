//! Programmatic circuit construction ([`CircuitBuilder`]) — used by bb-circuit's netlister.
//!
//! The builder produces the same [`Netlist`] model the text parser does, so everything works
//! the same way afterwards: `.cir` export ([`Netlist::to_spice_text`]), analyses
//! ([`Netlist::run`]) and co-simulation ([`crate::Session`]).
//!
//! Element names get their SPICE type letter prepended when missing (`resistor("LDR", …)`
//! creates `rldr`); the methods return the final name. Values are plain `f64`s; use the
//! `*_expr` variants for expressions (`{…}` syntax without braces, including `ext("name")`).
//!
//! ```
//! use bb_spice::{CircuitBuilder, ModelKind, Analysis};
//! let mut b = CircuitBuilder::new("led night light");
//! b.vsource_dc("1", "vcc", "0", 5.0);
//! b.resistor_expr("ldr", "vcc", "a1", "350k*pwr(max(ext(\"J2.lux\"),0.01),-0.7)").unwrap();
//! b.resistor("1", "a1", "0", 10e3);
//! b.model("led_amber", ModelKind::D, &[("is", 1e-19), ("n", 1.8), ("rs", 6.0)]);
//! b.diode("1", "a1", "0", "led_amber");
//! b.analysis(Analysis::Op);
//! let nl = b.build();
//! assert!(nl.to_spice_text().contains("Rldr vcc a1 {350000*pwr(max(ext_j2_lux,0.01),-0.7)}")
//!     || nl.to_spice_text().to_lowercase().contains("rldr vcc a1"));
//! ```

use crate::error::{SimError, Span};
use crate::expr::Expr;
use crate::netlist::*;
use bb_core::PinDrive;
use std::sync::Arc;

/// Builds a [`Netlist`] programmatically.
#[derive(Debug, Clone, Default)]
pub struct CircuitBuilder {
    nl: Netlist,
}

/// Handle to the element just added, for setting extra instance parameters.
pub struct ElementRef<'a> {
    card: &'a mut ElementCard,
}

impl ElementRef<'_> {
    /// Final (lower-case) element name.
    pub fn name(&self) -> &str {
        &self.card.name
    }
    /// Sets a numeric instance parameter (`ic`, `tc1`, `w`, `l`, `area`, `temp`, `tol`, …).
    pub fn param(self, key: &str, value: f64) -> Self {
        self.card
            .params
            .push((key.to_ascii_lowercase(), Expr::Num(value)));
        self
    }
    /// Sets an expression-valued instance parameter.
    pub fn param_expr(self, key: &str, expr: &str) -> Result<Self, SimError> {
        let e = parse(expr)?;
        self.card.params.push((key.to_ascii_lowercase(), e));
        Ok(self)
    }
}

fn parse(expr: &str) -> Result<Expr, SimError> {
    Expr::parse(expr).map_err(|e| SimError::invalid(format!("in expression `{expr}`: {}", e.msg)))
}

fn named(letter: char, name: &str) -> String {
    let l = name.to_ascii_lowercase();
    if l.starts_with(letter) {
        l
    } else {
        format!("{letter}{l}")
    }
}

fn node(n: &str) -> String {
    let l = n.to_ascii_lowercase();
    if l == "gnd" { "0".into() } else { l }
}

/// Source description for [`CircuitBuilder::vsource`] / [`CircuitBuilder::isource`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceDef {
    spec: SourceSpec,
}

impl SourceDef {
    pub fn dc(v: f64) -> Self {
        SourceDef {
            spec: SourceSpec {
                dc: Some(Expr::Num(v)),
                ..Default::default()
            },
        }
    }
    /// AC excitation (magnitude, phase in degrees).
    pub fn ac(mut self, mag: f64, phase_deg: f64) -> Self {
        self.spec.ac_mag = Some(Expr::Num(mag));
        self.spec.ac_phase = Some(Expr::Num(phase_deg));
        self
    }
    fn wave(mut self, w: Waveform) -> Self {
        self.spec.wave = Some(w);
        self
    }
    fn nums(v: &[f64]) -> Vec<Expr> {
        v.iter().map(|x| Expr::Num(*x)).collect()
    }
    /// `PULSE(v1 v2 td tr tf pw per)`
    #[allow(clippy::too_many_arguments)]
    pub fn pulse(self, v1: f64, v2: f64, td: f64, tr: f64, tf: f64, pw: f64, per: f64) -> Self {
        self.wave(Waveform::Pulse(Self::nums(&[v1, v2, td, tr, tf, pw, per])))
    }
    /// `SIN(vo va freq td theta phase)`
    pub fn sin(self, vo: f64, va: f64, freq: f64, td: f64, theta: f64, phase_deg: f64) -> Self {
        self.wave(Waveform::Sin(Self::nums(&[
            vo, va, freq, td, theta, phase_deg,
        ])))
    }
    /// `EXP(v1 v2 td1 tau1 td2 tau2)`
    pub fn exp(self, v1: f64, v2: f64, td1: f64, tau1: f64, td2: f64, tau2: f64) -> Self {
        self.wave(Waveform::Exp(Self::nums(&[v1, v2, td1, tau1, td2, tau2])))
    }
    /// `PWL(t1 v1 …) [r=repeat]`
    pub fn pwl(self, points: &[(f64, f64)], repeat: Option<f64>) -> Self {
        let pts = points
            .iter()
            .flat_map(|(t, v)| [Expr::Num(*t), Expr::Num(*v)])
            .collect();
        self.wave(Waveform::Pwl {
            points: pts,
            repeat: repeat.map(Expr::Num),
            delay: None,
        })
    }
    /// `SFFM(vo va fc mdi fs)`
    pub fn sffm(self, vo: f64, va: f64, fc: f64, mdi: f64, fs: f64) -> Self {
        self.wave(Waveform::Sffm(Self::nums(&[vo, va, fc, mdi, fs])))
    }
    /// `AM(va vo mf fc td)`
    pub fn am(self, va: f64, vo: f64, mf: f64, fc: f64, td: f64) -> Self {
        self.wave(Waveform::Am(Self::nums(&[va, vo, mf, fc, td])))
    }
}

impl CircuitBuilder {
    pub fn new(title: &str) -> Self {
        CircuitBuilder {
            nl: Netlist {
                title: title.to_string(),
                ..Default::default()
            },
        }
    }

    fn push(&mut self, name: String, nodes: Vec<String>, kind: ElemKind) -> ElementRef<'_> {
        // replace an element of the same name (builder is used incrementally by editors)
        self.nl.top.elements.retain(|e| e.name != name);
        self.nl.top.elements.push(ElementCard {
            name,
            nodes,
            kind,
            params: Vec::new(),
            span: Span::none(),
        });
        ElementRef {
            card: self.nl.top.elements.last_mut().unwrap(),
        }
    }

    pub fn resistor(&mut self, name: &str, a: &str, b: &str, ohms: f64) -> ElementRef<'_> {
        self.push(
            named('r', name),
            vec![node(a), node(b)],
            ElemKind::Resistor {
                value: Some(Expr::Num(ohms)),
                model: None,
            },
        )
    }
    pub fn resistor_expr(
        &mut self,
        name: &str,
        a: &str,
        b: &str,
        expr: &str,
    ) -> Result<ElementRef<'_>, SimError> {
        let e = parse(expr)?;
        Ok(self.push(
            named('r', name),
            vec![node(a), node(b)],
            ElemKind::Resistor {
                value: Some(e),
                model: None,
            },
        ))
    }
    pub fn capacitor(&mut self, name: &str, a: &str, b: &str, farads: f64) -> ElementRef<'_> {
        self.push(
            named('c', name),
            vec![node(a), node(b)],
            ElemKind::Capacitor {
                value: Some(Expr::Num(farads)),
                model: None,
            },
        )
    }
    pub fn capacitor_expr(
        &mut self,
        name: &str,
        a: &str,
        b: &str,
        expr: &str,
    ) -> Result<ElementRef<'_>, SimError> {
        let e = parse(expr)?;
        Ok(self.push(
            named('c', name),
            vec![node(a), node(b)],
            ElemKind::Capacitor {
                value: Some(e),
                model: None,
            },
        ))
    }
    pub fn inductor(&mut self, name: &str, a: &str, b: &str, henries: f64) -> ElementRef<'_> {
        self.push(
            named('l', name),
            vec![node(a), node(b)],
            ElemKind::Inductor {
                value: Some(Expr::Num(henries)),
                model: None,
            },
        )
    }
    /// Mutual coupling `k` between two inductors (names as returned by [`Self::inductor`]).
    pub fn coupling(&mut self, name: &str, l1: &str, l2: &str, k: f64) -> ElementRef<'_> {
        self.push(
            named('k', name),
            vec![],
            ElemKind::Coupling {
                inductors: vec![named('l', l1), named('l', l2)],
                k: Expr::Num(k),
            },
        )
    }
    pub fn vsource(&mut self, name: &str, p: &str, n: &str, src: SourceDef) -> ElementRef<'_> {
        self.push(
            named('v', name),
            vec![node(p), node(n)],
            ElemKind::Vsource(src.spec),
        )
    }
    pub fn vsource_dc(&mut self, name: &str, p: &str, n: &str, volts: f64) -> ElementRef<'_> {
        self.vsource(name, p, n, SourceDef::dc(volts))
    }
    /// Voltage source whose DC value is an expression (e.g. of `ext()`).
    pub fn vsource_expr(
        &mut self,
        name: &str,
        p: &str,
        n: &str,
        expr: &str,
    ) -> Result<ElementRef<'_>, SimError> {
        let e = parse(expr)?;
        Ok(self.push(
            named('v', name),
            vec![node(p), node(n)],
            ElemKind::Vsource(SourceSpec {
                dc: Some(e),
                ..Default::default()
            }),
        ))
    }
    pub fn isource(&mut self, name: &str, p: &str, n: &str, src: SourceDef) -> ElementRef<'_> {
        self.push(
            named('i', name),
            vec![node(p), node(n)],
            ElemKind::Isource(src.spec),
        )
    }
    pub fn vcvs(
        &mut self,
        name: &str,
        p: &str,
        n: &str,
        cp: &str,
        cn: &str,
        gain: f64,
    ) -> ElementRef<'_> {
        self.push(
            named('e', name),
            vec![node(p), node(n)],
            ElemKind::Vcvs(Control::Linear {
                ctrl_nodes: vec![node(cp), node(cn)],
                ctrl_src: None,
                gain: Expr::Num(gain),
            }),
        )
    }
    pub fn vccs(
        &mut self,
        name: &str,
        p: &str,
        n: &str,
        cp: &str,
        cn: &str,
        gm: f64,
    ) -> ElementRef<'_> {
        self.push(
            named('g', name),
            vec![node(p), node(n)],
            ElemKind::Vccs(Control::Linear {
                ctrl_nodes: vec![node(cp), node(cn)],
                ctrl_src: None,
                gain: Expr::Num(gm),
            }),
        )
    }
    pub fn cccs(&mut self, name: &str, p: &str, n: &str, vctrl: &str, gain: f64) -> ElementRef<'_> {
        self.push(
            named('f', name),
            vec![node(p), node(n)],
            ElemKind::Cccs(Control::Linear {
                ctrl_nodes: vec![],
                ctrl_src: Some(named('v', vctrl)),
                gain: Expr::Num(gain),
            }),
        )
    }
    pub fn ccvs(&mut self, name: &str, p: &str, n: &str, vctrl: &str, r: f64) -> ElementRef<'_> {
        self.push(
            named('h', name),
            vec![node(p), node(n)],
            ElemKind::Ccvs(Control::Linear {
                ctrl_nodes: vec![],
                ctrl_src: Some(named('v', vctrl)),
                gain: Expr::Num(r),
            }),
        )
    }
    /// Behavioural voltage source `V = expr`.
    pub fn bsource_v(
        &mut self,
        name: &str,
        p: &str,
        n: &str,
        expr: &str,
    ) -> Result<ElementRef<'_>, SimError> {
        let e = parse(expr)?;
        Ok(self.push(
            named('b', name),
            vec![node(p), node(n)],
            ElemKind::Behavioral {
                is_current: false,
                expr: e,
            },
        ))
    }
    /// Behavioural current source `I = expr` (flowing from `p` through the source to `n`).
    pub fn bsource_i(
        &mut self,
        name: &str,
        p: &str,
        n: &str,
        expr: &str,
    ) -> Result<ElementRef<'_>, SimError> {
        let e = parse(expr)?;
        Ok(self.push(
            named('b', name),
            vec![node(p), node(n)],
            ElemKind::Behavioral {
                is_current: true,
                expr: e,
            },
        ))
    }
    #[allow(clippy::too_many_arguments)]
    pub fn switch(
        &mut self,
        name: &str,
        p: &str,
        n: &str,
        cp: &str,
        cn: &str,
        model: &str,
        init_on: Option<bool>,
    ) -> ElementRef<'_> {
        self.push(
            named('s', name),
            vec![node(p), node(n), node(cp), node(cn)],
            ElemKind::Switch {
                model: model.to_ascii_lowercase(),
                init_on,
            },
        )
    }
    pub fn cswitch(
        &mut self,
        name: &str,
        p: &str,
        n: &str,
        vctrl: &str,
        model: &str,
        init_on: Option<bool>,
    ) -> ElementRef<'_> {
        self.push(
            named('w', name),
            vec![node(p), node(n)],
            ElemKind::CSwitch {
                ctrl: named('v', vctrl),
                model: model.to_ascii_lowercase(),
                init_on,
            },
        )
    }
    pub fn diode(&mut self, name: &str, anode: &str, cathode: &str, model: &str) -> ElementRef<'_> {
        self.push(
            named('d', name),
            vec![node(anode), node(cathode)],
            ElemKind::Diode {
                model: model.to_ascii_lowercase(),
                area: None,
                off: false,
            },
        )
    }
    /// BJT (`substrate` = `None` for a grounded substrate).
    pub fn bjt(
        &mut self,
        name: &str,
        c: &str,
        b: &str,
        e: &str,
        substrate: Option<&str>,
        model: &str,
    ) -> ElementRef<'_> {
        let mut nodes = vec![node(c), node(b), node(e)];
        if let Some(s) = substrate {
            nodes.push(node(s));
        }
        self.push(
            named('q', name),
            nodes,
            ElemKind::Bjt {
                model: model.to_ascii_lowercase(),
                area: None,
                off: false,
            },
        )
    }
    /// MOSFET (set `w`/`l` with [`ElementRef::param`]).
    pub fn mosfet(
        &mut self,
        name: &str,
        d: &str,
        g: &str,
        s: &str,
        b: &str,
        model: &str,
    ) -> ElementRef<'_> {
        self.push(
            named('m', name),
            vec![node(d), node(g), node(s), node(b)],
            ElemKind::Mosfet {
                model: model.to_ascii_lowercase(),
                off: false,
            },
        )
    }
    pub fn jfet(&mut self, name: &str, d: &str, g: &str, s: &str, model: &str) -> ElementRef<'_> {
        self.push(
            named('j', name),
            vec![node(d), node(g), node(s)],
            ElemKind::Jfet {
                model: model.to_ascii_lowercase(),
                area: None,
                off: false,
            },
        )
    }
    /// Lossless transmission line.
    #[allow(clippy::too_many_arguments)]
    pub fn tline(
        &mut self,
        name: &str,
        p1: &str,
        n1: &str,
        p2: &str,
        n2: &str,
        z0: f64,
        td: f64,
    ) -> ElementRef<'_> {
        let r = self.push(
            named('t', name),
            vec![node(p1), node(n1), node(p2), node(n2)],
            ElemKind::Tline,
        );
        r.param("z0", z0).param("td", td)
    }
    /// Subcircuit instance with numeric parameters.
    pub fn subckt(
        &mut self,
        name: &str,
        nodes: &[&str],
        subckt: &str,
        params: &[(&str, f64)],
    ) -> ElementRef<'_> {
        let r = self.push(
            named('x', name),
            nodes.iter().map(|n| node(n)).collect(),
            ElemKind::Subckt {
                name: subckt.to_ascii_lowercase(),
                text_params: vec![],
            },
        );
        r.card.params.extend(
            params
                .iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), Expr::Num(*v))),
        );
        r
    }
    /// Co-simulation port driving `p` relative to `n`; returns its [`crate::PortId`] in sessions
    /// built from this netlist (ports are numbered in creation order).
    pub fn port(&mut self, name: &str, p: &str, n: &str, drive: PinDrive) -> crate::PortId {
        let pname = crate::session::port_element_name(name);
        self.push(pname, vec![node(p), node(n)], ElemKind::Port { drive });
        let idx = self
            .nl
            .top
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElemKind::Port { .. }))
            .count()
            - 1;
        crate::PortId(idx)
    }

    /// Adds (or replaces) a `.model`.
    pub fn model(&mut self, name: &str, kind: ModelKind, params: &[(&str, f64)]) -> &mut Self {
        let name = name.to_ascii_lowercase();
        self.nl.top.models.retain(|m| m.name != name);
        self.nl.top.models.push(ModelCard {
            name,
            kind,
            params: params
                .iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), Expr::Num(*v)))
                .collect(),
            span: Span::none(),
        });
        self
    }
    /// Adds a `.param`.
    pub fn param(&mut self, name: &str, value: f64) -> &mut Self {
        self.nl.set_param(name, Expr::Num(value));
        self
    }
    pub fn param_expr(&mut self, name: &str, expr: &str) -> Result<&mut Self, SimError> {
        let e = parse(expr)?;
        self.nl.set_param(name, e);
        Ok(self)
    }
    /// Adds a `.options key=value`.
    pub fn option(&mut self, key: &str, value: f64) -> &mut Self {
        self.nl
            .options
            .push((key.to_ascii_lowercase(), Some(value), Span::none()));
        self
    }
    /// `.ic v(node)=value`
    pub fn ic(&mut self, node_name: &str, value: f64) -> &mut Self {
        self.nl
            .ics
            .push((node(node_name), Expr::Num(value), Span::none()));
        self
    }
    /// `.temp`
    pub fn temp(&mut self, celsius: f64) -> &mut Self {
        self.nl.temps = vec![celsius];
        self
    }
    pub fn analysis(&mut self, a: Analysis) -> &mut Self {
        self.nl.analyses.push((a, Span::none()));
        self
    }
    /// Registers a subcircuit/model library (e.g. parsed with [`Library::parse`]).
    pub fn library(&mut self, lib: Arc<Library>) -> &mut Self {
        self.nl.libraries.push(lib);
        self
    }
    /// Adds a `.subckt` definition parsed from text.
    pub fn subckt_def(&mut self, text: &str) -> Result<&mut Self, SimError> {
        let lib = Library::parse(text)?;
        self.nl.top.subckts.extend(lib.scope.subckts);
        self.nl.top.models.extend(lib.scope.models);
        Ok(self)
    }

    /// Finishes the netlist.
    pub fn build(&self) -> Netlist {
        self.nl.clone()
    }
}
