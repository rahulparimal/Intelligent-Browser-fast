//! Launching and supervising the browser process tree.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use command_group::{AsyncCommandGroup, AsyncGroupChild};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::chrome::{find_browser, parse_devtools_url, running_as_root};
use crate::conn::Conn;
use crate::error::CdpError;
use crate::page::Page;

/// How the program talks to the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Chromium's `--remote-debugging-pipe`: two private pipes, no network port, so no other program can attach. Unix only.
    Pipe,
    /// A DevTools WebSocket on a random loopback port. Any local program can connect to it.
    Port,
}

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
    /// Pipe by default where supported.
    pub transport: Transport,
    /// Start Chromium without its sandbox. Needed when running as root (Chromium refuses to sandbox there), but the pages the
    /// agent visits then run with the full rights of this user, so it is never enabled on its own: it takes this flag or
    /// `INTELLI_NO_SANDBOX=1`.
    pub allow_no_sandbox: bool,
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
            transport: if cfg!(unix) { Transport::Pipe } else { Transport::Port },
            allow_no_sandbox: std::env::var_os("INTELLI_NO_SANDBOX").is_some_and(|v| v == "1"),
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
        if !opts.allow_no_sandbox && running_as_root() {
            return Err(CdpError::Launch(
                "Chromium cannot use its security sandbox when running as root, and without it every page the agent visits can act with full \
                 rights on this machine. Run intelli-rust as a normal user. If you accept the risk (a disposable container, for example), \
                 pass --allow-no-sandbox or set INTELLI_NO_SANDBOX=1."
                    .into(),
            ));
        }
        if opts.allow_no_sandbox {
            tracing::warn!("starting Chromium WITHOUT its sandbox: pages can act with this user's full rights");
            eprintln!("warning: Chromium is starting without its sandbox. Only visit sites you trust.");
        }
        let profile = tempfile::tempdir().map_err(|e| CdpError::Launch(e.to_string()))?;
        let pipe = opts.transport == Transport::Pipe && cfg!(unix);
        let mut cmd = Command::new(&exe);
        cmd.arg(if pipe { "--remote-debugging-pipe" } else { "--remote-debugging-port=0" })
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
        if opts.allow_no_sandbox {
            cmd.arg("--no-sandbox");
        }
        cmd.args(&opts.extra_args)
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        let ends = if pipe { Some(attach_pipes(&mut cmd)?) } else { None };
        let mut child = cmd.group_spawn().map_err(|e| CdpError::Launch(format!("{}: {e}", exe.display())))?;
        drop(cmd); // closes this process's copies of the child's ends of the pipes
        let pid = child.id().ok_or_else(|| CdpError::Launch("the browser exited immediately".into()))?;
        let stderr = child.inner().stderr.take().ok_or_else(|| CdpError::Launch("no stderr".into()))?;
        let mut lines = BufReader::new(stderr).lines();

        #[cfg(unix)]
        if let Some((read, write)) = ends {
            tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} }); // keep draining so the pipe never fills
            let conn = Arc::new(Conn::over_pipe(read, write, opts.call_timeout));
            // the first reply proves the browser started; a browser that dies at start closes the pipe at once
            match tokio::time::timeout(opts.launch_timeout, conn.call("Browser.getVersion", json!({}), None)).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    let _ = child.start_kill();
                    return Err(CdpError::Launch(format!("the browser did not start: {e}")));
                }
                Err(_) => {
                    let _ = child.start_kill();
                    return Err(CdpError::LaunchTimeout(opts.launch_timeout));
                }
            }
            return Ok(Self { conn, child, pid, window: opts.window, profile });
        }

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

/// Create the two pipes Chromium expects (it reads requests on descriptor 3 and writes replies on descriptor 4), hand the
/// child its ends, and return this process's ends.
#[cfg(unix)]
fn attach_pipes(
    cmd: &mut Command,
) -> Result<(tokio::net::unix::pipe::Receiver, tokio::net::unix::pipe::Sender), CdpError> {
    use std::os::fd::OwnedFd;

    use command_fds::{CommandFdExt, FdMapping};
    let io = |e: std::io::Error| CdpError::Launch(format!("could not create the DevTools pipes: {e}"));
    let (to_child_read, to_child_write) = std::io::pipe().map_err(io)?;
    let (from_child_read, from_child_write) = std::io::pipe().map_err(io)?;
    cmd.as_std_mut()
        .fd_mappings(vec![
            FdMapping { parent_fd: OwnedFd::from(to_child_read), child_fd: 3 },
            FdMapping { parent_fd: OwnedFd::from(from_child_write), child_fd: 4 },
        ])
        .map_err(|e| CdpError::Launch(format!("could not pass the DevTools pipes to the browser: {e:?}")))?;
    let receiver =
        tokio::net::unix::pipe::Receiver::from_file(std::fs::File::from(OwnedFd::from(from_child_read))).map_err(io)?;
    let sender =
        tokio::net::unix::pipe::Sender::from_file(std::fs::File::from(OwnedFd::from(to_child_write))).map_err(io)?;
    Ok((receiver, sender))
}
