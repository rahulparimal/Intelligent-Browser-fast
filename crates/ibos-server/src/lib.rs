//! Local web server: search home page, admin pages, optional AI answers.
//!
//! Binds to `127.0.0.1` only. Rejects foreign `Host` headers (DNS rebinding) and state-changing requests without the custom
//! `X-Intelli` header (cross-site form posts). The API key is never sent to the page. The HTML pages are the same files the
//! Python implementation serves, so both backends share one UI.

mod answer;
mod job;
mod picker;
mod routes;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ibos_search::LiteStore;
use tokio::net::TcpListener;
use tokio::sync::watch;

pub use answer::{Answer, SYSTEM_PROMPT, check_citations};

const IDLE_AFTER_CLOSE: Duration = Duration::from_secs(20);
const NEVER_OPENED: Duration = Duration::from_mins(3);

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("could not bind 127.0.0.1:{0}: {1}")]
    Bind(u16, std::io::Error),
    #[error("server error: {0}")]
    Serve(#[from] std::io::Error),
}

/// How to run the server.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// `0` picks a free port.
    pub port: u16,
    pub open_browser: bool,
    /// Stop shortly after the browser tab is closed (the page pings every few seconds). For the desktop launcher.
    pub exit_when_closed: bool,
    /// Page to open: `/` (search) or `/admin`.
    pub path: String,
    /// Timings, overridable in tests.
    pub idle_after_close: Duration,
    pub never_opened: Duration,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            port: 8765,
            open_browser: true,
            exit_when_closed: false,
            path: "/".into(),
            idle_after_close: IDLE_AFTER_CLOSE,
            never_opened: NEVER_OPENED,
        }
    }
}

pub(crate) struct App {
    pub(crate) store: Arc<LiteStore>,
    pub(crate) job: job::Job,
    pub(crate) port: AtomicU64,
    /// Milliseconds since `started` of the last page heartbeat; `0` = never.
    pub(crate) last_ping: AtomicU64,
    pub(crate) started: Instant,
}

/// A running server, for embedding and tests.
#[derive(Debug)]
pub struct Running {
    pub addr: SocketAddr,
    shutdown: watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), std::io::Error>>,
}

impl Running {
    pub fn url(&self) -> String {
        format!("http://localhost:{}", self.addr.port())
    }

    /// Stop accepting connections and wait for in-flight requests.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }

    /// Wait until the server stops by itself (idle shutdown) or fails.
    pub async fn wait(self) -> Result<(), ServerError> {
        self.task.await.map_err(|e| ServerError::Serve(std::io::Error::other(e)))?.map_err(ServerError::Serve)
    }
}

/// Bind and start serving in the background.
pub async fn start(store: Arc<LiteStore>, opts: &ServeOptions) -> Result<Running, ServerError> {
    let listener = TcpListener::bind(("127.0.0.1", opts.port)).await.map_err(|e| ServerError::Bind(opts.port, e))?;
    let addr = listener.local_addr()?;
    let app = Arc::new(App {
        store,
        job: job::Job::default(),
        port: AtomicU64::new(u64::from(addr.port())),
        last_ping: AtomicU64::new(0),
        started: Instant::now(),
    });
    if app.store.settings().auto_reindex_on_start && app.store.folders().is_ok_and(|f| !f.is_empty()) {
        job::start_reindex(&app, false);
    }
    let (tx, mut rx) = watch::channel(false);
    if opts.exit_when_closed {
        tokio::spawn(watch_idle(Arc::clone(&app), tx.clone(), opts.idle_after_close, opts.never_opened));
    }
    let router = routes::router(Arc::clone(&app));
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = rx.wait_for(|stop| *stop).await;
            })
            .await
    });
    let running = Running { addr, shutdown: tx, task };
    if opts.open_browser {
        let url = format!("{}{}", running.url(), opts.path);
        if let Err(e) = webbrowser::open(&url) {
            tracing::warn!(error = %e, "could not open a browser; open {url} yourself");
        }
    }
    Ok(running)
}

async fn watch_idle(app: Arc<App>, stop: watch::Sender<bool>, idle: Duration, never: Duration) {
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let elapsed = app.started.elapsed();
        let last = app.last_ping.load(Ordering::Relaxed);
        let silent_for = elapsed.saturating_sub(Duration::from_millis(last));
        if (last > 0 && silent_for > idle) || (last == 0 && elapsed > never) {
            let _ = stop.send(true);
            return;
        }
    }
}
