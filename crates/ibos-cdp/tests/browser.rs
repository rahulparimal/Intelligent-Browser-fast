//! Against a real browser: launch, navigate, observe, act, screenshot, fail cleanly, and leave nothing behind.
//! Skipped when no browser is installed unless `IBOS_REQUIRE_BROWSER=1`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use ibos_cdp::{Browser, CdpError, LaunchOptions, Page, Transport, find_browser};
use serde_json::json;

const PAGE: &str = r#"<!doctype html><html><head><title>Login</title></head><body>
<h1>Sign in</h1>
<label>Email <input id="email" type="email"></label>
<button id="go" onclick="document.getElementById('result').textContent='Welcome ' + document.getElementById('email').value">Sign in</button>
<button id="buy" onclick="document.getElementById('result').textContent='ORDER PLACED'">Place order</button>
<div id="result"></div></body></html>"#;

fn data_url(html: &str) -> String {
    let enc: String = html
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("data:text/html;charset=utf-8,{enc}")
}

fn browser_available() -> bool {
    let ok = find_browser().is_some();
    assert!(
        ok || std::env::var("IBOS_REQUIRE_BROWSER").is_err(),
        "a browser is required for these tests (IBOS_REQUIRE_BROWSER is set)"
    );
    if !ok {
        eprintln!("skipping: no Chrome/Chromium/Edge found");
    }
    ok
}

async fn open() -> Option<(Browser, Page)> {
    if !browser_available() {
        return None;
    }
    let b = Browser::launch(LaunchOptions::default()).await.unwrap();
    let p = b.new_page().await.unwrap();
    p.navigate(&data_url(PAGE)).await.unwrap();
    Some((b, p))
}

#[tokio::test]
async fn navigate_evaluate_and_read_title() {
    let Some((_b, p)) = open().await else { return };
    assert_eq!(p.title().await.unwrap(), "Login");
    assert!(p.url().await.unwrap().starts_with("data:text/html"));
    assert_eq!(p.evaluate("1 + 2").await.unwrap(), json!(3));
    assert!(p.evaluate("throw new Error('boom')").await.is_err(), "page exceptions become errors");
    let v = p.viewport().await.unwrap();
    assert!((v.width - 1280.0).abs() < 1.0 && (v.height - 800.0).abs() < 1.0, "{v:?}");
}

#[tokio::test]
async fn observation_contains_the_accessibility_tree_and_layout() {
    let Some((_b, p)) = open().await else { return };
    let o = p.observe_raw().await.unwrap();
    assert_eq!(o.title, "Login");
    let ax: serde_json::Value = serde_json::from_str(o.ax.get()).unwrap();
    let names: Vec<&str> = ax["nodes"].as_array().unwrap().iter().filter_map(|n| n["name"]["value"].as_str()).collect();
    assert!(names.contains(&"Sign in") && names.contains(&"Place order"), "{names:?}");
    let dom: serde_json::Value = serde_json::from_str(o.dom.get()).unwrap();
    assert!(dom["documents"][0]["layout"]["bounds"].as_array().is_some_and(|b| !b.is_empty()));
}

#[tokio::test]
async fn type_and_click_change_the_page() {
    let Some((_b, p)) = open().await else { return };
    let email = p.backend_id_for_selector("#email").await.unwrap().expect("email input");
    let go = p.backend_id_for_selector("#go").await.unwrap().expect("sign in button");
    assert!(p.backend_id_for_selector("#nope").await.unwrap().is_none());
    p.type_text(email, "me@example.com", true).await.unwrap();
    assert_eq!(p.evaluate("document.getElementById('email').value").await.unwrap(), json!("me@example.com"));
    p.type_text(email, "other@example.com", true).await.unwrap();
    assert_eq!(
        p.evaluate("document.getElementById('email').value").await.unwrap(),
        json!("other@example.com"),
        "clear replaces the old value"
    );
    p.click(go).await.unwrap();
    assert_eq!(
        p.evaluate("document.getElementById('result').textContent").await.unwrap(),
        json!("Welcome other@example.com")
    );
}

#[tokio::test]
async fn scrolling_and_screenshots_work() {
    let Some((b, _)) = open().await else { return };
    let p = b.new_page().await.unwrap();
    p.navigate(&data_url(&format!("<html><body>{}</body></html>", "<p>line</p>".repeat(300)))).await.unwrap();
    assert_eq!(p.evaluate("window.scrollY").await.unwrap(), json!(0));
    p.scroll(400).await.unwrap();
    assert_eq!(p.evaluate("window.scrollY").await.unwrap(), json!(400));
    let jpeg = p.screenshot_jpeg(60).await.unwrap();
    assert!(jpeg.len() > 500 && jpeg[..2] == [0xFF, 0xD8], "a JPEG starts with FF D8");
}

#[tokio::test]
async fn failures_are_clear_errors_not_hangs() {
    let Some((_b, p)) = open().await else { return };
    let e = p.navigate("http://127.0.0.1:9/").await.unwrap_err();
    assert!(matches!(e, CdpError::Navigation(ref m) if m.contains("ERR_")), "{e}");
    assert!(matches!(p.click(999_999_999).await, Err(CdpError::Protocol { .. })), "a missing node is a protocol error");
    let slow = p.clone().with_navigation_timeout(Duration::from_millis(300));
    let t0 = std::time::Instant::now();
    let r = slow.navigate(&data_url("<script>for(;;){}</script>")).await;
    assert!(r.is_err() && t0.elapsed() < Duration::from_secs(20), "a page that never loads times out: {r:?}");
}

#[tokio::test]
async fn bad_executable_and_missing_browser_are_reported() {
    let e = Browser::launch(LaunchOptions { executable: Some("/no/such/browser".into()), ..LaunchOptions::default() })
        .await
        .unwrap_err();
    assert!(matches!(e, CdpError::Launch(_)), "{e}");
    let e = Browser::launch(LaunchOptions {
        executable: Some("/bin/true".into()),
        launch_timeout: Duration::from_secs(3),
        ..LaunchOptions::default()
    })
    .await
    .unwrap_err();
    assert!(
        matches!(e, CdpError::Launch(_) | CdpError::LaunchTimeout(_)),
        "a program that is not a browser must not hang the launch: {e}"
    );
}

/// Processes whose command line mentions `needle` (Linux only).
#[cfg(target_os = "linux")]
fn processes_mentioning(needle: &str) -> Vec<u32> {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| String::from_utf8_lossy(&c).contains(needle))
        })
        .collect()
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn dropping_the_browser_leaves_no_processes_behind() {
    let Some((b, p)) = open().await else { return };
    p.navigate(&data_url("<iframe src='about:blank'></iframe><p>renderers spawn helper processes</p>")).await.unwrap();
    let needle = b.profile_dir().to_string_lossy().into_owned();
    assert!(processes_mentioning(&needle).len() >= 2, "the browser has helper processes while running");
    drop(p);
    drop(b);
    for _ in 0..50 {
        if processes_mentioning(&needle).is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("orphaned browser processes: {:?}", processes_mentioning(&needle));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn graceful_close_also_leaves_nothing() {
    if !browser_available() {
        return;
    }
    let b = Browser::launch(LaunchOptions::default()).await.unwrap();
    let needle = b.profile_dir().to_string_lossy().into_owned();
    b.close().await;
    for _ in 0..50 {
        if processes_mentioning(&needle).is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("orphaned browser processes after close: {:?}", processes_mentioning(&needle));
}

/// TCP ports in LISTEN state that are owned by any process in the browser's process group (Linux).
#[cfg(target_os = "linux")]
fn listening_ports_of_group(pgid: u32) -> Vec<u16> {
    let mut inodes = std::collections::HashSet::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        let after = stat.rsplit_once(')').map_or("", |x| x.1);
        let group: Option<u32> = after.split_whitespace().nth(2).and_then(|g| g.parse().ok());
        if group != Some(pgid) {
            continue;
        }
        for fd in std::fs::read_dir(format!("/proc/{pid}/fd")).into_iter().flatten().flatten() {
            if let Ok(target) = std::fs::read_link(fd.path())
                && let Some(inode) =
                    target.to_str().and_then(|t| t.strip_prefix("socket:[")).and_then(|t| t.strip_suffix(']'))
            {
                inodes.insert(inode.to_owned());
            }
        }
    }
    let mut ports = Vec::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        for line in std::fs::read_to_string(table).unwrap_or_default().lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() > 9
                && cols[3] == "0A"
                && inodes.contains(cols[9])
                && let Some(port) = cols[1].rsplit(':').next().and_then(|p| u16::from_str_radix(p, 16).ok())
            {
                ports.push(port);
            }
        }
    }
    ports
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_pipe_transport_opens_no_network_port_and_the_port_transport_does() {
    if !browser_available() {
        return;
    }
    let piped = Browser::launch(LaunchOptions::default()).await.unwrap();
    assert!(piped.new_page().await.is_ok(), "the pipe works");
    assert_eq!(listening_ports_of_group(piped.pid()), Vec::<u16>::new(), "no DevTools port with the pipe transport");
    let ported =
        Browser::launch(LaunchOptions { transport: Transport::Port, ..LaunchOptions::default() }).await.unwrap();
    assert!(
        !listening_ports_of_group(ported.pid()).is_empty(),
        "the check can see a listener when there is one (so the empty result above means something)"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn running_as_root_needs_an_explicit_opt_in_to_disable_the_sandbox() {
    if !browser_available() {
        return;
    }
    let is_root = std::fs::read_to_string("/proc/self/status")
        .is_ok_and(|s| s.lines().any(|l| l.starts_with("Uid:") && l.split_whitespace().nth(1) == Some("0")));
    if !is_root {
        eprintln!("skipping: not running as root");
        return;
    }
    let refused =
        Browser::launch(LaunchOptions { allow_no_sandbox: false, ..LaunchOptions::default() }).await.unwrap_err();
    assert!(
        matches!(&refused, CdpError::Launch(m) if m.contains("sandbox") && m.contains("--allow-no-sandbox")),
        "{refused}"
    );
    assert!(Browser::launch(LaunchOptions { allow_no_sandbox: true, ..LaunchOptions::default() }).await.is_ok());
}
