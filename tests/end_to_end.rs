//! End-to-end test for Phase 2 deliverables: file walker, project-name
//! discovery via `pyproject.toml`, and the `only_asserts_on_mock` predicate.
//!
//! Runs the `coati` binary against `tests/fixtures/project/` and verifies the
//! emitted inventory against the `# coati-expected: ...` annotations in each
//! fixture test file.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::Value;

fn fixture_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/project");
    p
}

/// Per-file expectations extracted from a `# coati-expected: ...` header.
#[derive(Debug, Default, Clone)]
struct FileExpectations {
    tests: u64,
    asserts: u64,
    /// Test name that must have `only_asserts_on_mock = true`. When `None`,
    /// every test in the file must have `only_asserts_on_mock = false`.
    only_mock: Option<String>,
}

fn parse_expectations(source: &str) -> FileExpectations {
    let header = source
        .lines()
        .find_map(|l| l.trim_start_matches('#').trim().strip_prefix("coati-expected:"))
        .expect("fixture must declare a `# coati-expected:` header");
    let mut exp = FileExpectations::default();
    for kv in header.split_whitespace() {
        let (k, v) = kv.split_once('=').expect("token must be key=value");
        match k {
            "tests" => exp.tests = v.parse().expect("tests= must be a u64"),
            "asserts" => exp.asserts = v.parse().expect("asserts= must be a u64"),
            "only_mock" => exp.only_mock = Some(v.to_string()),
            _ => panic!("unknown coati-expected key: {k}"),
        }
    }
    exp
}

fn read_fixture(rel: &str) -> String {
    let mut p = fixture_root();
    p.push(rel);
    std::fs::read_to_string(&p).expect("read fixture")
}

fn run_coati() -> Value {
    let assert =
        Command::cargo_bin("coati").expect("binary built").arg(fixture_root()).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    serde_json::from_str(&stdout).expect("stdout must be valid JSON")
}

#[test]
fn inventory_reports_project_name_from_pyproject() {
    let v = run_coati();
    assert_eq!(v["project"]["name"], Value::String("myproj".to_string()));
}

#[test]
fn walker_discovers_five_test_files_and_ignores_helpers() {
    let v = run_coati();
    let files = v["files"].as_array().expect("files must be array");
    let names: Vec<String> = files
        .iter()
        .map(|f| {
            f["path"]
                .as_str()
                .expect("file.path is string")
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    assert_eq!(files.len(), 5, "expected 5 discovered test files, got {names:?}");
    assert!(
        !names.iter().any(|n| n == "helpers.py"),
        "helpers.py must not be discovered: {names:?}"
    );
    for expected in [
        "test_mock_only.py",
        "test_real.py",
        "test_mixed.py",
        "test_no_asserts.py",
        "other_test.py",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "expected {expected} in files list, got {names:?}"
        );
    }
}

#[test]
fn per_file_assertion_counts_match_annotations() {
    let v = run_coati();
    let files = v["files"].as_array().expect("files must be array");

    let by_basename: BTreeMap<String, &Value> = files
        .iter()
        .map(|f| {
            let path = f["path"].as_str().expect("file.path is string");
            let base = path.rsplit('/').next().unwrap_or("").to_string();
            (base, f)
        })
        .collect();

    for fixture in [
        "test_mock_only.py",
        "test_real.py",
        "test_mixed.py",
        "test_no_asserts.py",
        "other_test.py",
    ] {
        let source = read_fixture(&format!("tests/{fixture}"));
        let exp = parse_expectations(&source);
        let fr = by_basename.get(fixture).unwrap_or_else(|| panic!("no FileRecord for {fixture}"));
        assert_eq!(
            fr["test_count"].as_u64().expect("test_count u64"),
            exp.tests,
            "test_count mismatch for {fixture}"
        );
        assert_eq!(
            fr["assertion_count"].as_u64().expect("assertion_count u64"),
            exp.asserts,
            "assertion_count mismatch for {fixture}"
        );
    }
}

#[test]
fn only_asserts_on_mock_predicate_matches_annotations() {
    let v = run_coati();
    let tests = v["tests"].as_array().expect("tests must be array");

    let by_test_name: BTreeMap<String, &Value> = tests
        .iter()
        .map(|t| {
            let nodeid = t["nodeid"].as_str().expect("nodeid string");
            let name = nodeid.rsplit("::").next().unwrap_or("").to_string();
            (name, t)
        })
        .collect();

    // From the fixture annotations: test_repo_save_called is the only one
    // with only_asserts_on_mock = true.
    let true_names: std::collections::BTreeSet<&str> =
        std::iter::once("test_repo_save_called").collect();

    for (name, rec) in &by_test_name {
        let actual = rec["only_asserts_on_mock"].as_bool().expect("bool predicate");
        let expected = true_names.contains(name.as_str());
        assert_eq!(
            actual, expected,
            "only_asserts_on_mock mismatch for {name}: expected {expected}, got {actual}"
        );
    }
}

#[test]
fn nodeids_are_relative_to_project_root() {
    let v = run_coati();
    let tests = v["tests"].as_array().expect("tests must be array");
    for t in tests {
        let nodeid = t["nodeid"].as_str().expect("nodeid string");
        assert!(
            nodeid.starts_with("tests/"),
            "expected nodeid relative to project root (tests/...), got {nodeid:?}"
        );
        assert!(nodeid.contains("::"), "nodeid must contain `::`, got {nodeid:?}");
    }
}

#[test]
fn schema_invariants_preserved_under_walker_mode() {
    let v = run_coati();
    assert_eq!(v["schema_version"], Value::String("1".to_string()));
    assert_eq!(v["tool"]["ran_pytest"], Value::Bool(false));
    assert_eq!(v["tool"]["ran_coverage"], Value::Bool(false));
    assert_eq!(v["suite"]["test_count"], Value::Null);
    assert!(v["sut_calls"]["by_name"].as_array().expect("array").is_empty());
}

#[test]
fn tests_dir_override_flag_accepts_path() {
    let root = fixture_root();
    let assert = Command::cargo_bin("coati")
        .expect("binary built")
        .arg(&root)
        .arg("--tests-dir")
        .arg(root.join("tests"))
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8");
    let v: Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v["files"].as_array().expect("files array").len(), 5);
}

#[test]
fn project_package_flag_is_accepted_as_no_op() {
    let assert = Command::cargo_bin("coati")
        .expect("binary built")
        .arg(fixture_root())
        .arg("--project-package")
        .arg("myproj")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8");
    let _v: Value = serde_json::from_str(&stdout).expect("valid json");
}
