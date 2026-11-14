//! Linear solver for the MNA system.
//!
//! **Why hand-written?** SPICE matrices are small-to-medium, very sparse, unsymmetric, often have
//! zero diagonals (voltage-source branch rows) and are re-factored thousands of times with the
//! *same* structure. The classic answer (Kundert's Sparse 1.3, used by ngspice) is Markowitz
//! pivoting with a threshold test, followed by numeric-only refactorisation that reuses the pivot
//! order and fill pattern. General-purpose crates (e.g. `faer`) pick orderings from structure
//! alone (COLAMD + partial pivoting) and are heavyweight dependencies; implementing the
//! SPICE-style scheme directly is ~500 lines, keeps the dependency tree tiny, and lets us map
//! singular pivots back to node names for diagnostics.
//!
//! * [`Pattern`]: the set of structurally non-zero positions, registered by devices during setup
//!   (each gets a [`Slot`], an index into the value array; slot 0 swallows writes to ground).
//! * [`Matrix<T>`]: values for one pattern, plus the factorisation. `T` is `f64` (DC/transient)
//!   or [`C64`] (AC/noise).
//! * Ordering: Markowitz cost `(r-1)(c-1)` with a relative column threshold (`1e-3`), searching
//!   columns in increasing count order. Symbolic fill is computed once per ordering; later
//!   factorisations are numeric only, unless a pivot becomes too small (then we re-order).
//! * Systems with at most [`DENSE_MAX`] unknowns use a dense LU with partial pivoting.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub, SubAssign};

/// Unknown count at or below which the dense solver is used.
pub const DENSE_MAX: usize = 12;

// ---------------------------------------------------------------------------------------------
// Scalars
// ---------------------------------------------------------------------------------------------

/// Minimal complex number (re, im) used by AC and noise analysis.
#[derive(Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct C64 {
    pub re: f64,
    pub im: f64,
}

impl C64 {
    pub const fn new(re: f64, im: f64) -> Self {
        C64 { re, im }
    }
    pub fn from_polar(mag: f64, phase_rad: f64) -> Self {
        C64 {
            re: mag * phase_rad.cos(),
            im: mag * phase_rad.sin(),
        }
    }
    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    pub fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
    /// Phase in radians.
    pub fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }
    pub fn conj(self) -> Self {
        C64 {
            re: self.re,
            im: -self.im,
        }
    }
    pub fn scale(self, k: f64) -> Self {
        C64 {
            re: self.re * k,
            im: self.im * k,
        }
    }
    /// `e^z`.
    pub fn exp(self) -> Self {
        C64::from_polar(self.re.exp(), self.im)
    }
}

impl fmt::Debug for C64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({:e}{:+e}j)", self.re, self.im)
    }
}

impl Add for C64 {
    type Output = C64;
    fn add(self, o: C64) -> C64 {
        C64::new(self.re + o.re, self.im + o.im)
    }
}
impl Sub for C64 {
    type Output = C64;
    fn sub(self, o: C64) -> C64 {
        C64::new(self.re - o.re, self.im - o.im)
    }
}
impl Mul for C64 {
    type Output = C64;
    fn mul(self, o: C64) -> C64 {
        C64::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
}
impl Div for C64 {
    type Output = C64;
    fn div(self, o: C64) -> C64 {
        // Smith's algorithm for robustness.
        if o.re.abs() >= o.im.abs() {
            let r = o.im / o.re;
            let d = o.re + o.im * r;
            C64::new((self.re + self.im * r) / d, (self.im - self.re * r) / d)
        } else {
            let r = o.re / o.im;
            let d = o.re * r + o.im;
            C64::new((self.re * r + self.im) / d, (self.im * r - self.re) / d)
        }
    }
}
impl Neg for C64 {
    type Output = C64;
    fn neg(self) -> C64 {
        C64::new(-self.re, -self.im)
    }
}
impl AddAssign for C64 {
    fn add_assign(&mut self, o: C64) {
        self.re += o.re;
        self.im += o.im;
    }
}
impl SubAssign for C64 {
    fn sub_assign(&mut self, o: C64) {
        self.re -= o.re;
        self.im -= o.im;
    }
}
impl MulAssign for C64 {
    fn mul_assign(&mut self, o: C64) {
        *self = *self * o;
    }
}
impl From<f64> for C64 {
    fn from(re: f64) -> Self {
        C64::new(re, 0.0)
    }
}

/// Field operations needed by the LU solver.
pub trait Scalar:
    Copy
    + Default
    + fmt::Debug
    + PartialEq
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Neg<Output = Self>
    + AddAssign
    + SubAssign
    + MulAssign
    + Send
    + Sync
    + 'static
{
    const ZERO: Self;
    const ONE: Self;
    /// Magnitude used for pivot selection.
    fn mag(self) -> f64;
    fn is_finite(self) -> bool;
}

impl Scalar for f64 {
    const ZERO: f64 = 0.0;
    const ONE: f64 = 1.0;
    fn mag(self) -> f64 {
        self.abs()
    }
    fn is_finite(self) -> bool {
        f64::is_finite(self)
    }
}

impl Scalar for C64 {
    const ZERO: C64 = C64::new(0.0, 0.0);
    const ONE: C64 = C64::new(1.0, 0.0);
    fn mag(self) -> f64 {
        self.re.abs() + self.im.abs()
    }
    fn is_finite(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }
}

// ---------------------------------------------------------------------------------------------
// Pattern
// ---------------------------------------------------------------------------------------------

/// Index of a matrix entry in the value array. `Slot(0)` is the ground sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Slot(pub u32);

impl Slot {
    pub const GROUND: Slot = Slot(0);
}

/// Structure of the MNA matrix: unknowns are numbered `1..=n` (0 is ground).
#[derive(Debug, Clone, Default)]
pub struct Pattern {
    n: usize,
    /// (row, col), 1-based, per slot; slot 0 = ground sink (0, 0).
    pos: Vec<(u32, u32)>,
    map: HashMap<(u32, u32), u32>,
}

impl Pattern {
    pub fn new(n: usize) -> Self {
        Pattern {
            n,
            pos: vec![(0, 0)],
            map: HashMap::new(),
        }
    }

    /// Number of unknowns.
    pub fn n(&self) -> usize {
        self.n
    }

    /// Grows the number of unknowns (used while devices allocate internal nodes).
    pub fn set_n(&mut self, n: usize) {
        self.n = self.n.max(n);
    }

    /// Registers (or finds) the entry at `(row, col)`; any ground index gives [`Slot::GROUND`].
    pub fn slot(&mut self, row: usize, col: usize) -> Slot {
        if row == 0 || col == 0 {
            return Slot::GROUND;
        }
        debug_assert!(
            row <= self.n && col <= self.n,
            "slot ({row},{col}) outside n={}",
            self.n
        );
        let key = (row as u32, col as u32);
        if let Some(&s) = self.map.get(&key) {
            return Slot(s);
        }
        let s = self.pos.len() as u32;
        self.pos.push(key);
        self.map.insert(key, s);
        Slot(s)
    }

    /// Number of slots including the ground sink.
    pub fn len(&self) -> usize {
        self.pos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pos.len() <= 1
    }

    /// Position of a slot.
    pub fn position(&self, s: Slot) -> (usize, usize) {
        let (r, c) = self.pos[s.0 as usize];
        (r as usize, c as usize)
    }
}

// ---------------------------------------------------------------------------------------------
// Matrix + factorisation
// ---------------------------------------------------------------------------------------------

/// Why a factorisation failed: the 1-based row/column (unknown index) that had no usable pivot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SingularAt {
    pub row: usize,
    pub col: usize,
}

#[derive(Debug, Clone)]
struct SparseLu<T> {
    prow: Vec<u32>,
    pcol: Vec<u32>,
    row_ptr: Vec<u32>,
    cols: Vec<u32>,
    diag: Vec<u32>,
    lu: Vec<T>,
    slot_to_lu: Vec<u32>,
    work: Vec<T>,
    valid: bool,
}

#[derive(Debug, Clone)]
struct DenseLu<T> {
    a: Vec<T>,
    perm: Vec<usize>,
    y: Vec<T>,
    valid: bool,
}

#[derive(Debug, Clone)]
enum Factor<T> {
    None,
    Sparse(Box<SparseLu<T>>),
    Dense(DenseLu<T>),
}

/// Statistics about the linear solver (for performance tests / diagnostics).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SolverStats {
    pub orderings: u64,
    pub factorizations: u64,
    pub solves: u64,
    /// Non-zeros in L+U after fill (sparse path).
    pub lu_nnz: usize,
}

/// A matrix with a fixed [`Pattern`] and its LU factorisation.
#[derive(Debug, Clone)]
pub struct Matrix<T: Scalar> {
    n: usize,
    pos: Vec<(u32, u32)>,
    /// Values per slot (slot 0 = ground sink, ignored).
    pub vals: Vec<T>,
    factor: Factor<T>,
    force_dense: Option<bool>,
    /// Relative pivot threshold for Markowitz ordering.
    pub pivot_rel: f64,
    /// Absolute pivot threshold.
    pub pivot_abs: f64,
    pub stats: SolverStats,
}

impl<T: Scalar> Matrix<T> {
    /// Creates a matrix for `pattern` (values zeroed).
    pub fn new(pattern: &Pattern) -> Self {
        Matrix {
            n: pattern.n,
            pos: pattern.pos.clone(),
            vals: vec![T::ZERO; pattern.pos.len()],
            factor: Factor::None,
            force_dense: None,
            pivot_rel: 1e-3,
            pivot_abs: 1e-13,
            stats: SolverStats::default(),
        }
    }

    /// Forces the dense (`true`) or sparse (`false`) path regardless of size (for tests).
    pub fn force_dense(&mut self, dense: bool) {
        self.force_dense = Some(dense);
        self.factor = Factor::None;
    }

    pub fn n(&self) -> usize {
        self.n
    }

    /// Zeroes all values (keeps the factorisation structure for reuse).
    pub fn clear(&mut self) {
        self.vals.iter_mut().for_each(|v| *v = T::ZERO);
    }

    #[inline]
    pub fn add(&mut self, s: Slot, v: T) {
        self.vals[s.0 as usize] += v;
    }

    /// Value at a slot.
    pub fn get(&self, s: Slot) -> T {
        self.vals[s.0 as usize]
    }

    /// Discards the pivot order so the next [`Matrix::factor`] re-runs Markowitz ordering.
    pub fn invalidate_ordering(&mut self) {
        match &mut self.factor {
            Factor::Sparse(s) => s.valid = false,
            Factor::Dense(d) => d.valid = false,
            Factor::None => {}
        }
    }

    fn use_dense(&self) -> bool {
        self.force_dense.unwrap_or(self.n <= DENSE_MAX)
    }

    /// Factorises the current values. Reuses the previous pivot order when possible.
    pub fn factor(&mut self) -> Result<(), SingularAt> {
        self.stats.factorizations += 1;
        if self.n == 0 {
            return Ok(());
        }
        if self.use_dense() {
            return self.factor_dense();
        }
        let reuse = matches!(&self.factor, Factor::Sparse(s) if s.valid);
        if reuse {
            if self.refactor_sparse() {
                return Ok(());
            }
        }
        self.order_sparse()?;
        if self.refactor_sparse() {
            Ok(())
        } else {
            // Ordering chose acceptable pivots on these very values; failing here means the
            // matrix is numerically singular.
            let s = match &self.factor {
                Factor::Sparse(s) => s,
                _ => unreachable!(),
            };
            let k = (0..self.n)
                .find(|&k| s.lu[s.diag[k] as usize].mag() <= self.pivot_abs)
                .unwrap_or(0);
            Err(SingularAt {
                row: s.prow[k] as usize + 1,
                col: s.pcol[k] as usize + 1,
            })
        }
    }

    fn factor_dense(&mut self) -> Result<(), SingularAt> {
        let n = self.n;
        let mut d = match std::mem::replace(&mut self.factor, Factor::None) {
            Factor::Dense(d) => d,
            _ => DenseLu {
                a: vec![T::ZERO; n * n],
                perm: vec![0; n],
                y: vec![T::ZERO; n],
                valid: false,
            },
        };
        d.a.iter_mut().for_each(|v| *v = T::ZERO);
        for (s, &(r, c)) in self.pos.iter().enumerate().skip(1) {
            d.a[(r as usize - 1) * n + (c as usize - 1)] += self.vals[s];
        }
        for (i, p) in d.perm.iter_mut().enumerate() {
            *p = i;
        }
        let a = &mut d.a;
        for k in 0..n {
            // partial pivoting on column k
            let mut best = k;
            let mut bm = a[k * n + k].mag();
            for i in k + 1..n {
                let m = a[i * n + k].mag();
                if m > bm {
                    bm = m;
                    best = i;
                }
            }
            if bm <= self.pivot_abs || !bm.is_finite() {
                let row = d.perm[best];
                self.factor = Factor::Dense(DenseLu { valid: false, ..d });
                return Err(SingularAt {
                    row: row + 1,
                    col: k + 1,
                });
            }
            if best != k {
                for j in 0..n {
                    a.swap(k * n + j, best * n + j);
                }
                d.perm.swap(k, best);
            }
            let p = a[k * n + k];
            for i in k + 1..n {
                let l = a[i * n + k] / p;
                if l == T::ZERO {
                    continue;
                }
                a[i * n + k] = l;
                for j in k + 1..n {
                    let u = a[k * n + j];
                    a[i * n + j] -= l * u;
                }
            }
        }
        d.valid = true;
        self.factor = Factor::Dense(d);
        Ok(())
    }

    /// Markowitz ordering with threshold pivoting, followed by symbolic factorisation.
    fn order_sparse(&mut self) -> Result<(), SingularAt> {
        self.stats.orderings += 1;
        let n = self.n;
        // Active submatrix: rows as (col, value) lists; columns as row-index lists.
        let mut rows: Vec<Vec<(u32, T)>> = vec![Vec::new(); n];
        let mut colrows: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (s, &(r, c)) in self.pos.iter().enumerate().skip(1) {
            let (r, c) = (r - 1, c - 1);
            rows[r as usize].push((c, self.vals[s]));
            colrows[c as usize].push(r);
        }
        let mut row_active = vec![true; n];
        let mut col_active = vec![true; n];
        let mut row_count: Vec<usize> = rows.iter().map(|r| r.len()).collect();
        let mut col_count: Vec<usize> = colrows.iter().map(|c| c.len()).collect();
        let mut prow = Vec::with_capacity(n);
        let mut pcol = Vec::with_capacity(n);
        let mut pos_in_row: Vec<i64> = vec![-1; n];
        let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); n + 1];

        for _step in 0..n {
            // bucket active columns by count
            for b in buckets.iter_mut() {
                b.clear();
            }
            for c in 0..n {
                if col_active[c] {
                    buckets[col_count[c].min(n)].push(c as u32);
                }
            }
            let mut best: Option<(usize, f64, u32, u32)> = None; // (cost, mag, row, col)
            let mut examined_with_candidate = 0;
            'search: for (cnt, bucket) in buckets.iter().enumerate() {
                if let Some((cost, ..)) = best {
                    // any column with this count costs at least (cnt-1)*0; stop once we've looked
                    // at a few columns past the first candidate.
                    if examined_with_candidate >= 4 || cost == 0 {
                        break 'search;
                    }
                    let _ = cnt;
                }
                for &c in bucket {
                    let c = c as usize;
                    // column max over active rows
                    let mut cmax = 0.0f64;
                    for &r in &colrows[c] {
                        if row_active[r as usize] {
                            if let Some(v) = rows[r as usize].iter().find(|e| e.0 as usize == c) {
                                cmax = cmax.max(v.1.mag());
                            }
                        }
                    }
                    if cmax <= self.pivot_abs {
                        continue;
                    }
                    let thr = (self.pivot_rel * cmax).max(self.pivot_abs);
                    let mut found = false;
                    for &r in &colrows[c] {
                        let ru = r as usize;
                        if !row_active[ru] {
                            continue;
                        }
                        let Some(v) = rows[ru].iter().find(|e| e.0 as usize == c) else {
                            continue;
                        };
                        let m = v.1.mag();
                        if m < thr {
                            continue;
                        }
                        let cost = (row_count[ru] - 1) * (col_count[c] - 1);
                        let better = match best {
                            None => true,
                            Some((bc, bm, _, _)) => cost < bc || (cost == bc && m > bm),
                        };
                        if better {
                            best = Some((cost, m, r, c as u32));
                        }
                        found = true;
                    }
                    if found {
                        examined_with_candidate += 1;
                        if examined_with_candidate >= 4 {
                            break 'search;
                        }
                    }
                }
            }
            let Some((_, _, pr, pc)) = best else {
                // No acceptable pivot: report a remaining active row/column.
                let row = (0..n).find(|&r| row_active[r]).unwrap_or(0);
                let col = (0..n).find(|&c| col_active[c]).unwrap_or(0);
                // Prefer a column with no entries at all (floating node).
                let empty_col = (0..n).find(|&c| col_active[c] && col_count[c] == 0);
                let empty_row = (0..n).find(|&r| row_active[r] && row_count[r] == 0);
                return Err(SingularAt {
                    row: empty_row.unwrap_or(row) + 1,
                    col: empty_col.unwrap_or(col) + 1,
                });
            };
            let (pr, pc) = (pr as usize, pc as usize);
            let pivot_row: Vec<(u32, T)> = rows[pr]
                .iter()
                .copied()
                .filter(|e| col_active[e.0 as usize])
                .collect();
            let pval = pivot_row
                .iter()
                .find(|e| e.0 as usize == pc)
                .map(|e| e.1)
                .unwrap_or(T::ONE);
            // eliminate
            let col_rows: Vec<u32> = colrows[pc].clone();
            for &i in &col_rows {
                let iu = i as usize;
                if iu == pr || !row_active[iu] {
                    continue;
                }
                // position map for row i
                for (k, e) in rows[iu].iter().enumerate() {
                    pos_in_row[e.0 as usize] = k as i64;
                }
                let aic = {
                    let k = pos_in_row[pc];
                    if k < 0 {
                        T::ZERO
                    } else {
                        rows[iu][k as usize].1
                    }
                };
                let m = aic / pval;
                for &(j, arj) in &pivot_row {
                    let ju = j as usize;
                    if ju == pc {
                        continue;
                    }
                    let k = pos_in_row[ju];
                    if k >= 0 {
                        rows[iu][k as usize].1 -= m * arj;
                    } else {
                        rows[iu].push((j, -(m * arj)));
                        pos_in_row[ju] = (rows[iu].len() - 1) as i64;
                        colrows[ju].push(i);
                        col_count[ju] += 1;
                        row_count[iu] += 1;
                    }
                }
                for e in rows[iu].iter() {
                    pos_in_row[e.0 as usize] = -1;
                }
                row_count[iu] -= 1; // column pc leaves
            }
            for &(j, _) in &pivot_row {
                col_count[j as usize] -= 1; // row pr leaves
            }
            row_active[pr] = false;
            col_active[pc] = false;
            prow.push(pr as u32);
            pcol.push(pc as u32);
        }

        // Symbolic factorisation in pivot order.
        let mut row_step = vec![0u32; n];
        let mut col_step = vec![0u32; n];
        for k in 0..n {
            row_step[prow[k] as usize] = k as u32;
            col_step[pcol[k] as usize] = k as u32;
        }
        let mut orig_rows: Vec<Vec<u32>> = vec![Vec::new(); n];
        for &(r, c) in self.pos.iter().skip(1) {
            orig_rows[row_step[r as usize - 1] as usize].push(col_step[c as usize - 1]);
        }
        let mut row_ptr = Vec::with_capacity(n + 1);
        let mut cols: Vec<u32> = Vec::new();
        let mut diag = Vec::with_capacity(n);
        row_ptr.push(0u32);
        // U patterns (cols > k) per finished row, for merging.
        let mut upat: Vec<Vec<u32>> = Vec::with_capacity(n);
        for k in 0..n {
            let mut set: BTreeSet<u32> = orig_rows[k].iter().copied().collect();
            set.insert(k as u32);
            let mut cursor: Option<u32> = set.iter().next().copied();
            while let Some(j) = cursor {
                if j as usize >= k {
                    break;
                }
                for &q in &upat[j as usize] {
                    set.insert(q);
                }
                cursor = set.range(j + 1..).next().copied();
            }
            let mut u = Vec::new();
            for &q in &set {
                if q as usize == k {
                    diag.push(cols.len() as u32);
                }
                if q as usize > k {
                    u.push(q);
                }
                cols.push(q);
            }
            upat.push(u);
            row_ptr.push(cols.len() as u32);
        }
        let mut slot_to_lu = vec![0u32; self.pos.len()];
        for (s, &(r, c)) in self.pos.iter().enumerate().skip(1) {
            let k = row_step[r as usize - 1] as usize;
            let q = col_step[c as usize - 1];
            let row = &cols[row_ptr[k] as usize..row_ptr[k + 1] as usize];
            let p = row.binary_search(&q).expect("entry in symbolic pattern");
            slot_to_lu[s] = row_ptr[k] + p as u32;
        }
        self.stats.lu_nnz = cols.len();
        let nnz = cols.len();
        self.factor = Factor::Sparse(Box::new(SparseLu {
            prow,
            pcol,
            row_ptr,
            cols,
            diag,
            lu: vec![T::ZERO; nnz],
            slot_to_lu,
            work: vec![T::ZERO; n],
            valid: true,
        }));
        Ok(())
    }

    /// Numeric factorisation with the stored order. Returns false if a pivot is unacceptable.
    fn refactor_sparse(&mut self) -> bool {
        let n = self.n;
        let Factor::Sparse(f) = &mut self.factor else {
            return false;
        };
        let f = &mut **f;
        f.lu.iter_mut().for_each(|v| *v = T::ZERO);
        for s in 1..self.vals.len() {
            f.lu[f.slot_to_lu[s] as usize] += self.vals[s];
        }
        for k in 0..n {
            let (start, end) = (f.row_ptr[k] as usize, f.row_ptr[k + 1] as usize);
            let d = f.diag[k] as usize;
            for p in start..end {
                f.work[f.cols[p] as usize] = f.lu[p];
            }
            for p in start..d {
                let j = f.cols[p] as usize;
                let wj = f.work[j];
                if wj == T::ZERO {
                    continue;
                }
                let l = wj / f.lu[f.diag[j] as usize];
                f.work[j] = l;
                for q in f.diag[j] as usize + 1..f.row_ptr[j + 1] as usize {
                    let u = f.lu[q];
                    f.work[f.cols[q] as usize] -= l * u;
                }
            }
            for p in start..end {
                let c = f.cols[p] as usize;
                f.lu[p] = f.work[c];
                f.work[c] = T::ZERO;
            }
            // Like Sparse 1.3's spFactor: keep the order unless a pivot (nearly) vanished. A
            // relative test would need the reduced column, which the row-wise refactorisation
            // never forms; Newton's convergence test guards accuracy instead.
            let piv = f.lu[d].mag();
            if !(piv > self.pivot_abs) || !piv.is_finite() {
                f.valid = false;
                return false;
            }
        }
        f.valid = true;
        true
    }

    /// Solves `A x = b` in place. `b` is indexed by unknown (`b[0]` = ground, ignored and
    /// zeroed). Must be called after a successful [`Matrix::factor`].
    pub fn solve(&mut self, b: &mut [T]) {
        self.stats.solves += 1;
        let n = self.n;
        if n == 0 {
            return;
        }
        match &mut self.factor {
            Factor::Dense(d) => {
                let y = &mut d.y;
                for i in 0..n {
                    y[i] = b[d.perm[i] + 1];
                }
                let a = &d.a;
                for i in 0..n {
                    let mut s = y[i];
                    for (j, yj) in y.iter().enumerate().take(i) {
                        s -= a[i * n + j] * *yj;
                    }
                    y[i] = s;
                }
                for i in (0..n).rev() {
                    let mut s = y[i];
                    for (j, yj) in y.iter().enumerate().skip(i + 1) {
                        s -= a[i * n + j] * *yj;
                    }
                    y[i] = s / a[i * n + i];
                }
                b[0] = T::ZERO;
                b[1..=n].copy_from_slice(y);
            }
            Factor::Sparse(f) => {
                let f = &mut **f;
                let y = &mut f.work;
                for k in 0..n {
                    y[k] = b[f.prow[k] as usize + 1];
                }
                for k in 0..n {
                    let mut s = y[k];
                    for p in f.row_ptr[k] as usize..f.diag[k] as usize {
                        s -= f.lu[p] * y[f.cols[p] as usize];
                    }
                    y[k] = s;
                }
                for k in (0..n).rev() {
                    let d = f.diag[k] as usize;
                    let mut s = y[k];
                    for p in d + 1..f.row_ptr[k + 1] as usize {
                        s -= f.lu[p] * y[f.cols[p] as usize];
                    }
                    y[k] = s / f.lu[d];
                }
                b[0] = T::ZERO;
                for k in 0..n {
                    b[f.pcol[k] as usize + 1] = y[k];
                    y[k] = T::ZERO;
                }
            }
            Factor::None => {}
        }
    }

    /// Solves `Aᵀ x = b` in place (adjoint system, used by noise analysis).
    pub fn solve_transpose(&mut self, b: &mut [T]) {
        self.stats.solves += 1;
        let n = self.n;
        if n == 0 {
            return;
        }
        match &mut self.factor {
            Factor::Dense(d) => {
                // P A = L U  =>  Aᵀ = Uᵀ Lᵀ P ; solve Uᵀ w = b, Lᵀ z = w, x = Pᵀ z
                let a = &d.a;
                let mut w: Vec<T> = (0..n).map(|i| b[i + 1]).collect();
                for i in 0..n {
                    let mut s = w[i];
                    for j in 0..i {
                        s -= a[j * n + i] * w[j];
                    }
                    w[i] = s / a[i * n + i];
                }
                for i in (0..n).rev() {
                    let mut s = w[i];
                    for j in i + 1..n {
                        s -= a[j * n + i] * w[j];
                    }
                    w[i] = s;
                }
                b[0] = T::ZERO;
                for i in 0..n {
                    b[d.perm[i] + 1] = w[i];
                }
            }
            Factor::Sparse(f) => {
                let f = &mut **f;
                let v = &mut f.work;
                for k in 0..n {
                    v[k] = b[f.pcol[k] as usize + 1];
                }
                // Uᵀ v = rhs (forward, column-oriented over U rows)
                for k in 0..n {
                    let d = f.diag[k] as usize;
                    v[k] = v[k] / f.lu[d];
                    let vk = v[k];
                    for p in d + 1..f.row_ptr[k + 1] as usize {
                        v[f.cols[p] as usize] -= f.lu[p] * vk;
                    }
                }
                // Lᵀ u = v (backward)
                for k in (0..n).rev() {
                    let uk = v[k];
                    for p in f.row_ptr[k] as usize..f.diag[k] as usize {
                        v[f.cols[p] as usize] -= f.lu[p] * uk;
                    }
                }
                b[0] = T::ZERO;
                for k in 0..n {
                    b[f.prow[k] as usize + 1] = v[k];
                    v[k] = T::ZERO;
                }
            }
            Factor::None => {}
        }
    }

    /// Computes `A x` (used by tests and residual checks); `x`/result indexed like [`solve`].
    ///
    /// [`solve`]: Matrix::solve
    pub fn mul_vec(&self, x: &[T]) -> Vec<T> {
        let mut y = vec![T::ZERO; self.n + 1];
        for (s, &(r, c)) in self.pos.iter().enumerate().skip(1) {
            y[r as usize] += self.vals[s] * x[c as usize];
        }
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((*seed >> 11) as f64) / ((1u64 << 53) as f64)
    }

    fn random_system(
        n: usize,
        density: f64,
        seed: u64,
        zero_diag_every: usize,
    ) -> (Pattern, Vec<(Slot, f64)>) {
        let mut s = seed;
        let mut p = Pattern::new(n);
        let mut entries = Vec::new();
        for i in 1..=n {
            if zero_diag_every == 0 || i % zero_diag_every != 0 {
                let sl = p.slot(i, i);
                entries.push((sl, 4.0 + lcg(&mut s)));
            }
            for j in 1..=n {
                if i != j && lcg(&mut s) < density {
                    let sl = p.slot(i, j);
                    entries.push((sl, lcg(&mut s) - 0.5));
                }
            }
            // ensure each zero-diag row/col has a strong off-diagonal partner (like V sources)
            if zero_diag_every != 0 && i % zero_diag_every == 0 {
                let j = if i > 1 { i - 1 } else { i + 1 };
                let a = p.slot(i, j);
                let b = p.slot(j, i);
                entries.push((a, 1.0));
                entries.push((b, 1.0));
            }
        }
        (p, entries)
    }

    fn check_solve(dense: bool, n: usize, zero_diag_every: usize) {
        let (p, entries) = random_system(n, 3.0 / n as f64, 42 + n as u64, zero_diag_every);
        let mut m: Matrix<f64> = Matrix::new(&p);
        m.force_dense(dense);
        for _round in 0..3 {
            m.clear();
            for (sl, v) in &entries {
                m.add(*sl, *v * (1.0 + 0.01 * _round as f64));
            }
            m.factor().expect("nonsingular");
            let x_true: Vec<f64> = (0..=n)
                .map(|i| if i == 0 { 0.0 } else { (i as f64).sin() })
                .collect();
            let mut b = m.mul_vec(&x_true);
            m.solve(&mut b);
            for i in 1..=n {
                assert!(
                    (b[i] - x_true[i]).abs() < 1e-7,
                    "dense={dense} n={n} i={i}: {} vs {}",
                    b[i],
                    x_true[i]
                );
            }
            // transpose
            let mut bt = vec![0.0; n + 1];
            for (s, &(r, c)) in m.pos.iter().enumerate().skip(1) {
                bt[c as usize] += m.vals[s] * x_true[r as usize];
            }
            m.solve_transpose(&mut bt);
            for i in 1..=n {
                assert!(
                    (bt[i] - x_true[i]).abs() < 1e-7,
                    "transpose dense={dense} n={n} i={i} {} {} zd={zero_diag_every}",
                    bt[i],
                    x_true[i]
                );
            }
        }
    }

    #[test]
    fn dense_and_sparse_agree() {
        for &n in &[1, 2, 5, 12, 30, 200] {
            check_solve(true, n, 0);
            check_solve(false, n, 0);
            check_solve(false, n, 4);
            check_solve(true, n.max(2), 3);
        }
    }

    #[test]
    fn reuse_ordering() {
        let (p, entries) = random_system(100, 0.03, 7, 5);
        let mut m: Matrix<f64> = Matrix::new(&p);
        for k in 0..5 {
            m.clear();
            for (sl, v) in &entries {
                m.add(*sl, *v * (1.0 + 0.1 * k as f64));
            }
            m.factor().unwrap();
        }
        assert_eq!(m.stats.orderings, 1);
        assert_eq!(m.stats.factorizations, 5);
    }

    #[test]
    fn singular_reports_row() {
        let mut p = Pattern::new(3);
        let a = p.slot(1, 1);
        let b = p.slot(2, 2);
        let _c = p.slot(3, 3);
        let mut m: Matrix<f64> = Matrix::new(&p);
        m.force_dense(false);
        m.add(a, 1.0);
        m.add(b, 1.0);
        let e = m.factor().unwrap_err();
        assert_eq!(e.row, 3);
        let mut m: Matrix<f64> = Matrix::new(&p);
        m.add(a, 1.0);
        m.add(b, 1.0);
        let e = m.factor().unwrap_err();
        assert_eq!(e.col, 3);
    }

    #[test]
    fn complex_solve() {
        let mut p = Pattern::new(2);
        let s11 = p.slot(1, 1);
        let s12 = p.slot(1, 2);
        let s21 = p.slot(2, 1);
        let s22 = p.slot(2, 2);
        for dense in [true, false] {
            let mut m: Matrix<C64> = Matrix::new(&p);
            m.force_dense(dense);
            m.add(s11, C64::new(1.0, 1.0));
            m.add(s12, C64::new(-1.0, 0.0));
            m.add(s21, C64::new(-1.0, 0.0));
            m.add(s22, C64::new(2.0, -0.5));
            m.factor().unwrap();
            let x = [C64::default(), C64::new(0.3, -0.2), C64::new(-1.0, 0.7)];
            let mut b = m.mul_vec(&x);
            m.solve(&mut b);
            assert!((b[1] - x[1]).abs() < 1e-12 && (b[2] - x[2]).abs() < 1e-12);
        }
    }

    #[test]
    fn grid_fill_is_reasonable() {
        // 30x30 grid Laplacian: Markowitz should keep fill far below dense.
        let k = 30;
        let n = k * k;
        let mut p = Pattern::new(n);
        let mut ent = Vec::new();
        let id = |i: usize, j: usize| i * k + j + 1;
        for i in 0..k {
            for j in 0..k {
                let a = id(i, j);
                ent.push((p.slot(a, a), 4.1));
                if i + 1 < k {
                    let b = id(i + 1, j);
                    ent.push((p.slot(a, b), -1.0));
                    ent.push((p.slot(b, a), -1.0));
                }
                if j + 1 < k {
                    let b = id(i, j + 1);
                    ent.push((p.slot(a, b), -1.0));
                    ent.push((p.slot(b, a), -1.0));
                }
            }
        }
        let mut m: Matrix<f64> = Matrix::new(&p);
        for (s, v) in &ent {
            m.add(*s, *v);
        }
        m.factor().unwrap();
        assert!(m.stats.lu_nnz < 60_000, "fill {}", m.stats.lu_nnz);
        let x: Vec<f64> = (0..=n)
            .map(|i| if i == 0 { 0.0 } else { (i % 7) as f64 })
            .collect();
        let mut b = m.mul_vec(&x);
        m.solve(&mut b);
        for i in 1..=n {
            assert!((b[i] - x[i]).abs() < 1e-9);
        }
    }
}

#[cfg(test)]
mod mna_tests {
    use super::*;

    /// Grid of conductances plus voltage-source branch rows (zero diagonals), like an MNA mesh.
    #[test]
    fn mesh_with_voltage_sources() {
        let k = 32;
        let nn = k * k;
        let n = nn + 2;
        let mut p = Pattern::new(n);
        let id = |i: usize, j: usize| i * k + j + 1;
        let mut ent = Vec::new();
        for i in 0..k {
            for j in 0..k {
                let a = id(i, j);
                ent.push((p.slot(a, a), 0.68));
                for (ii, jj) in [(i + 1, j), (i, j + 1)] {
                    if ii < k && jj < k {
                        let b = id(ii, jj);
                        ent.push((p.slot(a, a), 0.01));
                        ent.push((p.slot(b, b), 0.01));
                        ent.push((p.slot(a, b), -0.01));
                        ent.push((p.slot(b, a), -0.01));
                    }
                }
            }
        }
        for (node, br) in [(id(0, 0), nn + 1), (id(k - 1, k - 1), nn + 2)] {
            ent.push((p.slot(node, br), 1.0));
            ent.push((p.slot(br, node), 1.0));
        }
        let mut m: Matrix<f64> = Matrix::new(&p);
        for (s, v) in &ent {
            m.add(*s, *v);
        }
        m.factor().unwrap_or_else(|e| panic!("{e:?}"));
        let x: Vec<f64> = (0..=n)
            .map(|i| if i == 0 { 0.0 } else { (i % 5) as f64 })
            .collect();
        let mut b = m.mul_vec(&x);
        m.solve(&mut b);
        for i in 1..=n {
            assert!((b[i] - x[i]).abs() < 1e-8, "{i}: {} vs {}", b[i], x[i]);
        }
    }
}
