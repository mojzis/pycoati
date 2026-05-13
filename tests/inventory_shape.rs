//! Schema-shape integration test for the Inventory output.
//!
//! Verifies that `coati::run_static` produces JSON whose top-level structure
//! exactly matches the `schema_version` "2" contract. Key-set equality is
//! asserted on `serde_json::Value` so cosmetic struct-field reordering does
//! not break the test.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;

fn fixture_path(rel: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(rel);
    p
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().expect("expected object").keys().cloned().collect()
}

#[test]
fn inventory_top_level_keys_match_schema_v2() {
    let path = fixture_path("tests/fixtures/simple/empty.py");
    let inv = coati::run_static(&path).expect("run_static on empty.py");
    let v = serde_json::to_value(&inv).expect("serialize inventory");

    let expected: BTreeSet<String> = [
        "schema_version",
        "project",
        "suite",
        "files",
        "test_functions",
        "sut_calls",
        "top_suspicious",
        "tool",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();

    assert_eq!(keys(&v), expected, "top-level keys must match schema v2");
}

#[test]
fn inventory_schema_version_is_string_two() {
    let path = fixture_path("tests/fixtures/simple/empty.py");
    let inv = coati::run_static(&path).expect("run_static on empty.py");
    let v = serde_json::to_value(&inv).expect("serialize inventory");
    assert_eq!(v["schema_version"], Value::String("2".to_string()));
}

#[test]
fn inventory_suite_fields_are_null_in_static_only_mode() {
    let path = fixture_path("tests/fixtures/simple/empty.py");
    let inv = coati::run_static(&path).expect("run_static on empty.py");
    let v = serde_json::to_value(&inv).expect("serialize inventory");

    let suite_keys: BTreeSet<String> =
        ["test_count", "runtime_seconds", "line_coverage_pct", "slowest_tests"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
    assert_eq!(keys(&v["suite"]), suite_keys);

    assert_eq!(v["suite"]["test_count"], Value::Null);
    assert_eq!(v["suite"]["runtime_seconds"], Value::Null);
    assert_eq!(v["suite"]["line_coverage_pct"], Value::Null);
    assert_eq!(v["suite"]["slowest_tests"], Value::Array(vec![]));
}

#[test]
fn inventory_tool_fields_are_static_only_defaults() {
    let path = fixture_path("tests/fixtures/simple/empty.py");
    let inv = coati::run_static(&path).expect("run_static on empty.py");
    let v = serde_json::to_value(&inv).expect("serialize inventory");

    let tool_keys: BTreeSet<String> = ["name", "version", "ran_pytest", "ran_coverage"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    assert_eq!(keys(&v["tool"]), tool_keys);

    assert_eq!(v["tool"]["name"], Value::String("coati".to_string()));
    assert_eq!(v["tool"]["ran_pytest"], Value::Bool(false));
    assert_eq!(v["tool"]["ran_coverage"], Value::Bool(false));
    assert_eq!(v["tool"]["version"], Value::String(env!("CARGO_PKG_VERSION").to_string()));
}

#[test]
fn inventory_project_fields_present() {
    let path = fixture_path("tests/fixtures/simple/empty.py");
    let inv = coati::run_static(&path).expect("run_static on empty.py");
    let v = serde_json::to_value(&inv).expect("serialize inventory");

    let project_keys: BTreeSet<String> =
        ["path", "name"].iter().map(|s| (*s).to_string()).collect();
    assert_eq!(keys(&v["project"]), project_keys);

    // The fixture lives under tests/fixtures/simple/, so `name` should be
    // the parent directory's basename and `path` should end with that
    // directory.
    assert_eq!(v["project"]["name"], Value::String("simple".to_string()));
    let project_path = v["project"]["path"].as_str().expect("project.path must be string");
    assert!(
        project_path.ends_with("tests/fixtures/simple"),
        "project.path={project_path:?} should end with tests/fixtures/simple"
    );
}

#[test]
fn inventory_sut_calls_and_top_suspicious_shape() {
    let path = fixture_path("tests/fixtures/simple/empty.py");
    let inv = coati::run_static(&path).expect("run_static on empty.py");
    let v = serde_json::to_value(&inv).expect("serialize inventory");

    let sut_keys: BTreeSet<String> =
        ["by_name", "top_called"].iter().map(|s| (*s).to_string()).collect();
    assert_eq!(keys(&v["sut_calls"]), sut_keys);
    assert_eq!(v["sut_calls"]["by_name"], Value::Array(vec![]));
    assert_eq!(v["sut_calls"]["top_called"], Value::Array(vec![]));

    let top_keys: BTreeSet<String> =
        ["test_functions", "files"].iter().map(|s| (*s).to_string()).collect();
    assert_eq!(keys(&v["top_suspicious"]), top_keys);
    assert_eq!(v["top_suspicious"]["test_functions"], Value::Array(vec![]));
    assert_eq!(v["top_suspicious"]["files"], Value::Array(vec![]));
}

#[test]
fn inventory_files_and_test_functions_are_arrays() {
    let path = fixture_path("tests/fixtures/simple/empty.py");
    let inv = coati::run_static(&path).expect("run_static on empty.py");
    let v = serde_json::to_value(&inv).expect("serialize inventory");

    assert!(v["files"].is_array(), "files must be array");
    assert!(v["test_functions"].is_array(), "test_functions must be array");
}
