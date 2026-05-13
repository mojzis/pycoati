//! End-to-end test for the Run 2 pytest path: collection, durations, and
//! coverage. Drives the `coati` binary against `tests/fixtures/project/`.
//!
//! Self-skips when pytest is not importable from the configured Python
//! interpreter — we use `python -c 'import pytest, pytest_cov'` as the probe
//! rather than `which pytest`, since `--python "uv run python"` is a
//! multi-token command and a `which` check can't represent it. When the
//! probe fails (no pytest available), the test prints a `SKIPPED:` line on
//! stderr and returns without asserting.
//!
//! The failure-path test (`--python false`) is the regression guard against
//! subprocess panics corrupting the JSON inventory: even when the pytest
//! subprocess fails entirely, coati must still exit 0 with a valid JSON
//! inventory on stdout and a warn-level log on stderr.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use serde_json::Value;

fn fixture_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/project");
    p
}

/// Whitespace-split a command-line string into program + args.
fn split_command(cmd: &str) -> Option<(String, Vec<String>)> {
    let mut tokens = cmd.split_whitespace();
    let prog = tokens.next()?.to_string();
    let args: Vec<String> = tokens.map(str::to_string).collect();
    Some((prog, args))
}

/// Probe for pytest + pytest-cov availability using the given python command.
/// Returns true iff `python -c 'import pytest, pytest_cov'` exits 0.
fn pytest_available(python_cmd: &str) -> bool {
    let Some((prog, args)) = split_command(python_cmd) else {
        return false;
    };
    let mut cmd = StdCommand::new(&prog);
    cmd.args(&args).args(["-c", "import pytest, pytest_cov"]);
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

/// Resolve the Python command to use for the integration tests. Honour the
/// `COATI_TEST_PYTHON` env var (e.g. `"uv run python"`) so CI can wire in a
/// venv; otherwise default to plain `python`.
fn integration_python() -> String {
    std::env::var("COATI_TEST_PYTHON").unwrap_or_else(|_| "python".to_string())
}

#[test]
fn default_flags_populate_all_suite_fields() {
    let python = integration_python();
    if !pytest_available(&python) {
        eprintln!("SKIPPED: pytest not available via `{python}`");
        return;
    }

    let assert = Command::cargo_bin("coati")
        .expect("binary built")
        .arg(fixture_root())
        .arg("--python")
        .arg(&python)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let v: Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");

    let suite = &v["suite"];
    assert!(
        suite["test_count"].as_u64().is_some(),
        "test_count must be populated, got {:?}",
        suite["test_count"]
    );
    assert!(
        suite["runtime_seconds"].as_f64().is_some(),
        "runtime_seconds must be populated, got {:?}",
        suite["runtime_seconds"]
    );
    assert!(
        suite["line_coverage_pct"].as_f64().is_some(),
        "line_coverage_pct must be populated, got {:?}",
        suite["line_coverage_pct"]
    );
    let slowest = suite["slowest_tests"].as_array().expect("slowest_tests array");
    assert!(!slowest.is_empty(), "slowest_tests must be non-empty when pytest ran");

    // Coverage > 0 requires the package to actually be exercised by a test.
    assert!(
        suite["line_coverage_pct"].as_f64().unwrap() > 0.0,
        "line_coverage_pct should be > 0 when myproj.greet is called from a test"
    );

    assert_eq!(v["tool"]["ran_pytest"], Value::Bool(true));
    assert_eq!(v["tool"]["ran_coverage"], Value::Bool(true));
}

#[test]
fn static_only_skips_all_pytest_invocations() {
    let assert = Command::cargo_bin("coati")
        .expect("binary built")
        .arg(fixture_root())
        .arg("--static-only")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let v: Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");

    let suite = &v["suite"];
    assert_eq!(suite["test_count"], Value::Null, "--static-only must leave test_count null");
    assert_eq!(suite["runtime_seconds"], Value::Null);
    assert_eq!(suite["line_coverage_pct"], Value::Null);
    assert_eq!(suite["slowest_tests"], Value::Array(vec![]));
    assert_eq!(v["tool"]["ran_pytest"], Value::Bool(false));
    assert_eq!(v["tool"]["ran_coverage"], Value::Bool(false));
}

#[test]
fn no_coverage_skips_only_the_coverage_run() {
    let python = integration_python();
    if !pytest_available(&python) {
        eprintln!("SKIPPED: pytest not available via `{python}`");
        return;
    }

    let assert = Command::cargo_bin("coati")
        .expect("binary built")
        .arg(fixture_root())
        .arg("--python")
        .arg(&python)
        .arg("--no-coverage")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let v: Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");

    let suite = &v["suite"];
    assert!(suite["test_count"].as_u64().is_some(), "test_count must be populated");
    assert!(suite["runtime_seconds"].as_f64().is_some(), "runtime_seconds must be populated");
    assert_eq!(
        suite["line_coverage_pct"],
        Value::Null,
        "--no-coverage must leave line_coverage_pct null"
    );
    assert_eq!(v["tool"]["ran_pytest"], Value::Bool(true));
    assert_eq!(v["tool"]["ran_coverage"], Value::Bool(false));
}

#[test]
fn no_python_flag_uses_auto_detect_and_emits_valid_inventory() {
    // Regression guard for the auto-detect default: running `coati <fixture>`
    // with no `--python` must produce a valid JSON inventory with the static
    // analysis intact, regardless of whether the auto-detected interpreter
    // can import pytest. The point of this test is the `Option<&[String]>`
    // plumbing — if it ever silently reverts to bare `python` (or worse,
    // panics on the None branch), this test catches it.
    let assert =
        Command::cargo_bin("coati").expect("binary built").arg(fixture_root()).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let v: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    let files = v["files"].as_array().expect("files array");
    assert!(!files.is_empty(), "static inventory must populate the files array");
    assert_eq!(v["schema_version"], Value::String("2".to_string()));
    // `tool.ran_pytest` is true *or* false depending on whether the
    // auto-detected interpreter can import pytest — either way the field
    // must exist as a bool, never null.
    assert!(
        v["tool"]["ran_pytest"].is_boolean(),
        "tool.ran_pytest must be a bool, got {:?}",
        v["tool"]["ran_pytest"]
    );
}

#[test]
fn broken_python_interpreter_does_not_crash_inventory() {
    // `false` is a real binary that exits non-zero and produces no output.
    // It models any deliberately-broken interpreter command — coati must
    // degrade gracefully, leave suite fields null, emit a warn on stderr,
    // and exit 0 with valid JSON on stdout.
    let assert = Command::cargo_bin("coati")
        .expect("binary built")
        .arg(fixture_root())
        .arg("--python")
        .arg("false")
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    let v: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    let suite = &v["suite"];
    assert_eq!(suite["test_count"], Value::Null);
    assert_eq!(suite["runtime_seconds"], Value::Null);
    assert_eq!(suite["line_coverage_pct"], Value::Null);
    assert_eq!(suite["slowest_tests"], Value::Array(vec![]));
    assert_eq!(v["tool"]["ran_pytest"], Value::Bool(false));
    assert_eq!(v["tool"]["ran_coverage"], Value::Bool(false));

    // Static analysis output must still be intact.
    let files = v["files"].as_array().expect("files must remain populated");
    assert!(!files.is_empty(), "static inventory must survive subprocess failures");

    // The graceful degradation must surface as a warn-level log on stderr.
    assert!(
        stderr.to_lowercase().contains("warn"),
        "expected a warn-level log on stderr when pytest invocation fails, got: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Phase 1 — repo addopts must not break pytest collection / durations / coverage
// ---------------------------------------------------------------------------

/// Build a self-contained pytest project at `project_root` with a `pytest.ini`
/// whose `addopts` line is `addopts_value`. The project ships one trivial
/// package (`mini`) with a single function, and one test that exercises it.
fn scaffold_pytest_project(project_root: &Path, addopts_value: &str) {
    std::fs::write(project_root.join("pyproject.toml"), "[project]\nname = \"mini\"\n")
        .expect("write pyproject.toml");
    std::fs::write(
        project_root.join("pytest.ini"),
        format!("[pytest]\naddopts = {addopts_value}\n"),
    )
    .expect("write pytest.ini");

    let pkg = project_root.join("mini");
    std::fs::create_dir(&pkg).expect("mkdir mini");
    std::fs::write(pkg.join("__init__.py"), "").expect("write __init__.py");
    std::fs::write(pkg.join("core.py"), "def greet():\n    return 'hi'\n").expect("write core.py");

    let tests = project_root.join("tests");
    std::fs::create_dir(&tests).expect("mkdir tests");
    std::fs::write(
        tests.join("test_greet.py"),
        "from mini.core import greet\n\n\
         def test_greet_returns_hi():\n    \
             assert greet() == 'hi'\n\n\
         def test_greet_is_truthy():\n    \
             assert greet()\n",
    )
    .expect("write test_greet.py");
}

#[test]
fn collection_survives_repo_addopts_with_quiet_flag() {
    // A `pytest.ini` with `addopts = -q` is benign on its own, but it
    // exercises the `-o addopts=` neutralisation path: coati should not
    // reach into the project's `addopts`, and the collection count must
    // match the number of test functions the fixture declares.
    let python = integration_python();
    if !pytest_available(&python) {
        eprintln!("SKIPPED: pytest not available via `{python}`");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    scaffold_pytest_project(tmp.path(), "-q");

    let assert = Command::cargo_bin("coati")
        .expect("binary built")
        .arg(tmp.path())
        .arg("--python")
        .arg(&python)
        .arg("--no-coverage")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let v: Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");
    let test_count = v["suite"]["test_count"].as_u64();
    assert_eq!(
        test_count,
        Some(2),
        "expected test_count == Some(2) when repo addopts is `-q`, got {test_count:?}"
    );
}

#[test]
fn collection_survives_repo_addopts_with_cov_flag() {
    // A `pytest.ini` with `addopts = --cov=foo` is the adversarial case
    // from the rollout report: when coati runs `pytest --collect-only`
    // without neutralising addopts, pytest tries to import the
    // `pytest-cov` plugin against a non-existent package and the
    // collection count comes back as `None`. With `-o addopts=` the
    // override clears the line for this run and collection succeeds.
    let python = integration_python();
    if !pytest_available(&python) {
        eprintln!("SKIPPED: pytest not available via `{python}`");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    scaffold_pytest_project(tmp.path(), "--cov=does_not_exist");

    let assert = Command::cargo_bin("coati")
        .expect("binary built")
        .arg(tmp.path())
        .arg("--python")
        .arg(&python)
        .arg("--no-coverage")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let v: Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");
    let test_count = v["suite"]["test_count"].as_u64();
    assert_eq!(
        test_count,
        Some(2),
        "expected test_count == Some(2) when repo addopts is `--cov=does_not_exist`, got {test_count:?}"
    );
}
