//! WebSocket CDP client. One reader task dispatches responses to waiting callers and events to subscribers; one writer task
//! owns the socket's write half. Everything is bounded and every call has a timeout.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::error::CdpError;

type Reply = Result<Box<RawValue>, CdpError>;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;

/// A protocol event (a message without an `id`).
#[derive(Debug, Clone)]
pub struct Event {
    pub method: String,
    pub session_id: Option<String>,
    /// Raw JSON of `params`.
    pub params: String,
}

#[derive(Deserialize)]
struct ProtoError {
    code: i64,
    message: String,
}

#[derive(Deserialize)]
struct Incoming<'a> {
    id: Option<u64>,
    #[serde(borrow)]
    result: Option<&'a RawValue>,
    error: Option<ProtoError>,
    method: Option<String>,
    #[serde(borrow)]
    params: Option<&'a RawValue>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

pub struct Conn {
    tx: mpsc::Sender<String>,
    pending: Pending,
    next_id: AtomicU64,
    events: broadcast::Sender<Event>,
    closed: Arc<AtomicBool>,
    timeout: Duration,
    tasks: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conn").field("timeout", &self.timeout).finish_non_exhaustive()
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Conn {
    /// Connect to a `ws://` DevTools endpoint. `timeout` is the default for every call.
    pub async fn connect(url: &str, timeout: Duration) -> Result<Self, CdpError> {
        let config = WebSocketConfig::default().max_message_size(Some(256 << 20)).max_frame_size(Some(64 << 20));
        let (ws, _) =
            tokio::time::timeout(timeout, tokio_tungstenite::connect_async_with_config(url, Some(config), false))
                .await
                .map_err(|_| CdpError::Timeout(timeout, "the DevTools connection".into()))?
                .map_err(|e| CdpError::Connect(e.to_string()))?;
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::channel::<String>(64);
        let (events, _) = broadcast::channel(1024);
        let pending: Pending = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));

        let writer = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if sink.send(Message::text(msg)).await.is_err() {
                    break;
                }
            }
        });
        let (p, ev, flag) = (Arc::clone(&pending), events.clone(), Arc::clone(&closed));
        let reader = tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                let Message::Text(text) = msg else {
                    if matches!(msg, Message::Close(_)) {
                        break;
                    }
                    continue;
                };
                let Ok(inc) = serde_json::from_str::<Incoming<'_>>(&text) else { continue };
                match (inc.id, inc.method) {
                    (Some(id), _) => {
                        let reply = match (inc.error, inc.result) {
                            (Some(e), _) => {
                                Err(CdpError::Protocol { method: String::new(), code: e.code, message: e.message })
                            }
                            (None, Some(r)) => Ok(r.to_owned()),
                            (None, None) => {
                                Err(CdpError::Format { method: String::new(), detail: "empty response".into() })
                            }
                        };
                        if let Some(waiter) = lock(&p).remove(&id) {
                            let _ = waiter.send(reply);
                        }
                    }
                    (None, Some(method)) => {
                        let _ = ev.send(Event {
                            method,
                            session_id: inc.session_id,
                            params: inc.params.map_or_else(|| "{}".into(), |r| r.get().to_owned()),
                        });
                    }
                    (None, None) => {}
                }
            }
            flag.store(true, Ordering::SeqCst); // set before draining: later calls see it, earlier ones are drained below
            for (_, waiter) in lock(&p).drain() {
                let _ = waiter.send(Err(CdpError::Closed)); // the socket is gone: nobody waits forever
            }
        });
        Ok(Self { tx, pending, next_id: AtomicU64::new(1), events, closed, timeout, tasks: vec![writer, reader] })
    }

    /// Subscribe to events. A slow subscriber loses old events rather than growing memory.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Call `method` and return the raw JSON `result`, with the default timeout.
    pub async fn call_raw(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Box<RawValue>, CdpError> {
        self.call_raw_with(method, params, session, self.timeout).await
    }

    pub async fn call_raw_with(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<Box<RawValue>, CdpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (otx, orx) = oneshot::channel();
        lock(&self.pending).insert(id, otx);
        if self.closed.load(Ordering::SeqCst) {
            lock(&self.pending).remove(&id); // registered after the reader drained the table: nobody would ever answer
            return Err(CdpError::Closed);
        }
        let mut msg = serde_json::json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            msg["sessionId"] = Value::String(s.to_owned());
        }
        if self.tx.send(msg.to_string()).await.is_err() {
            lock(&self.pending).remove(&id);
            return Err(CdpError::Closed);
        }
        match tokio::time::timeout(timeout, orx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(CdpError::Protocol { code, message, .. }))) => {
                Err(CdpError::Protocol { method: method.to_owned(), code, message })
            }
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(_)) => Err(CdpError::Closed),
            Err(_) => {
                lock(&self.pending).remove(&id); // do not leak the waiter of a call nobody will answer
                Err(CdpError::Timeout(timeout, method.to_owned()))
            }
        }
    }

    /// Call `method` and parse `result` as a JSON value.
    pub async fn call(&self, method: &str, params: Value, session: Option<&str>) -> Result<Value, CdpError> {
        let raw = self.call_raw(method, params, session).await?;
        serde_json::from_str(raw.get())
            .map_err(|e| CdpError::Format { method: method.to_owned(), detail: e.to_string() })
    }

    /// Number of calls waiting for a reply (for tests and diagnostics).
    pub fn pending_calls(&self) -> usize {
        lock(&self.pending).len()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::net::TcpListener;

    use super::*;

    /// A fake DevTools endpoint. `script` receives each request and returns the frames to send back.
    async fn fake(script: impl Fn(Value) -> Vec<Value> + Send + Sync + 'static) -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            let (s, _) = l.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
            while let Some(Ok(Message::Text(t))) = ws.next().await {
                for frame in script(serde_json::from_str(&t).unwrap()) {
                    ws.send(Message::text(frame.to_string())).await.unwrap();
                }
            }
        });
        url
    }

    #[tokio::test]
    async fn correlates_responses_and_forwards_session_ids() {
        let url =
            fake(|req| vec![json!({"id": req["id"], "result": {"echo": req["method"], "session": req["sessionId"]}})])
                .await;
        let c = Conn::connect(&url, Duration::from_secs(5)).await.unwrap();
        let (a, b) = tokio::join!(c.call("A.one", json!({}), None), c.call("B.two", json!({}), Some("sess-1")));
        assert_eq!(a.unwrap(), json!({"echo": "A.one", "session": null}));
        assert_eq!(b.unwrap(), json!({"echo": "B.two", "session": "sess-1"}));
        assert_eq!(c.pending_calls(), 0);
    }

    #[tokio::test]
    async fn protocol_errors_carry_the_method() {
        let url =
            fake(|req| vec![json!({"id": req["id"], "error": {"code": -32000, "message": "No node with given id"}})])
                .await;
        let c = Conn::connect(&url, Duration::from_secs(5)).await.unwrap();
        let e = c.call("DOM.getBoxModel", json!({}), None).await.unwrap_err();
        assert!(matches!(&e, CdpError::Protocol { method, code: -32000, .. } if method == "DOM.getBoxModel"), "{e}");
    }

    #[tokio::test]
    async fn a_silent_socket_times_out_and_does_not_leak_the_waiter() {
        let url = fake(|_| vec![]).await;
        let c = Conn::connect(&url, Duration::from_millis(150)).await.unwrap();
        let e = c.call("Never.answers", json!({}), None).await.unwrap_err();
        assert!(matches!(e, CdpError::Timeout(_, ref m) if m == "Never.answers"));
        assert_eq!(c.pending_calls(), 0, "the abandoned call is removed");
    }

    #[tokio::test]
    async fn events_are_broadcast_and_unrelated_frames_ignored() {
        let url = fake(|req| {
            vec![
                json!({"method": "Page.loadEventFired", "params": {"timestamp": 1.5}, "sessionId": "s"}),
                json!("not an object"),
                json!({"id": req["id"], "result": {}}),
            ]
        })
        .await;
        let c = Conn::connect(&url, Duration::from_secs(5)).await.unwrap();
        let mut sub = c.subscribe();
        c.call("Page.enable", json!({}), None).await.unwrap();
        let ev = tokio::time::timeout(Duration::from_secs(2), sub.recv()).await.unwrap().unwrap();
        assert_eq!((ev.method.as_str(), ev.session_id.as_deref()), ("Page.loadEventFired", Some("s")));
        assert!(ev.params.contains("1.5"));
    }

    #[tokio::test]
    async fn closing_the_socket_fails_waiting_calls_immediately() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            let (s, _) = l.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
            let _ = ws.next().await; // read one request, then hang up without replying
            let _ = ws.close(None).await;
        });
        let c = Conn::connect(&url, Duration::from_secs(10)).await.unwrap();
        let t0 = std::time::Instant::now();
        let e = c.call("X.y", json!({}), None).await.unwrap_err();
        assert!(matches!(e, CdpError::Closed), "{e}");
        assert!(t0.elapsed() < Duration::from_secs(3), "fails fast, not after the 10 s timeout");
        assert!(matches!(c.call("X.z", json!({}), None).await, Err(CdpError::Closed)));
    }

    #[tokio::test]
    async fn connecting_to_nothing_is_an_error() {
        let e = Conn::connect("ws://127.0.0.1:9", Duration::from_secs(2)).await.unwrap_err();
        assert!(matches!(e, CdpError::Connect(_) | CdpError::Timeout(..)));
    }
}
