//! Per-task token budget with a fixed degradation ladder.

/// What to give up as a task approaches its token budget, least valuable first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Full,
    NoScreenshots,
    CompactHistory,
    SummaryOnly,
    StopAndAsk,
}

/// Map spend to a level. A zero budget means "stop".
pub fn level(spent: u64, budget: u64) -> Level {
    match spent.saturating_mul(100).checked_div(budget).unwrap_or(100) {
        0..=49 => Level::Full,
        50..=69 => Level::NoScreenshots,
        70..=84 => Level::CompactHistory,
        85..=99 => Level::SummaryOnly,
        _ => Level::StopAndAsk,
    }
}

/// Concrete context decisions derived from a [`Level`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub level: Level,
    pub allow_screenshot: bool,
    /// How many raw history steps to keep in the prompt.
    pub raw_history_steps: usize,
    /// Whether the rolling summary is included.
    pub include_summary: bool,
    pub stop: bool,
}

/// Tracks token spend for one task.
#[derive(Debug, Clone)]
pub struct Governor {
    budget: u64,
    spent: u64,
    default_raw_steps: usize,
}

impl Governor {
    pub fn new(budget_tokens: u64, default_raw_steps: usize) -> Self {
        Self { budget: budget_tokens, spent: 0, default_raw_steps }
    }

    pub fn record(&mut self, tokens: u64) {
        self.spent = self.spent.saturating_add(tokens);
    }

    pub fn spent(&self) -> u64 {
        self.spent
    }

    pub fn level(&self) -> Level {
        level(self.spent, self.budget)
    }

    pub fn plan(&self) -> Plan {
        let level = self.level();
        Plan {
            level,
            allow_screenshot: level < Level::NoScreenshots,
            raw_history_steps: match level {
                Level::Full | Level::NoScreenshots => self.default_raw_steps,
                Level::CompactHistory => 2,
                Level::SummaryOnly | Level::StopAndAsk => 0,
            },
            include_summary: level != Level::StopAndAsk,
            stop: level == Level::StopAndAsk,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn ladder_boundaries() {
        let ls: Vec<_> = [0, 49, 50, 69, 70, 84, 85, 99, 100, 500].iter().map(|s| level(*s, 100)).collect();
        assert_eq!(
            ls,
            [
                Level::Full,
                Level::Full,
                Level::NoScreenshots,
                Level::NoScreenshots,
                Level::CompactHistory,
                Level::CompactHistory,
                Level::SummaryOnly,
                Level::SummaryOnly,
                Level::StopAndAsk,
                Level::StopAndAsk
            ]
        );
        assert_eq!(level(5, 0), Level::StopAndAsk);
        assert_eq!(level(u64::MAX, 1), Level::StopAndAsk, "no overflow");
    }

    #[test]
    fn plan_degrades_in_order() {
        let mut g = Governor::new(1000, 6);
        assert!(g.plan().allow_screenshot && g.plan().raw_history_steps == 6);
        g.record(500);
        assert!(!g.plan().allow_screenshot && g.plan().raw_history_steps == 6);
        g.record(200);
        assert_eq!(g.plan().raw_history_steps, 2);
        g.record(150);
        assert_eq!(g.plan().raw_history_steps, 0);
        assert!(!g.plan().stop);
        g.record(150);
        assert!(g.plan().stop);
    }

    proptest! {
        #[test]
        fn level_is_monotonic_in_spend(budget in 1u64..10_000, a in 0u64..20_000, b in 0u64..20_000) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(level(lo, budget) <= level(hi, budget));
        }
    }
}
