//! Compact vector storage and exact (brute-force) search.
//!
//! Vectors are stored in SQLite as int8 with one scale per vector (4x smaller than f32, cosine error well under 1%).
//! They are held in memory when the [`MemoryBudget`] allows; otherwise each query streams them from SQLite, which is slower
//! but uses constant memory. Both paths return identical results.

use ibos_mem::{MemoryBudget, Reservation};

pub(crate) fn quantise(v: &[f32]) -> (f32, Vec<i8>) {
    let max = v.iter().fold(0.0_f32, |m, x| m.max(x.abs()));
    if max == 0.0 {
        return (0.0, vec![0; v.len()]);
    }
    let scale = max / 127.0;
    (scale, v.iter().map(|x| (x / scale).round().clamp(-127.0, 127.0) as i8).collect())
}

pub(crate) fn bytes_to_i8(b: &[u8]) -> Vec<i8> {
    b.iter().map(|x| *x as i8).collect()
}

pub(crate) fn i8_to_bytes(v: &[i8]) -> Vec<u8> {
    v.iter().map(|x| *x as u8).collect()
}

/// Dot product of a unit float query with a stored vector (the stored vector is unit length before quantising).
pub(crate) fn dot(q: &[f32], v: &[i8], scale: f32) -> f32 {
    q.iter().zip(v).map(|(a, b)| a * f32::from(*b)).sum::<f32>() * scale
}

/// Keeps the best `k` (score, id) pairs seen so far, using bounded memory.
#[derive(Debug)]
pub(crate) struct TopK {
    k: usize,
    items: Vec<(f32, i64)>,
}

impl TopK {
    pub(crate) fn new(k: usize) -> Self {
        Self { k: k.max(1), items: Vec::new() }
    }

    pub(crate) fn push(&mut self, score: f32, id: i64) {
        self.items.push((score, id));
        if self.items.len() >= self.k * 4 + 16 {
            self.prune();
        }
    }

    fn prune(&mut self) {
        if self.items.len() > self.k {
            self.items.select_nth_unstable_by(self.k - 1, |a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            self.items.truncate(self.k);
        }
    }

    pub(crate) fn finish(mut self) -> Vec<(f32, i64)> {
        self.prune();
        self.items.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        self.items
    }
}

/// All vectors held in RAM, plus the reservation that pays for them.
#[derive(Debug)]
pub(crate) struct Resident {
    pub(crate) dim: usize,
    pub(crate) chunk_ids: Vec<i64>,
    pub(crate) doc_ids: Vec<i64>,
    pub(crate) scales: Vec<f32>,
    pub(crate) data: Vec<i8>,
    _reservation: Reservation,
}

impl Resident {
    /// Bytes needed for `n` vectors of `dim`.
    pub(crate) fn cost(n: usize, dim: usize) -> usize {
        n.saturating_mul(dim + 8 + 8 + 4)
    }

    pub(crate) fn try_new(budget: &MemoryBudget, n: usize, dim: usize) -> Option<ResidentBuilder> {
        let r = budget.reserve(Self::cost(n, dim))?;
        Some(ResidentBuilder {
            inner: Self {
                dim,
                chunk_ids: Vec::with_capacity(n),
                doc_ids: Vec::with_capacity(n),
                scales: Vec::with_capacity(n),
                data: Vec::with_capacity(n * dim),
                _reservation: r,
            },
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.chunk_ids.len()
    }

    pub(crate) fn search(&self, q: &[f32], k: usize, hidden: &std::collections::HashSet<i64>) -> Vec<(f32, i64)> {
        let mut top = TopK::new(k);
        for i in 0..self.len() {
            if hidden.contains(&self.doc_ids[i]) {
                continue;
            }
            top.push(dot(q, &self.data[i * self.dim..(i + 1) * self.dim], self.scales[i]), self.chunk_ids[i]);
        }
        top.finish()
    }
}

pub(crate) struct ResidentBuilder {
    inner: Resident,
}

impl ResidentBuilder {
    pub(crate) fn push(&mut self, chunk: i64, doc: i64, scale: f32, v: &[i8]) {
        if v.len() == self.inner.dim {
            self.inner.chunk_ids.push(chunk);
            self.inner.doc_ids.push(doc);
            self.inner.scales.push(scale);
            self.inner.data.extend_from_slice(v);
        }
    }

    pub(crate) fn done(self) -> Resident {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn quantising_keeps_cosine_close() {
        let v: Vec<f32> = (0..64).map(|i| ((i * 37 % 17) as f32 - 8.0) / 10.0).collect();
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let u: Vec<f32> = v.iter().map(|x| x / n).collect();
        let (s, q) = quantise(&u);
        assert!((dot(&u, &q, s) - 1.0).abs() < 0.01);
    }

    #[test]
    fn zero_vector_scores_zero() {
        let (s, q) = quantise(&[0.0; 8]);
        assert!(dot(&[1.0; 8], &q, s).abs() < 1e-9);
    }

    #[test]
    fn bytes_round_trip() {
        let v: Vec<i8> = vec![-127, -1, 0, 1, 127];
        assert_eq!(bytes_to_i8(&i8_to_bytes(&v)), v);
    }

    #[test]
    fn budget_refusal_is_reported_not_overrun() {
        let b = MemoryBudget::new(100, 1000);
        assert!(Resident::try_new(&b, 1000, 384).is_none());
        assert_eq!(b.used(), 0);
        let r = Resident::try_new(&b, 10, 8).expect("fits");
        assert!(b.used() > 0);
        drop(r);
        assert_eq!(b.used(), 0);
    }

    proptest! {
        #[test]
        fn topk_matches_a_full_sort(scores in proptest::collection::vec(-1.0_f32..1.0, 0..200), k in 1usize..20) {
            let mut t = TopK::new(k);
            for (i, s) in scores.iter().enumerate() { t.push(*s, i as i64); }
            let got = t.finish();
            let mut all: Vec<(f32, i64)> = scores.iter().enumerate().map(|(i, s)| (*s, i as i64)).collect();
            all.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            all.truncate(k);
            prop_assert_eq!(got, all);
        }
    }
}
