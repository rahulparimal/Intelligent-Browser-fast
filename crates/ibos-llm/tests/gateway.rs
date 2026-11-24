//! The clients against a local fake endpoint: request shape, auth, usage parsing, retries, and secret hygiene.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use ibos_ctx::prompt::{Parts, assemble};
use ibos_llm::{Anthropic, ChatRequest, Llm, LlmError, OpenAiCompat, RetryPolicy, ScriptedLlm};
use serde_json::{Value, json};

#[derive(Default)]
struct Fake {
    calls: AtomicU32,
    fail_first: u32,
    status: u16,
    last: std::sync::Mutex<Option<(HeaderMap, Value)>>,
}

async fn handler(
    State(f): State<Arc<Fake>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, HeaderMap, Json<Value>) {
    let n = f.calls.fetch_add(1, Ordering::SeqCst);
    *f.last.lock().unwrap() = Some((headers.clone(), body));
    let mut h = HeaderMap::new();
    if n < f.fail_first {
        h.insert("retry-after", "0".parse().unwrap());
        return (
            StatusCode::from_u16(f.status).unwrap(),
            h,
            Json(json!({"error": "overloaded, key was sk-secretsecret99"})),
        );
    }
    let reply = if headers.contains_key("x-api-key") {
        json!({"content": [{"type": "text", "text": "anthropic ok"}], "usage": {"input_tokens": 10, "cache_read_input_tokens": 90, "output_tokens": 3}})
    } else {
        json!({"choices": [{"message": {"content": "openai ok"}}], "usage": {"prompt_tokens": 100, "completion_tokens": 4, "prompt_tokens_details": {"cached_tokens": 80}}})
    };
    (StatusCode::OK, h, Json(reply))
}

async fn serve(f: Arc<Fake>) -> String {
    let app =
        Router::new().route("/v1/chat/completions", post(handler)).route("/v1/messages", post(handler)).with_state(f);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

fn request() -> ChatRequest {
    let prompt = assemble(&Parts {
        system: "SYS",
        tools: "TOOLS",
        skills: "",
        task: "log in",
        ledger: "",
        history: "",
        observation: "page",
    });
    ChatRequest { model: "test-model".into(), prompt, max_tokens: 64, temperature: 0.0 }
}

fn fast() -> RetryPolicy {
    RetryPolicy { max_attempts: 3, base: Duration::from_millis(1), cap: Duration::from_millis(5) }
}

#[tokio::test]
async fn openai_request_shape_auth_and_usage() {
    let f = Arc::new(Fake { status: 200, ..Fake::default() });
    let base = serve(f.clone()).await;
    let c = OpenAiCompat::new(&format!("{base}/v1"), Some("sk-secretsecret99".into()), Duration::from_secs(5)).unwrap();
    let r = c.chat(&request()).await.unwrap();
    assert_eq!((r.text.as_str(), r.usage.prompt, r.usage.cached_prompt), ("openai ok", 100, 80));
    let (headers, body) = f.last.lock().unwrap().clone().unwrap();
    assert_eq!(headers["authorization"], "Bearer sk-secretsecret99");
    assert_eq!((body["model"].as_str(), body["messages"][0]["role"].as_str()), (Some("test-model"), Some("system")));
    assert_eq!(body["messages"][0]["content"], "SYS\n\nTOOLS", "the stable prefix is the system message");
}

#[tokio::test]
async fn anthropic_request_has_cache_control_and_usage_is_summed() {
    let f = Arc::new(Fake { status: 200, ..Fake::default() });
    let base = serve(f.clone()).await;
    let c = Anthropic::with_base_url(&base, "sk-ant-key", Duration::from_secs(5)).unwrap();
    let r = c.chat(&request()).await.unwrap();
    assert_eq!((r.text.as_str(), r.usage.prompt, r.usage.cached_prompt), ("anthropic ok", 100, 90));
    let (headers, body) = f.last.lock().unwrap().clone().unwrap();
    assert_eq!(
        (headers["x-api-key"].to_str().unwrap(), headers["anthropic-version"].to_str().unwrap()),
        ("sk-ant-key", "2023-06-01")
    );
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
}

#[tokio::test]
async fn transient_failures_are_retried_and_secrets_never_surface() {
    let f = Arc::new(Fake { fail_first: 2, status: 503, ..Fake::default() });
    let base = serve(f.clone()).await;
    let c = OpenAiCompat::new(&format!("{base}/v1"), Some("sk-secretsecret99".into()), Duration::from_secs(5))
        .unwrap()
        .with_retry(fast());
    assert_eq!(c.chat(&request()).await.unwrap().text, "openai ok");
    assert_eq!(f.calls.load(Ordering::SeqCst), 3);

    let f = Arc::new(Fake { fail_first: 99, status: 503, ..Fake::default() });
    let base = serve(f.clone()).await;
    let c = OpenAiCompat::new(&format!("{base}/v1"), Some("sk-secretsecret99".into()), Duration::from_secs(5))
        .unwrap()
        .with_retry(fast());
    let err = c.chat(&request()).await.unwrap_err();
    assert!(matches!(err, LlmError::Http { status: 503, .. }));
    assert!(!err.to_string().contains("sk-secretsecret99"), "the key echoed by a server must be redacted: {err}");
    assert_eq!(f.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn client_errors_are_not_retried_and_unreachable_endpoints_are() {
    let f = Arc::new(Fake { fail_first: 99, status: 401, ..Fake::default() });
    let base = serve(f.clone()).await;
    let c = OpenAiCompat::new(&format!("{base}/v1"), None, Duration::from_secs(5)).unwrap().with_retry(fast());
    assert!(matches!(c.chat(&request()).await, Err(LlmError::Http { status: 401, .. })));
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    let dead = OpenAiCompat::new("http://127.0.0.1:9/v1", None, Duration::from_secs(2))
        .unwrap()
        .with_retry(RetryPolicy::none());
    assert!(matches!(dead.chat(&request()).await, Err(LlmError::Transport(_))));
}

#[tokio::test]
async fn scripted_llm_replays_and_records() {
    let s = ScriptedLlm::new(["one", "two"]);
    assert_eq!(s.chat(&request()).await.unwrap().text, "one");
    assert_eq!(s.chat(&request()).await.unwrap().text, "two");
    assert!(matches!(s.chat(&request()).await, Err(LlmError::ScriptExhausted)));
    assert_eq!(s.requests().len(), 3);
}
