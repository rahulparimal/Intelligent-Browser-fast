//! Launching and supervising the browser process tree.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use command_group::{AsyncCommandGroup, AsyncGroupChild};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::chrome::{find_browser, needs_no_sandbox, parse_devtools_url};
use crate::conn::Conn;
use crate::error::CdpError;
use crate::page::Page;

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub headless: bool,
    /// Browser executable; found automatically if `None`.
    pub executable: Option<PathBuf>,
    pub extra_args: Vec<String>,
    /// Window size in CSS pixels.
    pub window: (u32, u32),
    pub launch_timeout: Duration,
    /// Default timeout for protocol calls.
    pub call_timeout: Duration,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            headless: true,
            executable: None,
            extra_args: Vec::new(),
            window: (1280, 800),
            launch_timeout: Duration::from_secs(20),
            call_timeout: Duration::from_secs(30),
        }
    }
}

/// A running browser. The whole process tree is killed when this is dropped.
pub struct Browser {
    conn: Arc<Conn>,
    child: AsyncGroupChild,
    pid: u32,
    window: (u32, u32),
    profile: tempfile::TempDir,
}

impl std::fmt::Debug for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Browser").field("pid", &self.pid).finish_non_exhaustive()
    }
}

impl Browser {
    /// Start a browser with a throwaway profile and connect to it.
    pub async fn launch(opts: LaunchOptions) -> Result<Self, CdpError> {
        let exe = opts.executable.clone().or_else(find_browser).ok_or(CdpError::NoBrowser)?;
        let profile = tempfile::tempdir().map_err(|e| CdpError::Launch(e.to_string()))?;
        let mut cmd = Command::new(&exe);
        cmd.arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "--disable-extensions",
                "--disable-sync",
                "--disable-default-apps",
                "--mute-audio",
                "--hide-scrollbars",
            ])
            .arg(format!("--window-size={},{}", opts.window.0, opts.window.1));
        if opts.headless {
            cmd.args(["--headless=new", "--disable-gpu"]);
        }
        if needs_no_sandbox() {
            cmd.arg("--no-sandbox");
        }
        cmd.args(&opts.extra_args)
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.group_spawn().map_err(|e| CdpError::Launch(format!("{}: {e}", exe.display())))?;
        let pid = child.id().ok_or_else(|| CdpError::Launch("the browser exited immediately".into()))?;
        let stderr = child.inner().stderr.take().ok_or_else(|| CdpError::Launch("no stderr".into()))?;
        let mut lines = BufReader::new(stderr).lines();
        let url = tokio::time::timeout(opts.launch_timeout, async {
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(u) = parse_devtools_url(&line) {
                    return Some(u);
                }
            }
            None
        })
        .await;
        let url = match url {
            Ok(Some(u)) => u,
            Ok(None) => {
                let _ = child.start_kill();
                return Err(CdpError::Launch("the browser exited before reporting a DevTools address".into()));
            }
            Err(_) => {
                let _ = child.start_kill();
                return Err(CdpError::LaunchTimeout(opts.launch_timeout));
            }
        };
        tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} }); // keep draining so the pipe never fills
        let conn = match Conn::connect(&url, opts.call_timeout).await {
            Ok(c) => Arc::new(c),
            Err(e) => {
                let _ = child.start_kill();
                return Err(e);
            }
        };
        Ok(Self { conn, child, pid, window: opts.window, profile })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The throwaway profile directory (unique per browser; useful for diagnostics).
    pub fn profile_dir(&self) -> &std::path::Path {
        self.profile.path()
    }

    pub fn conn(&self) -> &Arc<Conn> {
        &self.conn
    }

    /// Open a new tab and attach to it.
    pub async fn new_page(&self) -> Result<Page, CdpError> {
        let created = self.conn.call("Target.createTarget", json!({"url": "about:blank"}), None).await?;
        let target = created["targetId"]
            .as_str()
            .ok_or_else(|| CdpError::Format { method: "Target.createTarget".into(), detail: "no targetId".into() })?
            .to_owned();
        let attached =
            self.conn.call("Target.attachToTarget", json!({"targetId": target, "flatten": true}), None).await?;
        let session = attached["sessionId"]
            .as_str()
            .ok_or_else(|| CdpError::Format { method: "Target.attachToTarget".into(), detail: "no sessionId".into() })?
            .to_owned();
        let page = Page::new(Arc::clone(&self.conn), session, target);
        page.setup(self.window).await?;
        Ok(page)
    }

    /// Ask the browser to quit, then make sure the whole process tree is gone.
    pub async fn close(mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(2), self.conn.call("Browser.close", json!({}), None)).await;
        let _ = tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await;
        let _ = self.child.start_kill();
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.start_kill(); // kills the group: renderers, GPU and utility processes too
    }
}
