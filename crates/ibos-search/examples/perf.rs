//! Index and query timing on a generated corpus (same workload as the Python measurement: 2,000 documents of about 3 KB).
//! Usage: `cargo run --release -p ibos-search --example perf`
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

use ibos_search::{IndexOptions, LiteStore};

fn main() {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("c");
    std::fs::create_dir_all(&docs).unwrap();
    let words: Vec<&str> = "budget policy travel leave security laptop onboarding benefits holiday remote expense review quarterly planning hiring".split(' ').collect();
    let mut bytes = 0;
    for i in 0..2000 {
        let body: Vec<&str> = (0..400).map(|j| words[(i + j) % words.len()]).collect();
        let text = format!("{} unique{i}token", body.join(" "));
        bytes += text.len();
        std::fs::write(docs.join(format!("doc{i}.txt")), text).unwrap();
    }
    let store = LiteStore::open(Some(&dir.path().join("i.db")), None).unwrap();
    let t = std::time::Instant::now();
    let st = store.index_path(&docs, IndexOptions::default(), None).unwrap();
    let first = t.elapsed();
    let t = std::time::Instant::now();
    store.index_path(&docs, IndexOptions::default(), None).unwrap();
    let rescan = t.elapsed();
    let t = std::time::Instant::now();
    for _ in 0..200 {
        store.search("quarterly planning budget", None, None).unwrap();
    }
    let per_query = t.elapsed().as_secs_f64() * 1000.0 / 200.0;
    println!(
        "{} documents ({} KB): first index {first:.2?}, unchanged re-scan {rescan:.2?}, query {per_query:.2} ms; {:?}",
        st.indexed,
        bytes / 1024,
        store.stats().unwrap()
    );
}
