//! A tab: navigation, observation, and actions addressed by backend node id.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::conn::Conn;
use crate::error::CdpError;

/// Decides whether a request to a URL may go out.
pub type RequestFilter = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub width: f64,
    pub height: f64,
}

/// Raw material for the distiller: the accessibility tree and layout rectangles, as the browser sent them (not parsed here, so
/// the parser can borrow from them without copying).
#[derive(Debug)]
pub struct RawObservation {
    pub url: String,
    pub title: String,
    /// `Accessibility.getFullAXTree` result.
    pub ax: Box<RawValue>,
    /// `DOMSnapshot.captureSnapshot` result.
    pub dom: Box<RawValue>,
    pub viewport: Viewport,
}

#[derive(Clone)]
pub struct Page {
    conn: Arc<Conn>,
    session: String,
    target: String,
    nav_timeout: Duration,
}

impl std::fmt::Debug for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Page").field("target", &self.target).finish_non_exhaustive()
    }
}

fn fmt_err(method: &str, detail: impl Into<String>) -> CdpError {
    CdpError::Format { method: method.to_owned(), detail: detail.into() }
}

impl Page {
    pub(crate) fn new(conn: Arc<Conn>, session: String, target: String) -> Self {
        Self { conn, session, target, nav_timeout: Duration::from_secs(20) }
    }

    pub(crate) async fn setup(&self, window: (u32, u32)) -> Result<(), CdpError> {
        for m in ["Page.enable", "DOM.enable", "Accessibility.enable"] {
            self.call(m, json!({})).await?;
        }
        self.call(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": window.0, "height": window.1, "deviceScaleFactor": 1, "mobile": false}),
        )
        .await?;
        Ok(())
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, CdpError> {
        self.conn.call(method, params, Some(&self.session)).await
    }

    #[must_use]
    pub fn with_navigation_timeout(mut self, t: Duration) -> Self {
        self.nav_timeout = t;
        self
    }

    /// Check every network request this tab makes (page, scripts, images, `fetch`, frames) against `allow`, and block those it
    /// refuses before they leave the browser. Returns once interception is active, so nothing slips through between the call
    /// and the first navigation.
    ///
    /// Not covered: requests made by other targets (cross-origin frames in their own process, workers), and `WebSockets`.
    pub async fn restrict_requests(&self, allow: RequestFilter) -> Result<(), CdpError> {
        let mut events = self.conn.subscribe();
        self.call("Fetch.enable", json!({"patterns": [{"urlPattern": "*"}]})).await?;
        let (conn, session) = (Arc::clone(&self.conn), self.session.clone());
        tokio::spawn(async move {
            loop {
                let ev = match events.recv().await {
                    Ok(ev) => ev,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(missed = n, "request filter fell behind; some requests may stay paused");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if ev.method != "Fetch.requestPaused" || ev.session_id.as_deref() != Some(session.as_str()) {
                    continue;
                }
                let Ok(p) = serde_json::from_str::<Value>(&ev.params) else { continue };
                let (Some(id), Some(url)) = (p["requestId"].as_str(), p["request"]["url"].as_str()) else { continue };
                let _ = if allow(url) {
                    conn.call("Fetch.continueRequest", json!({"requestId": id}), Some(&session)).await
                } else {
                    tracing::info!(%url, "blocked a request to a site that is not allowed");
                    conn.call(
                        "Fetch.failRequest",
                        json!({"requestId": id, "errorReason": "BlockedByClient"}),
                        Some(&session),
                    )
                    .await
                };
            }
        });
        Ok(())
    }

    /// Evaluate a JavaScript expression and return its JSON value.
    pub async fn evaluate(&self, expression: &str) -> Result<Value, CdpError> {
        let r = self
            .call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true, "awaitPromise": true}))
            .await?;
        if let Some(ex) = r.get("exceptionDetails") {
            return Err(fmt_err("Runtime.evaluate", ex["text"].as_str().unwrap_or("exception")));
        }
        Ok(r["result"]["value"].clone())
    }

    pub async fn url(&self) -> Result<String, CdpError> {
        Ok(self.evaluate("location.href").await?.as_str().unwrap_or_default().to_owned())
    }

    pub async fn title(&self) -> Result<String, CdpError> {
        Ok(self.evaluate("document.title").await?.as_str().unwrap_or_default().to_owned())
    }

    /// Navigate and wait until the document is complete (or the navigation timeout passes).
    pub async fn navigate(&self, url: &str) -> Result<(), CdpError> {
        let r = self.call("Page.navigate", json!({"url": url})).await?;
        if let Some(err) = r["errorText"].as_str().filter(|e| !e.is_empty()) {
            return Err(CdpError::Navigation(format!("{url}: {err}")));
        }
        self.wait_ready(self.nav_timeout).await
    }

    /// Wait for `document.readyState == "complete"`. The deadline covers the whole wait, including a page that is so busy
    /// (an endless script) that even a single poll would not return.
    pub async fn wait_ready(&self, timeout: Duration) -> Result<(), CdpError> {
        let poll = async {
            loop {
                if self.evaluate("document.readyState").await.is_ok_and(|v| v == "complete") {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
        };
        tokio::time::timeout(timeout, poll)
            .await
            .map_err(|_| CdpError::Timeout(timeout, "the page to finish loading".into()))
    }

    pub async fn viewport(&self) -> Result<Viewport, CdpError> {
        let m = self.call("Page.getLayoutMetrics", json!({})).await?;
        let v = &m["cssVisualViewport"];
        Ok(Viewport {
            width: v["clientWidth"].as_f64().unwrap_or(1280.0),
            height: v["clientHeight"].as_f64().unwrap_or(800.0),
        })
    }

    /// Fetch what the distiller needs: accessibility tree, layout rectangles, URL, title and viewport.
    pub async fn observe_raw(&self) -> Result<RawObservation, CdpError> {
        let s = Some(self.session.as_str());
        let (ax, dom, viewport, url, title) = tokio::try_join!(
            self.conn.call_raw("Accessibility.getFullAXTree", json!({}), s),
            self.conn.call_raw(
                "DOMSnapshot.captureSnapshot",
                json!({"computedStyles": [], "includeDOMRects": true}),
                s
            ),
            self.viewport(),
            self.url(),
            self.title(),
        )?;
        Ok(RawObservation { url, title, ax, dom, viewport })
    }

    /// The stable backend node id of the first element matching a CSS selector, if any.
    pub async fn backend_id_for_selector(&self, selector: &str) -> Result<Option<i64>, CdpError> {
        let doc = self.call("DOM.getDocument", json!({"depth": 0})).await?;
        let root = doc["root"]["nodeId"].as_i64().ok_or_else(|| fmt_err("DOM.getDocument", "no root"))?;
        let found = self.call("DOM.querySelector", json!({"nodeId": root, "selector": selector})).await?;
        let node = found["nodeId"].as_i64().unwrap_or(0);
        if node == 0 {
            return Ok(None);
        }
        let d = self.call("DOM.describeNode", json!({"nodeId": node})).await?;
        Ok(d["node"]["backendNodeId"].as_i64())
    }

    async fn center_of(&self, backend_id: i64) -> Result<(f64, f64), CdpError> {
        let _ = self.call("DOM.scrollIntoViewIfNeeded", json!({"backendNodeId": backend_id})).await;
        let m = self.call("DOM.getBoxModel", json!({"backendNodeId": backend_id})).await?;
        let q: Vec<f64> = m["model"]["content"]
            .as_array()
            .ok_or_else(|| fmt_err("DOM.getBoxModel", "no content quad"))?
            .iter()
            .filter_map(Value::as_f64)
            .collect();
        if q.len() < 8 {
            return Err(fmt_err("DOM.getBoxModel", "malformed quad"));
        }
        Ok(((q[0] + q[2] + q[4] + q[6]) / 4.0, (q[1] + q[3] + q[5] + q[7]) / 4.0))
    }

    /// Click the centre of the element, like a user.
    pub async fn click(&self, backend_id: i64) -> Result<(), CdpError> {
        let (x, y) = self.center_of(backend_id).await?;
        for (kind, buttons) in [("mouseMoved", 0), ("mousePressed", 1), ("mouseReleased", 0)] {
            self.call("Input.dispatchMouseEvent", json!({"type": kind, "x": x, "y": y, "button": if kind == "mouseMoved" { "none" } else { "left" }, "buttons": buttons, "clickCount": 1})).await?;
        }
        Ok(())
    }

    /// Focus the element and type `text` (replacing the current value when `clear`).
    pub async fn type_text(&self, backend_id: i64, text: &str, clear: bool) -> Result<(), CdpError> {
        let obj = self.call("DOM.resolveNode", json!({"backendNodeId": backend_id})).await?;
        let object_id =
            obj["object"]["objectId"].as_str().ok_or_else(|| fmt_err("DOM.resolveNode", "no objectId"))?.to_owned();
        let js = if clear {
            "function(){ this.focus(); if (typeof this.select === 'function') this.select(); }"
        } else {
            "function(){ this.focus(); }"
        };
        self.call("Runtime.callFunctionOn", json!({"objectId": object_id, "functionDeclaration": js})).await?;
        self.call("Input.insertText", json!({"text": text})).await?;
        Ok(())
    }

    /// Scroll the page vertically by `dy` CSS pixels.
    pub async fn scroll(&self, dy: i64) -> Result<(), CdpError> {
        self.evaluate(&format!("window.scrollBy(0, {dy})")).await.map(|_| ())
    }

    /// JPEG screenshot of the viewport.
    pub async fn screenshot_jpeg(&self, quality: u8) -> Result<Vec<u8>, CdpError> {
        let r =
            self.call("Page.captureScreenshot", json!({"format": "jpeg", "quality": quality.clamp(1, 100)})).await?;
        let b64 = r["data"].as_str().ok_or_else(|| fmt_err("Page.captureScreenshot", "no data"))?;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| fmt_err("Page.captureScreenshot", e.to_string()))
    }

    /// Close the tab.
    pub async fn close(&self) {
        let _ = self.conn.call("Target.closeTarget", json!({"targetId": self.target}), None).await;
    }
}
