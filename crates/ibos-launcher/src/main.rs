//! `intelli-search`: what a double-click on the desktop icon runs.
//!
//! It starts the sibling `intelli-rust` executable as `ui --exit-when-closed` (the search page opens in the browser, and the
//! program quits shortly after the tab is closed). On Windows this program has no console window of its own and neither does the
//! child, so nothing flashes on screen. If the child cannot start, the reason is written to `launcher.log` in the config folder,
//! because there is no console to print it to.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

/// Same rule as `ibos-search`'s `config_dir`, repeated here so the launcher stays tiny.
fn log_dir() -> PathBuf {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let home = || var("HOME").or_else(|| var("USERPROFILE")).map_or_else(|| PathBuf::from("."), PathBuf::from);
    if let Some(p) = var("INTELLI_HOME") {
        return PathBuf::from(p);
    }
    if cfg!(windows) && var("XDG_CONFIG_HOME").is_none() {
        let base = var("APPDATA").map_or_else(|| home().join("AppData").join("Roaming"), PathBuf::from);
        return base.join("intelli-browser-os");
    }
    var("XDG_CONFIG_HOME").map_or_else(|| home().join(".config"), PathBuf::from).join("intelli-browser-os")
}

fn note(message: &str) {
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("launcher.log")) {
        let _ = writeln!(f, "{message}");
    }
}

fn main() -> ExitCode {
    let Ok(me) = std::env::current_exe() else {
        note("could not find this program's own location");
        return ExitCode::FAILURE;
    };
    let name = if cfg!(windows) { "intelli-rust.exe" } else { "intelli-rust" };
    let target = me.with_file_name(name);
    let mut cmd = Command::new(&target);
    cmd.args(["ui", "--exit-when-closed"]).args(std::env::args_os().skip(1));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    match cmd.status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => {
            note(&format!("{} exited with {status}", target.display()));
            ExitCode::FAILURE
        }
        Err(e) => {
            note(&format!("could not start {}: {e}", target.display()));
            ExitCode::FAILURE
        }
    }
}
