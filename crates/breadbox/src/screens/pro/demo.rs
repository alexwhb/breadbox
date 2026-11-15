//! The design's garden-node netlist (Pro screen) as a built-in demo, plus stand-ins for the
//! Breadbox parts only the co-simulation knows. In co-simulation `bb-sim` replaces `XU1` with
//! the real AVR running `garden-node.hex`; for the analog-only run the MCU stand-in replays the
//! firmware's outputs (D9 PWM at 490 Hz, 47.8 % — the sketch's `led_pwm=122` — and D8 low).

use super::model::Profile;

/// Verbatim from `docs/design/source/3-pro-spice.html`.
pub const NETLIST: &str = "* garden-node — generated, editable
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

pub const FILE_NAME: &str = "garden-node.cir";

/// Stand-in subcircuits for parts the analog-only solver doesn't know (`(name, .subckt)`).
pub const STUB_PARTS: &[(&str, &str)] = &[
    (
        "CAPSOIL",
        ".subckt capsoil vcc gnd out params: vwc=30
* capacitive soil sensor v1.2: 3.0 V (dry) → 1.4 V (wet), τ ≈ 0.1 ms here
Bs  s gnd V=v(vcc,gnd)*(0.62 - {vwc}*0.0055)
Rs  s out 1k
Cs  out gnd 100n
.ends
",
    ),
    (
        "ATMEGA328P",
        ".subckt atmega328p vcc gnd a0 a1 d9 d8 params: sync=1u
* MCU stand-in: supply draw, ADC inputs, firmware outputs replayed (D9 PWM 490 Hz 47.8 %, D8 low)
Rvcc vcc gnd 1k
Ra0  a0 gnd 100meg
Ra1  a1 gnd 100meg
Vd9  d9i gnd PULSE(0 5 0 1u 1u 0.97449m 2.04082m)
Rd9  d9i d9 25
Vd8  d8i gnd DC 0
Rd8  d8i d8 25
.ends
",
    ),
];

/// The design's panel titles for the demo.
pub fn profile() -> Profile {
    Profile {
        tran_title: Some("night LED drive".into()),
        tran_traces: vec!["v(d9)".into(), "i(d1)".into()],
        dc_vector: Some("v(a1)".into()),
        dc_title: Some("V(A1) vs light".into()),
        step_label: Some(("r1v".into(), "R1".into())),
        mc_vector: Some("v(a1)".into()),
        mc_threshold: Some((2.75, "firmware threshold 2.75 V".into())),
        mc_note: None,
        op_rows: vec![
            "V(VCC)".into(),
            "V(A0)".into(),
            "V(A1)".into(),
            "I(D1) avg".into(),
            "P(R2)".into(),
        ],
    }
}
