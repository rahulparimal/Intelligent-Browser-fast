//! Capture raw accessibility + layout JSON for pages, to use as browser-free test fixtures.
//! Usage: `cargo run -p ibos-cdp --example capture -- OUT_DIR PAGE.html...`
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

use ibos_cdp::{Browser, LaunchOptions};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (out, pages) = args.split_first().expect("usage: capture OUT_DIR PAGE...");
    std::fs::create_dir_all(out).unwrap();
    let browser = Browser::launch(LaunchOptions::default()).await.unwrap();
    for page_path in pages {
        let abs = std::fs::canonicalize(page_path).unwrap();
        let page = browser.new_page().await.unwrap();
        page.navigate(&format!("file://{}", abs.display())).await.unwrap();
        let obs = page.observe_raw().await.unwrap();
        let name = abs.file_stem().unwrap().to_string_lossy().into_owned();
        std::fs::write(format!("{out}/{name}.ax.json"), obs.ax.get()).unwrap();
        std::fs::write(format!("{out}/{name}.dom.json"), obs.dom.get()).unwrap();
        let meta = serde_json::json!({"url": obs.url, "title": obs.title, "viewport": [obs.viewport.width, obs.viewport.height]});
        std::fs::write(format!("{out}/{name}.meta.json"), meta.to_string()).unwrap();
        println!("{name}: ax {} bytes, dom {} bytes", obs.ax.get().len(), obs.dom.get().len());
        page.close().await;
    }
}
