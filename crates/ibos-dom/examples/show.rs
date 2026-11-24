//! Show what the distiller produces for a captured page. Usage: cargo run -p ibos-dom --example show -- NAME [full]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

use ibos_ctx::observe::render_full;
use ibos_ctx::tokens::{ApproxTokenizer, Tokenizer};
use ibos_dom::{DistillOptions, distill};
use ibos_mem::StepArena;

fn main() {
    let name = std::env::args().nth(1).expect("page name");
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let ax = std::fs::read_to_string(format!("{dir}/{name}.ax.json")).unwrap();
    let dom = std::fs::read_to_string(format!("{dir}/{name}.dom.json")).unwrap();
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{dir}/{name}.meta.json")).unwrap()).unwrap();
    let vp = (meta["viewport"][0].as_f64().unwrap(), meta["viewport"][1].as_f64().unwrap());
    let opts = DistillOptions {
        include_offscreen: std::env::args().nth(2).as_deref() == Some("full"),
        ..DistillOptions::default()
    };
    let arena = StepArena::new();
    let t0 = std::time::Instant::now();
    let d = distill(meta["url"].as_str().unwrap(), meta["title"].as_str().unwrap(), &ax, Some(&dom), vp, &opts, &arena)
        .unwrap();
    let took = t0.elapsed();
    let text = render_full(&d.snapshot);
    println!("{text}");
    println!(
        "--- {:?}\n--- {} chars, ~{} tokens (approx), input {} bytes, distilled in {took:?}",
        d.stats,
        text.len(),
        ApproxTokenizer.count(&text),
        d.stats.input_bytes
    );
}
