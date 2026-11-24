//! A cheap, deterministic reranker over the fused candidates (no model, microseconds per passage).
//!
//! It rewards passages that contain *all* the query terms, contain them close together, contain adjacent query words as a
//! phrase, or match the title. It is a heuristic: it improves precision at the top for keyword-style questions but cannot
//! judge meaning. A cross-encoder model could implement the same [`Reranker`] trait.

use std::collections::HashSet;

use ibos_search::text::query_terms;

use crate::embed::{stem, words};

pub trait Reranker: Send + Sync + std::fmt::Debug {
    /// Score in `[0, 1]`; higher is better. `base` is the fused score scaled to `[0, 1]`.
    fn score(&self, query: &str, title: &str, text: &str, base: f32) -> f32;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct HeuristicReranker;

impl Reranker for HeuristicReranker {
    fn score(&self, query: &str, title: &str, text: &str, base: f32) -> f32 {
        let terms: Vec<String> = {
            let mut seen = HashSet::new();
            query_terms(query).iter().map(|t| stem(t)).filter(|t| seen.insert(t.clone())).collect()
        };
        if terms.is_empty() {
            return base;
        }
        let toks: Vec<String> = words(text).iter().map(|w| stem(w)).collect();
        // position of every query term occurrence
        let mut hits: Vec<(usize, usize)> = Vec::new();
        for (pos, t) in toks.iter().enumerate() {
            if let Some(ti) = terms.iter().position(|q| q == t) {
                hits.push((pos, ti));
            }
        }
        let matched: HashSet<usize> = hits.iter().map(|h| h.1).collect();
        let coverage = matched.len() as f32 / terms.len() as f32;
        let proximity = smallest_window(&hits, matched.len()).map_or(0.0, |w| matched.len() as f32 / w as f32);
        let phrase = f32::from(
            terms.len() > 1 && terms.windows(2).any(|pair| toks.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1])),
        );
        let title_toks: HashSet<String> = words(title).iter().map(|w| stem(w)).collect();
        let title_hit = terms.iter().filter(|t| title_toks.contains(*t)).count() as f32 / terms.len() as f32;
        (0.45 * base + 0.30 * coverage + 0.10 * proximity + 0.10 * phrase + 0.05 * title_hit).clamp(0.0, 1.0)
    }
}

/// Length of the shortest token window containing every one of `need` distinct terms.
fn smallest_window(hits: &[(usize, usize)], need: usize) -> Option<usize> {
    if need == 0 {
        return None;
    }
    let mut count = vec![0_usize; hits.iter().map(|h| h.1).max()? + 1];
    let (mut have, mut best, mut left) = (0, None::<usize>, 0);
    for right in 0..hits.len() {
        let t = hits[right].1;
        if count[t] == 0 {
            have += 1;
        }
        count[t] += 1;
        while have == need {
            let w = hits[right].0 - hits[left].0 + 1;
            best = Some(best.map_or(w, |b| b.min(w)));
            let lt = hits[left].1;
            count[lt] -= 1;
            if count[lt] == 0 {
                have -= 1;
            }
            left += 1;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_coverage_and_proximity_beat_scattered_partial_matches() {
        let r = HeuristicReranker;
        let close = r.score("refund policy", "Terms", "Our refund policy allows returns within 30 days.", 0.5);
        let far =
            r.score("refund policy", "Terms", "A refund may be issued. Many pages later: the company policy.", 0.5);
        let partial = r.score("refund policy", "Terms", "Shipping takes five days.", 0.5);
        assert!(close > far && far > partial, "{close} {far} {partial}");
    }

    #[test]
    fn stemming_matches_endings() {
        let r = HeuristicReranker;
        assert!(r.score("invoices", "", "the invoice was paid", 0.0) > 0.2);
    }

    #[test]
    fn window_is_the_shortest_cover() {
        // terms 0 and 1 at positions 0, 5, 6
        assert_eq!(smallest_window(&[(0, 0), (5, 0), (6, 1)], 2), Some(2));
        assert_eq!(smallest_window(&[], 1), None);
    }

    #[test]
    fn no_usable_terms_keeps_the_base_score() {
        assert!((HeuristicReranker.score("", "t", "text", 0.42) - 0.42).abs() < f32::EPSILON);
    }
}
