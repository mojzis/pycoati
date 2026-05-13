//! End-to-end tests for Run 3 deliverables: suspicion scoring, top-N
//! rankings, and `--format pretty` output.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::Value;

fn fixture_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/project");
    p
}

fn run_coati_json(extra_args: &[&str]) -> Value {
    let mut cmd = Command::cargo_bin("coati").expect("binary built");
    cmd.arg(fixture_root()).arg("--static-only");
    for a in extra_args {
        cmd.arg(a);
    }
    let assert = cmd.assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    serde_json::from_str(&stdout).expect("stdout must be valid JSON")
}

fn run_coati_pretty(extra_args: &[&str]) -> String {
    let mut cmd = Command::cargo_bin("coati").expect("binary built");
    cmd.arg(fixture_root()).arg("--static-only").arg("--format").arg("pretty");
    for a in extra_args {
        cmd.arg(a);
    }
    let assert = cmd.assert().success();
    String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout")
}

#[test]
fn top_suspicious_contains_mock_only_test() {
    let v = run_coati_json(&[]);
    let nodeids: Vec<&str> = v["top_suspicious"]["test_functions"]
        .as_array()
        .expect("test_functions array")
        .iter()
        .filter_map(|x| x.as_str())
        .collect();
    assert!(
        nodeids.contains(&"tests/test_mock_only.py::test_repo_save_called"),
        "expected mock_only test in top_suspicious, got {nodeids:?}"
    );
}

#[test]
fn top_suspicious_contains_overmocked_test() {
    let v = run_coati_json(&[]);
    let nodeids: Vec<&str> = v["top_suspicious"]["test_functions"]
        .as_array()
        .expect("test_functions array")
        .iter()
        .filter_map(|x| x.as_str())
        .collect();
    assert!(
        nodeids.contains(&"tests/test_overmocked.py::test_three_mocks_one_assert"),
        "expected overmocked test in top_suspicious, got {nodeids:?}"
    );
}

#[test]
fn top_suspicious_caps_at_twenty_by_default() {
    let v = run_coati_json(&[]);
    let n = v["top_suspicious"]["test_functions"].as_array().expect("test_functions array").len();
    assert!(n <= 20, "expected at most 20 entries, got {n}");
}

#[test]
fn top_suspicious_n_cli_override() {
    let v = run_coati_json(&["--top-suspicious", "3"]);
    let n = v["top_suspicious"]["test_functions"].as_array().expect("test_functions array").len();
    assert!(n <= 3, "expected at most 3 entries, got {n}");
}

#[test]
fn top_suspicious_files_also_populated() {
    let v = run_coati_json(&[]);
    let files: Vec<&str> = v["top_suspicious"]["files"]
        .as_array()
        .expect("files array")
        .iter()
        .filter_map(|x| x.as_str())
        .collect();
    assert!(!files.is_empty(), "expected at least one file in top_suspicious.files");
    // The overmocked file is the most-suspicious: 3 Mock() constructions
    // against 1 assert → mock_overuse smell + a healthy mean-of-tests.
    assert!(
        files.contains(&"tests/test_overmocked.py"),
        "expected test_overmocked.py among the top suspicious files, got {files:?}"
    );
}

#[test]
fn suspicion_score_max_above_threshold() {
    let v = run_coati_json(&[]);
    let tfs = v["test_functions"].as_array().expect("test_functions array");
    let max = tfs.iter().filter_map(|t| t["suspicion_score"].as_f64()).fold(0.0_f64, f64::max);
    assert!(max > 0.4, "expected max suspicion_score > 0.4, got {max}");
}

#[test]
fn suspicion_scores_written_back_to_test_records() {
    let v = run_coati_json(&[]);
    let tfs = v["test_functions"].as_array().expect("test_functions array");
    // Every test must carry a numeric suspicion_score (never null).
    for t in tfs {
        let s = t["suspicion_score"].as_f64().expect("suspicion_score numeric");
        assert!(s >= 0.0, "suspicion_score must be non-negative, got {s}");
    }
}

#[test]
fn format_pretty_writes_aligned_columns_no_markdown() {
    let out = run_coati_pretty(&[]);
    assert!(out.starts_with("coati audit"), "pretty output must start with title, got:\n{out}");
    assert!(out.contains("Suite\n-----"), "must contain Suite header, got:\n{out}");
    assert!(
        out.contains("Top suspicious tests\n--------------------"),
        "must contain Top suspicious tests header, got:\n{out}"
    );
    assert!(!out.contains('|'), "no pipe characters allowed, got:\n{out}");
}

#[test]
fn format_pretty_and_json_agree_on_top_suspicious() {
    // Same invocation arguments. Compare top-1 nodeid.
    let v = run_coati_json(&[]);
    let top_json = v["top_suspicious"]["test_functions"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .expect("top_suspicious.test_functions[0] string")
        .to_string();
    let pretty = run_coati_pretty(&[]);
    assert!(
        pretty.contains(&top_json),
        "pretty output must mention top-1 nodeid {top_json:?}, got:\n{pretty}"
    );
}
