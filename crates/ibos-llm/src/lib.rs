//! Model gateway.
//!
//! One trait, [`Llm`], and three implementations: [`OpenAiCompat`] (OpenAI, Ollama, LM Studio, vLLM, ...),
//! [`Anthropic`] (with prompt-cache breakpoints from the context engine's layout) and [`ScriptedLlm`] for tests.
//!
//! Every request has a timeout. Transient failures (429, 5xx, timeouts, connection errors) are retried with exponential
//! backoff and `Retry-After` is honoured. API keys never appear in errors or logs.

mod anthropic;
mod openai;
mod retry;
mod scripted;

use async_trait::async_trait;
use ibos_ctx::Prompt;
use ibos_ctx::usage::Usage;

pub use anthropic::Anthropic;
pub use openai::OpenAiCompat;
pub use retry::RetryPolicy;
pub use scripted::{FnLlm, ScriptedLlm};

/// What to send.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub prompt: Prompt,
    pub max_tokens: u32,
    pub temperature: f32,
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatResponse {
    pub text: String,
    pub usage: Usage,
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("the endpoint returned HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("could not reach the endpoint: {0}")]
    Transport(String),
    #[error("unexpected response from the endpoint: {0}")]
    Format(String),
    #[error("no scripted reply left")]
    ScriptExhausted,
}

impl LlmError {
    /// Worth retrying: rate limits, server errors and network trouble.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Http { status, .. } => *status == 429 || *status >= 500,
            Self::Transport(_) => true,
            Self::Format(_) | Self::ScriptExhausted => false,
        }
    }
}

#[async_trait]
pub trait Llm: Send + Sync {
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, LlmError>;
}

/// Trim a response body for inclusion in an error message.
pub(crate) fn clip(body: &str) -> String {
    let mut s: String = body.chars().take(300).collect();
    if body.chars().count() > 300 {
        s.push_str("...");
    }
    s
}
