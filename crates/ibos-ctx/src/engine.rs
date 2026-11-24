//! [`ContextEngine`]: per-task orchestration of observation diffing, history, budgets and prompt assembly.

use std::sync::Arc;

use crate::artifact::ArtifactStore;
use crate::governor::{Governor, Level, Plan};
use crate::history::{History, StepRecord};
use crate::observe::{Observation, Snapshot, observe, render_full};
use crate::prompt::{Parts, Prompt, assemble};
use crate::tokens::{Tokenizer, truncate_to_tokens};
use crate::usage::{Usage, UsageLedger};

/// Static configuration for one task.
#[derive(Debug, Clone)]
pub struct ContextConfig {
    pub system: String,
    pub tools: String,
    pub skills: String,
    /// Total token budget for the task (prompt + completion across all calls).
    pub token_budget: u64,
    /// Raw history steps kept in the prompt at full level.
    pub raw_history_steps: usize,
    /// Send only what changed since the previous step.
    ///
    /// **Only valid when the model keeps the earlier page in context** (an append-only conversation). Each prompt built here is
    /// stateless, so a delta would reference elements the model can no longer see. The default is `false`: the full distilled
    /// page every step, plus a one-line hint when nothing changed.
    pub deltas: bool,
    /// With `deltas`, force a full page observation every N steps.
    pub full_refresh_every: u32,
    /// Cap on the observation section.
    pub max_observation_tokens: usize,
    /// Cap on the rolling summary.
    pub max_summary_tokens: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            system: String::new(),
            tools: String::new(),
            skills: String::new(),
            token_budget: 200_000,
            raw_history_steps: 4,
            deltas: false,
            full_refresh_every: 8,
            max_observation_tokens: 3000,
            max_summary_tokens: 400,
        }
    }
}

/// Per-step input.
#[derive(Debug, Clone)]
pub struct StepInputs<'a> {
    pub task: &'a str,
    pub snapshot: Snapshot,
    /// One-off extra context for this step only (for example the result of expanding a collapsed list). It is shown after the
    /// observation, counted against the observation cap, and not remembered.
    pub note: Option<String>,
}

/// A prompt ready to send, with the numbers that explain it.
#[derive(Debug, Clone)]
pub struct Built {
    pub prompt: Prompt,
    pub plan: Plan,
    pub observation_was_delta: bool,
    pub prefix_tokens: usize,
    pub task_tokens: usize,
    pub tail_tokens: usize,
}

impl Built {
    pub fn total_tokens(&self) -> usize {
        self.prefix_tokens + self.task_tokens + self.tail_tokens
    }
}

/// Owns all per-task context state.
pub struct ContextEngine {
    cfg: ContextConfig,
    tok: Arc<dyn Tokenizer>,
    history: History,
    governor: Governor,
    prev: Option<Snapshot>,
    steps_since_full: u32,
    usage: UsageLedger,
    artifacts: ArtifactStore,
}

impl std::fmt::Debug for ContextEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextEngine")
            .field("tokenizer", &self.tok.name())
            .field("level", &self.governor.level())
            .field("history_len", &self.history.len())
            .finish_non_exhaustive()
    }
}

impl ContextEngine {
    pub fn new(cfg: ContextConfig, tok: Arc<dyn Tokenizer>) -> Self {
        let governor = Governor::new(cfg.token_budget, cfg.raw_history_steps);
        let history = History::new(cfg.raw_history_steps.max(1) * 8);
        Self {
            cfg,
            tok,
            history,
            governor,
            prev: None,
            steps_since_full: 0,
            usage: UsageLedger::default(),
            artifacts: ArtifactStore::new(),
        }
    }

    /// Build the prompt for the next model call. The snapshot becomes the baseline for the next delta.
    pub fn next(&mut self, input: StepInputs<'_>) -> Built {
        let plan = self.governor.plan();
        let obs = if self.cfg.deltas {
            let force_full = self.steps_since_full >= self.cfg.full_refresh_every;
            if force_full {
                Observation::Full(render_full(&input.snapshot))
            } else {
                observe(self.prev.as_ref(), &input.snapshot)
            }
        } else {
            let unchanged = self.prev.as_ref().is_some_and(|p| {
                p.url == input.snapshot.url
                    && p.elems == input.snapshot.elems
                    && p.more_below == input.snapshot.more_below
            });
            let full = render_full(&input.snapshot);
            Observation::Full(if unchanged { format!("page unchanged since your last action\n{full}") } else { full })
        };
        self.steps_since_full = if obs.is_delta() { self.steps_since_full + 1 } else { 0 };
        let mut observation = obs.text().to_owned();
        if let Some(note) = input.note.as_deref().filter(|n| !n.is_empty()) {
            observation.push('\n');
            observation.push_str(note);
        }
        let observation = truncate_to_tokens(&observation, self.cfg.max_observation_tokens, self.tok.as_ref());
        let history = self.history.render(
            plan.raw_history_steps,
            plan.include_summary,
            self.cfg.max_summary_tokens,
            self.tok.as_ref(),
        );
        let ledger = self.history.ledger.render();
        let prompt = assemble(&Parts {
            system: &self.cfg.system,
            tools: &self.cfg.tools,
            skills: &self.cfg.skills,
            task: input.task,
            ledger: &ledger,
            history: &history,
            observation: &observation,
        });
        self.prev = Some(input.snapshot);
        Built {
            plan,
            observation_was_delta: obs.is_delta(),
            prefix_tokens: self.tok.count(&prompt.prefix),
            task_tokens: self.tok.count(&prompt.task_block),
            tail_tokens: self.tok.count(&prompt.tail),
            prompt,
        }
    }

    pub fn record_step(&mut self, step: StepRecord) {
        self.history.push(step);
    }

    pub fn ledger_mut(&mut self) -> &mut crate::history::FactsLedger {
        &mut self.history.ledger
    }

    /// Record provider-reported usage; it counts against the task budget.
    pub fn record_usage(&mut self, u: Usage) {
        self.usage.add(u);
        self.governor.record(u.prompt + u.completion);
    }

    pub fn level(&self) -> Level {
        self.governor.level()
    }

    pub fn usage(&self) -> &UsageLedger {
        &self.usage
    }

    pub fn artifacts_mut(&mut self) -> &mut ArtifactStore {
        &mut self.artifacts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::Elem;
    use crate::tokens::ApproxTokenizer;

    fn snap(url: &str, n: u32, extra: Option<&str>) -> Snapshot {
        let mut s = Snapshot { url: url.into(), title: "T".into(), ..Snapshot::default() };
        for i in 1..=n {
            s.elems.insert(i, Elem { role: "a".into(), text: format!("link number {i} to some destination page") });
        }
        if let Some(x) = extra {
            s.elems.insert(n + 1, Elem { role: "div".into(), text: x.into() });
        }
        s
    }

    fn engine(budget: u64) -> ContextEngine {
        ContextEngine::new(
            ContextConfig {
                system: "SYSTEM ".repeat(200),
                tools: "TOOLS".into(),
                token_budget: budget,
                ..ContextConfig::default()
            },
            Arc::new(ApproxTokenizer),
        )
    }

    #[test]
    fn full_then_deltas_then_full_after_navigation_and_periodic_refresh() {
        let mut e = ContextEngine::new(
            ContextConfig { deltas: true, full_refresh_every: 3, ..ContextConfig::default() },
            Arc::new(ApproxTokenizer),
        );
        let kinds: Vec<bool> = [
            snap("u1", 30, None),
            snap("u1", 30, Some("Welcome")),
            snap("u1", 30, Some("Welcome")),
            snap("u1", 30, Some("Welcome")),
            snap("u1", 30, Some("Welcome")),
            snap("u2", 5, None),
        ]
        .into_iter()
        .map(|s| e.next(StepInputs { task: "t", snapshot: s, note: None }).observation_was_delta)
        .collect();
        // full, delta, delta, delta, then forced full (3 deltas since the last full), then full (navigation)
        assert_eq!(kinds, [false, true, true, true, false, false]);
    }

    #[test]
    fn prefix_and_task_block_never_change_and_tail_shrinks_with_deltas() {
        let mut e = ContextEngine::new(
            ContextConfig {
                system: "SYSTEM ".repeat(200),
                tools: "TOOLS".into(),
                token_budget: 1_000_000,
                deltas: true,
                ..ContextConfig::default()
            },
            Arc::new(ApproxTokenizer),
        );
        let a = e.next(StepInputs { task: "log in", snapshot: snap("u", 60, None), note: None });
        e.record_step(StepRecord { n: 1, action: "click 1".into(), outcome: "ok".into() });
        let b = e.next(StepInputs { task: "log in", snapshot: snap("u", 60, Some("Welcome")), note: None });
        assert_eq!(a.prompt.prefix, b.prompt.prefix);
        assert_eq!(a.prompt.task_block, b.prompt.task_block);
        assert!(
            b.observation_was_delta && b.tail_tokens < a.tail_tokens / 4,
            "delta tail {} vs full {}",
            b.tail_tokens,
            a.tail_tokens
        );
        assert!(a.prefix_tokens > 300 && a.total_tokens() == a.prefix_tokens + a.task_tokens + a.tail_tokens);
    }

    #[test]
    fn default_mode_sends_the_full_page_every_step_with_an_unchanged_hint() {
        let mut e = engine(1_000_000);
        let a = e.next(StepInputs { task: "t", snapshot: snap("u", 20, None), note: None });
        let b = e.next(StepInputs { task: "t", snapshot: snap("u", 20, None), note: None });
        let c = e.next(StepInputs { task: "t", snapshot: snap("u", 20, Some("Welcome")), note: None });
        assert!(
            !a.observation_was_delta && !b.observation_was_delta && !c.observation_was_delta,
            "a stateless prompt never gets a delta"
        );
        assert!(!a.prompt.tail.contains("unchanged"));
        assert!(
            b.prompt.tail.contains("page unchanged since your last action") && b.prompt.tail.contains("link number 20"),
            "the hint comes with the whole page"
        );
        assert!(
            c.prompt.tail.contains("Welcome")
                && c.prompt.tail.contains("link number 1 ")
                && !c.prompt.tail.contains("unchanged")
        );
    }

    #[test]
    fn budget_pressure_degrades_the_plan_and_finally_stops() {
        let mut e = engine(1000);
        assert_eq!(e.next(StepInputs { task: "t", snapshot: snap("u", 3, None), note: None }).plan.level, Level::Full);
        e.record_usage(Usage { prompt: 500, completion: 10, cached_prompt: 0 });
        assert_eq!(e.level(), Level::NoScreenshots);
        e.record_usage(Usage { prompt: 200, completion: 10, cached_prompt: 100 });
        assert_eq!(
            e.next(StepInputs { task: "t", snapshot: snap("u", 3, None), note: None }).plan.level,
            Level::CompactHistory
        );
        e.record_usage(Usage { prompt: 400, completion: 0, cached_prompt: 0 });
        assert!(e.next(StepInputs { task: "t", snapshot: snap("u", 3, None), note: None }).plan.stop);
        assert!(e.usage().cache_hit_rate() > 0.0 && e.usage().calls() == 3);
    }

    #[test]
    fn observation_is_capped_by_tokens() {
        let mut e = ContextEngine::new(
            ContextConfig { max_observation_tokens: 50, ..ContextConfig::default() },
            Arc::new(ApproxTokenizer),
        );
        let b = e.next(StepInputs { task: "t", snapshot: snap("u", 500, None), note: None });
        assert!(b.tail_tokens < 120 && b.prompt.tail.contains("tokens omitted"));
    }

    #[test]
    fn a_note_is_shown_once_and_not_remembered() {
        let mut e = engine(1_000_000);
        let with =
            e.next(StepInputs { task: "t", snapshot: snap("u", 3, None), note: Some("EXPANDED: items 7-9".into()) });
        assert!(with.prompt.tail.contains("EXPANDED: items 7-9"));
        let without = e.next(StepInputs { task: "t", snapshot: snap("u", 3, None), note: None });
        assert!(
            !without.prompt.tail.contains("EXPANDED")
                && !without.prompt.task_block.contains("EXPANDED")
                && !without.prompt.prefix.contains("EXPANDED")
        );
    }

    #[test]
    fn debug_does_not_dump_state() {
        assert!(format!("{:?}", engine(10)).contains("ContextEngine"));
    }
}
