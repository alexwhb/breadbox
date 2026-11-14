//! Deterministic random numbers (SplitMix64 + xoshiro256**). Every random source in the
//! simulator takes an explicit seed so runs replay bit-identically (ARCHITECTURE §2).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rng {
    s: [u64; 4],
}

fn splitmix(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        let mut x = seed;
        Rng {
            s: [splitmix(&mut x), splitmix(&mut x), splitmix(&mut x), splitmix(&mut x)],
        }
    }

    /// A child generator for a named stream (stable for a given parent seed and key).
    pub fn derive(seed: u64, key: &str) -> Rng {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in key.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        Rng::new(seed ^ h.rotate_left(17))
    }

    pub fn next_u64(&mut self) -> u64 {
        let r = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        r
    }

    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.uniform()
    }

    /// Standard normal (Box–Muller).
    pub fn gauss(&mut self) -> f64 {
        let u1 = self.uniform().max(1e-300);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }

    /// Integer in [0, n).
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_well_spread() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut r = Rng::new(42);
        let n = 20_000;
        let (mut sum, mut sq) = (0.0, 0.0);
        for _ in 0..n {
            let g = r.gauss();
            sum += g;
            sq += g * g;
        }
        let mean = sum / n as f64;
        let var = sq / n as f64 - mean * mean;
        assert!(mean.abs() < 0.03, "{mean}");
        assert!((var - 1.0).abs() < 0.05, "{var}");
        assert_ne!(Rng::derive(1, "a").next_u64(), Rng::derive(1, "b").next_u64());
    }
}
