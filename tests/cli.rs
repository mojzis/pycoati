//! End-to-end CLI tests for the `pycoati` binary. Exercises argument parsing,
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
        Command::cargo_bin("pycoati").expect("binary built").arg(&fixture).assert().success();

    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be valid UTF-8");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    assert_eq!(v["schema_version"], serde_json::Value::String("2".to_string()));
    assert_eq!(v["tool"]["name"], serde_json::Value::String("pycoati".to_string()));
}

#[test]
fn missing_path_returns_nonzero_and_writes_to_stderr() {
    let bogus = fixture_path("tests/fixtures/this/path/does/not/exist.py");
    Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&bogus)
        .assert()
        .failure()
        .stderr(predicate::str::starts_with("pycoati:"));
}

#[test]
fn format_json_emits_valid_json() {
    let fixture = fixture_path("tests/fixtures/simple/test_basic.py");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&fixture)
        .arg("--format")
        .arg("json")
        .assert()
        .success();
    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be valid UTF-8");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    assert_eq!(v["schema_version"], serde_json::Value::String("2".to_string()));
}

#[test]
fn format_pretty_exit_zero_non_empty_stdout() {
    let fixture = fixture_path("tests/fixtures/simple/test_basic.py");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&fixture)
        .arg("--format")
        .arg("pretty")
        .assert()
        .success();
    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be valid UTF-8");
    assert!(!stdout.is_empty(), "pretty stdout must be non-empty");
    assert!(
        stdout.starts_with("pycoati audit"),
        "pretty output must start with `pycoati audit`, got: {stdout:?}"
    );
}

#[test]
fn default_format_is_json_when_unset() {
    // No `--format` flag → output parses as JSON (the default).
    let fixture = fixture_path("tests/fixtures/simple/test_basic.py");
    let assert =
        Command::cargo_bin("pycoati").expect("binary built").arg(&fixture).assert().success();
    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be valid UTF-8");
    serde_json::from_str::<serde_json::Value>(&stdout).expect("default output must parse as JSON");
}

#[test]
fn output_flag_writes_to_file_and_omits_stdout() {
    let fixture = fixture_path("tests/fixtures/simple/test_basic.py");
    let tmp = tempfile::tempdir().expect("create tempdir");
    let out_path = tmp.path().join("inventory.json");

    let assert = Command::cargo_bin("pycoati")
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
