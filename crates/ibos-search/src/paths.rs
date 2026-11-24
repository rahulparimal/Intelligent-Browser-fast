//! Where local state lives. Matches the Python implementation so both read the same files.

use std::path::PathBuf;

/// `$INTELLI_HOME`, else `%APPDATA%\intelli-browser-os` on Windows (unless `XDG_CONFIG_HOME` is set), else
/// `$XDG_CONFIG_HOME/intelli-browser-os`, else `~/.config/intelli-browser-os`.
pub fn config_dir() -> PathBuf {
    if let Some(p) = non_empty("INTELLI_HOME") {
        return expand_home(&p);
    }
    if cfg!(windows) && non_empty("XDG_CONFIG_HOME").is_none() {
        let base = non_empty("APPDATA").map_or_else(|| home().join("AppData").join("Roaming"), PathBuf::from);
        return base.join("intelli-browser-os");
    }
    non_empty("XDG_CONFIG_HOME").map_or_else(|| home().join(".config"), PathBuf::from).join("intelli-browser-os")
}

fn non_empty(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

fn home() -> PathBuf {
    non_empty("HOME").or_else(|| non_empty("USERPROFILE")).map_or_else(|| PathBuf::from("."), PathBuf::from)
}

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(p),
    }
}

/// Default index location (`<config>/lite/index.db`).
pub fn default_db_path() -> PathBuf {
    config_dir().join("lite").join("index.db")
}

/// Default settings location (`<config>/lite/settings.json`).
pub fn settings_path() -> PathBuf {
    config_dir().join("lite").join("settings.json")
}
