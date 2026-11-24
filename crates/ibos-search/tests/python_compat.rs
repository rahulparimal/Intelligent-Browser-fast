//! Cross-implementation compatibility: an index written by the Python implementation is read by Rust (and the reverse),
//! with no re-index and no "stale" flag. Skipped when Python is unavailable unless `IBOS_REQUIRE_PYTHON_COMPAT=1`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use ibos_search::{IndexOptions, LiteStore};

fn repo_root() -> PathBuf {
    std::env::var_os("IBOS_PY_REPO")
        .map_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."), std::path::PathBuf::from)
        .canonicalize()
        .unwrap()
}

/// Run a Python snippet with the repo's package importable. `None` if Python or the package is unavailable.
fn python(code: &str, env: &[(&str, &Path)]) -> Option<String> {
    let mut cmd = Command::new("python3");
    cmd.arg("-c").arg(code).env("PYTHONPATH", repo_root()).env("ANONYMIZED_TELEMETRY", "false");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn available() -> bool {
    let ok = python("import intelli_browser_os.lite.store", &[]).is_some();
    assert!(
        ok || std::env::var("IBOS_REQUIRE_PYTHON_COMPAT").is_err(),
        "Python compatibility check required but Python/package is unavailable"
    );
    if !ok {
        eprintln!("skipping: python3 or the intelli_browser_os package is not available");
    }
    ok
}

#[test]
fn rust_reads_an_index_written_by_python() {
    if !available() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let docs = t.path().join("docs");
    common::write_corpus(&docs);
    let db = t.path().join("lite").join("index.db");
    let code = "import sys; from pathlib import Path; from intelli_browser_os.lite.store import LiteStore; s = LiteStore(Path(sys.argv[1])); print(s.index_path(sys.argv[2]))";
    let out =
        Command::new("python3").args(["-c", code]).arg(&db).arg(&docs).env("PYTHONPATH", repo_root()).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let s = LiteStore::open(Some(&db), None).unwrap();
    assert_eq!(s.stats().unwrap().documents, 10);
    assert!(!s.needs_reindex().unwrap(), "same settings signature: the Python-built index must not look stale");
    for (q, want) in common::GOLD {
        let hits = s.search(q, Some(3), None).unwrap();
        assert_eq!(hits.first().map(|h| h.title.as_str()), Some(want), "query {q:?}");
    }
    let again = s.index_path(&docs, IndexOptions::default(), None).unwrap();
    assert_eq!((again.unchanged, again.indexed), (10, 0), "identical content hashes: nothing is re-read");
}

#[test]
fn python_reads_an_index_written_by_rust() {
    if !available() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let docs = t.path().join("docs");
    common::write_corpus(&docs);
    let db = t.path().join("lite").join("index.db");
    LiteStore::open(Some(&db), None).unwrap().index_path(&docs, IndexOptions::default(), None).unwrap();

    let code = "import sys, json; from pathlib import Path; from intelli_browser_os.lite.store import LiteStore; s = LiteStore(Path(sys.argv[1])); \
                print(json.dumps({'docs': s.stats()['documents'], 'stale': s.needs_reindex(), 'top': s.search('annual leave')[0].title, 'email': s.search('email')[0].title}))";
    let out = Command::new("python3").args(["-c", code]).arg(&db).env("PYTHONPATH", repo_root()).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v, serde_json::json!({"docs": 10, "stale": false, "top": "leave-policy", "email": "security"}));
}
