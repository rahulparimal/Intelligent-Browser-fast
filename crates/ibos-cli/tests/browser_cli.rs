//! The binary end to end with a real browser: `distill` on captured pages, and `run` talking to a fake OpenAI-compatible
//! server. Skipped without a browser unless `IBOS_REQUIRE_BROWSER=1`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use assert_cmd::Command;
use axum::extract::State;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use predicates::prelude::*;
use serde_json::{Value, json};

fn browser_available() -> bool {
    let ok = ibos_cdp::find_browser().is_some();
    assert!(
        ok || std::env::var("IBOS_REQUIRE_BROWSER").is_err(),
        "a browser is required (IBOS_REQUIRE_BROWSER is set)"
    );
    if !ok {
        eprintln!("skipping: no browser");
    }
    ok
}

fn page(name: &str) -> String {
    format!("{}/../../fixtures/pages/{name}.html", env!("CARGO_MANIFEST_DIR"))
}

fn cmd(home: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("intelli-rust").unwrap();
    c.env("INTELLI_HOME", home).env_remove("INTELLI_LOG");
    c
}

#[test]
fn distill_shows_the_page_a_model_would_see() {
    if !browser_available() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let out = cmd(t.path()).args(["distill", &page("login")]).assert().success().get_output().clone();
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    for want in ["<h1> Sign in", "<input> Email [required]", "<btn> Place order", "<a> Forgot password?"] {
        assert!(stdout.contains(want), "{want} missing:\n{stdout}");
    }
    assert!(stderr.contains("raw:") && stderr.contains("tokens") && stderr.contains("accessibility nodes"), "{stderr}");

    let json_out =
        cmd(t.path()).args(["distill", &page("products"), "--json"]).assert().success().get_output().stdout.clone();
    let v: Value = serde_json::from_slice(&json_out).unwrap();
    assert!(
        v["stats"]["collapsed_items"].as_u64().unwrap() > 100 && v["stats"]["tokens"].as_u64().unwrap() < 600,
        "{}",
        v["stats"]
    );
    assert!(v["stats"]["input_bytes"].as_u64().unwrap() > 500_000);

    let full =
        cmd(t.path()).args(["distill", &page("products"), "--full"]).assert().success().get_output().stdout.clone();
    assert!(String::from_utf8_lossy(&full).contains("model 120"), "--full lists everything");
    cmd(t.path())
        .args(["distill", "/no/such/file.html"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("neither a URL nor an existing file"));
}

const LOGIN: &str = r#"<!doctype html><html><head><title>Login</title></head><body><h1>Sign in</h1>
<label>Email <input id="email" type="email"></label>
<button id="go" onclick="var v=document.getElementById('email').value;fetch('/submit?email='+encodeURIComponent(v)).then(function(){document.getElementById('result').textContent='Welcome '+v})">Sign in</button>
<button id="buy" onclick="document.getElementById('result').textContent='ORDER PLACED'">Place order</button><div id="result"></div></body></html>"#;

fn handle(tail: &str, role: &str, needle: &str) -> u32 {
    tail.lines()
        .find_map(|l| {
            let (h, rest) = l.strip_prefix('[')?.split_once("]<")?;
            let (r, text) = rest.split_once("> ")?;
            (r == role && text.contains(needle)).then(|| h.parse().unwrap())
        })
        .unwrap_or_else(|| panic!("no <{role}> {needle:?} in:\n{tail}"))
}

/// A website plus a fake OpenAI-compatible endpoint whose replies depend on the page the agent was shown.
async fn serve(buy: bool) -> (String, String, Arc<Mutex<Vec<String>>>, Arc<AtomicUsize>) {
    let submissions = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let site = Router::new().route("/", get(|| async { Html(LOGIN) })).route(
        "/submit",
        get(
            |State(s): State<Arc<Mutex<Vec<String>>>>,
             axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>| async move {
                s.lock().unwrap().push(q.get("email").cloned().unwrap_or_default());
                "ok"
            },
        ),
    );
    let site = site.with_state(Arc::clone(&submissions));
    let sl = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let site_url = format!("http://{}/", sl.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(sl, site).await.unwrap() });

    let site_for_llm = site_url.clone();
    let c2 = Arc::clone(&calls);
    let llm = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let (site, calls) = (site_for_llm.clone(), Arc::clone(&c2));
            async move {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                let tail = body["messages"][1]["content"].as_str().unwrap().to_owned();
                let reply = match (n, buy) {
                    (0, _) => json!({"a": "go", "url": site}),
                    (1, false) => json!({"a": "type", "h": handle(&tail, "input", "Email"), "t": "me@example.com"}),
                    (2, false) => json!({"a": "click", "h": handle(&tail, "btn", "Sign in")}),
                    (1, true) => json!({"a": "click", "h": handle(&tail, "btn", "Place order")}),
                    _ => json!({"a": "done", "t": if tail.contains("Welcome me@example.com") { "signed in as me@example.com" } else { "finished" }, "ok": true}),
                };
                Json(json!({"choices": [{"message": {"content": reply.to_string()}}], "usage": {"prompt_tokens": 400, "completion_tokens": 20, "prompt_tokens_details": {"cached_tokens": 300}}}))
            }
        }),
    );
    let ll = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let llm_url = format!("http://{}/v1", ll.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(ll, llm).await.unwrap() });
    (site_url, llm_url, submissions, calls)
}

async fn run_cli(home: std::path::PathBuf, args: Vec<String>) -> std::process::Output {
    tokio::task::spawn_blocking(move || cmd(&home).env("OPENAI_API_KEY", "sk-test").args(&args).output().unwrap())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn run_completes_a_task_through_the_whole_stack() {
    if !browser_available() {
        return;
    }
    let (_site, llm, submissions, calls) = serve(false).await;
    let t = tempfile::tempdir().unwrap();
    let audit = t.path().join("audit.jsonl");
    let out = run_cli(
        t.path().to_path_buf(),
        [
            "run",
            "log in as me@example.com",
            "--model",
            "openai:test-model",
            "--base-url",
            &llm,
            "--headless",
            "--confirm",
            "off",
            "--audit-log",
            audit.to_str().unwrap(),
        ]
        .map(String::from)
        .to_vec(),
    )
    .await;
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(stdout.trim(), "signed in as me@example.com");
    assert_eq!(*submissions.lock().unwrap(), ["me@example.com"]);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert!(
        stderr.contains("model: openai:test-model")
            && stderr.contains("step 1: go")
            && stderr.contains("tokens: prompt=1600 (cached=1200, 75%)"),
        "{stderr}"
    );
    let log = std::fs::read_to_string(&audit).unwrap();
    assert_eq!(log.lines().count(), 4, "every action is audited: {log}");
}

#[tokio::test(flavor = "multi_thread")]
async fn run_denies_a_purchase_when_nobody_can_approve_it() {
    if !browser_available() {
        return;
    }
    let (_site, llm, _subs, _calls) = serve(true).await;
    let t = tempfile::tempdir().unwrap();
    let audit = t.path().join("audit.jsonl");
    let out = run_cli(
        t.path().to_path_buf(),
        [
            "run",
            "buy it",
            "--model",
            "openai",
            "--base-url",
            &llm,
            "--headless",
            "--audit-log",
            audit.to_str().unwrap(),
        ]
        .map(String::from)
        .to_vec(),
    )
    .await;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("Blocked by guardrail") && stderr.contains("1 blocked by guardrails"), "{stderr}");
    assert!(std::fs::read_to_string(&audit).unwrap().contains("\"decision\":\"denied\""));
}

#[test]
fn run_without_any_model_configured_explains_what_to_do() {
    let t = tempfile::tempdir().unwrap();
    cmd(t.path())
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .args(["run", "anything"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("No LLM configured").and(predicate::str::contains("ollama")));
}
