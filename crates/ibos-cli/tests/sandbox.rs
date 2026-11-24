//! Document extraction runs in a restricted worker process, started by the real binary.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;

fn cmd(home: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("intelli-rust").unwrap();
    c.env("INTELLI_HOME", home).env_remove("INTELLI_LOG").env_remove("INTELLI_NO_EXTRACT_SANDBOX");
    c
}

#[test]
fn indexing_through_the_worker_works_and_a_bad_file_does_not_stop_the_run() {
    let home = tempfile::tempdir().unwrap();
    let docs = tempfile::tempdir().unwrap();
    std::fs::write(docs.path().join("good.md"), "# Leave\n\nStaff get twenty five days of leave.").unwrap();
    std::fs::write(
        docs.path().join("page.html"),
        "<html><title>Handbook</title><body><p>Remote work is allowed.</p></body></html>",
    )
    .unwrap();
    std::fs::write(docs.path().join("broken.pdf"), b"%PDF-1.4 this is not really a pdf \x00\xff\xfe").unwrap();
    std::fs::write(docs.path().join("broken.docx"), b"PK\x03\x04 not a real zip").unwrap();
    let out = cmd(home.path()).arg("index").arg(docs.path()).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("\"indexed\":2") && text.contains("\"failed\":2"), "{text}");
    cmd(home.path())
        .args(["search", "twenty five days", "--mode", "keyword"])
        .assert()
        .success()
        .stdout(predicates::str::contains("good"));
    cmd(home.path()).args(["search", "remote work"]).assert().success().stdout(predicates::str::contains("Handbook"));
}

#[test]
fn the_worker_cannot_read_other_files_or_connect_out_where_the_kernel_allows_it() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello").unwrap();
    let out = Command::cargo_bin("intelli-rust").unwrap().args(["__sandbox-selftest"]).arg(&file).output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let applied = v["applied"].as_str().unwrap();
    assert!(applied.contains("no-core-dumps"), "{v}");
    if applied.contains("landlock") {
        assert_eq!(v["other_file_readable"], false, "{v}");
        assert_eq!(v["tcp_allowed"], false, "{v}");
    } else {
        eprintln!("this kernel does not enforce Landlock; file and network restriction not tested: {v}");
    }
}

#[test]
fn the_worker_protocol_round_trips_through_the_binary() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("note.md");
    std::fs::write(&file, "plain words").unwrap();
    let out =
        Command::cargo_bin("intelli-rust").unwrap().arg("__extract-worker").arg(&file).env_clear().output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["result"], "ok");
    assert_eq!(v["pages"][0], "plain words");
    let unsupported = dir.path().join("x.xyz");
    std::fs::write(&unsupported, "x").unwrap();
    let out = Command::cargo_bin("intelli-rust")
        .unwrap()
        .arg("__extract-worker")
        .arg(&unsupported)
        .env_clear()
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!((v["result"].as_str(), v["kind"].as_str()), (Some("error"), Some("unsupported")));
}
