//! End-to-end CLI smoke tests against the fixture repo.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use assert_cmd::Command;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/simple-ts")
}

#[test]
fn index_json_emits_chunks() {
    let output = Command::cargo_bin("spelunk")
        .unwrap()
        .args(["index", "--json"])
        .arg(fixture_root())
        .output()
        .unwrap();
    assert!(output.status.success());

    let chunks: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let chunks = chunks.as_array().unwrap();
    assert!(!chunks.is_empty());

    assert!(
        chunks
            .iter()
            .any(|c| c["name"] == "rateLimit" && c["path"] == "src/rateLimit.ts"),
        "expected the rateLimit chunk in JSON output"
    );
    assert!(
        chunks
            .iter()
            .all(|c| !c["path"].as_str().unwrap().starts_with("dist/")),
        "ignored files must not be indexed"
    );
    // Fields the --json contract promises.
    let first = &chunks[0];
    for field in ["path", "language", "kind", "start_line", "end_line", "text"] {
        assert!(first.get(field).is_some(), "missing field {field}");
    }
}

#[test]
fn index_prints_summary() {
    Command::cargo_bin("spelunk")
        .unwrap()
        .arg("index")
        .arg(fixture_root())
        .assert()
        .success()
        .stdout(predicates::str::contains("chunks"));
}

#[test]
fn search_is_not_implemented_yet() {
    Command::cargo_bin("spelunk")
        .unwrap()
        .arg("where is rate limiting implemented?")
        .assert()
        .failure()
        .stderr(predicates::str::contains("milestone 2"));
}
