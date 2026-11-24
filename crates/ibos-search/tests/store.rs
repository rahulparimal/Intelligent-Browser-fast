#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Index and search behaviour: ranking on a realistic corpus, robustness, concurrency, and settings interaction.

mod common;

use std::path::Path;

use ibos_search::settings::SettingsStore;
use ibos_search::{IndexOptions, LiteStore};
use serde_json::json;

fn opts() -> IndexOptions {
    IndexOptions::default()
}

fn store_in(dir: &Path) -> LiteStore {
    LiteStore::open(Some(&dir.join("idx").join("i.db")), None).unwrap()
}

fn corpus_store() -> (tempfile::TempDir, std::path::PathBuf, LiteStore) {
    let t = tempfile::tempdir().unwrap();
    let docs = t.path().join("docs");
    common::write_corpus(&docs);
    let s = store_in(t.path());
    assert_eq!(s.index_path(&docs, opts(), None).unwrap().indexed, 10);
    (t, docs, s)
}

#[test]
fn gold_queries_rank_the_right_document_first() {
    let (_t, _d, s) = corpus_store();
    let misses: Vec<_> = common::GOLD
        .iter()
        .filter(|(q, want)| s.search(q, Some(3), None).unwrap().first().is_none_or(|h| h.title != *want))
        .collect();
    assert!(misses.is_empty(), "wrong top result for: {misses:?}");
}

#[test]
fn mean_reciprocal_rank_is_perfect() {
    let (_t, _d, s) = corpus_store();
    let ranks: Vec<f64> = common::GOLD
        .iter()
        .map(|(q, want)| {
            s.search(q, Some(5), None)
                .unwrap()
                .iter()
                .position(|h| h.title == *want)
                .map_or(0.0, |i| 1.0 / (i as f64 + 1.0))
        })
        .collect();
    assert!((ranks.iter().sum::<f64>() / ranks.len() as f64 - 1.0).abs() < 1e-12);
}

#[test]
fn junk_and_syntax_queries_never_error() {
    let (_t, _d, s) = corpus_store();
    for q in [
        "NEAR(a b)",
        "\"; DROP TABLE docs; --",
        "c++",
        "leave\" OR title:* NEAR(",
        "\0",
        "🙂",
        &"a".repeat(600),
        "   ",
        "()",
        "*",
        "AND OR NOT",
    ] {
        assert!(s.search(q, None, None).is_ok(), "{q:?}");
    }
    assert_eq!(s.stats().unwrap().documents, 10);
    assert!(s.search("NEAR(a b)", None, None).unwrap().is_empty());
}

#[test]
fn each_document_appears_once_by_default() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("big");
    std::fs::create_dir_all(&d).unwrap();
    let long: Vec<String> = (0..30)
        .map(|i| {
            format!("Section {i}: the quarterly budget review covers budget planning and forecasts in detail. ")
                .repeat(3)
        })
        .collect();
    std::fs::write(d.join("long.md"), long.join("\n\n")).unwrap();
    std::fs::write(
        d.join("short.md"),
        "A short note about the budget for the year ahead and what it means for planning.",
    )
    .unwrap();
    let s = store_in(t.path());
    s.index_path(&d, opts(), None).unwrap();
    let grouped = s.search("budget", Some(10), None).unwrap();
    assert_eq!(grouped.len(), 2);
    assert!(s.search("budget", Some(10), Some(false)).unwrap().len() > 2);
}

#[test]
fn deleted_files_are_pruned_and_other_roots_are_untouched() {
    let (t, docs, s) = corpus_store();
    let other = t.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("x.md"), "Standalone note about gardening and composting for beginners at home.")
        .unwrap();
    s.index_path(&other, opts(), None).unwrap();
    std::fs::remove_file(docs.join("code-of-conduct.md")).unwrap();
    std::fs::remove_file(other.join("x.md")).unwrap();
    let st = s.index_path(&docs, opts(), None).unwrap();
    assert_eq!(st.removed, 1);
    assert!(s.search("retaliation", None, None).unwrap().is_empty());
    assert_eq!(s.stats().unwrap().documents, 10, "other/x.md is other's business until other is re-scanned");
    assert_eq!(s.index_path(&other, opts(), None).unwrap().removed, 1);
    assert_eq!(s.stats().unwrap().documents, 9);
}

#[test]
fn incremental_indexing_and_removal() {
    let (_t, docs, s) = corpus_store();
    assert_eq!(s.index_path(&docs, opts(), None).unwrap().unchanged, 10);
    std::fs::write(
        docs.join("leave-policy.md"),
        "Completely different text about parental leave lasting twelve weeks in total.",
    )
    .unwrap();
    assert_eq!(s.index_path(&docs, opts(), None).unwrap().indexed, 1);
    assert!(
        !s.search("twelve weeks", None, None).unwrap().is_empty()
            && s.search("carry over", None, None).unwrap().is_empty()
    );
    assert!(s.remove(&docs.join("leave-policy.md")).unwrap() && !s.remove(&docs.join("nope")).unwrap());
    assert!(s.index_path(&docs.join("missing"), opts(), None).is_err());
}

#[test]
fn empty_text_hidden_dirs_and_unreadable_files_are_reported() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("h");
    std::fs::create_dir_all(d.join(".git")).unwrap();
    std::fs::write(d.join(".git").join("config.txt"), "secret hidden words that should not be indexed at all here")
        .unwrap();
    std::fs::write(d.join("bad.pdf"), b"not a pdf").unwrap();
    std::fs::write(d.join("blank.txt"), "   \n\n  ").unwrap();
    std::fs::write(d.join("ok.md"), "Readable content about quarterly reports and annual planning for everyone.")
        .unwrap();
    std::fs::write(d.join("pic.png"), b"\x89PNG").unwrap();
    let s = store_in(t.path());
    let st = s.index_path(&d, opts(), None).unwrap();
    assert_eq!((st.indexed, st.failed, st.empty, st.skipped), (1, 1, 1, 1));
    assert!(s.search("secret hidden words", None, None).unwrap().is_empty());
}

#[test]
fn settings_drive_filters_chunking_and_staleness() {
    let t = tempfile::tempdir().unwrap();
    let docs = t.path().join("docs");
    common::write_corpus(&docs);
    let settings = SettingsStore::open(t.path().join("s.json"));
    let s = LiteStore::open(Some(&t.path().join("i.db")), Some(settings)).unwrap();
    let only_md = json!({"extensions": [".md"]});
    s.settings_store().update(only_md.as_object().unwrap(), false).unwrap();
    assert_eq!(s.index_path(&docs, opts(), None).unwrap().skipped, 2);
    assert!(s.search("holiday", None, None).unwrap().is_empty());
    assert!(!s.needs_reindex().unwrap());
    let more = json!({"chunk_chars": 400, "extensions": [".md", ".txt", ".html"]});
    s.settings_store().update(more.as_object().unwrap(), false).unwrap();
    assert!(s.needs_reindex().unwrap());
    s.index_path(&docs, IndexOptions { prune: true, force: true }, None).unwrap();
    assert!(!s.needs_reindex().unwrap() && !s.search("holiday", None, None).unwrap().is_empty());
}

#[test]
fn folders_reindex_and_filtering_of_documents() {
    let (t, docs, s) = corpus_store();
    let canon = s.add_folder(&docs).unwrap();
    assert_eq!(s.folders().unwrap(), std::slice::from_ref(&canon));
    std::fs::write(docs.join("new.md"), "A brand new note about onboarding schedules and hiring plans for teams.")
        .unwrap();
    let st = s.reindex_all(None, false).unwrap();
    assert_eq!((st.indexed, st.unchanged), (1, 10));
    let (found, total) = s.documents("leave", 0, 50).unwrap();
    assert_eq!((total, found[0].title.as_str()), (1, "leave-policy"));
    assert_eq!(s.documents("100%", 0, 50).unwrap().1, 0, "percent is literal, not a wildcard");
    assert_eq!(s.documents("", 0, 3).unwrap().0.len(), 3);
    assert_eq!(s.remove_folder(&docs, true).unwrap(), 11);
    assert_eq!(s.stats().unwrap().documents, 0);
    s.clear().unwrap();
    drop(t);
}

#[test]
fn concurrent_indexing_and_searching_is_safe() {
    let (t, _d, s) = corpus_store();
    let more = t.path().join("more");
    std::fs::create_dir_all(&more).unwrap();
    for i in 0..60 {
        std::fs::write(more.join(format!("n{i}.md")), format!("Note number {i}: quarterly planning, budget forecasts and hiring plans for team {i}, written for the review.")).unwrap();
    }
    let stop = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        let readers: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    let mut n = 0;
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        s.search("budget planning", None, None).unwrap();
                        s.stats().unwrap();
                        s.documents("n", 0, 20).unwrap();
                        n += 1;
                    }
                    n
                })
            })
            .collect();
        s.index_path(&more, opts(), None).unwrap();
        std::fs::remove_file(more.join("n1.md")).unwrap();
        s.index_path(&more, opts(), None).unwrap();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(readers.into_iter().all(|r| r.join().unwrap() > 0));
    });
    assert_eq!(s.stats().unwrap().documents, 10 + 59);
}

#[test]
fn larger_corpus_is_fast() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("many");
    std::fs::create_dir_all(&d).unwrap();
    let words: Vec<&str> = "budget policy travel leave security laptop onboarding benefits holiday remote expense review quarterly planning hiring".split(' ').collect();
    for i in 0..300 {
        let body: Vec<&str> = (0..120).map(|j| words[(i + j) % words.len()]).collect();
        std::fs::write(d.join(format!("doc{i}.txt")), format!("{} unique{i}token", body.join(" "))).unwrap();
    }
    let s = store_in(t.path());
    let t0 = std::time::Instant::now();
    assert_eq!(s.index_path(&d, opts(), None).unwrap().indexed, 300);
    let index_s = t0.elapsed().as_secs_f64();
    let t0 = std::time::Instant::now();
    for _ in 0..50 {
        s.search("quarterly planning budget", None, None).unwrap();
    }
    let per_query_ms = t0.elapsed().as_secs_f64() * 1000.0 / 50.0;
    assert_eq!(s.search("unique217token", None, None).unwrap()[0].title, "doc217");
    assert!(index_s < 60.0 && per_query_ms < 100.0, "index {index_s}s, query {per_query_ms}ms");
}

#[test]
fn old_schema_is_migrated_not_crashed() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("old.db");
    {
        let db = rusqlite::Connection::open(&p).unwrap();
        db.execute_batch("CREATE TABLE docs(id INTEGER PRIMARY KEY, path TEXT UNIQUE, title TEXT, sha TEXT); CREATE VIRTUAL TABLE chunks USING fts5(text, title, doc_id UNINDEXED, page UNINDEXED); INSERT INTO docs(path,title,sha) VALUES('/x/a.md','a','abc'); PRAGMA user_version=1;").unwrap();
    }
    let s = LiteStore::open(Some(&p), None).unwrap();
    assert_eq!(s.stats().unwrap().documents, 1);
    assert!(s.search("anything", None, None).unwrap().is_empty());
}
