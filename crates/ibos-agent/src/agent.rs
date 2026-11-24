use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use ibos_cdp::{CdpError, Page, RawObservation};
use ibos_ctx::history::StepRecord;
use ibos_ctx::usage::{Usage, UsageLedger};
use ibos_ctx::{ApproxTokenizer, ContextConfig, ContextEngine, StepInputs, Tokenizer};
use ibos_dom::{DistillOptions, Distilled, distill};
use ibos_llm::{ChatRequest, Llm};
use ibos_mem::StepArena;
use serde_json::json;

use crate::action::{Action, Dir, parse_action};
use crate::guard::{Guard, GuardConfig, Verdict};
use crate::prompt::SYSTEM_PROMPT;

/// What a `more` action asks for.
#[derive(Debug, Clone, Copy)]
struct Expansion<'a> {
    marker: u32,
    query: Option<&'a str>,
    skip: usize,
    limit: usize,
    context: usize,
}

/// How many hidden items one `more` action lists by default, and at most.
const MORE_DEFAULT: usize = 25;
const MORE_MAX: usize = 200;

#[derive(Clone)]
pub struct AgentConfig {
    pub model: String,
    pub max_steps: u32,
    /// Token budget for the whole task (prompt + completion across all calls).
    pub token_budget: u64,
    pub max_output_tokens: u32,
    pub distill: DistillOptions,
    /// Pause after an action for the page to react.
    pub settle: Duration,
    /// Stop after this many replies in a row that are not valid actions.
    pub max_invalid_replies: u32,
    pub guard: GuardConfig,
    pub tokenizer: Option<Arc<dyn Tokenizer>>,
    /// Called after every step (for progress output).
    pub on_step: Option<StepCallback>,
}

/// A progress callback.
pub type StepCallback = Arc<dyn Fn(&StepLog) + Send + Sync>;

impl std::fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentConfig")
            .field("model", &self.model)
            .field("max_steps", &self.max_steps)
            .field("token_budget", &self.token_budget)
            .finish_non_exhaustive()
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            max_steps: 30,
            token_budget: 200_000,
            max_output_tokens: 256,
            distill: DistillOptions::default(),
            settle: Duration::from_millis(150),
            max_invalid_replies: 3,
            guard: GuardConfig::default(),
            tokenizer: None,
            on_step: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    Done,
    MaxSteps,
    Budget,
    TooManyInvalidReplies,
    ModelError(String),
    BrowserError(String),
}

#[derive(Debug, Clone)]
pub struct StepLog {
    pub n: u32,
    pub action: String,
    pub outcome: String,
    /// Estimated tokens of the whole prompt, of its stable prefix, and of the volatile tail.
    pub prompt_tokens: usize,
    pub prefix_tokens: usize,
    pub tail_tokens: usize,
    pub observation_was_delta: bool,
    pub usage: Usage,
}

#[derive(Debug, Clone)]
pub struct RunReport {
    pub success: bool,
    pub answer: String,
    pub stop: StopReason,
    pub steps: Vec<StepLog>,
    pub usage: UsageLedger,
    pub denied: usize,
}

pub struct Agent {
    page: Page,
    llm: Arc<dyn Llm>,
    guard: Guard,
    cfg: AgentConfig,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent").field("cfg", &self.cfg).finish_non_exhaustive()
    }
}

fn is_fatal(e: &CdpError) -> bool {
    matches!(e, CdpError::Closed | CdpError::Launch(_) | CdpError::Connect(_))
}

fn report(
    steps: Vec<StepLog>,
    usage: UsageLedger,
    guard: &Guard,
    stop: StopReason,
    answer: &str,
    success: bool,
) -> RunReport {
    RunReport { success, answer: answer.to_owned(), stop, steps, usage, denied: guard.denied() }
}

impl Agent {
    /// `guard` decides which actions need approval; with no one to ask use [`Guard::deny_risky`].
    pub fn new(page: Page, llm: Arc<dyn Llm>, cfg: AgentConfig, guard: Guard) -> Self {
        Self { page, llm, guard, cfg }
    }

    fn push_step(&self, steps: &mut Vec<StepLog>, log: StepLog) {
        if let Some(cb) = &self.cfg.on_step {
            cb(&log);
        }
        steps.push(log);
    }

    async fn settle(&self) {
        let _ = self.page.wait_ready(Duration::from_secs(3)).await;
        tokio::time::sleep(self.cfg.settle).await;
    }

    /// Run `task` to completion, a stop condition, or the step limit.
    pub async fn run(&mut self, task: &str) -> RunReport {
        let tok: Arc<dyn Tokenizer> = self.cfg.tokenizer.clone().unwrap_or_else(|| Arc::new(ApproxTokenizer));
        let mut ctx = ContextEngine::new(
            ContextConfig {
                system: SYSTEM_PROMPT.to_owned(),
                token_budget: self.cfg.token_budget,
                ..ContextConfig::default()
            },
            Arc::clone(&tok),
        );
        let mut arena = StepArena::with_capacity(64 * 1024);
        let mut shown: HashMap<u32, String> = HashMap::new();
        let mut last_url = String::new();
        let mut note: Option<String> = None;
        let (mut invalid, mut steps) = (0u32, Vec::new());
        // With site rules set, every request the page makes (images, scripts, fetch), not only the agent's own navigation, is checked.
        // If that cannot be switched on, stop rather than browse unprotected.
        if let Some(filter) = self.guard.request_filter()
            && let Err(e) = self.page.restrict_requests(filter).await
        {
            let why = StopReason::BrowserError(format!("could not restrict the page's requests: {e}"));
            return report(steps, ctx.usage().clone(), &self.guard, why, "", false);
        }

        for n in 1..=self.cfg.max_steps {
            arena.reset();
            let obs = match self.page.observe_raw().await {
                Ok(o) => o,
                Err(e) => {
                    return report(
                        steps,
                        ctx.usage().clone(),
                        &self.guard,
                        StopReason::BrowserError(e.to_string()),
                        "",
                        false,
                    );
                }
            };
            let d = match Self::distil(&obs, &arena, &self.cfg.distill) {
                Ok(d) => d,
                Err(e) => {
                    return report(steps, ctx.usage().clone(), &self.guard, StopReason::BrowserError(e), "", false);
                }
            };
            if obs.url != last_url {
                shown.clear();
                last_url.clone_from(&obs.url);
            }
            shown.extend(d.snapshot.elems.iter().map(|(h, e)| (h, format!("{} {}", e.role, e.text))));

            let built = ctx.next(StepInputs { task, snapshot: d.snapshot, note: note.take() });
            if built.plan.stop {
                return report(steps, ctx.usage().clone(), &self.guard, StopReason::Budget, "", false);
            }
            let req = ChatRequest {
                model: self.cfg.model.clone(),
                prompt: built.prompt.clone(),
                max_tokens: self.cfg.max_output_tokens,
                temperature: 0.0,
            };
            let resp = match self.llm.chat(&req).await {
                Ok(r) => r,
                Err(e) => {
                    return report(
                        steps,
                        ctx.usage().clone(),
                        &self.guard,
                        StopReason::ModelError(e.to_string()),
                        "",
                        false,
                    );
                }
            };
            ctx.record_usage(resp.usage);
            let mut log = StepLog {
                n,
                action: String::new(),
                outcome: String::new(),
                prompt_tokens: built.total_tokens(),
                prefix_tokens: built.prefix_tokens,
                tail_tokens: built.tail_tokens,
                observation_was_delta: built.observation_was_delta,
                usage: resp.usage,
            };

            let action = match parse_action(&resp.text) {
                Ok(a) => {
                    invalid = 0;
                    a
                }
                Err(e) => {
                    invalid += 1;
                    log.action = "(invalid reply)".into();
                    log.outcome = e;
                    ctx.record_step(StepRecord { n, action: log.action.clone(), outcome: log.outcome.clone() });
                    self.push_step(&mut steps, log);
                    if invalid >= self.cfg.max_invalid_replies {
                        return report(
                            steps,
                            ctx.usage().clone(),
                            &self.guard,
                            StopReason::TooManyInvalidReplies,
                            "",
                            false,
                        );
                    }
                    continue;
                }
            };
            log.action = action.describe();

            if let Action::Done { t, ok } = &action {
                self.guard.audit(n, &json!({"action": log.action}));
                self.push_step(&mut steps, log);
                return report(steps, ctx.usage().clone(), &self.guard, StopReason::Done, t, *ok);
            }

            // Name the target from what was shown; if it was not shown (revealed through `more`, say), look it up in the
            // full page. A handle that exists nowhere stays `None`, and the guard refuses it.
            let element: Option<String> = match &action {
                Action::Click { h } | Action::Type { h, .. } => {
                    shown.get(h).cloned().or_else(|| Self::lookup_full(&obs, *h, &arena))
                }
                _ => None,
            };
            let verdict = self.guard.check_action(&action, element.as_deref());
            let outcome = match verdict {
                Verdict::Deny(reason) => {
                    self.guard.audit(n, &json!({"action": log.action, "decision": "denied", "reason": reason}));
                    reason
                }
                Verdict::Allow { approved } => {
                    let outcome = self.execute(&action, &obs, &mut note, &arena).await;
                    self.guard.audit(n, &json!({"action": log.action, "decision": if approved { "approved" } else { "allowed" }, "outcome": outcome.as_ref().map_or_else(ToString::to_string, Clone::clone)}));
                    match outcome {
                        Ok(o) => o,
                        Err(e) if is_fatal(&e) => {
                            self.push_step(&mut steps, log);
                            return report(
                                steps,
                                ctx.usage().clone(),
                                &self.guard,
                                StopReason::BrowserError(e.to_string()),
                                "",
                                false,
                            );
                        }
                        Err(e) => format!("error: {e}"),
                    }
                }
            };
            log.outcome = outcome.chars().take(300).collect();
            ctx.record_step(StepRecord { n, action: log.action.clone(), outcome: log.outcome.clone() });
            self.push_step(&mut steps, log);
        }
        report(steps, ctx.usage().clone(), &self.guard, StopReason::MaxSteps, "", false)
    }

    /// "role text" of element `h` in the full, uncollapsed page, if it exists.
    fn lookup_full(obs: &RawObservation, h: u32, arena: &StepArena) -> Option<String> {
        let opts = DistillOptions {
            keep_repeat: usize::MAX,
            include_offscreen: true,
            max_elements: usize::MAX,
            ..DistillOptions::default()
        };
        let full = Self::distil(obs, arena, &opts).ok()?;
        full.snapshot.elems.iter().find(|(eh, _)| *eh == h).map(|(_, e)| format!("{} {}", e.role, e.text))
    }

    fn distil(obs: &RawObservation, arena: &StepArena, opts: &DistillOptions) -> Result<Distilled, String> {
        distill(
            &obs.url,
            &obs.title,
            obs.ax.get(),
            Some(obs.dom.get()),
            (obs.viewport.width, obs.viewport.height),
            opts,
            arena,
        )
        .map_err(|e| e.to_string())
    }

    /// After any action that can navigate, make sure the browser is still on an allowed site; if not, reset it to a blank page.
    async fn enforce_origin(&self, outcome: &str) -> String {
        let now = self.page.url().await.unwrap_or_default();
        if !now.is_empty()
            && let Err(reason) = self.guard.check_navigation(&now)
        {
            let _ = self.page.navigate("about:blank").await;
            return format!(
                "Blocked by guardrail: the page moved outside the allowed sites ({reason}); the browser was reset to a blank page."
            );
        }
        outcome.to_owned()
    }

    async fn execute(
        &self,
        action: &Action,
        obs: &RawObservation,
        note: &mut Option<String>,
        arena: &StepArena,
    ) -> Result<String, CdpError> {
        match action {
            Action::Go { url } => {
                self.page.navigate(url).await?;
                // a redirect can leave the allowed domains: check where we actually are
                let now = self.page.url().await.unwrap_or_default();
                if let Err(reason) = self.guard.check_navigation(&now) {
                    let _ = self.page.navigate("about:blank").await;
                    return Ok(format!(
                        "Blocked by guardrail: the page redirected outside the allowed sites ({reason})."
                    ));
                }
                Ok(format!("opened {}", self.page.title().await.unwrap_or_default()))
            }
            Action::Click { h } => {
                self.page.click(i64::from(*h)).await?;
                self.settle().await;
                Ok(self.enforce_origin("clicked").await)
            }
            Action::Type { h, t, clear } => {
                self.page.type_text(i64::from(*h), t, *clear).await?;
                Ok(format!("typed {} chars", t.chars().count()))
            }
            Action::Scroll { d } => {
                let vh = self.page.viewport().await?.height;
                let step = (vh * 0.8) as i64;
                self.page.scroll(if *d == Dir::Down { step } else { -step }).await?;
                Ok("scrolled".into())
            }
            Action::Back => {
                self.page.evaluate("history.back()").await?;
                self.settle().await;
                Ok(self.enforce_origin("went back").await)
            }
            Action::More { h, q, skip, n, ctx } => {
                let (listed, count) = self.expand(
                    obs,
                    &Expansion {
                        marker: *h,
                        query: q.as_deref(),
                        skip: *skip,
                        limit: n.unwrap_or(MORE_DEFAULT).clamp(1, MORE_MAX),
                        context: ctx.unwrap_or(3).min(10),
                    },
                    arena,
                );
                *note = Some(format!("hidden items for [{h}]:\n{listed}"));
                Ok(format!("listed {count} hidden items; they are shown below the page"))
            }
            Action::Done { .. } => Ok(String::new()),
        }
    }

    /// The items hidden behind marker `marker`, in document order: optionally only those containing `query`, after skipping
    /// `skip`, at most `limit`. Returns the text and how many items it lists.
    fn expand(&self, raw: &RawObservation, req: &Expansion<'_>, arena: &StepArena) -> (String, usize) {
        let Expansion { marker, query, skip, limit, context } = *req;
        let opts = DistillOptions {
            keep_repeat: usize::MAX,
            include_offscreen: true,
            max_elements: usize::MAX,
            ..self.cfg.distill.clone()
        };
        let Ok(full) = Self::distil(raw, arena, &opts) else { return (String::new(), 0) };
        let query = query.map(str::to_lowercase);
        let after: Vec<(u32, &ibos_ctx::Elem)> = full.snapshot.elems.iter().skip_while(|(h, _)| *h != marker).collect();
        let matching: Vec<(u32, &ibos_ctx::Elem)> = match &query {
            None => after,
            Some(q) => {
                // each match comes with the elements that follow it (the rest of its row or card), without repeating any
                let mut keep = vec![false; after.len()];
                for (i, (_, e)) in after.iter().enumerate() {
                    if e.text.to_lowercase().contains(q.as_str()) {
                        keep[i..=(i + context).min(after.len() - 1)].fill(true);
                    }
                }
                after.into_iter().zip(keep).filter_map(|(item, k)| k.then_some(item)).collect()
            }
        };
        let total = matching.len();
        let mut out = String::new();
        let shown: Vec<_> = matching.into_iter().skip(skip).take(limit).collect();
        for (h, e) in &shown {
            let _ = writeln!(out, "[{h}]<{}> {}", e.role, e.text);
        }
        let end = skip + shown.len();
        if shown.is_empty() {
            out.push_str("(nothing matches)");
        } else if end < total {
            let _ = write!(out, "(showing {}-{end} of {total}; use \"skip\":{end} for the next ones)", skip + 1);
        }
        (out.trim_end().to_owned(), shown.len())
    }
}
