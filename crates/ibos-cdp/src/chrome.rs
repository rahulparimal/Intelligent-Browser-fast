//! Finding a browser and reading its DevTools address.

use std::path::{Path, PathBuf};

/// `ws://127.0.0.1:PORT/devtools/browser/ID` from a line like `DevTools listening on ws://...`.
pub fn parse_devtools_url(line: &str) -> Option<String> {
    let rest = line.split("DevTools listening on ").nth(1)?.trim();
    (rest.starts_with("ws://") || rest.starts_with("wss://")).then(|| rest.to_owned())
}

fn is_file(p: &Path) -> bool {
    p.is_file()
}

fn on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|d| d.join(name))
        .chain(std::env::split_paths(&paths).map(|d| d.join(format!("{name}.exe"))))
        .find(|p| is_file(p))
}

/// Newest `chromium-*` directory under `dir` containing `rel`.
fn playwright_chromium(dir: &Path, rel: &[&str]) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("chromium-")))
        .collect();
    dirs.sort();
    dirs.into_iter().rev().map(|d| rel.iter().fold(d, |acc, part| acc.join(part))).find(|p| is_file(p))
}

/// A browser executable: `INTELLI_BROWSER` / `CHROME` if set, else the usual install locations and `PATH`.
pub fn find_browser() -> Option<PathBuf> {
    for var in ["INTELLI_BROWSER", "CHROME", "CHROME_PATH"] {
        if let Some(p) = std::env::var_os(var).map(PathBuf::from).filter(|p| is_file(p)) {
            return Some(p);
        }
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        let apps = [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Chromium.app/Contents/MacOS/Chromium",
            "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "Brave Browser.app/Contents/MacOS/Brave Browser",
        ];
        return apps.iter().map(|a| Path::new("/Applications").join(a)).find(|p| is_file(p));
    }
    if cfg!(windows) {
        let roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"]
            .iter()
            .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
            .collect();
        let rels = [
            "Google/Chrome/Application/chrome.exe",
            "Microsoft/Edge/Application/msedge.exe",
            "BraveSoftware/Brave-Browser/Application/brave.exe",
        ];
        return roots.iter().flat_map(|r| rels.iter().map(move |rel| r.join(rel))).find(|p| is_file(p));
    }
    for name in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome",
        "microsoft-edge",
        "brave-browser",
    ] {
        if let Some(p) = on_path(name) {
            return Some(p);
        }
    }
    let mut caches = vec![PathBuf::from("/opt/pw-browsers")];
    caches.extend(home.map(|h| h.join(".cache").join("ms-playwright")));
    if let Some(p) = std::env::var_os("PLAYWRIGHT_BROWSERS_PATH").map(PathBuf::from) {
        caches.insert(0, p);
    }
    caches.iter().find_map(|c| {
        playwright_chromium(c, &["chrome-linux64", "chrome"])
            .or_else(|| playwright_chromium(c, &["chrome-linux", "chrome"]))
    })
}

/// True when the process runs as root on Linux, where Chromium refuses to start with its sandbox.
pub(crate) fn running_as_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .is_ok_and(|s| s.lines().any(|l| l.starts_with("Uid:") && l.split_whitespace().nth(1) == Some("0")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_devtools_line() {
        assert_eq!(
            parse_devtools_url("DevTools listening on ws://127.0.0.1:41233/devtools/browser/abc-123\n").as_deref(),
            Some("ws://127.0.0.1:41233/devtools/browser/abc-123")
        );
        assert_eq!(parse_devtools_url("[0101/000000.1:ERROR:foo] something else"), None);
        assert_eq!(parse_devtools_url("DevTools listening on http://nope"), None);
        assert_eq!(parse_devtools_url(""), None);
    }

    #[test]
    fn environment_override_wins_and_missing_files_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        let fake = d.path().join("my-chrome");
        std::fs::write(&fake, "#!/bin/sh\n").unwrap();
        assert!(is_file(&fake) && !is_file(&d.path().join("nope")));
        assert!(playwright_chromium(d.path(), &["chrome-linux64", "chrome"]).is_none());
        let pw = d.path().join("chromium-1100").join("chrome-linux64");
        std::fs::create_dir_all(&pw).unwrap();
        std::fs::write(pw.join("chrome"), "x").unwrap();
        let newer = d.path().join("chromium-1243").join("chrome-linux64");
        std::fs::create_dir_all(&newer).unwrap();
        std::fs::write(newer.join("chrome"), "x").unwrap();
        assert_eq!(
            playwright_chromium(d.path(), &["chrome-linux64", "chrome"]).unwrap(),
            newer.join("chrome"),
            "the newest build is chosen"
        );
    }
}
