//! Token usage accounting, including prompt-cache hits as reported by the provider.

use serde::{Deserialize, Serialize};

/// Usage for one model call, as reported by the provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub prompt: u64,
    pub completion: u64,
    /// Prompt tokens served from the provider's cache (a subset of `prompt`).
    pub cached_prompt: u64,
}

/// Accumulates usage over a task.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageLedger {
    calls: u64,
    total: Usage,
}

impl UsageLedger {
    pub fn add(&mut self, u: Usage) {
        self.calls += 1;
        self.total.prompt += u.prompt;
        self.total.completion += u.completion;
        self.total.cached_prompt += u.cached_prompt.min(u.prompt);
    }

    pub fn calls(&self) -> u64 {
        self.calls
    }

    pub fn total(&self) -> Usage {
        self.total
    }

    /// Fraction of prompt tokens served from cache, `0.0` before any call.
    pub fn cache_hit_rate(&self) -> f64 {
        if self.total.prompt == 0 { 0.0 } else { self.total.cached_prompt as f64 / self.total.prompt as f64 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_rate_and_clamping() {
        let mut l = UsageLedger::default();
        assert!(l.cache_hit_rate().abs() < f64::EPSILON);
        l.add(Usage { prompt: 1000, completion: 50, cached_prompt: 0 });
        l.add(Usage { prompt: 1000, completion: 50, cached_prompt: 900 });
        l.add(Usage { prompt: 100, completion: 5, cached_prompt: 5000 });
        assert_eq!(l.calls(), 3);
        assert_eq!(l.total().cached_prompt, 1000, "cached tokens can never exceed the prompt");
        assert!((l.cache_hit_rate() - 1000.0 / 2100.0).abs() < 1e-12);
    }
}
