//! History without linear growth: a facts ledger, a few raw steps, and a rolling one-line-per-step summary.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;

use crate::tokens::Tokenizer;

/// One executed step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    pub n: u32,
    pub action: String,
    pub outcome: String,
}

impl StepRecord {
    fn line(&self, max_outcome_chars: usize) -> String {
        let outcome: String = self.outcome.chars().take(max_outcome_chars).collect();
        let ellipsis = if self.outcome.chars().count() > max_outcome_chars { "..." } else { "" };
        format!("{}. {} -> {outcome}{ellipsis}", self.n, self.action)
    }
}

/// What the agent has established, independent of how many steps it took to learn it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FactsLedger {
    pub plan: Vec<String>,
    pub facts: BTreeMap<String, String>,
    pub open_questions: Vec<String>,
}

impl FactsLedger {
    pub fn render(&self) -> String {
        let mut out = String::new();
        if !self.plan.is_empty() {
            let _ = writeln!(out, "plan: {}", self.plan.join(" > "));
        }
        for (k, v) in &self.facts {
            let _ = writeln!(out, "fact {k}: {v}");
        }
        for q in &self.open_questions {
            let _ = writeln!(out, "open: {q}");
        }
        out.trim_end().to_owned()
    }
}

/// Bounded history. Steps older than the raw window are shown as one-line summaries; summaries older than the cap are
/// dropped with a marker.
#[derive(Debug, Clone)]
pub struct History {
    pub ledger: FactsLedger,
    recent: VecDeque<StepRecord>,
    max_recent: usize,
}

impl History {
    /// `max_recent` bounds the number of steps kept at all (older ones are discarded).
    pub fn new(max_recent: usize) -> Self {
        Self { ledger: FactsLedger::default(), recent: VecDeque::new(), max_recent: max_recent.max(1) }
    }

    pub fn push(&mut self, step: StepRecord) {
        self.recent.push_back(step);
        while self.recent.len() > self.max_recent {
            self.recent.pop_front();
        }
    }

    pub fn len(&self) -> usize {
        self.recent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recent.is_empty()
    }

    /// Render the last `raw_steps` in full and everything older as summary lines, within `summary_tokens`.
    pub fn render(
        &self,
        raw_steps: usize,
        include_summary: bool,
        summary_tokens: usize,
        tok: &dyn Tokenizer,
    ) -> String {
        let split = self.recent.len().saturating_sub(raw_steps);
        let mut out = String::new();
        if include_summary && split > 0 {
            let lines: Vec<String> = self.recent.iter().take(split).map(|s| s.line(60)).collect();
            // keep the newest summary lines that fit
            let mut kept: Vec<&str> = Vec::new();
            let mut used = 0;
            for l in lines.iter().rev() {
                let c = tok.count(l);
                if used + c > summary_tokens {
                    break;
                }
                used += c;
                kept.push(l);
            }
            kept.reverse();
            if kept.len() < lines.len() {
                let _ = writeln!(out, "(earlier steps omitted)");
            }
            let _ = writeln!(out, "earlier:");
            for l in kept {
                let _ = writeln!(out, "  {l}");
            }
        }
        for s in self.recent.iter().skip(split) {
            let _ = writeln!(out, "{}", s.line(400));
        }
        out.trim_end().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::ApproxTokenizer;

    fn step(n: u32) -> StepRecord {
        StepRecord { n, action: format!("click {n}"), outcome: format!("ok {}", "x".repeat(100)) }
    }

    #[test]
    fn recent_steps_are_raw_older_are_one_line_summaries() {
        let mut h = History::new(20);
        for n in 1..=6 {
            h.push(step(n));
        }
        let out = h.render(2, true, 1000, &ApproxTokenizer);
        assert!(out.contains("earlier:") && out.contains("1. click 1 -> ok xxx") && out.contains("..."));
        let raw: Vec<_> = out.lines().filter(|l| l.starts_with("5.") || l.starts_with("6.")).collect();
        assert!(raw.iter().all(|l| l.len() > 100), "raw steps keep their full outcome");
    }

    #[test]
    fn size_is_bounded_regardless_of_step_count() {
        let tok = ApproxTokenizer;
        let mut h = History::new(30);
        let mut sizes = Vec::new();
        for n in 1..=200 {
            h.push(step(n));
            sizes.push(tok.count(&h.render(3, true, 120, &tok)));
        }
        assert!(sizes.iter().max().unwrap() < &400, "history does not grow linearly: {sizes:?}");
        assert_eq!(h.len(), 30);
    }

    #[test]
    fn summary_only_mode_and_no_summary_mode() {
        let mut h = History::new(10);
        (1..=5).for_each(|n| h.push(step(n)));
        assert!(!h.render(0, true, 1000, &ApproxTokenizer).is_empty());
        assert!(h.render(2, false, 1000, &ApproxTokenizer).lines().all(|l| !l.contains("earlier")));
    }

    #[test]
    fn tight_summary_budget_drops_oldest_with_marker() {
        let mut h = History::new(50);
        (1..=20).for_each(|n| h.push(step(n)));
        let out = h.render(1, true, 40, &ApproxTokenizer);
        assert!(out.contains("(earlier steps omitted)") && !out.contains("1. click 1 ->"));
    }

    #[test]
    fn ledger_renders_compactly() {
        let mut l = FactsLedger { plan: vec!["open site".into(), "log in".into()], ..FactsLedger::default() };
        l.facts.insert("email".into(), "me@example.com".into());
        l.open_questions.push("which plan?".into());
        assert_eq!(l.render(), "plan: open site > log in\nfact email: me@example.com\nopen: which plan?");
        assert_eq!(FactsLedger::default().render(), "");
    }
}
