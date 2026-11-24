//! End-to-end tests against a real FTS5 index in a temp directory.
#![allow(clippy::unwrap_used, clippy::many_single_char_names)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;

use ibos_mem::MemoryBudget;
use ibos_retrieval::{
    EmbedError, Embedder, HashingEmbedder, Mode, Policy, Query, RemoteEmbedder, Retriever, VectorMode, ground,
};
use ibos_search::{IndexOptions, LiteStore};

const DOCS: &[(&str, &str)] = &[
    (
        "refunds.txt",
        "Refund policy\n\nCustomers may return damaged goods within thirty days of delivery for a full refund.",
    ),
    (
        "invoicing.txt",
        "Invoicing guide\n\nInvoices are issued monthly. Payment terms are net 30 days from the invoice date.",
    ),
    (
        "shipping.txt",
        "Shipping rates\n\nParcels to remote regions can take up to three weeks depending on the carrier.",
    ),
    (
        "salaries.txt",
        "Confidential salaries\n\nThe executive compensation table lists base salary and bonus for each director.",
    ),
    (
        "copy.txt",
        "Refund policy\n\nCustomers may return damaged goods within thirty days of delivery for a full refund!",
    ),
];

fn corpus(dir: &Path) -> LiteStore {
    let docs = dir.join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    for (n, t) in DOCS {
        std::fs::write(docs.join(n), t).unwrap();
    }
    let store = LiteStore::open(Some(&dir.join("idx.db")), None).unwrap();
    store.index_path(&docs, IndexOptions::default(), None).unwrap();
    store
}

fn retriever(store: &LiteStore, soft_hard: (usize, usize)) -> Retriever {
    let e: Arc<dyn Embedder> = Arc::new(HashingEmbedder::default());
    let r = Retriever::open(store, Some(e), MemoryBudget::new(soft_hard.0, soft_hard.1)).unwrap();
    r.sync_vectors(None).unwrap();
    r
}

fn paths(o: &ibos_retrieval::Outcome) -> Vec<String> {
    o.passages.iter().map(|p| p.path.rsplit('/').next().unwrap().to_string()).collect()
}

#[test]
fn misspelled_queries_are_fixed_by_fuzzy_words_and_by_hybrid() {
    let dir = tempfile::tempdir().unwrap();
    let store = corpus(dir.path());
    let r = retriever(&store, (1 << 20, 1 << 24));
    let q = "invoise paymnt";
    assert!(paths(&r.search(&Query::new(q).mode(Mode::Lexical).fuzzy(false)).unwrap()).is_empty());
    assert_eq!(
        paths(&r.search(&Query::new(q).mode(Mode::Lexical)).unwrap())[0],
        "invoicing.txt",
        "fuzzy words alone fix it"
    );
    let hybrid = r.search(&Query::new(q).mode(Mode::Hybrid).fuzzy(false)).unwrap();
    assert_eq!(hybrid.mode_used, Mode::Hybrid);
    assert_eq!(paths(&hybrid)[0], "invoicing.txt");
}

#[test]
fn hybrid_falls_back_to_keywords_and_says_so_when_there_are_no_vectors() {
    let dir = tempfile::tempdir().unwrap();
    let store = corpus(dir.path());
    let r = Retriever::open(&store, Some(Arc::new(HashingEmbedder::default())), MemoryBudget::new(1 << 20, 1 << 24))
        .unwrap();
    let o = r.search(&Query::new("refund damaged goods")).unwrap();
    assert_eq!(o.mode_used, Mode::Lexical);
    assert!(o.notes.iter().any(|n| n.contains("no vectors")));
    assert!(paths(&o)[0].contains("refund") || paths(&o)[0] == "copy.txt");
}

#[test]
fn restricted_documents_never_appear_in_any_mode_and_do_not_shift_ranks() {
    let dir = tempfile::tempdir().unwrap();
    let store = corpus(dir.path());
    let r = retriever(&store, (1 << 20, 1 << 24));
    let salaries = store.documents("salaries", 0, 5).unwrap().0[0].path.clone();
    r.set_acl(&salaries, &["hr"]).unwrap();
    for mode in [Mode::Lexical, Mode::Dense, Mode::Hybrid] {
        for rerank in [true, false] {
            let q = Query::new("executive compensation salary bonus directors").mode(mode).rerank(rerank).limit(20);
            let anon = r.search(&q).unwrap();
            assert!(!paths(&anon).contains(&"salaries.txt".to_string()), "{mode:?} leaked to anonymous caller");
            let other = r.search(&q.clone().principals(["sales"])).unwrap();
            assert!(!paths(&other).contains(&"salaries.txt".to_string()), "{mode:?} leaked to wrong principal");
        }
    }
    let hr = r.search(&Query::new("executive compensation salary bonus").principals(["hr"])).unwrap();
    assert_eq!(paths(&hr)[0], "salaries.txt");
}

#[test]
fn an_empty_acl_hides_a_document_from_everyone_and_clearing_it_restores_access() {
    let dir = tempfile::tempdir().unwrap();
    let store = corpus(dir.path());
    let r = retriever(&store, (1 << 20, 1 << 24));
    let p = store.documents("shipping", 0, 5).unwrap().0[0].path.clone();
    r.set_acl(&p, &[]).unwrap();
    assert!(
        !paths(&r.search(&Query::new("shipping remote carrier").principals(["admin"])).unwrap())
            .contains(&"shipping.txt".to_string())
    );
    r.clear_acl(&p).unwrap();
    assert_eq!(paths(&r.search(&Query::new("shipping remote carrier")).unwrap())[0], "shipping.txt");
}

#[test]
fn closed_policy_hides_documents_nobody_granted() {
    let dir = tempfile::tempdir().unwrap();
    let store = corpus(dir.path());
    let r = retriever(&store, (1 << 20, 1 << 24)).with_policy(Policy::Closed);
    assert!(r.search(&Query::new("refund damaged goods").mode(Mode::Hybrid)).unwrap().passages.is_empty());
    let p = store.documents("refunds", 0, 5).unwrap().0[0].path.clone();
    r.set_acl(&p, &["support"]).unwrap();
    let o = r.search(&Query::new("refund damaged goods").principals(["support"])).unwrap();
    assert_eq!(paths(&o), vec!["refunds.txt"]);
}

#[test]
fn near_duplicates_collapse_and_can_be_kept_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let store = corpus(dir.path());
    let r = retriever(&store, (1 << 20, 1 << 24));
    let on = r.search(&Query::new("return damaged goods refund")).unwrap();
    assert_eq!(paths(&on).iter().filter(|p| p.contains("refund") || *p == "copy.txt").count(), 1);
    let mut q = Query::new("return damaged goods refund");
    q.dedupe = false;
    assert_eq!(paths(&r.search(&q).unwrap()).iter().filter(|p| p.contains("refund") || *p == "copy.txt").count(), 2);
}

#[test]
fn streaming_from_disk_gives_the_same_answers_as_memory() {
    let dir = tempfile::tempdir().unwrap();
    let store = corpus(dir.path());
    let big = retriever(&store, (1 << 20, 1 << 24));
    let tiny = retriever(&store, (10, 20));
    assert_eq!(big.vector_mode().unwrap(), VectorMode::Resident);
    assert_eq!(tiny.vector_mode().unwrap(), VectorMode::Streaming);
    for q in ["invoise paymnt terms", "remote carrier weeks", "refund damaged goods"] {
        let a = big.search(&Query::new(q).mode(Mode::Dense)).unwrap();
        let b = tiny.search(&Query::new(q).mode(Mode::Dense)).unwrap();
        assert_eq!(paths(&a), paths(&b), "{q}");
    }
    assert_eq!(MemoryBudget::new(10, 20).used(), 0);
}

#[test]
fn resident_vectors_are_paid_for_and_released() {
    let dir = tempfile::tempdir().unwrap();
    let store = corpus(dir.path());
    let budget = MemoryBudget::new(1 << 20, 1 << 24);
    let e: Arc<dyn Embedder> = Arc::new(HashingEmbedder::default());
    let r = Retriever::open(&store, Some(e), budget.clone()).unwrap();
    r.sync_vectors(None).unwrap();
    assert_eq!(budget.used(), 0);
    r.vector_mode().unwrap();
    assert!(budget.used() > 0);
    drop(r);
    assert_eq!(budget.used(), 0);
}

#[test]
fn sync_is_incremental_and_follows_changes_and_deletions() {
    let dir = tempfile::tempdir().unwrap();
    let store = corpus(dir.path());
    let r = retriever(&store, (1 << 20, 1 << 24));
    let again = r.sync_vectors(None).unwrap();
    assert_eq!((again.embedded, again.removed), (0, 0));
    let docs = dir.path().join("docs");
    std::fs::write(
        docs.join("shipping.txt"),
        "Shipping rates\n\nExpress parcels arrive overnight anywhere in the country.",
    )
    .unwrap();
    std::fs::remove_file(docs.join("salaries.txt")).unwrap();
    store.index_path(&docs, IndexOptions::default(), None).unwrap();
    let s = r.sync_vectors(None).unwrap();
    assert!(s.embedded >= 1 && s.removed >= 1, "{s:?}");
    assert_eq!(r.vector_count().unwrap(), store.stats().unwrap().chunks);
    let o = r.search(&Query::new("overnight express parcels").mode(Mode::Dense)).unwrap();
    assert_eq!(paths(&o)[0], "shipping.txt");
    assert!(
        !paths(&r.search(&Query::new("executive compensation").mode(Mode::Dense)).unwrap())
            .contains(&"salaries.txt".to_string())
    );
}

#[test]
fn switching_embedder_discards_vectors_from_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = corpus(dir.path());
    let a = retriever(&store, (1 << 20, 1 << 24));
    let n = a.vector_count().unwrap();
    let e: Arc<dyn Embedder> = Arc::new(HashingEmbedder::new(128));
    let b = Retriever::open(&store, Some(e), MemoryBudget::new(1 << 20, 1 << 24)).unwrap();
    let s = b.sync_vectors(None).unwrap();
    assert_eq!(s.removed as i64, n);
    assert_eq!(b.vector_count().unwrap(), n);
    assert!(!paths(&b.search(&Query::new("invoise paymnt").mode(Mode::Dense)).unwrap()).is_empty());
}

#[test]
fn evidence_fits_the_budget_and_limits_passages_per_document() {
    let dir = tempfile::tempdir().unwrap();
    let store = corpus(dir.path());
    let r = retriever(&store, (1 << 20, 1 << 24));
    let (_, ev) = r.context(&Query::new("refund days invoice shipping carrier"), 60, 1).unwrap();
    assert!(!ev.is_empty());
    assert!(ev.iter().map(|e| e.tokens).sum::<usize>() <= 60, "{ev:?}");
    let mut docs: Vec<&str> = ev.iter().map(|e| e.path.as_str()).collect();
    docs.sort_unstable();
    docs.dedup();
    assert_eq!(docs.len(), ev.len());
    assert_eq!(ev[0].n, 1);
}

#[test]
fn a_grounded_answer_passes_and_an_invented_one_is_caught() {
    let dir = tempfile::tempdir().unwrap();
    let store = corpus(dir.path());
    let r = retriever(&store, (1 << 20, 1 << 24));
    let (_, ev) = r.context(&Query::new("payment terms invoice"), 400, 2).unwrap();
    let good = ground::check("Payment terms are net 30 days from the invoice date [1].", &ev, 0.6);
    assert!(good.supported_ratio > 0.99, "{good:?}");
    let bad = ground::check("Payment terms are net 90 days and include a 5% discount [1].", &ev, 0.6);
    assert!(bad.supported_ratio < 0.5, "{bad:?}");
}

// ---- remote embedder against a local fake server ----------------------------------------------------------------------

fn fake_embeddings_server(status: u16) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        let mut seen = Vec::new();
        l.set_nonblocking(false).unwrap();
        for _ in 0..2 {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = vec![0_u8; 65536];
            let mut got = 0;
            loop {
                let n = s.read(&mut buf[got..]).unwrap();
                got += n;
                let text = String::from_utf8_lossy(&buf[..got]).to_string();
                if let Some(i) = text.find("\r\n\r\n") {
                    let len = text
                        .to_lowercase()
                        .split("content-length:")
                        .nth(1)
                        .and_then(|x| x.split("\r\n").next())
                        .and_then(|x| x.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if got >= i + 4 + len {
                        seen.push(text);
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let last = seen.last().cloned().unwrap_or_default();
            let n_inputs = last
                .split("\"input\":[")
                .nth(1)
                .map_or(1, |x| x.split("],").next().unwrap_or("").matches("\",\"").count() + 1);
            let data: Vec<String> = (0..n_inputs)
                .map(|i| format!("{{\"index\":{i},\"embedding\":[0.5,{},0.25,0.1]}}", i as f64 / 10.0))
                .collect();
            let body = if status == 200 {
                format!("{{\"data\":[{}]}}", data.join(","))
            } else {
                "{\"error\":\"nope sk-leak\"}".to_string()
            };
            let reply = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            s.write_all(reply.as_bytes()).unwrap();
        }
        seen
    });
    (url, h)
}

#[test]
fn remote_embedder_speaks_the_openai_protocol_and_sends_the_key_only_as_a_header() {
    let (url, h) = fake_embeddings_server(200);
    let e = RemoteEmbedder::connect(&url, "text-embed-test", Some("sk-test-123".into())).unwrap();
    assert_eq!(e.dim(), 4);
    assert_eq!(e.id(), "remote:text-embed-test:4");
    let v = e.embed(&["alpha", "beta"]).unwrap();
    assert_eq!(v.len(), 2);
    assert!((v[1][1] - 0.1).abs() < 1e-6);
    let seen = h.join().unwrap();
    assert!(seen[1].contains("POST /embeddings"));
    assert!(seen[1].to_lowercase().contains("authorization: bearer sk-test-123"));
    assert!(seen[1].contains("\"model\":\"text-embed-test\""));
    assert!(!seen[1].split("\r\n\r\n").nth(1).unwrap().contains("sk-test-123"));
}

#[test]
fn remote_failures_are_errors_that_do_not_echo_the_server_body() {
    let (url, h) = fake_embeddings_server(500);
    let err = RemoteEmbedder::connect(&url, "m", Some("sk-x".into())).unwrap_err();
    assert!(matches!(err, EmbedError::Request(_)));
    assert!(!err.to_string().contains("sk-leak"));
    drop(h); // the server thread stays blocked on its second accept; the test process exiting ends it
}

// ---- a synthetic semantic embedder: proves the pipeline benefits from meaning, says nothing about real models ----------

#[derive(Debug)]
struct ConceptEmbedder;

impl Embedder for ConceptEmbedder {
    fn id(&self) -> String {
        "concepts-test".into()
    }

    fn dim(&self) -> usize {
        3
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let groups: [&[&str]; 3] = [
            &["refund", "reimbursement", "money back", "return"],
            &["invoice", "bill", "payment"],
            &["shipping", "delivery", "parcel"],
        ];
        Ok(texts
            .iter()
            .map(|t| {
                let t = t.to_lowercase();
                groups.iter().map(|g| g.iter().filter(|w| t.contains(*w)).count() as f32).collect()
            })
            .collect())
    }
}

#[test]
fn with_a_meaningful_embedder_a_query_sharing_no_words_still_finds_the_document() {
    let dir = tempfile::tempdir().unwrap();
    let store = corpus(dir.path());
    let r = Retriever::open(&store, Some(Arc::new(ConceptEmbedder)), MemoryBudget::new(1 << 20, 1 << 24)).unwrap();
    r.sync_vectors(None).unwrap();
    let q = "I want my money back";
    assert!(paths(&r.search(&Query::new(q).mode(Mode::Lexical).fuzzy(false)).unwrap()).is_empty());
    let o = r.search(&Query::new(q).mode(Mode::Hybrid)).unwrap();
    assert!(paths(&o)[0].contains("refund") || paths(&o)[0] == "copy.txt");
}
