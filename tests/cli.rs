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

// ---------------------------------------------------------------------------
// Run 4 — uv workspace support
// ---------------------------------------------------------------------------

#[test]
fn workspace_root_emits_wrapper_with_two_members() {
    let fixture = fixture_path("tests/fixtures/uv_workspace");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&fixture)
        .arg("--static-only")
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    // Workspace shape: top-level `workspace_root` + `members` array.
    assert!(v.get("workspace_root").is_some(), "workspace_root must be present, got: {v:#?}");
    let members = v["members"].as_array().expect("members array");
    assert_eq!(members.len(), 2, "expected 2 members, got: {members:?}");

    let names: Vec<String> =
        members.iter().map(|m| m["project"]["name"].as_str().unwrap().to_string()).collect();
    assert!(names.contains(&"pkg_a".to_string()), "missing pkg_a, got: {names:?}");
    assert!(names.contains(&"pkg_b".to_string()), "missing pkg_b, got: {names:?}");

    // Every member must have at least one discovered test function — the
    // fixture ships real test_*.py files with assertions.
    for m in members {
        let tests = m["test_functions"].as_array().expect("test_functions array");
        assert!(
            !tests.is_empty(),
            "expected non-empty test_functions for member {:?}, got: {m:#?}",
            m["project"]["name"]
        );
    }
}

#[test]
fn single_project_regression_keeps_bare_inventory_shape() {
    // A non-workspace project dir must still produce a bare `Inventory`
    // (no `workspace_root` key). Use the existing single-project fixture.
    let fixture = fixture_path("tests/fixtures/project");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&fixture)
        .arg("--static-only")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    assert!(
        v.get("workspace_root").is_none(),
        "single-project payload must not carry workspace_root, got: {v:#?}"
    );
    assert_eq!(v["schema_version"], serde_json::Value::String("2".to_string()));
}

#[test]
fn empty_dir_with_no_tests_or_workspace_is_hard_error() {
    // The new strict-missing-tests contract: a directory that has neither
    // a tests/ dir nor a uv-workspace declaration must exit non-zero with
    // a clear error message, NOT silently emit an empty inventory.
    let tmp = tempfile::tempdir().expect("tempdir");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(tmp.path())
        .arg("--static-only")
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    assert!(stdout.is_empty(), "stdout must stay empty on hard error, got: {stdout:?}");
    assert!(
        stderr.contains("no tests directory"),
        "stderr must mention 'no tests directory', got: {stderr:?}"
    );
}

#[test]
fn tests_dir_with_workspace_is_incompatible_error() {
    let fixture = fixture_path("tests/fixtures/uv_workspace");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&fixture)
        .arg("--tests-dir")
        .arg("custom-tests")
        .arg("--static-only")
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    assert!(
        stderr.contains("--tests-dir") && stderr.to_lowercase().contains("workspace"),
        "stderr must explain --tests-dir+workspace incompatibility, got: {stderr:?}"
    );
}

#[test]
fn project_package_with_workspace_is_incompatible_error() {
    let fixture = fixture_path("tests/fixtures/uv_workspace");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&fixture)
        .arg("--project-package")
        .arg("pkg_a")
        .arg("--static-only")
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    assert!(
        stderr.contains("--project-package") && stderr.to_lowercase().contains("workspace"),
        "stderr must explain --project-package+workspace incompatibility, got: {stderr:?}"
    );
}

#[test]
fn workspace_pretty_format_emits_header_and_per_member_sections() {
    let fixture = fixture_path("tests/fixtures/uv_workspace");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&fixture)
        .arg("--static-only")
        .arg("--format")
        .arg("pretty")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    assert!(
        stdout.contains("pycoati workspace audit — uv_workspace (2 members)"),
        "expected workspace pretty header, got: {stdout}"
    );
    assert!(stdout.contains("Member: pkg_a"), "missing Member: pkg_a, got: {stdout}");
    assert!(stdout.contains("Member: pkg_b"), "missing Member: pkg_b, got: {stdout}");
}

#[test]
fn workspace_partial_member_without_tests_still_appears_with_empty_inventory() {
    let fixture = fixture_path("tests/fixtures/uv_workspace_partial");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&fixture)
        .arg("--static-only")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    let members = v["members"].as_array().expect("members array");
    assert_eq!(members.len(), 2, "expected 2 members, got: {members:?}");

    let pkg_b = members
        .iter()
        .find(|m| m["project"]["name"].as_str() == Some("pkg_b"))
        .expect("pkg_b must appear in the wrapper even without tests/");
    let tests = pkg_b["test_functions"].as_array().expect("test_functions array");
    assert!(
        tests.is_empty(),
        "pkg_b has no tests/ dir, so its test_functions must be empty, got: {tests:?}"
    );

    // Stderr should carry a warn for the skipped tests dir.
    assert!(
        stderr.to_lowercase().contains("warn"),
        "expected a warn-level log for the missing tests/ dir, got: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Pytest-per-member (gated on python availability).
// ---------------------------------------------------------------------------

fn python_with_pytest() -> Option<String> {
    let candidates = ["python", "python3"];
    for candidate in candidates {
        let probe = std::process::Command::new(candidate).args(["-c", "import pytest"]).status();
        if let Ok(s) = probe {
            if s.success() {
                return Some(candidate.to_string());
            }
        }
    }
    None
}

#[test]
fn workspace_pytest_member_cwd_root_runs_each_member_collection() {
    // The whole point of `--member-cwd=root` (the default) is to run
    // each member's pytest from the workspace root and address tests
    // via `<member>/tests`. Confirm that with pytest available, every
    // member's `suite.test_count` populates.
    let Some(python) = python_with_pytest() else {
        eprintln!("SKIPPED: pytest not importable from python on PATH");
        return;
    };
    let fixture = fixture_path("tests/fixtures/uv_workspace");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&fixture)
        .arg("--python")
        .arg(&python)
        .arg("--no-coverage")
        .arg("--member-cwd")
        .arg("root")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    let members = v["members"].as_array().expect("members array");
    assert_eq!(members.len(), 2);
    for m in members {
        let count = m["suite"]["test_count"].as_u64();
        assert!(
            count.is_some(),
            "expected pytest test_count populated for member {:?}, got: {m:#?}",
            m["project"]["name"]
        );
    }
}

#[test]
fn workspace_pytest_member_cwd_member_runs_each_member_collection() {
    let Some(python) = python_with_pytest() else {
        eprintln!("SKIPPED: pytest not importable from python on PATH");
        return;
    };
    let fixture = fixture_path("tests/fixtures/uv_workspace");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&fixture)
        .arg("--python")
        .arg(&python)
        .arg("--no-coverage")
        .arg("--member-cwd")
        .arg("member")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    let members = v["members"].as_array().expect("members array");
    assert_eq!(members.len(), 2);
    for m in members {
        let count = m["suite"]["test_count"].as_u64();
        assert!(
            count.is_some(),
            "expected pytest test_count populated for member {:?} (cwd=member), got: {m:#?}",
            m["project"]["name"]
        );
    }
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
