//! Lossless transmission line (T element: `Z0`, `TD` or `F`/`NL`), method of characteristics.
//!
//! Each port is a Thevenin source `v_k = Z0·i_k + E_k` whose EMF is the wave launched from the
//! other port `TD` seconds earlier: `E1(t) = v2(t−TD) + Z0·i2(t−TD)`. In DC the line is a
//! through-connection; in AC the delay becomes `e^{−jωTD}`.

use super::*;

#[derive(Debug, Clone)]
pub struct Tline {
    pub name: String,
    /// p1+, p1-, p2+, p2-
    pub nodes: [usize; 4],
    pub z0: f64,
    pub td: f64,
    pub ic: [f64; 4],
    br1: usize,
    br2: usize,
    s: [Slot; 16],
    /// (t, v1, i1, v2, i2) at accepted time points.
    hist: Vec<[f64; 5]>,
    corners: Vec<f64>,
}

impl Tline {
    pub fn new(name: String, nodes: [usize; 4], z0: f64, td: f64, ic: [f64; 4]) -> Self {
        Tline {
            name,
            nodes,
            z0,
            td,
            ic,
            br1: 0,
            br2: 0,
            s: [Slot::GROUND; 16],
            hist: Vec::new(),
            corners: Vec::new(),
        }
    }

    fn port_values(&self, x: &[f64]) -> [f64; 4] {
        let [a, b, c, d] = self.nodes;
        [x[a] - x[b], x[self.br1], x[c] - x[d], x[self.br2]]
    }

    /// History values interpolated at time `t`.
    fn at(&self, t: f64) -> [f64; 4] {
        let h = &self.hist;
        if h.is_empty() {
            return [0.0; 4];
        }
        if t <= h[0][0] {
            return [h[0][1], h[0][2], h[0][3], h[0][4]];
        }
        let n = h.len();
        if t >= h[n - 1][0] {
            // extrapolate linearly from the last two points (only happens if step > TD)
            if n >= 2 {
                let (p, q) = (h[n - 2], h[n - 1]);
                let dt = q[0] - p[0];
                if dt > 0.0 {
                    let f = (t - q[0]) / dt;
                    return [
                        q[1] + f * (q[1] - p[1]),
                        q[2] + f * (q[2] - p[2]),
                        q[3] + f * (q[3] - p[3]),
                        q[4] + f * (q[4] - p[4]),
                    ];
                }
            }
            return [h[n - 1][1], h[n - 1][2], h[n - 1][3], h[n - 1][4]];
        }
        let i = h.partition_point(|r| r[0] <= t).clamp(1, n - 1);
        let (p, q) = (h[i - 1], h[i]);
        let dt = q[0] - p[0];
        let f = if dt > 0.0 { (t - p[0]) / dt } else { 1.0 };
        [
            p[1] + f * (q[1] - p[1]),
            p[2] + f * (q[2] - p[2]),
            p[3] + f * (q[3] - p[3]),
            p[4] + f * (q[4] - p[4]),
        ]
    }

    fn stamp_common(&self, ck: &mut Ckt) {
        let s = &self.s;
        // KCL
        ck.add(s[0], 1.0);
        ck.add(s[1], -1.0);
        ck.add(s[2], 1.0);
        ck.add(s[3], -1.0);
        // port equations: v_k − Z0·i_k
        ck.add(s[4], 1.0);
        ck.add(s[5], -1.0);
        ck.add(s[6], -self.z0);
        ck.add(s[7], 1.0);
        ck.add(s[8], -1.0);
        ck.add(s[9], -self.z0);
    }

    fn stamp_coupling(&self, ck: &mut Ckt, k: f64) {
        let s = &self.s;
        // row br1: −k·(v2 + Z0·i2)
        ck.add(s[10], -k);
        ck.add(s[11], k);
        ck.add(s[12], -k * self.z0);
        // row br2: −k·(v1 + Z0·i1)
        ck.add(s[13], -k);
        ck.add(s[14], k);
        ck.add(s[15], -k * self.z0);
    }
}

impl Dev for Tline {
    fn name(&self) -> &str {
        &self.name
    }
    fn setup_unknowns(&mut self, s: &mut Setup) -> Result<(), SimError> {
        self.br1 = s.alloc(format!("{}#branch1", self.name), UKind::Branch);
        self.br2 = s.alloc(format!("{}#branch2", self.name), UKind::Branch);
        Ok(())
    }
    fn setup_matrix(&mut self, s: &mut Setup) -> Result<(), SimError> {
        let [a, b, c, d] = self.nodes;
        let (b1, b2) = (self.br1, self.br2);
        self.s = [
            s.slot(a, b1),
            s.slot(b, b1),
            s.slot(c, b2),
            s.slot(d, b2),
            s.slot(b1, a),
            s.slot(b1, b),
            s.slot(b1, b1),
            s.slot(b2, c),
            s.slot(b2, d),
            s.slot(b2, b2),
            s.slot(b1, c),
            s.slot(b1, d),
            s.slot(b1, b2),
            s.slot(b2, a),
            s.slot(b2, b),
            s.slot(b2, b1),
        ];
        Ok(())
    }
    fn load(&mut self, ck: &mut Ckt) {
        self.stamp_common(ck);
        if ck.mode == Mode::Dc {
            self.stamp_coupling(ck, 1.0);
            return;
        }
        let d = self.at(ck.time - self.td);
        let e1 = d[2] + self.z0 * d[3];
        let e2 = d[0] + self.z0 * d[1];
        ck.add_rhs(self.br1, e1);
        ck.add_rhs(self.br2, e2);
    }
    fn ac_load(&self, ac: &mut AcCtx) {
        let s = &self.s;
        let one = C64::new(1.0, 0.0);
        ac.add(s[0], one);
        ac.add(s[1], -one);
        ac.add(s[2], one);
        ac.add(s[3], -one);
        ac.add(s[4], one);
        ac.add(s[5], -one);
        ac.add_re(s[6], -self.z0);
        ac.add(s[7], one);
        ac.add(s[8], -one);
        ac.add_re(s[9], -self.z0);
        let k = C64::from_polar(1.0, -ac.omega * self.td);
        ac.add(s[10], -k);
        ac.add(s[11], k);
        ac.add(s[12], -k.scale(self.z0));
        ac.add(s[13], -k);
        ac.add(s[14], k);
        ac.add(s[15], -k.scale(self.z0));
    }
    fn trunc(&self, _ck: &Ckt) -> f64 {
        self.td
    }
    fn accept(&mut self, ck: &Ckt) {
        let v = self.port_values(&ck.x);
        let t = if ck.mode == Mode::Dc { 0.0 } else { ck.time };
        if let Some(last) = self.hist.last() {
            if t <= last[0] {
                // re-accept at the same time (discontinuity re-solve): replace
                self.hist.pop();
            }
        }
        self.hist.push([t, v[0], v[1], v[2], v[3]]);
        // corner detection → breakpoint one delay later
        let n = self.hist.len();
        if n >= 3 {
            let (a, b, c) = (self.hist[n - 3], self.hist[n - 2], self.hist[n - 1]);
            let (d1, d2) = (b[0] - a[0], c[0] - b[0]);
            if d1 > 0.0 && d2 > 0.0 {
                for k in 1..5 {
                    let s1 = (b[k] - a[k]) / d1;
                    let s2 = (c[k] - b[k]) / d2;
                    let scale = b[k].abs().max(1e-3) / d1.max(d2);
                    if (s1 - s2).abs() > 0.5 * s1.abs().max(s2.abs())
                        && (s1 - s2).abs() > 1e-3 * scale
                    {
                        self.corners.push(b[0] + self.td);
                        break;
                    }
                }
            }
        }
        // prune history older than needed
        let keep_from = t - 2.0 * self.td;
        let first_needed = self
            .hist
            .partition_point(|r| r[0] < keep_from)
            .saturating_sub(2);
        if first_needed > 64 {
            self.hist.drain(..first_needed);
        }
        self.corners.retain(|&c| c > t);
    }
    fn breakpoints(&self, t: f64, t_end: f64, out: &mut Vec<f64>) {
        for &c in &self.corners {
            if c > t && c <= t_end {
                out.push(c);
            }
        }
    }
    fn init_uic(&mut self, _ck: &mut Ckt) {
        self.hist.clear();
        self.hist
            .push([0.0, self.ic[0], self.ic[1], self.ic[2], self.ic[3]]);
    }
    fn is_reactive(&self) -> bool {
        true
    }
    fn current(&self, ck: &Ckt) -> Option<f64> {
        Some(ck.x[self.br1])
    }
}
