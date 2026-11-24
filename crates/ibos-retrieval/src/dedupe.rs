//! Near-duplicate removal: the same paragraph copied into several files should not fill the answer context.
//! Passages are compared by the overlap of their 3-word shingles (Jaccard). The better-ranked one is kept.

use std::collections::HashSet;

use crate::embed::words;

fn shingles(text: &str) -> HashSet<u64> {
    let w = words(text);
    if w.len() < 3 {
        return w.iter().map(|x| hash(x)).collect();
    }
    w.windows(3).map(|t| hash(&t.join(" "))).collect()
}

fn hash(s: &str) -> u64 {
    let b = blake3::hash(s.as_bytes());
    u64::from_le_bytes(b.as_bytes()[..8].try_into().unwrap_or([0; 8]))
}

pub(crate) fn jaccard(a: &HashSet<u64>, b: &HashSet<u64>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    a.intersection(b).count() as f32 / a.union(b).count() as f32
}

/// Indices (into `texts`, in order) to keep.
pub(crate) fn keep(texts: &[&str], threshold: f32) -> Vec<usize> {
    let sets: Vec<HashSet<u64>> = texts.iter().map(|t| shingles(t)).collect();
    let mut kept: Vec<usize> = Vec::new();
    for i in 0..texts.len() {
        if !kept.iter().any(|j| jaccard(&sets[i], &sets[*j]) >= threshold) {
            kept.push(i);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_collapse_but_different_text_stays() {
        let a = "The refund policy allows returns within thirty days of delivery for unused items.";
        let b = "The refund policy allows returns within thirty days of delivery for unused items!";
        let c = "Shipping to remote regions can take up to three weeks depending on the carrier.";
        assert_eq!(keep(&[a, b, c], 0.8), vec![0, 2]);
    }

    #[test]
    fn a_small_edit_in_a_long_passage_is_still_a_duplicate() {
        let base: Vec<String> = (0..60).map(|i| format!("word{i}")).collect();
        let a = base.join(" ");
        let mut e = base.clone();
        e[30] = "changed".into();
        assert_eq!(keep(&[&a, &e.join(" ")], 0.8), vec![0]);
    }

    #[test]
    fn empty_input() {
        assert!(keep(&[], 0.8).is_empty());
    }
}
