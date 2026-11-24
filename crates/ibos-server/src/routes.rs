use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::extract::{Query, Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, COOKIE, HOST, LOCATION, SET_COOKIE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use ibos_search::SearchError;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{App, answer, job, picker};

const HOME: &str = include_str!("../assets/home.html");
const ADMIN: &str = include_str!("../assets/admin.html");
const CSP: &str = "default-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'";

type Api = Result<Json<Value>, ApiError>;

#[derive(Debug)]
pub(crate) struct ApiError(StatusCode, String);

impl ApiError {
    fn new(status: StatusCode, msg: impl Into<String>) -> Self {
        Self(status, msg.into())
    }

    fn invalid(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, msg)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

impl From<SearchError> for ApiError {
    fn from(e: SearchError) -> Self {
        match e {
            SearchError::NotFound(_) => Self::new(StatusCode::NOT_FOUND, "path not found"),
            SearchError::Settings(s) => Self::invalid(s.to_string()),
            other => {
                tracing::error!(error = %other, "store error");
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
        }
    }
}

pub(crate) fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/", get(|| async { Html(HOME) }))
        .route("/admin", get(|| async { Html(ADMIN) }))
        .route("/health", get(health))
        .route("/api/ping", get(ping))
        .route("/api/search", get(search))
        .route("/api/admin/state", get(admin_state_route))
        .route("/api/admin/docs", get(admin_docs))
        .route("/api/pick", post(pick))
        .route("/api/index", post(index))
        .route("/api/answer", post(answer_route))
        .route("/api/admin/settings", post(settings))
        .route("/api/admin/folders/add", post(folders_add))
        .route("/api/admin/folders/remove", post(folders_remove))
        .route("/api/admin/reindex", post(reindex))
        .route("/api/admin/docs/remove", post(docs_remove))
        .route("/api/admin/clear", post(clear))
        .route("/api/admin/llm/test", post(llm_test))
        .layer(axum::extract::DefaultBodyLimit::max(20_000))
        .layer(middleware::from_fn_with_state(Arc::clone(&app), guard))
        .with_state(app)
}

/// Host and header checks plus security headers on every response.
async fn guard(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let port = app.port.load(Ordering::Relaxed);
    let host = req.headers().get(HOST).and_then(|h| h.to_str().ok()).unwrap_or("").to_lowercase();
    let host_ok = host == format!("localhost:{port}") || host == format!("127.0.0.1:{port}");
    let is_post = req.method() == Method::POST;
    let mut resp = if !host_ok {
        ApiError::new(StatusCode::FORBIDDEN, if is_post { "forbidden" } else { "bad host" }).into_response()
    } else if let Some(redirect) = token_in_url(&app, &req) {
        // first visit with the launch token: remember it in a cookie and go to the clean address
        redirect
    } else if !has_token(&app, &req) {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden: open the link that intelli-rust printed, which includes the launch token",
        )
        .into_response()
    } else if is_post && req.headers().get("x-intelli").and_then(|v| v.to_str().ok()) != Some("1") {
        ApiError::new(StatusCode::FORBIDDEN, "forbidden").into_response()
    } else {
        next.run(req).await
    };
    let h = resp.headers_mut();
    h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    resp
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0_u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn cookie_name(app: &App) -> String {
    format!("ibos_token_{}", app.port.load(Ordering::Relaxed))
}

/// Does the request carry this launch's token, in the `X-Intelli-Token` header or the cookie set on the first visit?
fn has_token(app: &App, req: &Request) -> bool {
    if req.headers().get("x-intelli-token").and_then(|v| v.to_str().ok()).is_some_and(|t| same(t, &app.token)) {
        return true;
    }
    let name = cookie_name(app);
    req.headers()
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .any(|(k, v)| k == name && same(v, &app.token))
}

/// `GET /path?t=TOKEN`: a redirect to `/path` that sets the cookie. Anything else (including a wrong token): `None`.
fn token_in_url(app: &App, req: &Request) -> Option<Response> {
    if req.method() != Method::GET {
        return None;
    }
    let query = req.uri().query()?;
    let given = query.split('&').find_map(|kv| kv.strip_prefix("t="))?;
    if !same(given, &app.token) {
        return None;
    }
    let rest: Vec<&str> = query.split('&').filter(|kv| !kv.starts_with("t=")).collect();
    let target =
        if rest.is_empty() { req.uri().path().to_owned() } else { format!("{}?{}", req.uri().path(), rest.join("&")) };
    let cookie = format!("{}={}; HttpOnly; SameSite=Strict; Path=/", cookie_name(app), app.token);
    let mut resp = StatusCode::SEE_OTHER.into_response();
    resp.headers_mut().insert(LOCATION, HeaderValue::from_str(&target).ok()?);
    resp.headers_mut().insert(SET_COOKIE, HeaderValue::from_str(&cookie).ok()?);
    Some(resp)
}

/// Run store work off the async runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))
}

fn body_object(bytes: &Bytes) -> Result<Map<String, Value>, ApiError> {
    let v: Value = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(bytes).map_err(|_| ApiError::invalid("expected a JSON object"))?
    };
    match v {
        Value::Object(m) => Ok(m),
        _ => Err(ApiError::invalid("expected a JSON object")),
    }
}

fn required_str(m: &Map<String, Value>, key: &str) -> Result<String, ApiError> {
    m.get(key).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| ApiError::invalid(key.to_owned()))
}

/// Escape everything, then restore only our own `<mark>` tags, so indexed content can never inject HTML.
fn safe_snippet(s: &str) -> String {
    let escaped = s
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;");
    escaped.replace("&lt;mark&gt;", "<mark>").replace("&lt;/mark&gt;", "</mark>")
}

async fn admin_state(app: &Arc<App>) -> Result<Value, ApiError> {
    let a = Arc::clone(app);
    let (stats, folders, stale) =
        blocking(move || (a.store.stats(), a.store.folders(), a.store.needs_reindex())).await?;
    let folders: Vec<Value> =
        folders?.into_iter().map(|p| json!({"exists": Path::new(&p).exists(), "path": p})).collect();
    Ok(json!({
        "settings": app.store.settings().public(),
        "stats": stats?,
        "folders": folders,
        "needs_reindex": stale?,
        "job": app.job.snapshot(),
        "index_path": app.store.path().to_string_lossy(),
    }))
}

async fn health(State(app): State<Arc<App>>) -> Api {
    let a = Arc::clone(&app);
    let stats = blocking(move || a.store.stats()).await??;
    Ok(Json(
        json!({"status": "ok", "mode": "lite", "answers": app.store.settings().answers_enabled, "documents": stats.documents, "chunks": stats.chunks, "bytes": stats.bytes, "folders": stats.folders}),
    ))
}

async fn ping(State(app): State<Arc<App>>) -> Json<Value> {
    let ms = u64::try_from(app.started.elapsed().as_millis()).unwrap_or(u64::MAX).max(1);
    app.last_ping.store(ms, Ordering::Relaxed);
    Json(json!({"ok": true}))
}

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
    limit: Option<String>,
}

async fn search(State(app): State<Arc<App>>, Query(p): Query<SearchParams>) -> Api {
    let q: String = p.q.unwrap_or_default().chars().take(500).collect();
    let limit = p.limit.and_then(|l| l.parse::<usize>().ok());
    let (a, q2) = (Arc::clone(&app), q.clone());
    let hits = blocking(move || a.store.search_as(&q2, limit, None, &a.access)).await??;
    let results: Vec<Value> = hits.into_iter().map(|h| json!({"title": h.title, "path": h.path, "page": h.page, "snippet": safe_snippet(&h.snippet), "score": h.score, "doc_id": h.doc_id})).collect();
    Ok(Json(json!({"query": q, "results": results})))
}

async fn admin_state_route(State(app): State<Arc<App>>) -> Api {
    Ok(Json(admin_state(&app).await?))
}

#[derive(Deserialize)]
struct DocsParams {
    q: Option<String>,
    offset: Option<String>,
    limit: Option<String>,
}

async fn admin_docs(State(app): State<Arc<App>>, Query(p): Query<DocsParams>) -> Api {
    let num = |v: Option<String>, d: usize| {
        v.map_or(Ok(d), |s| s.parse::<usize>().map_err(|_| ApiError::invalid("offset and limit must be numbers")))
    };
    let (offset, limit) = (num(p.offset, 0)?, num(p.limit, 50)?);
    let q: String = p.q.unwrap_or_default().chars().take(200).collect();
    let (docs, total) = blocking(move || app.store.documents_as(&q, offset, limit, &app.access)).await??;
    Ok(Json(json!({"total": total, "docs": docs})))
}

async fn pick() -> Api {
    let path = tokio::task::spawn_blocking(picker::pick_folder).await.ok().flatten();
    Ok(Json(json!({"path": path})))
}

async fn index(State(app): State<Arc<App>>, bytes: Bytes) -> Api {
    let path = PathBuf::from(required_str(&body_object(&bytes)?, "path")?);
    let stats = blocking(move || {
        let expanded = expand_user(&path);
        if expanded.is_dir() {
            app.store.add_folder(&expanded)?; // remembered, so Admin > Re-index covers it
        }
        app.store.index_path(&expanded, ibos_search::IndexOptions::default(), None)
    })
    .await??;
    Ok(Json(serde_json::to_value(stats).unwrap_or_default()))
}

fn expand_user(p: &Path) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix("~/")) {
        Some(rest) => std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map_or_else(|| p.to_path_buf(), |h| PathBuf::from(h).join(rest)),
        None => p.to_path_buf(),
    }
}

async fn answer_route(State(app): State<Arc<App>>, bytes: Bytes) -> Api {
    let q: String =
        body_object(&bytes)?.get("q").and_then(Value::as_str).unwrap_or("").trim().chars().take(500).collect();
    if q.is_empty() {
        return Err(ApiError::invalid("Ask a question"));
    }
    let cfg = app.store.settings();
    let n = usize::try_from(cfg.answer_passages).unwrap_or(5);
    let (a, q2) = (Arc::clone(&app), q.clone());
    let hits = blocking(move || a.store.search_as(&q2, Some(n), Some(true), &a.access)).await??;
    match answer::ask(&cfg, &q, &hits).await {
        Ok(a) => Ok(Json(json!({"text": a.text, "sources": a.sources, "warnings": a.warnings}))),
        Err(e) => Err(ApiError::new(StatusCode::BAD_GATEWAY, e)),
    }
}

async fn settings(State(app): State<Arc<App>>, bytes: Bytes) -> Api {
    let mut changes = body_object(&bytes)?;
    let clear = changes.remove("clear_api_key").and_then(|v| v.as_bool()).unwrap_or(false);
    let a = Arc::clone(&app);
    blocking(move || a.store.settings_store().update(&changes, clear))
        .await?
        .map_err(|e| ApiError::invalid(e.to_string()))?;
    Ok(Json(admin_state(&app).await?))
}

fn with_state(state: Value, extra: Value) -> Json<Value> {
    let (Value::Object(mut s), Value::Object(e)) = (state, extra) else { return Json(json!({})) };
    s.extend(e);
    Json(Value::Object(s))
}

async fn folders_add(State(app): State<Arc<App>>, bytes: Bytes) -> Result<Response, ApiError> {
    let path = PathBuf::from(required_str(&body_object(&bytes)?, "path")?);
    let a = Arc::clone(&app);
    let added = blocking(move || a.store.add_folder(&expand_user(&path))).await??;
    let started = job::start_index(&app, PathBuf::from(added));
    let state = admin_state(&app).await?;
    Ok(if started {
        Json(state).into_response()
    } else {
        (StatusCode::CONFLICT, with_state(state, json!({"error": "Another job is running"}))).into_response()
    })
}

async fn folders_remove(State(app): State<Arc<App>>, bytes: Bytes) -> Api {
    let path = PathBuf::from(required_str(&body_object(&bytes)?, "path")?);
    let a = Arc::clone(&app);
    let dropped = blocking(move || a.store.remove_folder(&expand_user(&path), true)).await??;
    Ok(with_state(admin_state(&app).await?, json!({"dropped": dropped})))
}

async fn reindex(State(app): State<Arc<App>>, bytes: Bytes) -> Result<Response, ApiError> {
    let force = body_object(&bytes)?.get("force").and_then(Value::as_bool).unwrap_or(false);
    let started = job::start_reindex(&app, force);
    let state = admin_state(&app).await?;
    Ok(if started {
        Json(state).into_response()
    } else {
        (StatusCode::CONFLICT, with_state(state, json!({"error": "Another job is running"}))).into_response()
    })
}

async fn docs_remove(State(app): State<Arc<App>>, bytes: Bytes) -> Result<Response, ApiError> {
    let id = body_object(&bytes)?.get("id").and_then(Value::as_i64).ok_or_else(|| ApiError::invalid("id"))?;
    let a = Arc::clone(&app);
    let removed = blocking(move || a.store.remove_id(id)).await??;
    let state = admin_state(&app).await?;
    Ok((if removed { StatusCode::OK } else { StatusCode::NOT_FOUND }, Json(state)).into_response())
}

async fn clear(State(app): State<Arc<App>>, bytes: Bytes) -> Api {
    if body_object(&bytes)?.get("confirm").and_then(Value::as_str) != Some("clear") {
        return Err(ApiError::invalid("Send {\"confirm\": \"clear\"} to empty the index"));
    }
    let a = Arc::clone(&app);
    blocking(move || a.store.clear()).await??;
    app.job.clear_result(); // the old "Done: 10 indexed" banner would be misleading now
    Ok(Json(admin_state(&app).await?))
}

async fn llm_test(State(app): State<Arc<App>>) -> Api {
    let cfg = app.store.settings();
    if cfg.llm_base_url.is_empty() || cfg.llm_model.is_empty() {
        return Err(ApiError::invalid("Save an endpoint and model first"));
    }
    Ok(Json(match answer::test_connection(&cfg).await {
        Ok(message) => json!({"ok": true, "message": message}),
        Err(message) => json!({"ok": false, "message": message}),
    }))
}

#[cfg(test)]
mod tests {
    use super::safe_snippet;

    #[test]
    fn snippets_cannot_inject_html_but_keep_marks() {
        assert_eq!(
            safe_snippet("<b>x</b> <mark>hit</mark> & \"q\" 'a'"),
            "&lt;b&gt;x&lt;/b&gt; <mark>hit</mark> &amp; &quot;q&quot; &#x27;a&#x27;"
        );
        assert!(!safe_snippet("<img src=x onerror=alert(1)>").contains("<img"));
    }
}
