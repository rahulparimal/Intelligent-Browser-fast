//! The HTTP API end to end: pages, security checks, settings, folders and background jobs, and AI answers against a fake model.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use axum::http::HeaderMap;
use axum::routing::post;
use ibos_search::LiteStore;
use ibos_server::{Running, ServeOptions, start};
use serde_json::{Value, json};

const CORPUS: [(&str, &str); 4] = [
    (
        "leave-policy.md",
        "# Leave Policy\n\nFull-time employees receive 25 days of paid annual leave per calendar year. Unused leave may be carried over, capped at 5 days.",
    ),
    (
        "security.md",
        "# Security\n\nAll laptops must use full-disk encryption. Multi-factor authentication (MFA) is mandatory for e-mail, VPN and admin consoles.",
    ),
    (
        "faq.md",
        "# FAQ\n\nHow do I reset my password? Use the self-service portal. How many vacation days do I get? See the leave policy.",
    ),
    (
        "expenses.md",
        "# Expenses\n\nSubmit expense reports within 30 days of travel. Receipts are required for any purchase above $25 for all staff.",
    ),
];

struct Fixture {
    dir: tempfile::TempDir,
    docs: std::path::PathBuf,
    store: Arc<LiteStore>,
    server: Running,
    http: reqwest::Client,
    base: String,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    for (n, t) in CORPUS {
        std::fs::write(docs.join(n), t).unwrap();
    }
    let store = Arc::new(LiteStore::open(Some(&dir.path().join("idx").join("i.db")), None).unwrap());
    store.index_path(&docs, ibos_search::IndexOptions::default(), None).unwrap();
    let server = start(Arc::clone(&store), &ServeOptions { port: 0, open_browser: false, ..ServeOptions::default() })
        .await
        .unwrap();
    let base = server.url();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-intelli-token", server.token().parse().unwrap());
    let http = reqwest::Client::builder()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    Fixture { dir, docs, store, server, http, base }
}

impl Fixture {
    async fn get(&self, path: &str) -> (u16, Value) {
        let r = self.http.get(format!("{}{path}", self.base)).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        let r =
            self.http.post(format!("{}{path}", self.base)).header("X-Intelli", "1").json(body).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }

    async fn wait_job(&self) -> Value {
        for _ in 0..400 {
            let job = self.get("/api/admin/state").await.1["job"].clone();
            if job["running"] == json!(false) {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("job did not finish");
    }
}

#[tokio::test]
async fn pages_are_served_with_security_headers() {
    let f = fixture().await;
    for (path, marker) in [("/", "Intelli Search"), ("/admin", "AI answers")] {
        let r = f.http.get(format!("{}{path}", f.base)).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert!(r.headers()["content-security-policy"].to_str().unwrap().starts_with("default-src 'self'"));
        assert_eq!(
            (r.headers()["x-content-type-options"].to_str().unwrap(), r.headers()["cache-control"].to_str().unwrap()),
            ("nosniff", "no-store")
        );
        assert!(r.text().await.unwrap().contains(marker));
    }
    f.server.shutdown().await;
}

#[tokio::test]
async fn foreign_hosts_and_missing_header_are_rejected() {
    let f = fixture().await;
    let evil = f.http.get(format!("{}/api/admin/state", f.base)).header("Host", "evil.example").send().await.unwrap();
    assert_eq!(evil.status(), 403);
    let no_header =
        f.http.post(format!("{}/api/admin/clear", f.base)).json(&json!({"confirm": "clear"})).send().await.unwrap();
    assert_eq!(no_header.status(), 403);
    let evil_post = f
        .http
        .post(format!("{}/api/admin/clear", f.base))
        .header("X-Intelli", "1")
        .header("Host", "evil.example")
        .json(&json!({"confirm": "clear"}))
        .send()
        .await
        .unwrap();
    assert_eq!(evil_post.status(), 403);
    assert_eq!(f.get("/api/admin/state").await.1["stats"]["documents"], 4, "nothing was cleared");
    f.server.shutdown().await;
}

#[tokio::test]
async fn search_health_and_admin_state() {
    let f = fixture().await;
    let (code, out) = f.get("/api/search?q=annual+leave").await;
    assert_eq!((code, out["results"][0]["title"].as_str()), (200, Some("leave-policy")));
    assert!(out["results"][0]["snippet"].as_str().unwrap().contains("<mark>"));
    assert_eq!(f.get("/api/search?q=email").await.1["results"][0]["title"], "security", "e-mail is found by 'email'");
    let (_, h) = f.get("/health").await;
    assert_eq!(
        (h["mode"].as_str(), h["documents"].as_i64(), h["answers"].as_bool()),
        (Some("lite"), Some(4), Some(false))
    );
    let (_, st) = f.get("/api/admin/state").await;
    assert_eq!((st["needs_reindex"].clone(), st["settings"]["has_api_key"].clone()), (json!(false), json!(false)));
    assert!(st["index_path"].as_str().unwrap().ends_with("i.db"));
    assert_eq!(f.get("/api/search?q=policy&limit=abc").await.0, 200);
    f.server.shutdown().await;
}

#[tokio::test]
async fn settings_roundtrip_validation_and_search_limit() {
    let f = fixture().await;
    let (code, st) = f
        .post("/api/admin/settings", &json!({"result_limit": 1, "group_by_document": false, "extensions": [".md"]}))
        .await;
    assert_eq!((code, st["settings"]["result_limit"].clone()), (200, json!(1)));
    assert_eq!(f.get("/api/search?q=policy").await.1["results"].as_array().unwrap().len(), 1);
    assert_eq!(f.get("/api/search?q=policy&limit=5").await.1["results"].as_array().unwrap().len(), 2);
    assert_eq!(f.post("/api/admin/settings", &json!({"llm_base_url": "ftp://nope"})).await.0, 422);
    assert_eq!(f.post("/api/admin/settings", &json!({"answers_enabled": true})).await.0, 422);
    assert_eq!(
        f.post("/api/admin/settings", &json!({"result_limit": 5})).await.1["settings"]["extensions"],
        json!([".md"]),
        "a partial update keeps the rest"
    );
    assert_eq!(
        f.post("/api/admin/settings", &json!({"extensions": "x", "result_limit": "bogus"})).await.0,
        200,
        "junk is sanitised, never a 500"
    );
    f.server.shutdown().await;
}

#[tokio::test]
async fn api_key_never_leaves_the_server() {
    let f = fixture().await;
    f.post(
        "/api/admin/settings",
        &json!({"llm_api_key": "sk-topsecret-xyz", "llm_base_url": "https://x.test/v1", "llm_model": "m"}),
    )
    .await;
    let (_, st) = f.get("/api/admin/state").await;
    assert!(!st.to_string().contains("sk-topsecret-xyz") && st["settings"]["has_api_key"] == json!(true));
    assert!(!reqwest::get(format!("{}/admin", f.base)).await.is_ok_and(|_| false));
    let (_, st2) = f.post("/api/admin/settings", &json!({"clear_api_key": true, "answers_enabled": false})).await;
    assert_eq!(st2["settings"]["has_api_key"], json!(false));
    f.server.shutdown().await;
}

#[tokio::test]
async fn folder_lifecycle_background_reindex_and_job_ids() {
    let f = fixture().await;
    let fresh = f.dir.path().join("fresh");
    std::fs::create_dir_all(&fresh).unwrap();
    std::fs::write(fresh.join("a.md"), "Quarterly planning notes about hiring and budget forecasts for next year.")
        .unwrap();
    assert_eq!(f.get("/api/admin/state").await.1["job"]["id"], 0);
    assert_eq!(f.post("/api/admin/folders/add", &json!({"path": fresh})).await.0, 200);
    let job = f.wait_job().await;
    assert_eq!(
        (job["error"].clone(), job["result"]["indexed"].clone(), job["id"].clone()),
        (json!(null), json!(1), json!(1))
    );
    std::fs::write(fresh.join("b.md"), "Another document about hiring plans and onboarding schedules for new teams.")
        .unwrap();
    std::fs::remove_file(fresh.join("a.md")).unwrap();
    assert_eq!(f.post("/api/admin/reindex", &json!({})).await.0, 200);
    let job = f.wait_job().await;
    assert_eq!(
        (job["result"]["indexed"].clone(), job["result"]["removed"].clone(), job["id"].clone()),
        (json!(1), json!(1), json!(2))
    );
    let (code, out) = f.post("/api/admin/folders/remove", &json!({"path": fresh})).await;
    assert_eq!((code, out["dropped"].clone(), out["stats"]["documents"].clone()), (200, json!(1), json!(4)));
    assert_eq!(f.post("/api/admin/folders/add", &json!({"path": "/no/such/dir"})).await.0, 404);
    assert_eq!(f.post("/api/admin/folders/add", &json!({})).await.0, 422);
    f.server.shutdown().await;
}

#[tokio::test]
async fn index_endpoint_registers_folders() {
    let f = fixture().await;
    let d = f.dir.path().join("viaindex");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("n.md"), "Notes about the annual company offsite planning and venue selection process.")
        .unwrap();
    let (code, stats) = f.post("/api/index", &json!({"path": d})).await;
    assert_eq!((code, stats["indexed"].clone()), (200, json!(1)));
    assert!(f.store.folders().unwrap().iter().any(|p| p.ends_with("viaindex")));
    assert_eq!(f.post("/api/index", &json!({"path": "/no/such/dir"})).await.0, 404);
    assert_eq!(f.post("/api/index", &json!({"nope": 1})).await.0, 422);
    f.server.shutdown().await;
}

#[tokio::test]
async fn documents_listing_removal_and_clear() {
    let f = fixture().await;
    let (_, out) = f.get("/api/admin/docs?limit=3").await;
    assert_eq!((out["total"].clone(), out["docs"].as_array().unwrap().len()), (json!(4), 3));
    let (_, faq) = f.get("/api/admin/docs?q=faq").await;
    let id = faq["docs"][0]["id"].as_i64().unwrap();
    assert_eq!(f.get("/api/admin/docs?offset=x").await.0, 422);
    assert_eq!(f.post("/api/admin/docs/remove", &json!({"id": id})).await.0, 200);
    assert_eq!(f.post("/api/admin/docs/remove", &json!({"id": 999_999})).await.0, 404);
    assert_eq!(f.post("/api/admin/clear", &json!({})).await.0, 422, "needs an explicit confirmation");
    assert_eq!(f.store.stats().unwrap().documents, 3);
    let (code, out) = f.post("/api/admin/clear", &json!({"confirm": "clear"})).await;
    assert_eq!((code, out["stats"]["documents"].clone(), out["job"]["result"].clone()), (200, json!(0), json!(null)));
    assert_eq!(f.get("/api/admin/docs?q=zzz").await.1, json!({"total": 0, "docs": []}));
    f.server.shutdown().await;
}

#[tokio::test]
async fn bad_bodies_are_422_not_500() {
    let f = fixture().await;
    for body in ["[1,2]", "not json", "\"str\""] {
        let r = f
            .http
            .post(format!("{}/api/admin/settings", f.base))
            .header("X-Intelli", "1")
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 422, "{body}");
    }
    let huge = f
        .http
        .post(format!("{}/api/admin/settings", f.base))
        .header("X-Intelli", "1")
        .body("x".repeat(100_000))
        .send()
        .await
        .unwrap();
    assert_eq!(huge.status(), 413, "bodies are capped");
    f.server.shutdown().await;
}

// ------------------------------------------------------------------------- AI answers against a fake model

#[derive(Default)]
struct FakeLlm {
    calls: AtomicU32,
    reply: std::sync::Mutex<String>,
    last: std::sync::Mutex<Option<(HeaderMap, Value)>>,
}

async fn fake_llm(reply: &str) -> (Arc<FakeLlm>, String) {
    let st = Arc::new(FakeLlm { reply: std::sync::Mutex::new(reply.into()), ..FakeLlm::default() });
    let app = axum::Router::new()
        .route(
            "/v1/chat/completions",
            post(
                |axum::extract::State(s): axum::extract::State<Arc<FakeLlm>>,
                 h: HeaderMap,
                 axum::Json(b): axum::Json<Value>| async move {
                    s.calls.fetch_add(1, Ordering::SeqCst);
                    *s.last.lock().unwrap() = Some((h.clone(), b));
                    if h["authorization"] == "Bearer wrong" {
                        return (axum::http::StatusCode::UNAUTHORIZED, axum::Json(json!({"error": "bad key"})));
                    }
                    let reply = s.reply.lock().unwrap().clone();
                    (axum::http::StatusCode::OK, axum::Json(json!({"choices": [{"message": {"content": reply}}]})))
                },
            ),
        )
        .with_state(st.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (st, format!("http://{addr}/v1"))
}

async fn configure(f: &Fixture, llm: &str, key: &str) {
    let (code, _) = f
        .post(
            "/api/admin/settings",
            &json!({"answers_enabled": true, "llm_base_url": llm, "llm_model": "test-model", "llm_api_key": key}),
        )
        .await;
    assert_eq!(code, 200);
}

#[tokio::test]
async fn answer_is_cited_checked_and_sends_the_right_request() {
    let f = fixture().await;
    let (llm, url) = fake_llm("Employees get 25 days of annual leave [1].").await;
    let (code, out) = f.post("/api/answer", &json!({"q": "leave"})).await;
    assert_eq!(code, 502, "answers are off by default");
    assert!(
        out["error"].as_str().unwrap().contains("turned off") && llm.calls.load(Ordering::SeqCst) == 0,
        "nothing was sent anywhere"
    );
    configure(&f, &url, "good").await;
    let (code, out) = f.post("/api/answer", &json!({"q": "annual leave days"})).await;
    assert_eq!(
        (code, out["text"].as_str(), out["warnings"].clone()),
        (200, Some("Employees get 25 days of annual leave [1]."), json!([]))
    );
    assert_eq!(out["sources"][0]["title"], "leave-policy");
    let (headers, body) = llm.last.lock().unwrap().clone().unwrap();
    assert_eq!(
        (headers["authorization"].to_str().unwrap(), body["model"].as_str(), body["temperature"].as_f64()),
        ("Bearer good", Some("test-model"), Some(0.0))
    );
    let user = body["messages"][1]["content"].as_str().unwrap();
    assert!(
        user.contains("[1] (leave-policy)") && user.contains("Question: annual leave days") && !user.contains("<mark>")
    );
    assert!(body["messages"][0]["content"].as_str().unwrap().contains("ONLY the numbered passages"));
    assert_eq!(f.post("/api/answer", &json!({"q": "  "})).await.0, 422);
    f.server.shutdown().await;
}

#[tokio::test]
async fn bad_citations_are_removed_and_no_hits_means_no_call() {
    let f = fixture().await;
    let (llm, url) = fake_llm("You get 25 days [1] and a bonus [7].").await;
    configure(&f, &url, "good").await;
    let (_, out) = f.post("/api/answer", &json!({"q": "annual leave"})).await;
    assert!(
        !out["text"].as_str().unwrap().contains("[7]") && out["warnings"][0].as_str().unwrap().contains("do not exist")
    );
    *llm.reply.lock().unwrap() = "Lots of leave.".into();
    assert!(
        f.post("/api/answer", &json!({"q": "annual leave"})).await.1["warnings"][0]
            .as_str()
            .unwrap()
            .contains("cites no source")
    );
    let before = llm.calls.load(Ordering::SeqCst);
    let (_, out) = f.post("/api/answer", &json!({"q": "zzzxqv nothing"})).await;
    assert!(out["text"].as_str().unwrap().contains("could not find") && llm.calls.load(Ordering::SeqCst) == before);
    f.server.shutdown().await;
}

#[tokio::test]
async fn model_errors_are_friendly_and_connection_test_works() {
    let f = fixture().await;
    let (_, url) = fake_llm("ok").await;
    assert_eq!(f.post("/api/admin/llm/test", &json!({})).await.0, 422, "nothing saved yet");
    configure(&f, &url, "good").await;
    let (_, out) = f.post("/api/admin/llm/test", &json!({})).await;
    assert_eq!((out["ok"].clone(), out["message"].as_str().unwrap().starts_with("Connected")), (json!(true), true));
    configure(&f, &url, "wrong").await;
    let (code, out) = f.post("/api/answer", &json!({"q": "annual leave"})).await;
    assert!(
        code == 502
            && out["error"].as_str().unwrap().contains("HTTP 401")
            && !out["error"].as_str().unwrap().contains("wrong")
    );
    let (_, t) = f.post("/api/admin/llm/test", &json!({})).await;
    assert_eq!(t["ok"], json!(false));
    configure(&f, "http://127.0.0.1:9/v1", "k").await;
    let (code, out) = f.post("/api/answer", &json!({"q": "annual leave"})).await;
    assert!(code == 502 && out["error"].as_str().unwrap().contains("Could not reach"));
    assert_eq!(f.get("/health").await.1["answers"], json!(true));
    f.server.shutdown().await;
}

// ------------------------------------------------------------------------- lifecycle

#[tokio::test]
async fn server_exits_after_the_tab_closes_and_gives_up_if_never_opened() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(LiteStore::open(Some(&dir.path().join("i.db")), None).unwrap());
    let opts = ServeOptions {
        port: 0,
        open_browser: false,
        exit_when_closed: true,
        idle_after_close: Duration::from_millis(600),
        never_opened: Duration::from_mins(1),
        ..ServeOptions::default()
    };
    let s = start(Arc::clone(&store), &opts).await.unwrap();
    let base = s.url();
    let with_token = |t: &str| {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("x-intelli-token", t.parse().unwrap());
        reqwest::Client::builder().default_headers(h).build().unwrap()
    };
    let client = with_token(s.token());
    for _ in 0..3 {
        client.get(format!("{base}/api/ping")).send().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert!(client.get(format!("{base}/health")).send().await.is_ok(), "alive while the page keeps pinging");
    tokio::time::timeout(Duration::from_secs(5), s.wait())
        .await
        .expect("the server stops by itself after pings stop")
        .unwrap();
    assert!(client.get(format!("{base}/health")).send().await.is_err());

    let opts = ServeOptions {
        port: 0,
        open_browser: false,
        exit_when_closed: true,
        never_opened: Duration::from_millis(600),
        ..ServeOptions::default()
    };
    let s = start(store, &opts).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), s.wait()).await.expect("never-opened server gives up").unwrap();
}

#[tokio::test]
async fn auto_reindex_on_start_picks_up_registered_folders() {
    let f = fixture().await;
    f.post("/api/admin/settings", &json!({"auto_reindex_on_start": true})).await;
    f.store.add_folder(&f.docs).unwrap();
    f.server.shutdown().await;
    std::fs::write(
        f.docs.join("later.md"),
        "A note added while the server was stopped, about quarterly budget planning.",
    )
    .unwrap();
    let again = start(Arc::clone(&f.store), &ServeOptions { port: 0, open_browser: false, ..ServeOptions::default() })
        .await
        .unwrap();
    let base = again.url();
    let http = {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("x-intelli-token", again.token().parse().unwrap());
        reqwest::Client::builder().default_headers(h).build().unwrap()
    };
    for _ in 0..200 {
        let st: Value = http.get(format!("{base}/api/admin/state")).send().await.unwrap().json().await.unwrap();
        if st["job"]["running"] == json!(false) && st["job"]["id"] == json!(1) {
            assert_eq!(st["stats"]["documents"], 5);
            again.shutdown().await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("auto re-index did not run");
}

#[test]
fn ui_assets_match_the_python_implementation_when_present() {
    let py = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../intelli_browser_os/lite");
    for page in ["home.html", "admin.html"] {
        let Ok(theirs) = std::fs::read_to_string(py.join(page)) else { continue };
        let ours = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join(page))
            .unwrap();
        assert_eq!(ours, theirs, "{page} drifted from the Python copy; both backends must serve the same UI");
    }
}

// ------------------------------------------------------------------------- access lists

#[tokio::test]
async fn restricted_documents_reach_neither_the_page_nor_the_model() {
    let f = fixture().await;
    let leave = f.docs.join("leave-policy.md").to_string_lossy().to_string();
    f.store.set_acl(&leave, &["hr"]).unwrap();

    // the page, acting as nobody in particular
    let (_, out) = f.get("/api/search?q=paid+annual+leave+days").await;
    let titles: Vec<&str> = out["results"].as_array().unwrap().iter().filter_map(|r| r["title"].as_str()).collect();
    assert!(!titles.contains(&"leave-policy"), "search leaked a restricted document: {titles:?}");
    let (_, docs) = f.get("/api/admin/docs?q=leave").await;
    assert!(docs["docs"].as_array().unwrap().iter().all(|d| !d["path"].as_str().unwrap().contains("leave-policy")));

    // the AI answer: the restricted text must never be sent to the model
    let (llm, url) = fake_llm("I could not find this in the indexed documents.").await;
    configure(&f, &url, "good").await;
    f.post("/api/answer", &json!({"q": "how many days of paid annual leave do employees get"})).await;
    if let Some((_, body)) = llm.last.lock().unwrap().clone() {
        let sent = body.to_string();
        assert!(
            !sent.contains("25 days of paid annual leave") && !sent.contains("carried over"),
            "restricted text was sent: {sent}"
        );
    }

    // the same store acting as the group that may see it
    let hr = start(
        Arc::clone(&f.store),
        &ServeOptions { port: 0, open_browser: false, principals: vec!["hr".into()], ..ServeOptions::default() },
    )
    .await
    .unwrap();
    let r: Value = f
        .http
        .get(format!("{}/api/search?q=paid+annual+leave+days", hr.url()))
        .header("x-intelli-token", hr.token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["results"][0]["title"], "leave-policy", "{r}");
    hr.shutdown().await;
    f.server.shutdown().await;
}

// ------------------------------------------------------------------------- launch token

#[tokio::test]
async fn nothing_works_without_the_launch_token() {
    let f = fixture().await;
    let bare = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    for path in ["/", "/admin", "/health", "/api/search?q=leave", "/api/admin/state", "/api/admin/docs", "/api/ping"] {
        let r = bare.get(format!("{}{path}", f.base)).send().await.unwrap();
        assert_eq!(r.status(), 403, "{path} answered without a token");
    }
    let r = bare
        .post(format!("{}/api/index", f.base))
        .header("X-Intelli", "1")
        .json(&json!({"path": "/"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403, "a POST with the CSRF header but no token is refused");
    let wrong =
        bare.get(format!("{}/api/admin/state", f.base)).header("x-intelli-token", "0".repeat(48)).send().await.unwrap();
    assert_eq!(wrong.status(), 403);
    let wrong_url = bare.get(format!("{}/?t={}", f.base, "0".repeat(48))).send().await.unwrap();
    assert_eq!(wrong_url.status(), 403, "a wrong token in the URL does not set a cookie");

    // the link that is opened in the browser: sets a cookie, redirects to the clean address, and the cookie then works
    let r = bare.get(f.server.open_url("/admin")).send().await.unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(r.headers()["location"], "/admin");
    let cookie = r.headers()["set-cookie"].to_str().unwrap().to_owned();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"), "{cookie}");
    let pair = cookie.split(';').next().unwrap();
    let ok = bare.get(format!("{}/admin", f.base)).header("cookie", pair).send().await.unwrap();
    assert_eq!(ok.status(), 200);
    let state = bare.get(format!("{}/api/admin/state", f.base)).header("cookie", pair).send().await.unwrap();
    assert_eq!(state.status(), 200);
    f.server.shutdown().await;
}
