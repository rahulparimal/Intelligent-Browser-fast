//! Retrieval evaluation: the same judged queries run under several configurations, scored with standard metrics.
//!
//! A judgment names relevant documents by a path substring. Relevance is binary and per document (a document counts at the
//! rank of its first passage). `forbidden` lists path substrings that must never be returned for that caller: any appearance
//! is counted as an access-control leak.

use std::fmt::Write as _;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::{Mode, Query, RetrievalError, Retriever};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Judged {
    pub query: String,
    #[serde(default)]
    pub relevant: Vec<String>,
    #[serde(default)]
    pub principals: Vec<String>,
    #[serde(default)]
    pub forbidden: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Config {
    pub label: String,
    pub mode: Mode,
    pub rerank: bool,
    pub fuzzy: bool,
}

impl Config {
    pub fn new(label: &str, mode: Mode, rerank: bool, fuzzy: bool) -> Self {
        Self { label: label.into(), mode, rerank, fuzzy }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub label: String,
    pub queries: usize,
    pub ndcg10: f64,
    pub mrr: f64,
    pub recall10: f64,
    pub leaks: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
}

fn dcg(ranks: &[usize]) -> f64 {
    ranks.iter().map(|r| 1.0 / ((*r as f64) + 1.0).log2()).sum()
}

/// Metrics for one ranked list of document paths. Returns (ndcg@10, reciprocal rank, recall@10).
pub fn score(ranked_paths: &[String], relevant: &[String]) -> (f64, f64, f64) {
    if relevant.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut seen_rel = vec![false; relevant.len()];
    let mut hit_ranks = Vec::new();
    let mut first = None;
    for (i, p) in ranked_paths.iter().take(10).enumerate() {
        if let Some(j) = relevant.iter().position(|r| p.contains(r.as_str()))
            && !seen_rel[j]
        {
            seen_rel[j] = true;
            hit_ranks.push(i + 1);
            first.get_or_insert(i + 1);
        }
    }
    let ideal: Vec<usize> = (1..=relevant.len().min(10)).collect();
    let recall = hit_ranks.len() as f64 / relevant.len() as f64;
    (dcg(&hit_ranks) / dcg(&ideal), first.map_or(0.0, |r| 1.0 / r as f64), recall)
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[(((sorted.len() - 1) as f64) * p).round() as usize]
}

pub fn evaluate(r: &Retriever, judged: &[Judged], configs: &[Config]) -> Result<Vec<Report>, RetrievalError> {
    let mut reports = Vec::new();
    for c in configs {
        let (mut n, mut m, mut rc, mut leaks) = (0.0, 0.0, 0.0, 0);
        let mut times = Vec::new();
        for j in judged {
            let q = Query::new(&j.query)
                .mode(c.mode)
                .rerank(c.rerank)
                .fuzzy(c.fuzzy)
                .principals(j.principals.clone())
                .limit(10);
            let t = Instant::now();
            let out = r.search(&q)?;
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            let mut docs: Vec<String> = Vec::new();
            for p in &out.passages {
                if !docs.contains(&p.path) {
                    docs.push(p.path.clone());
                }
            }
            leaks += out.passages.iter().filter(|p| j.forbidden.iter().any(|f| p.path.contains(f.as_str()))).count();
            let (a, b, d) = score(&docs, &j.relevant);
            n += a;
            m += b;
            rc += d;
        }
        times.sort_by(f64::total_cmp);
        let k = judged.len().max(1) as f64;
        reports.push(Report {
            label: c.label.clone(),
            queries: judged.len(),
            ndcg10: n / k,
            mrr: m / k,
            recall10: rc / k,
            leaks,
            p50_ms: pct(&times, 0.5),
            p95_ms: pct(&times, 0.95),
        });
    }
    Ok(reports)
}

/// Markdown table of reports.
pub fn table(reports: &[Report]) -> String {
    let mut s = String::from(
        "| configuration | nDCG@10 | MRR | recall@10 | ACL leaks | p50 | p95 |\n|---|---|---|---|---|---|---|\n",
    );
    for r in reports {
        let _ = writeln!(
            s,
            "| {} | {:.3} | {:.3} | {:.3} | {} | {:.2} ms | {:.2} ms |",
            r.label, r.ndcg10, r.mrr, r.recall10, r.leaks, r.p50_ms, r.p95_ms
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn perfect_and_missing_rankings() {
        let (n, m, r) = score(&v(&["a.txt", "b.txt"]), &v(&["a.txt"]));
        assert!((n - 1.0).abs() < 1e-9 && (m - 1.0).abs() < 1e-9 && (r - 1.0).abs() < 1e-9);
        let (n, m, r) = score(&v(&["x", "y"]), &v(&["a"]));
        assert!(n.abs() + m.abs() + r.abs() < 1e-12);
    }

    #[test]
    fn rank_two_halves_the_reciprocal_rank_and_discounts_ndcg() {
        let (n, m, _) = score(&v(&["x", "a"]), &v(&["a"]));
        assert!((m - 0.5).abs() < 1e-9);
        assert!((n - 1.0 / 3.0_f64.log2()).abs() < 1e-9);
    }

    #[test]
    fn a_document_counts_once() {
        let (_, _, r) = score(&v(&["a", "a", "a"]), &v(&["a", "b"]));
        assert!((r - 0.5).abs() < 1e-9);
    }

    #[test]
    fn percentiles() {
        assert!((pct(&[1.0, 2.0, 3.0, 4.0, 5.0], 0.5) - 3.0).abs() < 1e-9);
        assert!(pct(&[], 0.9).abs() < 1e-9);
    }
}
