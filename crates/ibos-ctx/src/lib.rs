//! The context engine: decides what the model sees on each step.
//!
//! Goal: **the smallest prompt that still lets the model act correctly, and never the same bytes twice at full price.**
//!
//! * [`tokens`] - token counting (approximate by default, exact for `OpenAI` models behind a feature).
//! * [`observe`] - page snapshots and step-to-step deltas.
//! * [`prompt`] - cache-aware layout (stable prefix + task block + volatile tail) and provider renderers.
//! * [`governor`] - per-task token budget with a fixed degradation ladder.
//! * [`history`] - facts ledger, rolling summary and compaction.
//! * [`artifact`] - content-addressed store for large tool outputs, with recall by query.
//! * [`engine`] - [`ContextEngine`] ties it together.

pub mod artifact;
pub mod engine;
pub mod governor;
pub mod history;
pub mod observe;
pub mod prompt;
pub mod tokens;
pub mod usage;

pub use engine::{Built, ContextConfig, ContextEngine, StepInputs};
pub use governor::{Governor, Level};
pub use observe::{Elem, Observation, Page, Snapshot};
pub use prompt::Prompt;
pub use tokens::{ApproxTokenizer, Tokenizer};
