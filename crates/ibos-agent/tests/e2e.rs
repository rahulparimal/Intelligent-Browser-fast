//! The whole agent against real Chromium and a local website, driven by a fake model that reads element handles from the
//! prompt it was given (handles are browser node ids, unknowable in advance). Skipped without a browser unless
//! `IBOS_REQUIRE_BROWSER=1`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::response::Html;
use axum::routing::get;
use ibos_agent::{Agent, AgentConfig, ConfirmMode, Guard, GuardConfig, StopReason};
use ibos_cdp::{Browser, LaunchOptions, Page, find_browser};
use ibos_llm::{ChatRequest, ChatResponse, FnLlm, Llm, LlmError};
use serde_json::json;

#[derive(Default)]
struct Site {
    submissions: Mutex<Vec<String>>,
    hits: Mutex<Vec<String>>,
    home_loads: AtomicUsize,
}

const LOGIN: &str = r#"<!doctype html><html><head><title>Login</title></head><body>
<h1>Sign in</h1>
<label>Email <input id="email" type="email"></label>
<label>Password <input id="pw" type="password"></label>
<button id="go" onclick="var v=document.getElementById('email').value;fetch('/submit?email='+encodeURIComponent(v)).then(function(){document.getElementById('result').textContent='Welcome '+v})">Sign in</button>
<button id="buy" onclick="document.getElementById('result').textContent='ORDER PLACED'">Place order</button>
<div id="result"></div></body></html>"#;

async fn serve_site() -> (Arc<Site>, String) {
    let site = Arc::new(Site::default());
    let products =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/pages/products.html")).unwrap();
    let table =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/pages/table.html")).unwrap();
    let app = Router::new()
        .route("/", get(|State(s): State<Arc<Site>>| async move { s.home_loads.fetch_add(1, Ordering::SeqCst); Html(LOGIN) }))
        .route("/submit", get(|State(s): State<Arc<Site>>, Query(q): Query<std::collections::HashMap<String, String>>| async move {
            s.submissions.lock().unwrap().push(q.get("email").cloned().unwrap_or_default());
            "ok"
        }))
        .route("/many", get(|| async {
            let mut rows = String::new();
            for i in 1..=20 {
                let _ = write!(rows, "<li><span>Row {i}</span> <button onclick=\"document.getElementById('result').textContent='deleted {i}'\">Delete item {i}</button></li>");
            }
            Html(format!("<html><head><title>Many</title></head><body><h1>Items</h1><ul>{rows}</ul><div id=\"result\"></div></body></html>"))
        }))
        .route("/leave", get(|headers: axum::http::HeaderMap| async move {
            let host = headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("127.0.0.1").replace("127.0.0.1", "localhost");
            Html(format!("<html><head><title>Leave</title></head><body><a href=\"http://{host}/p/1\">Elsewhere</a></body></html>"))
        }))
        .route("/table", get(move || { let t = table.clone(); async move { Html(t) } }))
        .route("/products", get(move || { let p = products.clone(); async move { Html(p) } }))
        .route("/p/{id}", get(|State(s): State<Arc<Site>>, Path(id): Path<u32>| async move {
            s.hits.lock().unwrap().push(format!("/p/{id}"));
            Html(format!("<html><head><title>Product {id}</title></head><body><h1>Product {id} detail</h1><button>Add to cart</button></body></html>"))
        }))
        .with_state(Arc::clone(&site));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (site, base)
}

async fn browser() -> Option<(Browser, Page)> {
    let ok = find_browser().is_some();
    assert!(
        ok || std::env::var("IBOS_REQUIRE_BROWSER").is_err(),
        "a browser is required (IBOS_REQUIRE_BROWSER is set)"
    );
    if !ok {
        eprintln!("skipping: no browser");
        return None;
    }
    let b = Browser::launch(LaunchOptions::default()).await.unwrap();
    let p = b.new_page().await.unwrap();
    Some((b, p))
}

/// The handle of the first `[N]<role> text` line in the prompt whose role matches and whose text contains `needle`.
fn handle(req: &ChatRequest, role: &str, needle: &str) -> u32 {
    let tail = &req.prompt.tail;
    tail.lines()
        .find_map(|l| {
            let rest = l.strip_prefix('[')?;
            let (h, rest) = rest.split_once("]<")?;
            let (r, text) = rest.split_once("> ")?;
            (r == role && text.contains(needle)).then(|| h.parse().unwrap())
        })
        .unwrap_or_else(|| panic!("no <{role}> containing {needle:?} in the prompt:\n{tail}"))
}

fn cfg() -> AgentConfig {
    AgentConfig { model: "test".into(), settle: std::time::Duration::from_millis(30), ..AgentConfig::default() }
}

fn deny() -> Guard {
    Guard::deny_risky(GuardConfig::default())
}

#[tokio::test]
async fn logs_in_and_answers_with_a_stable_prefix() {
    let Some((_b, page)) = browser().await else { return };
    let (site, base) = serve_site().await;
    let url = format!("{base}/");
    let llm = Arc::new(FnLlm::new(move |req: &ChatRequest, n| match n {
        0 => json!({"a": "go", "url": url}).to_string(),
        1 => json!({"a": "type", "h": handle(req, "input", "Email"), "t": "me@example.com"}).to_string(),
        2 => json!({"a": "click", "h": handle(req, "btn", "Sign in")}).to_string(),
        _ => {
            assert!(
                req.prompt.tail.contains("Welcome me@example.com"),
                "the page after sign-in is visible:\n{}",
                req.prompt.tail
            );
            json!({"a": "done", "t": "Welcome me@example.com", "ok": true}).to_string()
        }
    }));
    let report = Agent::new(page, llm.clone(), cfg(), deny()).run("log in as me@example.com").await;
    assert!(
        report.success && report.stop == StopReason::Done && report.answer == "Welcome me@example.com",
        "{report:?}"
    );
    assert_eq!(*site.submissions.lock().unwrap(), ["me@example.com"]);
    assert_eq!(report.steps.len(), 4);

    let reqs = llm.requests();
    assert!(
        reqs.windows(2)
            .all(|w| w[0].prompt.prefix == w[1].prompt.prefix && w[0].prompt.task_block == w[1].prompt.task_block),
        "prefix and task block are byte-identical on every step"
    );
    assert!(
        report.steps.iter().all(|s| !s.observation_was_delta),
        "every prompt is stateless, so every observation is the whole page"
    );
    assert!(report.steps[0].prefix_tokens < 600, "the whole static prompt is {} tokens", report.steps[0].prefix_tokens);
    println!(
        "static prefix {} tokens; tails {:?}",
        report.steps[0].prefix_tokens,
        report.steps.iter().map(|s| s.tail_tokens).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn typed_secrets_are_not_kept_in_history() {
    let Some((_b, page)) = browser().await else { return };
    let (_site, base) = serve_site().await;
    let url = format!("{base}/");
    let llm = Arc::new(FnLlm::new(move |req: &ChatRequest, n| match n {
        0 => json!({"a": "go", "url": url}).to_string(),
        1 => json!({"a": "type", "h": handle(req, "input", "Password"), "t": "hunter2-secret"}).to_string(),
        _ => json!({"a": "done", "t": "ok", "ok": true}).to_string(),
    }));
    let report = Agent::new(page, llm.clone(), cfg(), deny()).run("type a password").await;
    assert!(report.success);
    let last = llm.requests().last().unwrap().prompt.flatten();
    assert!(last.contains("type ") && last.contains("[14 chars]"), "the history says what happened:\n{last}");
    assert!(!last.contains("hunter2"), "but never what was typed:\n{last}");
}

#[tokio::test]
async fn risky_click_is_denied_logged_and_never_reaches_the_page() {
    let Some((_b, page)) = browser().await else { return };
    let (_site, base) = serve_site().await;
    let (url, dir) = (format!("{base}/"), tempfile::tempdir().unwrap());
    let audit = dir.path().join("audit.jsonl");
    let llm = Arc::new(FnLlm::new(move |req: &ChatRequest, n| match n {
        0 => json!({"a": "go", "url": url}).to_string(),
        1 => json!({"a": "click", "h": handle(req, "btn", "Place order")}).to_string(),
        _ => {
            assert!(req.prompt.tail.contains("Blocked by guardrail"), "the model is told why:\n{}", req.prompt.tail);
            json!({"a": "done", "t": "stopped", "ok": true}).to_string()
        }
    }));
    let guard = Guard::deny_risky(GuardConfig { audit_path: Some(audit.clone()), ..GuardConfig::default() });
    let report = Agent::new(page.clone(), llm, cfg(), guard).run("buy it").await;
    assert!(report.success && report.denied == 1, "{report:?}");
    assert_eq!(
        page.evaluate("document.getElementById('result').textContent").await.unwrap(),
        json!(""),
        "the click never happened"
    );
    let log = std::fs::read_to_string(&audit).unwrap();
    assert!(log.lines().any(|l| l.contains("\"decision\":\"denied\"") && l.contains("click")), "{log}");
}

#[tokio::test]
async fn approved_risky_click_goes_through_and_confirm_off_skips_the_question() {
    let Some((_b, page)) = browser().await else { return };
    let (_site, base) = serve_site().await;
    let url = format!("{base}/");
    let make = |url: String| {
        FnLlm::new(move |req: &ChatRequest, n| match n {
            0 => json!({"a": "go", "url": url}).to_string(),
            1 => json!({"a": "click", "h": handle(req, "btn", "Place order")}).to_string(),
            _ => json!({"a": "done", "t": "x", "ok": true}).to_string(),
        })
    };
    let asked = Arc::new(AtomicUsize::new(0));
    let a2 = Arc::clone(&asked);
    let guard = Guard::new(
        GuardConfig::default(),
        Box::new(move |_| {
            a2.fetch_add(1, Ordering::SeqCst);
            true
        }),
    );
    let r = Agent::new(page.clone(), Arc::new(make(url.clone())), cfg(), guard).run("buy").await;
    assert!(r.success && r.denied == 0 && asked.load(Ordering::SeqCst) == 1);
    assert_eq!(page.evaluate("document.getElementById('result').textContent").await.unwrap(), json!("ORDER PLACED"));

    let off = Guard::new(
        GuardConfig { confirm_mode: ConfirmMode::Off, ..GuardConfig::default() },
        Box::new(|_| panic!("must not be asked")),
    );
    let r = Agent::new(page, Arc::new(make(url)), cfg(), off).run("buy").await;
    assert!(r.success && r.denied == 0);
}

#[tokio::test]
async fn domain_limits_stop_navigation_before_any_request_is_made() {
    let Some((_b, page)) = browser().await else { return };
    let (site, base) = serve_site().await;
    let url = format!("{base}/");
    let llm = Arc::new(FnLlm::new(move |req: &ChatRequest, n| {
        if n == 0 {
            return json!({"a": "go", "url": url}).to_string();
        }
        assert!(req.prompt.tail.contains("not in the allowed domains"), "{}", req.prompt.tail);
        json!({"a": "done", "t": "blocked", "ok": false}).to_string()
    }));
    let guard =
        Guard::deny_risky(GuardConfig { allowed_domains: vec!["example.com".into()], ..GuardConfig::default() });
    let r = Agent::new(page, llm, cfg(), guard).run("visit").await;
    assert!(!r.success && r.denied == 1);
    assert_eq!(site.home_loads.load(Ordering::SeqCst), 0, "the off-limits site was never requested");
}

#[tokio::test]
async fn more_lists_collapsed_items_and_they_can_be_clicked() {
    let Some((_b, page)) = browser().await else { return };
    let (site, base) = serve_site().await;
    let url = format!("{base}/products");
    let llm = Arc::new(FnLlm::new(move |req: &ChatRequest, n| match n {
        0 => json!({"a": "go", "url": url}).to_string(),
        1 => {
            assert!(!req.prompt.tail.contains("model 50 "), "item 50 is hidden behind the marker at first");
            json!({"a": "more", "h": handle(req, "+", "more similar items"), "q": "model 50"}).to_string()
        }
        2 => {
            assert!(req.prompt.tail.contains("hidden items for ["), "{}", req.prompt.tail);
            let h = req
                .prompt
                .tail
                .lines()
                .find(|l| l.contains("<a>") && l.ends_with("model 50"))
                .and_then(|l| l.strip_prefix('[')?.split_once(']')?.0.parse::<u32>().ok())
                .expect("model 50 is in the expansion");
            json!({"a": "click", "h": h}).to_string()
        }
        _ => {
            assert!(!req.prompt.tail.contains("hidden items for ["), "the expansion is a one-off, not remembered");
            assert!(req.prompt.tail.contains("Product 50 detail"));
            json!({"a": "done", "t": "model 50 opened", "ok": true}).to_string()
        }
    }));
    let r = Agent::new(page, llm, cfg(), deny()).run("open model 50").await;
    assert!(r.success, "{r:?}");
    assert!(site.hits.lock().unwrap().iter().any(|h| h == "/p/50"), "an item that was never on screen was clicked");
}

#[tokio::test]
async fn invalid_replies_recover_and_too_many_stop_the_run() {
    let Some((_b, page)) = browser().await else { return };
    let recover = Arc::new(FnLlm::new(|_, n| match n {
        0 => "I think I should look around.".into(),
        1 => r#"{"a":"fly"}"#.into(),
        _ => r#"```json
{"a":"done","t":"fine","ok":true}
```"#
            .into(),
    }));
    let r = Agent::new(page.clone(), recover, cfg(), deny()).run("t").await;
    assert!(r.success && r.steps.len() == 3 && r.steps[0].outcome.contains("JSON"), "{r:?}");
    let hopeless = Arc::new(FnLlm::new(|_, _| "no idea".into()));
    let r = Agent::new(page, hopeless, cfg(), deny()).run("t").await;
    assert_eq!((r.stop, r.steps.len()), (StopReason::TooManyInvalidReplies, 3));
}

struct Broken;

#[async_trait]
impl Llm for Broken {
    async fn chat(&self, _: &ChatRequest) -> Result<ChatResponse, LlmError> {
        Err(LlmError::Http { status: 401, body: "bad key".into() })
    }
}

#[tokio::test]
async fn limits_and_failures_stop_cleanly() {
    let Some((_b, page)) = browser().await else { return };
    let scroller = Arc::new(FnLlm::new(|_, _| r#"{"a":"scroll","d":"down"}"#.into()));
    let r = Agent::new(page.clone(), scroller, AgentConfig { max_steps: 3, ..cfg() }, deny()).run("t").await;
    assert_eq!((r.stop, r.steps.len()), (StopReason::MaxSteps, 3));

    let chatty = Arc::new(FnLlm::new(|_, _| r#"{"a":"scroll","d":"down"}"#.into()));
    let r = Agent::new(page.clone(), chatty, AgentConfig { token_budget: 300, ..cfg() }, deny()).run("t").await;
    assert_eq!(
        r.stop,
        StopReason::Budget,
        "the static prompt alone exceeds 300 tokens, so the budget is spent after one call: {r:?}"
    );
    assert!(r.steps.len() <= 2 && r.usage.total().prompt > 300);

    let r = Agent::new(page, Arc::new(Broken), cfg(), deny()).run("t").await;
    assert!(matches!(&r.stop, StopReason::ModelError(m) if m.contains("401")), "{:?}", r.stop);
}

#[tokio::test]
async fn a_dead_browser_ends_the_run_instead_of_hanging() {
    let Some((b, page)) = browser().await else { return };
    let llm = Arc::new(FnLlm::new(|_, _| r#"{"a":"scroll","d":"down"}"#.into()));
    drop(b); // the browser process tree is killed
    let t0 = std::time::Instant::now();
    let r = Agent::new(page, llm, cfg(), deny()).run("t").await;
    assert!(matches!(r.stop, StopReason::BrowserError(_)), "{:?}", r.stop);
    assert!(t0.elapsed() < std::time::Duration::from_secs(10), "{:?}", t0.elapsed());
}

#[tokio::test]
async fn more_pages_through_hidden_items_and_reports_when_nothing_matches() {
    let Some((_b, page)) = browser().await else { return };
    let (_site, base) = serve_site().await;
    let url = format!("{base}/products");
    let llm = Arc::new(FnLlm::new(move |req: &ChatRequest, n| match n {
        0 => json!({"a": "go", "url": url}).to_string(),
        1 => json!({"a": "more", "h": handle(req, "+", "more similar items"), "n": 3}).to_string(),
        2 => {
            assert!(
                req.prompt.tail.contains("(showing 1-3 of ") && req.prompt.tail.contains("\"skip\":3"),
                "{}",
                req.prompt.tail
            );
            json!({"a": "more", "h": handle(req, "+", "more similar items"), "q": "zzz-no-such-thing"}).to_string()
        }
        3 => {
            assert!(req.prompt.tail.contains("(nothing matches)"), "{}", req.prompt.tail);
            json!({"a": "scroll", "d": "up"}).to_string()
        }
        _ => {
            assert!(
                req.prompt.tail.contains("page unchanged since your last action"),
                "an action with no effect is called out:\n{}",
                req.prompt.tail
            );
            json!({"a": "done", "t": "ok", "ok": true}).to_string()
        }
    }));
    assert!(Agent::new(page, llm, cfg(), deny()).run("browse").await.success);
}

#[tokio::test]
async fn a_filtered_more_returns_the_whole_matching_row() {
    let Some((_b, page)) = browser().await else { return };
    let (_site, base) = serve_site().await;
    let url = format!("{base}/table");
    let llm = Arc::new(FnLlm::new(move |req: &ChatRequest, n| match n {
        0 => json!({"a": "go", "url": url}).to_string(),
        1 => json!({"a": "more", "h": handle(req, "+", "more similar items"), "q": "Employee 45"}).to_string(),
        _ => {
            let note = req.prompt.tail.split("hidden items for [").nth(1).expect("the expansion note is in the prompt");
            let row: Vec<&str> = note.lines().skip(1).take(4).collect();
            assert!(
                row.len() == 4 && row[0].contains("<a>") && row[1..].iter().all(|l| l.contains("<text>")),
                "the link and its three cells: {row:?}"
            );
            assert!(row[3].contains("days"), "the leave column is there: {row:?}");
            json!({"a": "done", "t": row[3].to_string(), "ok": true}).to_string()
        }
    }));
    let r = Agent::new(page, llm, cfg(), deny()).run("how many leave days does Employee 45 have").await;
    assert!(r.success && r.answer.contains("days"), "{r:?}");
}

#[tokio::test]
async fn a_risky_button_only_revealed_by_more_still_needs_approval() {
    let Some((_b, page)) = browser().await else { return };
    let (_site, base) = serve_site().await;
    let url = format!("{base}/many");
    let llm = Arc::new(FnLlm::new(move |req: &ChatRequest, n| match n {
        0 => json!({"a": "go", "url": url}).to_string(),
        1 => {
            assert!(!req.prompt.tail.contains("Delete item 15"), "item 15 is collapsed at first:\n{}", req.prompt.tail);
            json!({"a": "more", "h": handle(req, "+", "more similar items"), "q": "Delete item 15"}).to_string()
        }
        2 => {
            let h = req
                .prompt
                .tail
                .lines()
                .find(|l| l.contains("<btn>") && l.ends_with("Delete item 15"))
                .and_then(|l| l.strip_prefix('[')?.split_once(']')?.0.parse::<u32>().ok())
                .expect("the hidden button is listed");
            json!({"a": "click", "h": h}).to_string()
        }
        _ => {
            assert!(req.prompt.tail.contains("Blocked by guardrail"), "{}", req.prompt.tail);
            json!({"a": "done", "t": "stopped", "ok": true}).to_string()
        }
    }));
    let report = Agent::new(page.clone(), llm, cfg(), deny()).run("delete item 15").await;
    assert!(report.success && report.denied == 1, "{report:?}");
    assert_eq!(
        page.evaluate("document.getElementById('result').textContent").await.unwrap(),
        json!(""),
        "nothing was deleted"
    );
}

#[tokio::test]
async fn an_invented_handle_is_refused_and_nothing_is_clicked() {
    let Some((_b, page)) = browser().await else { return };
    let (_site, base) = serve_site().await;
    let url = format!("{base}/");
    let llm = Arc::new(FnLlm::new(move |req: &ChatRequest, n| match n {
        0 => json!({"a": "go", "url": url}).to_string(),
        1 => json!({"a": "click", "h": 987_654}).to_string(),
        _ => {
            assert!(req.prompt.tail.contains("does not exist"), "{}", req.prompt.tail);
            json!({"a": "done", "t": "gave up", "ok": true}).to_string()
        }
    }));
    let report = Agent::new(page.clone(), llm, cfg(), deny()).run("click something").await;
    assert_eq!(report.denied, 1, "{report:?}");
    assert_eq!(page.evaluate("document.getElementById('result').textContent").await.unwrap(), json!(""));
}

#[tokio::test]
async fn a_link_that_leaves_the_allowed_sites_ends_on_a_blank_page() {
    let Some((_b, page)) = browser().await else { return };
    let (_site, base) = serve_site().await;
    let url = format!("{base}/leave");
    let llm = Arc::new(FnLlm::new(move |req: &ChatRequest, n| match n {
        0 => json!({"a": "go", "url": url}).to_string(),
        1 => json!({"a": "click", "h": handle(req, "a", "Elsewhere")}).to_string(),
        _ => {
            assert!(req.prompt.tail.contains("outside the allowed sites"), "{}", req.prompt.tail);
            json!({"a": "done", "t": "stopped", "ok": true}).to_string()
        }
    }));
    let guard = Guard::deny_risky(GuardConfig { allowed_domains: vec!["127.0.0.1".into()], ..GuardConfig::default() });
    let report = Agent::new(page.clone(), llm, cfg(), guard).run("follow the link").await;
    assert!(report.success, "{report:?}");
    assert_eq!(page.url().await.unwrap(), "about:blank", "the agent did not stay on the other site");
}
