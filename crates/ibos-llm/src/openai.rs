use std::time::Duration;

use async_trait::async_trait;
use ibos_ctx::usage::Usage;
use serde_json::{Value, json};

use crate::retry::RetryPolicy;
use crate::{ChatRequest, ChatResponse, Llm, LlmError, clip};

/// Any OpenAI-compatible `/chat/completions` endpoint.
pub struct OpenAiCompat {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    retry: RetryPolicy,
}

impl std::fmt::Debug for OpenAiCompat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompat")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

impl OpenAiCompat {
    /// # Errors
    /// If the HTTP client cannot be constructed.
    pub fn new(base_url: &str, api_key: Option<String>, timeout: Duration) -> Result<Self, LlmError> {
        let http =
            reqwest::Client::builder().timeout(timeout).build().map_err(|e| LlmError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.filter(|k| !k.is_empty()),
            retry: RetryPolicy::default(),
        })
    }

    #[must_use]
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    pub(crate) fn body(req: &ChatRequest) -> Value {
        json!({"model": req.model, "messages": req.prompt.to_openai(), "max_tokens": req.max_tokens, "temperature": req.temperature})
    }

    pub(crate) fn parse(v: &Value) -> Result<ChatResponse, LlmError> {
        let text = v["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| LlmError::Format("missing choices[0].message.content".into()))?;
        let u = &v["usage"];
        Ok(ChatResponse {
            text: text.to_owned(),
            usage: Usage {
                prompt: u["prompt_tokens"].as_u64().unwrap_or(0),
                completion: u["completion_tokens"].as_u64().unwrap_or(0),
                cached_prompt: u["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0),
            },
        })
    }
}

#[async_trait]
impl Llm for OpenAiCompat {
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let body = Self::body(req);
        self.retry
            .run(|| async {
                let mut r = self.http.post(format!("{}/chat/completions", self.base_url)).json(&body);
                if let Some(k) = &self.api_key {
                    r = r.bearer_auth(k);
                }
                let resp = r
                    .send()
                    .await
                    .map_err(|e| (LlmError::Transport(redact(&e.to_string(), self.api_key.as_deref())), None))?;
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
                        LlmError::Http { status: status.as_u16(), body: clip(&redact(&text, self.api_key.as_deref())) },
                        retry_after,
                    ));
                }
                let v: Value = serde_json::from_str(&text).map_err(|e| (LlmError::Format(e.to_string()), None))?;
                Self::parse(&v).map_err(|e| (e, None))
            })
            .await
    }
}

/// Remove the API key from text that may be shown to the user or logged.
pub(crate) fn redact(text: &str, key: Option<&str>) -> String {
    match key {
        Some(k) if k.len() >= 8 => text.replace(k, "<redacted>"),
        _ => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_usage_including_cached_tokens() {
        let v = json!({"choices": [{"message": {"content": "hi"}}], "usage": {"prompt_tokens": 1000, "completion_tokens": 20, "prompt_tokens_details": {"cached_tokens": 900}}});
        let r = OpenAiCompat::parse(&v).unwrap();
        assert_eq!((r.text.as_str(), r.usage), ("hi", Usage { prompt: 1000, completion: 20, cached_prompt: 900 }));
        assert_eq!(
            OpenAiCompat::parse(&json!({"choices": [{"message": {"content": "x"}}]})).unwrap().usage,
            Usage::default()
        );
        assert!(matches!(OpenAiCompat::parse(&json!({"choices": []})), Err(LlmError::Format(_))));
    }

    #[test]
    fn redaction_and_debug_never_leak_the_key() {
        assert_eq!(redact("bad key sk-abcdefgh1234", Some("sk-abcdefgh1234")), "bad key <redacted>");
        let c = OpenAiCompat::new("http://x/v1/", Some("sk-abcdefgh1234".into()), Duration::from_secs(5)).unwrap();
        assert!(!format!("{c:?}").contains("sk-abcdefgh1234") && c.base_url == "http://x/v1");
    }
}
