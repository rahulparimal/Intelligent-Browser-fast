//! Reciprocal rank fusion: combines ranked lists without comparing their scores (BM25 and cosine are on different scales).
//! `score(d) = sum over lists of weight / (K + rank)`, rank starting at 1. K=60 is the value from the original paper.

use std::collections::HashMap;

pub(crate) const K: f32 = 60.0;

/// Fuse ranked id lists. Ties break on the smaller id so the output is deterministic.
pub(crate) fn rrf(lists: &[(&[i64], f32)]) -> Vec<(i64, f32)> {
    let mut score: HashMap<i64, f32> = HashMap::new();
    for (list, weight) in lists {
        for (i, id) in list.iter().enumerate() {
            *score.entry(*id).or_default() += weight / (K + (i + 1) as f32);
        }
    }
    let mut v: Vec<(i64, f32)> = score.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agreement_beats_a_single_first_place() {
        let a = [1, 2, 3];
        let b = [2, 4, 1];
        let f = rrf(&[(&a, 1.0), (&b, 1.0)]);
        assert_eq!(f[0].0, 2);
    }

    #[test]
    fn weights_shift_the_order() {
        let a = [1, 2];
        let b = [2, 1];
        assert_eq!(rrf(&[(&a, 3.0), (&b, 1.0)])[0].0, 1);
        assert_eq!(rrf(&[(&a, 1.0), (&b, 3.0)])[0].0, 2);
    }

    #[test]
    fn empty_input_and_ties_are_stable() {
        assert!(rrf(&[]).is_empty());
        let f = rrf(&[(&[5, 3][..], 1.0), (&[3, 5][..], 1.0)]);
        assert_eq!(f.iter().map(|x| x.0).collect::<Vec<_>>(), vec![3, 5]);
    }
}
