use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum CdpError {
    #[error("no Chrome, Chromium or Edge found; install one or set INTELLI_BROWSER to its path")]
    NoBrowser,
    #[error("could not start the browser: {0}")]
    Launch(String),
    #[error("the browser did not report a DevTools address within {0:?}")]
    LaunchTimeout(Duration),
    #[error("could not connect to the browser: {0}")]
    Connect(String),
    #[error("the browser connection closed")]
    Closed,
    #[error("timed out after {0:?} waiting for {1}")]
    Timeout(Duration, String),
    #[error("{method} failed: {message} (code {code})")]
    Protocol { method: String, code: i64, message: String },
    #[error("unexpected response to {method}: {detail}")]
    Format { method: String, detail: String },
    #[error("navigation failed: {0}")]
    Navigation(String),
}
