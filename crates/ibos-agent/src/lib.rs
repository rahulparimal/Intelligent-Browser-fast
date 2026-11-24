//! The browser agent.
//!
//! One step: observe the page, distil it, let the context engine build the smallest useful prompt, ask the model for one
//! action, check it against the guardrails, perform it. Large tool results never bloat the prompt: expansions arrive as a
//! one-off note, history is compacted, and the whole task runs under a token budget.

mod action;
mod agent;
mod guard;
mod prompt;

pub use action::{Action, parse_action};
pub use agent::{Agent, AgentConfig, RunReport, StepCallback, StepLog, StopReason};
pub use guard::{ConfirmMode, Guard, GuardConfig, Verdict, has_card_number, host_allowed};
pub use prompt::SYSTEM_PROMPT;
