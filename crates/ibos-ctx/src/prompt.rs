//! Cache-aware prompt layout.
//!
//! A prompt has three tiers, ordered from least to most volatile:
//!
//! 1. **prefix**: system prompt, tool schema, skills. Byte-identical across every step of every task.
//! 2. **task block**: the goal and facts ledger. Identical across the steps of one task.
//! 3. **tail**: history window and the current observation. Changes every step.
//!
//! Providers cache by exact prefix, so anything volatile must never leak into an earlier tier.

use serde_json::{Value, json};

/// An assembled prompt, split at the cache boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub prefix: String,
    pub task_block: String,
    pub tail: String,
}

impl Prompt {
    /// Byte offset at which a provider cache breakpoint belongs (end of the stable prefix).
    pub fn prefix_len(&self) -> usize {
        self.prefix.len()
    }

    /// Everything, in order. Useful for token counting and logging.
    pub fn flatten(&self) -> String {
        format!("{}\n{}\n{}", self.prefix, self.task_block, self.tail)
    }

    /// OpenAI-compatible chat messages. Providers with automatic prefix caching reuse the longest identical prefix, so the
    /// system message (prefix) comes first and nothing volatile precedes the user turn.
    pub fn to_openai(&self) -> Vec<Value> {
        let user = [self.task_block.as_str(), self.tail.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("\n\n");
        vec![json!({"role": "system", "content": self.prefix}), json!({"role": "user", "content": user})]
    }

    /// Anthropic Messages API payload parts: `(system, messages)` with `cache_control` breakpoints after the prefix and after
    /// the task block, so both tiers are served from cache on later steps.
    pub fn to_anthropic(&self) -> (Value, Vec<Value>) {
        let system = json!([{"type": "text", "text": self.prefix, "cache_control": {"type": "ephemeral"}}]);
        let user = json!({
            "role": "user",
            "content": [
                {"type": "text", "text": self.task_block, "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": self.tail},
            ],
        });
        (system, vec![user])
    }
}

/// Inputs for [`assemble`].
#[derive(Debug, Clone, Copy)]
pub struct Parts<'a> {
    pub system: &'a str,
    pub tools: &'a str,
    pub skills: &'a str,
    pub task: &'a str,
    pub ledger: &'a str,
    pub history: &'a str,
    pub observation: &'a str,
}

/// Build a [`Prompt`]. The prefix depends only on `system`, `tools` and `skills`.
pub fn assemble(p: &Parts<'_>) -> Prompt {
    let prefix =
        [p.system, p.tools, p.skills].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("\n\n");
    let task_block = format!("task: {}\n{}", p.task, p.ledger).trim_end().to_owned();
    let tail = [p.history, p.observation].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("\n");
    Prompt { prefix, task_block, tail }
}

/// Cost over `steps` calls in full-price token equivalents, when cached prefix reads cost `read_ratio` of normal.
///
/// With caching, the first call pays full price for the prefix and every later call pays `read_ratio`. The tail is always
/// full price. `read_ratio` is provider specific; measure it rather than assuming.
pub fn effective_tokens(prefix: u64, tail_per_step: u64, steps: u64, read_ratio: f64, cached: bool) -> f64 {
    if steps == 0 {
        return 0.0;
    }
    if !cached {
        return (prefix + tail_per_step) as f64 * steps as f64;
    }
    prefix as f64 * (1.0 + steps.saturating_sub(1) as f64 * read_ratio) + (tail_per_step * steps) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts<'a>(history: &'a str, obs: &'a str) -> Parts<'a> {
        Parts {
            system: "SYS",
            tools: "TOOLS",
            skills: "",
            task: "log in",
            ledger: "facts: none",
            history,
            observation: obs,
        }
    }

    #[test]
    fn prefix_and_task_block_are_byte_stable_across_steps() {
        let a = assemble(&parts("step 1", "page A"));
        let b = assemble(&parts("step 1; step 2", "page B, much longer than A"));
        assert_eq!(a.prefix, b.prefix);
        assert_eq!(a.task_block, b.task_block);
        assert_ne!(a.tail, b.tail);
        assert_eq!(a.prefix_len(), b.prefix_len());
    }

    #[test]
    fn nothing_volatile_leaks_into_the_prefix() {
        let p = assemble(&parts("HISTORY-MARKER", "OBS-MARKER"));
        assert!(!p.prefix.contains("HISTORY-MARKER") && !p.prefix.contains("OBS-MARKER"));
        assert!(!p.task_block.contains("OBS-MARKER"));
        assert!(p.flatten().contains("OBS-MARKER"));
    }

    #[test]
    fn openai_shape_puts_prefix_first() {
        let m = assemble(&parts("h", "o")).to_openai();
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[0]["content"], "SYS\n\nTOOLS");
        assert_eq!(m[1]["role"], "user");
        assert!(m[1]["content"].as_str().unwrap().ends_with("h\no"));
    }

    #[test]
    fn anthropic_shape_has_two_cache_breakpoints_and_an_uncached_tail() {
        let (system, msgs) = assemble(&parts("h", "o")).to_anthropic();
        assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
        let content = &msgs[0]["content"];
        assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
        assert!(content[1].get("cache_control").is_none(), "the volatile tail must not be a cache breakpoint");
    }

    #[test]
    fn caching_arithmetic_matches_the_design_document() {
        // Measured baseline prefix ~6,000 tokens; illustrative 1,500-token tail, 10 steps, cached reads at 10%.
        let plain = effective_tokens(6000, 1500, 10, 0.1, false);
        let cached = effective_tokens(6000, 1500, 10, 0.1, true);
        assert!((plain - 75_000.0).abs() < 1e-9 && (cached - 26_400.0).abs() < 1e-9);
        assert!((effective_tokens(6000, 1500, 1, 0.1, true) - 7500.0).abs() < 1e-9, "one step: caching cannot help");
        assert!(effective_tokens(1, 1, 0, 0.1, true).abs() < 1e-9);
    }
}
