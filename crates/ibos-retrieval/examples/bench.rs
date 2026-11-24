//! Synthetic retrieval benchmark: `cargo run --release -p ibos-retrieval --example bench -- [documents]`.
//!
//! Builds a corpus of generated documents, each with three rare "key" words, then asks for documents by those words written
//! exactly, with typos, and with a different word ending. Also probes access control. It measures keyword search, dense
//! search with the built-in hashing embedder, and hybrid search. It cannot measure synonym or paraphrase handling: the
//! hashing embedder has none, and no real embedding model is available offline.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Instant;

use ibos_mem::MemoryBudget;
use ibos_retrieval::eval::{Config, Judged, evaluate, table};
use ibos_retrieval::{HashingEmbedder, Mode, Retriever};
use ibos_search::{IndexOptions, LiteStore};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const SYL: &[&str] =
    &["ba", "ko", "ri", "tu", "sen", "mal", "dor", "vi", "qua", "lex", "nor", "pel", "zan", "hu", "fro", "gri"];
const COMMON: &[&str] = &[
    "the",
    "report",
    "covers",
    "quarterly",
    "results",
    "for",
    "each",
    "region",
    "and",
    "explains",
    "how",
    "teams",
    "planned",
    "their",
    "work",
    "customers",
    "asked",
    "about",
    "delivery",
    "pricing",
    "support",
    "policy",
    "training",
    "budget",
    "schedule",
    "review",
    "approval",
];

fn word(rng: &mut Rng) -> String {
    (0..3 + rng.below(2)).map(|_| SYL[rng.below(SYL.len())]).collect()
}

fn typo(w: &str, rng: &mut Rng) -> String {
    let mut c: Vec<char> = w.chars().collect();
    let i = 1 + rng.below(c.len() - 2);
    if rng.below(2) == 0 {
        c.remove(i);
    } else {
        c.swap(i, i + 1);
    }
    c.into_iter().collect()
}

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(3000);
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    let mut rng = Rng(42);
    let mut keys: Vec<[String; 3]> = Vec::new();
    for i in 0..n {
        let k = [word(&mut rng), word(&mut rng), word(&mut rng)];
        let mut text = format!("Document {i} about {} {}\n\n", k[0], k[1]);
        for p in 0..6 {
            let mut para: Vec<&str> = (0..60).map(|_| COMMON[rng.below(COMMON.len())]).collect();
            if p == 1 {
                para.insert(10, &k[0]);
                para.insert(20, &k[1]);
                para.insert(30, &k[2]);
            }
            text.push_str(&para.join(" "));
            text.push_str("\n\n");
        }
        std::fs::write(docs.join(format!("d{i:05}.txt")), text).unwrap();
        keys.push(k);
    }
    let store = LiteStore::open(Some(&dir.path().join("idx.db")), None).unwrap();
    let t = Instant::now();
    store.index_path(&docs, IndexOptions::default(), None).unwrap();
    let stats = store.stats().unwrap();
    println!(
        "corpus: {n} documents, {} passages; FTS5 index built in {:.2} s",
        stats.chunks,
        t.elapsed().as_secs_f64()
    );

    let budget = MemoryBudget::new(256 << 20, 512 << 20);
    let r = Retriever::open(&store, Some(Arc::new(HashingEmbedder::default())), budget.clone()).unwrap();
    let t = Instant::now();
    let s = r.sync_vectors(None).unwrap();
    println!("embedded {} passages (hashing, 384 dims, int8) in {:.2} s", s.embedded, t.elapsed().as_secs_f64());
    r.vector_mode().unwrap();
    println!(
        "vector memory held: {:.1} MiB for {} vectors\n",
        budget.used() as f64 / 1_048_576.0,
        r.vector_count().unwrap()
    );

    // restrict every 10th document to "hr"
    let restricted: Vec<usize> = (0..n).step_by(10).collect();
    for i in &restricted {
        r.set_acl(&docs.join(format!("d{i:05}.txt")).to_string_lossy(), &["hr"]).unwrap();
    }
    let sample: Vec<usize> = (0..n).filter(|i| i % 10 != 0).step_by(3).take(300).collect();
    let make = |f: &dyn Fn(&[String; 3], &mut Rng) -> String| -> Vec<Judged> {
        let mut rg = Rng(7);
        sample
            .iter()
            .map(|i| Judged {
                query: f(&keys[*i], &mut rg),
                relevant: vec![format!("d{i:05}.txt")],
                principals: vec![],
                forbidden: restricted.iter().map(|x| format!("d{x:05}.txt")).collect(),
            })
            .collect()
    };
    let cases: [(&str, Vec<Judged>); 3] = [
        ("exact words", make(&|k, _| format!("{} {} {}", k[0], k[1], k[2]))),
        (
            "typos (one letter dropped or swapped per word)",
            make(&|k, g| format!("{} {} {}", typo(&k[0], g), typo(&k[1], g), typo(&k[2], g))),
        ),
        ("different word endings", make(&|k, _| format!("{}s {}ing {}ed", k[0], k[1], k[2]))),
    ];
    let configs = [
        Config::new("keyword (BM25)", Mode::Lexical, false, false),
        Config::new("keyword + fuzzy words", Mode::Lexical, false, true),
        Config::new("dense (hashing)", Mode::Dense, false, false),
        Config::new("hybrid (RRF)", Mode::Hybrid, false, false),
        Config::new("hybrid + fuzzy + rerank", Mode::Hybrid, true, true),
    ];
    for (name, judged) in &cases {
        println!("### {name} ({} queries)\n", judged.len());
        println!("{}", table(&evaluate(&r, judged, &configs).unwrap()));
    }
    // access-control probe: ask for restricted documents by their own words, as an anonymous caller and as another group
    let probe: Vec<Judged> = restricted
        .iter()
        .take(200)
        .map(|i| Judged {
            query: format!("{} {} {}", keys[*i][0], keys[*i][1], keys[*i][2]),
            relevant: vec![],
            principals: vec!["sales".into()],
            forbidden: vec![format!("d{i:05}.txt")],
        })
        .collect();
    let rep = evaluate(&r, &probe, &configs).unwrap();
    println!("### access control: {} probes for restricted documents by a caller without access\n", probe.len());
    for x in &rep {
        println!("- {}: {} leaks", x.label, x.leaks);
    }
}
