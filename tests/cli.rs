//! End-to-end CLI tests for the `coati` binary. Exercises argument parsing,
//! stdout-vs-`--output` routing, exit codes, and error formatting that the
//! library-level tests cannot reach.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;

fn fixture_path(rel: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(rel);
    p
}

#[test]
fn happy_path_prints_valid_json_to_stdout() {
    let fixture = fixture_path("tests/fixtures/simple/test_basic.py");
    let assert =
        Command::cargo_bin("coati").expect("binary built").arg(&fixture).assert().success();

    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be valid UTF-8");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    assert_eq!(v["schema_version"], serde_json::Value::String("2".to_string()));
    assert_eq!(v["tool"]["name"], serde_json::Value::String("coati".to_string()));
}

#[test]
fn missing_path_returns_nonzero_and_writes_to_stderr() {
    let bogus = fixture_path("tests/fixtures/this/path/does/not/exist.py");
    Command::cargo_bin("coati")
        .expect("binary built")
        .arg(&bogus)
        .assert()
        .failure()
        .stderr(predicate::str::starts_with("coati:"));
}

#[test]
fn output_flag_writes_to_file_and_omits_stdout() {
    let fixture = fixture_path("tests/fixtures/simple/test_basic.py");
    let tmp = tempfile::tempdir().expect("create tempdir");
    let out_path = tmp.path().join("inventory.json");

    let assert = Command::cargo_bin("coati")
        .expect("binary built")
        .arg(&fixture)
        .arg("--output")
        .arg(&out_path)
        .assert()
        .success();

    assert_eq!(assert.get_output().stdout.len(), 0, "stdout must be empty when --output is set");

    let contents = std::fs::read_to_string(&out_path).expect("read output file");
    let v: serde_json::Value =
        serde_json::from_str(&contents).expect("output file must be valid JSON");
    assert_eq!(v["schema_version"], serde_json::Value::String("2".to_string()));
}
