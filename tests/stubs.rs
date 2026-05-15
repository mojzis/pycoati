//! Phase 2 — `stubs_count` integration tests.
//!
//! Runs the `pycoati` binary against `tests/fixtures/stubs_style/` and
//! verifies that fixture-driven patching (`monkeypatch.*`, `mocker.*`) is
//! detected, aggregated to `stubs_count` at both per-test and per-file
//! scope, fires `mock_overuse`, serializes through the JSON output, and
//! appears as a `stubs` column in the pretty renderer.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::Value;

fn fixture_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/stubs_style");
    p
}

fn run_pycoati_json() -> Value {
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(fixture_root())
        .arg("--tests-dir")
        .arg(fixture_root())
        .arg("--static-only")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    serde_json::from_str(&stdout).expect("stdout must be valid JSON")
}

fn run_pycoati_pretty() -> String {
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(fixture_root())
        .arg("--tests-dir")
        .arg(fixture_root())
        .arg("--static-only")
        .arg("--format")
        .arg("pretty")
        .assert()
        .success();
    String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout")
}

#[test]
fn stubs_count_populated_on_test_records() {
    let v = run_pycoati_json();
    let tfs = v["test_functions"].as_array().expect("test_functions array");
    let by_name: std::collections::BTreeMap<&str, u64> = tfs
        .iter()
        .map(|t| {
            let name = t["nodeid"].as_str().unwrap_or("").rsplit("::").next().unwrap_or("");
            let stubs = t["stubs_count"].as_u64().expect("stubs_count u64");
            (name, stubs)
        })
        .collect();
    assert_eq!(by_name.get("test_monkeypatch_basic"), Some(&1));
    assert_eq!(by_name.get("test_monkeypatch_heavy"), Some(&4));
    assert_eq!(by_name.get("test_mocker_patch"), Some(&3));
}

#[test]
fn stubs_count_aggregated_on_file_record() {
    let v = run_pycoati_json();
    let files = v["files"].as_array().expect("files array");
    let file = files
        .iter()
        .find(|f| f["path"].as_str().is_some_and(|p| p.ends_with("test_stubs.py")))
        .expect("file record for test_stubs.py");
    // 1 + 4 + 3 = 8 stub call sites across the file.
    assert_eq!(file["stubs_count"].as_u64().expect("stubs_count u64"), 8);
}

#[test]
fn stub_heavy_test_fires_mock_overuse_smell() {
    let v = run_pycoati_json();
    let tfs = v["test_functions"].as_array().expect("test_functions array");
    let rec = tfs
        .iter()
        .find(|t| t["nodeid"].as_str().is_some_and(|s| s.ends_with("::test_monkeypatch_heavy")))
        .expect("record for test_monkeypatch_heavy");
    let categories: Vec<&str> = rec["smell_hits"]
        .as_array()
        .expect("smell_hits array")
        .iter()
        .filter_map(|h| h["category"].as_str())
        .collect();
    assert!(
        categories.contains(&"mock_overuse"),
        "expected mock_overuse on test_monkeypatch_heavy (4 stubs, 1 assert), got {categories:?}"
    );
}

#[test]
fn file_level_mock_overuse_fires_on_stub_heavy_file() {
    let v = run_pycoati_json();
    let files = v["files"].as_array().expect("files array");
    let file = files
        .iter()
        .find(|f| f["path"].as_str().is_some_and(|p| p.ends_with("test_stubs.py")))
        .expect("file record for test_stubs.py");
    let categories: Vec<&str> = file["smell_hits"]
        .as_array()
        .expect("smell_hits array")
        .iter()
        .filter_map(|h| h["category"].as_str())
        .collect();
    // 8 stubs vs 3 asserts: bound = max(3, 2) = 3, 8 > 3, 8/3 ≈ 2.67 > 2.0.
    assert!(
        categories.contains(&"mock_overuse"),
        "expected file-level mock_overuse on test_stubs.py, got {categories:?}"
    );
}

#[test]
fn stubs_count_serializes_in_json_output() {
    // Tighten the JSON contract: every TestRecord and FileRecord must
    // expose `stubs_count` as a u64 (never null, never absent).
    let v = run_pycoati_json();
    for t in v["test_functions"].as_array().expect("test_functions array") {
        assert!(
            t["stubs_count"].is_u64(),
            "every TestRecord must serialize stubs_count as a number, got {t}"
        );
    }
    for f in v["files"].as_array().expect("files array") {
        assert!(
            f["stubs_count"].is_u64(),
            "every FileRecord must serialize stubs_count as a number, got {f}"
        );
    }
    // Schema version stays at "2" — adding fields inside existing records
    // is permitted by the schema-v2 lock.
    assert_eq!(v["schema_version"], Value::String("2".to_string()));
}

#[test]
fn pretty_output_contains_stubs_column() {
    let out = run_pycoati_pretty();
    // Both the tests and files headers expose a `stubs` column.
    let header_lines: Vec<&str> =
        out.lines().filter(|l| l.contains("score") && l.contains("stubs")).collect();
    assert!(
        header_lines.len() >= 2,
        "expected at least two `stubs`-bearing headers (tests + files), got {} in:\n{out}",
        header_lines.len()
    );
    // The stub-heavy test's value (`4`) renders somewhere — its nodeid line
    // must contain a `4` after the assertion column.
    let row = out
        .lines()
        .find(|l| l.contains("::test_monkeypatch_heavy"))
        .expect("row for test_monkeypatch_heavy rendered");
    assert!(
        row.contains(" 4 "),
        "expected stubs value 4 in test_monkeypatch_heavy row, got: {row:?}"
    );
}
