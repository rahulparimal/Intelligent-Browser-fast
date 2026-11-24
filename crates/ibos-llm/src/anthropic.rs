use std::time::Duration;

use async_trait::async_trait;
use ibos_ctx::usage::Usage;
use serde_json::{Value, json};

use crate::openai::redact;
use crate::retry::RetryPolicy;
use crate::{ChatRequest, ChatResponse, Llm, LlmError, clip};

const VERSION: &str = "2023-06-01";

/// The Anthropic Messages API. The prompt's stable prefix and task block carry `cache_control` breakpoints, so later steps
/// read them from the provider's cache.
pub struct Anthropic {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    retry: RetryPolicy,
}

impl std::fmt::Debug for Anthropic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Anthropic")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl Anthropic {
    /// # Errors
    /// If the HTTP client cannot be constructed.
    pub fn new(api_key: &str, timeout: Duration) -> Result<Self, LlmError> {
        Self::with_base_url("https://api.anthropic.com", api_key, timeout)
    }

    /// # Errors
    /// If the HTTP client cannot be constructed.
    pub fn with_base_url(base_url: &str, api_key: &str, timeout: Duration) -> Result<Self, LlmError> {
        let http =
            reqwest::Client::builder().timeout(timeout).build().map_err(|e| LlmError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.to_owned(),
            retry: RetryPolicy::default(),
        })
    }

    #[must_use]
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    pub(crate) fn body(req: &ChatRequest) -> Value {
        let (system, messages) = req.prompt.to_anthropic();
        json!({"model": req.model, "max_tokens": req.max_tokens, "temperature": req.temperature, "system": system, "messages": messages})
    }

    pub(crate) fn parse(v: &Value) -> Result<ChatResponse, LlmError> {
        let text: String = v["content"]
            .as_array()
            .ok_or_else(|| LlmError::Format("missing content".into()))?
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect();
        let u = &v["usage"];
        let (fresh, read, created) = (
            u["input_tokens"].as_u64().unwrap_or(0),
            u["cache_read_input_tokens"].as_u64().unwrap_or(0),
            u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
        );
        // Anthropic reports uncached, cache-read and cache-write input separately; the prompt is all three.
        Ok(ChatResponse {
            text,
            usage: Usage {
                prompt: fresh + read + created,
                completion: u["output_tokens"].as_u64().unwrap_or(0),
                cached_prompt: read,
            },
        })
    }
}

#[async_trait]
impl Llm for Anthropic {
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let body = Self::body(req);
        self.retry
            .run(|| async {
                let resp = self
                    .http
                    .post(format!("{}/v1/messages", self.base_url))
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", VERSION)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| (LlmError::Transport(redact(&e.to_string(), Some(&self.api_key))), None))?;
                let status = resp.status();
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(Duration::from_secs);
                let text = resp.text().await.map_err(|e| (LlmError::Transport(e.to_string()), None))?;
                if !status.is_success() {
                    return Err((
                        LlmError::Http { status: status.as_u16(), body: clip(&redact(&text, Some(&self.api_key))) },
                        retry_after,
                    ));
                }
                let v: Value = serde_json::from_str(&text).map_err(|e| (LlmError::Format(e.to_string()), None))?;
                Self::parse(&v).map_err(|e| (e, None))
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use ibos_ctx::prompt::{Parts, assemble};

    use super::*;

    fn req() -> ChatRequest {
        let prompt = assemble(&Parts {
            system: "SYS",
            tools: "T",
            skills: "",
            task: "t",
            ledger: "",
            history: "h",
            observation: "o",
        });
        ChatRequest { model: "m".into(), prompt, max_tokens: 100, temperature: 0.0 }
    }

    #[test]
    fn request_carries_cache_breakpoints() {
        let b = Anthropic::body(&req());
        assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(b["messages"][0]["content"][0]["cache_control"]["type"], "ephemeral");
        assert!(b["messages"][0]["content"][1].get("cache_control").is_none());
        assert_eq!(b["max_tokens"], 100);
    }

    #[test]
    fn usage_sums_fresh_read_and_written_input() {
        let v = json!({"content": [{"type": "text", "text": "he"}, {"type": "text", "text": "llo"}], "usage": {"input_tokens": 50, "cache_read_input_tokens": 900, "cache_creation_input_tokens": 100, "output_tokens": 7}});
        let r = Anthropic::parse(&v).unwrap();
        assert_eq!((r.text.as_str(), r.usage), ("hello", Usage { prompt: 1050, completion: 7, cached_prompt: 900 }));
        assert!(Anthropic::parse(&json!({})).is_err());
    }
}
