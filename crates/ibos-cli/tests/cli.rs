//! The binary as a user runs it: index, search, doctor and error handling, against an isolated config directory.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::*;

fn cmd(home: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("intelli-rust").unwrap();
    c.env("INTELLI_HOME", home).env_remove("INTELLI_LOG");
    c
}

fn corpus(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("leave.md"),
        "Employees receive twenty five days of annual leave per year.\n\nCarry over is capped at five days.",
    )
    .unwrap();
    std::fs::write(
        dir.join("security.md"),
        "All laptops must use encryption. MFA is mandatory for e-mail and VPN access everywhere.",
    )
    .unwrap();
}

#[test]
fn help_and_version() {
    let t = tempfile::tempdir().unwrap();
    cmd(t.path())
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("search").and(predicate::str::contains("admin")));
    cmd(t.path()).arg("--version").assert().success().stdout(predicate::str::contains("intelli-rust"));
}

#[test]
fn index_then_search_text_and_json() {
    let t = tempfile::tempdir().unwrap();
    let docs = t.path().join("docs");
    corpus(&docs);
    cmd(t.path())
        .args(["index"])
        .arg(&docs)
        .assert()
        .success()
        .stdout(predicate::str::contains("\"indexed\":2").and(predicate::str::contains("documents: 2")));
    cmd(t.path())
        .args(["search", "annual leave"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[1] leave").and(predicate::str::contains("annual leave")));
    cmd(t.path())
        .args(["search", "email", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"title\": \"security\""));
    cmd(t.path()).args(["search", "zzzxqv"]).assert().success().stdout("No results.\n");
    cmd(t.path()).args(["index"]).arg(&docs).assert().success().stdout(predicate::str::contains("\"unchanged\":2"));
}

#[test]
fn missing_paths_and_bad_usage_fail_clearly() {
    let t = tempfile::tempdir().unwrap();
    cmd(t.path())
        .args(["index", "/no/such/path"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found or unreadable"));
    cmd(t.path()).arg("index").assert().failure().code(2);
    cmd(t.path()).arg("frobnicate").assert().failure().code(2);
}

#[test]
fn doctor_reports_config_and_index() {
    let t = tempfile::tempdir().unwrap();
    cmd(t.path())
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .arg("doctor")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("[ok  ] config dir")
                .and(predicate::str::contains("[warn] api key"))
                .and(predicate::str::contains("0 documents")),
        );
    cmd(t.path())
        .env("OPENAI_API_KEY", "x")
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("[ok  ] api key: OPENAI_API_KEY"));
}
