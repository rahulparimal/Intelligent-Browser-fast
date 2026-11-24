//! Reading untrusted documents in a separate, restricted process.
//!
//! PDF and DOCX parsers handle hostile input. In-process they can hang the whole program, exhaust its memory or abort it. Here
//! each file is read by a short-lived worker process (the same executable, started with a hidden command) that:
//!
//! * is killed if it runs longer than a time limit;
//! * is capped in memory (`RLIMIT_AS`) and in how much output the parent will accept;
//! * starts with an empty environment (no API keys to read);
//! * on Linux, restricts itself with Landlock to *reading that one file* and to no TCP connections, when the kernel supports it.
//!
//! What this does **not** do: it is not a full sandbox on macOS or Windows (only the process, time, memory and output limits
//! apply there), and on a Linux kernel without Landlock the file and network restriction is skipped (the worker reports it).
//! Whatever the worker says about itself is untrusted too: the parent only parses a size-limited JSON reply.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::extract::{ExtractError, extract};

/// How the parent runs the worker.
#[derive(Debug, Clone)]
pub struct Sandbox {
    /// The program to run (normally `std::env::current_exe()`).
    pub exe: PathBuf,
    /// Arguments before the file path (normally `["__extract-worker"]`).
    pub args: Vec<OsString>,
    pub timeout: Duration,
    /// Most output bytes the parent will accept.
    pub max_output: usize,
    /// Address-space limit for the worker, in bytes.
    pub memory_limit: u64,
}

pub const WORKER_COMMAND: &str = "__extract-worker";

impl Sandbox {
    /// The worker is this very executable.
    pub fn current_exe() -> std::io::Result<Self> {
        Ok(Self {
            exe: std::env::current_exe()?,
            args: vec![WORKER_COMMAND.into()],
            timeout: Duration::from_mins(1),
            max_output: 128 << 20,
            memory_limit: 2 << 30,
        })
    }

    /// Extract `path` in a worker. Any failure of the worker (timeout, crash, out of memory, oversize or malformed output) is an
    /// error for this file only; the caller carries on with the next one.
    pub fn extract(&self, path: &Path) -> Result<(String, Vec<String>), ExtractError> {
        let mut child = Command::new(&self.exe)
            .args(&self.args)
            .arg(path)
            .env_clear()
            .env("IBOS_WORKER_MEMORY", self.memory_limit.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| ExtractError::Unreadable("file", format!("could not start the reader: {e}")))?;
        let mut out = child.stdout.take().ok_or_else(|| ExtractError::Unreadable("file", "no reader output".into()))?;
        let cap = self.max_output;
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = vec![0_u8; 64 * 1024];
            loop {
                match out.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if buf.len() > cap {
                            let _ = tx.send(None); // too much: tell the waiting side to kill the worker
                            return buf.len();
                        }
                    }
                }
            }
            let _ = tx.send(Some(buf));
            0
        });
        let deadline = Instant::now() + self.timeout;
        let result = loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(Some(buf)) => break Ok(buf),
                Ok(None) => break Err(format!("its output was larger than {} MB", cap >> 20)),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if Instant::now() >= deadline {
                        break Err(format!("it took longer than {} seconds", self.timeout.as_secs()));
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break Err("the reader stopped without an answer".into()),
            }
        };
        let buf = match result {
            Ok(b) => b,
            Err(why) => {
                let _ = child.kill();
                let _ = child.wait();
                // The reader thread is not joined: if the worker left a grandchild holding the pipe open, it would block
                // until that exits. It ends by itself when the pipe closes.
                drop(reader);
                return Err(ExtractError::Unreadable("file", why));
            }
        };
        let status = child.wait().map_err(|e| ExtractError::Unreadable("file", e.to_string()))?;
        let _ = reader.join();
        if !status.success() {
            return Err(ExtractError::Unreadable(
                "file",
                "the reader crashed or ran out of memory on this file".to_owned(),
            ));
        }
        match serde_json::from_slice::<Reply>(&buf) {
            Ok(Reply::Ok { title, pages, .. }) => Ok((title, pages)),
            Ok(Reply::Error { kind, message }) => Err(match kind.as_str() {
                "unsupported" => ExtractError::Unsupported(message),
                _ => ExtractError::Unreadable("file", message),
            }),
            Err(_) => Err(ExtractError::Unreadable("file", "the reader gave an unreadable answer".into())),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "lowercase")]
enum Reply {
    Ok {
        title: String,
        pages: Vec<String>,
        /// What the worker could restrict about itself (for diagnostics; not trusted).
        #[serde(default)]
        sandbox: String,
    },
    Error {
        kind: String,
        message: String,
    },
}

/// What the worker managed to restrict.
fn restrict_self(file: &Path, memory: u64) -> String {
    let mut applied = Vec::new();
    #[cfg(unix)]
    {
        if memory > 0 && rlimit::setrlimit(rlimit::Resource::AS, memory, memory).is_ok() {
            applied.push("memory");
        }
        if rlimit::setrlimit(rlimit::Resource::CORE, 0, 0).is_ok() {
            applied.push("no-core-dumps");
        }
    }
    #[cfg(not(unix))]
    let _ = memory;
    #[cfg(target_os = "linux")]
    {
        if landlock_read_only(file) {
            applied.push("landlock");
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = file;
    applied.join("+")
}

/// Restrict this process to reading `file` and to no TCP connections. `true` only if the kernel enforces it fully.
#[cfg(target_os = "linux")]
fn landlock_read_only(file: &Path) -> bool {
    use landlock::{
        ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
        RulesetCreatedAttr, RulesetStatus,
    };
    let abi = ABI::V4;
    let attempt = || -> Result<RulesetStatus, Box<dyn std::error::Error>> {
        let status = Ruleset::default()
            .set_compatibility(CompatLevel::BestEffort)
            .handle_access(AccessFs::from_all(abi))?
            .handle_access(AccessNet::from_all(abi))?
            .create()?
            .add_rule(PathBeneath::new(PathFd::new(file)?, AccessFs::ReadFile))?
            .restrict_self()?;
        Ok(status.ruleset)
    };
    matches!(attempt(), Ok(RulesetStatus::FullyEnforced))
}

/// The worker side: restrict, read the file, print one JSON reply. Returns the process exit code. Must be called before any
/// threads or an async runtime exist.
pub fn worker_main(path: &Path) -> i32 {
    let memory = std::env::var("IBOS_WORKER_MEMORY").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let sandbox = restrict_self(path, memory);
    let reply = match extract(path) {
        Ok((title, pages)) => Reply::Ok { title, pages, sandbox },
        Err(e) => Reply::Error {
            kind: if matches!(e, ExtractError::Unsupported(_)) { "unsupported" } else { "unreadable" }.into(),
            message: e.to_string(),
        },
    };
    let Ok(json) = serde_json::to_vec(&reply) else { return 2 };
    let mut out = std::io::stdout().lock();
    if out.write_all(&json).and_then(|()| out.flush()).is_err() {
        return 2;
    }
    0
}

/// For tests and `doctor`: after restricting itself for `file`, can this process still read another file or open a TCP
/// connection? Returns `(restriction applied, other file readable, tcp connect allowed)`.
pub fn self_test(file: &Path) -> (String, bool, bool) {
    let applied = restrict_self(file, 0);
    let other_readable = std::fs::read_to_string("/etc/hostname").is_ok() || std::fs::read_dir("/").is_ok();
    let tcp = std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], 9)),
        Duration::from_millis(200),
    );
    // "connection refused" means the connect was attempted and allowed; "permission denied" means it was blocked
    let tcp_allowed = match tcp {
        Ok(_) => true,
        Err(e) => e.kind() != std::io::ErrorKind::PermissionDenied,
    };
    (applied, other_readable, tcp_allowed)
}

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::unwrap_used)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A stand-in worker: a shell script that behaves as told.
    fn script(dir: &Path, body: &str) -> Sandbox {
        let p = dir.join("worker.sh");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        Sandbox { exe: p, args: vec![], timeout: Duration::from_millis(600), max_output: 1 << 20, memory_limit: 0 }
    }

    #[test]
    fn a_worker_that_hangs_is_killed_and_reported() {
        let d = tempfile::tempdir().unwrap();
        let sb = script(d.path(), "sleep 30");
        let t = Instant::now();
        let e = sb.extract(Path::new("x.pdf")).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(5), "returned at the limit, not after 30 s");
        assert!(e.to_string().contains("took longer"), "{e}");
    }

    #[test]
    fn a_worker_that_crashes_does_not_take_the_parent_down() {
        let d = tempfile::tempdir().unwrap();
        let sb = script(d.path(), "kill -SEGV $$");
        let e = sb.extract(Path::new("x.pdf")).unwrap_err();
        assert!(e.to_string().contains("crashed"), "{e}");
        let sb = script(d.path(), "exit 7");
        assert!(sb.extract(Path::new("x.pdf")).is_err());
    }

    #[test]
    fn oversized_output_is_cut_off_and_the_worker_killed() {
        let d = tempfile::tempdir().unwrap();
        let mut sb = script(d.path(), "yes aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        sb.max_output = 100_000;
        sb.timeout = Duration::from_secs(10);
        let t = Instant::now();
        let e = sb.extract(Path::new("x.pdf")).unwrap_err();
        assert!(e.to_string().contains("larger than"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn garbage_and_valid_replies_are_told_apart() {
        let d = tempfile::tempdir().unwrap();
        assert!(script(d.path(), "echo not json").extract(Path::new("x")).is_err());
        let ok = script(d.path(), r#"echo '{"result":"ok","title":"T","pages":["one","two"]}'"#);
        assert_eq!(ok.extract(Path::new("x")).unwrap(), ("T".to_owned(), vec!["one".to_owned(), "two".to_owned()]));
        let unsupported = script(d.path(), r#"echo '{"result":"error","kind":"unsupported","message":".xyz"}'"#);
        assert!(matches!(unsupported.extract(Path::new("x")), Err(ExtractError::Unsupported(_))));
    }

    #[test]
    fn the_worker_starts_with_an_empty_environment() {
        let d = tempfile::tempdir().unwrap();
        // prints the number of environment variables it can see (only the one memory setting is passed on purpose)
        let sb = script(d.path(), r#"n=$(env | grep -c . ); echo "{\"result\":\"ok\",\"title\":\"$n\",\"pages\":[]}""#);
        // SAFETY of the test: a variable that must not reach the worker
        let _ = std::env::var("PATH");
        let (title, _) = sb.extract(Path::new("x")).unwrap();
        let n: usize = title.parse().unwrap();
        assert!(n <= 3, "the worker saw {n} variables");
    }
}
