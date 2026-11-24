use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use ibos_ctx::usage::Usage;

use crate::{ChatRequest, ChatResponse, Llm, LlmError};

/// Replies from a fixed script and records every request. For tests and offline demos: no network, no key.
#[derive(Debug, Default)]
pub struct ScriptedLlm {
    replies: Mutex<VecDeque<String>>,
    seen: Mutex<Vec<ChatRequest>>,
}

impl ScriptedLlm {
    pub fn new<I, S>(replies: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self { replies: Mutex::new(replies.into_iter().map(Into::into).collect()), seen: Mutex::new(Vec::new()) }
    }

    /// Every request received so far.
    pub fn requests(&self) -> Vec<ChatRequest> {
        self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl Llm for ScriptedLlm {
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, LlmError> {
        self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(req.clone());
        let text = self
            .replies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .ok_or(LlmError::ScriptExhausted)?;
        let prompt = req.prompt.flatten().chars().count().div_ceil(4) as u64;
        Ok(ChatResponse {
            usage: Usage { prompt, completion: text.chars().count().div_ceil(4) as u64, cached_prompt: 0 },
            text,
        })
    }
}

/// Replies computed from the request, for tests where the right answer depends on what the model was shown (for example an
/// element handle that is only known at run time). The closure gets the request and the zero-based call number.
pub struct FnLlm<F> {
    reply: F,
    calls: std::sync::atomic::AtomicUsize,
    seen: Mutex<Vec<ChatRequest>>,
}

impl<F> std::fmt::Debug for FnLlm<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FnLlm").field("calls", &self.calls).finish_non_exhaustive()
    }
}

impl<F: Fn(&ChatRequest, usize) -> String + Send + Sync> FnLlm<F> {
    pub fn new(reply: F) -> Self {
        Self { reply, calls: std::sync::atomic::AtomicUsize::new(0), seen: Mutex::new(Vec::new()) }
    }

    /// Every request received so far.
    pub fn requests(&self) -> Vec<ChatRequest> {
        self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl<F: Fn(&ChatRequest, usize) -> String + Send + Sync> Llm for FnLlm<F> {
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(req.clone());
        let text = (self.reply)(req, n);
        let prompt = req.prompt.flatten().chars().count().div_ceil(4) as u64;
        Ok(ChatResponse {
            usage: Usage { prompt, completion: text.chars().count().div_ceil(4) as u64, cached_prompt: 0 },
            text,
        })
    }
}
