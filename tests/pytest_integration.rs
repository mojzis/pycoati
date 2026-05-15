//! End-to-end test for the Run 2 pytest path: collection, durations, and
//! coverage. Drives the `pycoati` binary against `tests/fixtures/project/`.
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
//! subprocess fails entirely, pycoati must still exit 0 with a valid JSON
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

fn hyphen_fixture_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/hyphen_project");
    p
}

/// Recursively copy a fixture tree into `dst`, skipping the build artifacts
/// pytest/coverage.py leave behind. We use this to give each integration
/// test that targets `hyphen_project` its own writable copy: `pytest-cov`
/// writes `.coverage` (and `.pytest_cache/`) to the project root, and two
/// tests pointed at the same directory race on those files in parallel
/// `cargo test` runs — see the test docstrings below for the failure mode
/// (the override test would observe a non-null coverage value leaked from
/// the default test). Tempdir copies eliminate the race and also keep the
/// source tree clean.
fn copy_fixture_tree(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name();
        // Skip artifacts a previous run may have left in the source tree.
        let name_str = name.to_string_lossy();
        if matches!(name_str.as_ref(), ".coverage" | ".pytest_cache" | "__pycache__" | ".pycoati") {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        let ft = entry.file_type().expect("file type");
        if ft.is_dir() {
            std::fs::create_dir_all(&dst_path).expect("create dir in tempdir");
            copy_fixture_tree(&src_path, &dst_path);
        } else if ft.is_file() {
            std::fs::copy(&src_path, &dst_path).expect("copy fixture file");
        }
        // Symlinks / others: fixture tree has none today; skip if encountered.
    }
}

/// Stage the hyphenated-distribution fixture in a fresh tempdir and return
/// the (`TempDir` guard, project root) pair. Holding the guard alive for the
/// duration of the test keeps the tempdir on disk; dropping it removes the
/// copy plus any `.coverage` / `.pytest_cache` pytest-cov writes there.
fn staged_hyphen_fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("hyphen_project");
    std::fs::create_dir_all(&root).expect("create staged fixture root");
    copy_fixture_tree(&hyphen_fixture_root(), &root);
    (tmp, root)
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

    let assert = Command::cargo_bin("pycoati")
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
    let assert = Command::cargo_bin("pycoati")
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

    let assert = Command::cargo_bin("pycoati")
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
    // Regression guard for the auto-detect default: running `pycoati <fixture>`
    // with no `--python` must produce a valid JSON inventory with the static
    // analysis intact, regardless of whether the auto-detected interpreter
    // can import pytest. The point of this test is the `Option<&[String]>`
    // plumbing — if it ever silently reverts to bare `python` (or worse,
    // panics on the None branch), this test catches it.
    let assert =
        Command::cargo_bin("pycoati").expect("binary built").arg(fixture_root()).assert().success();
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
    // It models any deliberately-broken interpreter command — pycoati must
    // degrade gracefully, leave suite fields null, emit a warn on stderr,
    // and exit 0 with valid JSON on stdout.
    let assert = Command::cargo_bin("pycoati")
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
    // exercises the `-o addopts=` neutralisation path: pycoati should not
    // reach into the project's `addopts`, and the collection count must
    // match the number of test functions the fixture declares.
    let python = integration_python();
    if !pytest_available(&python) {
        eprintln!("SKIPPED: pytest not available via `{python}`");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    scaffold_pytest_project(tmp.path(), "-q");

    let assert = Command::cargo_bin("pycoati")
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

// ---------------------------------------------------------------------------
// Phase 3 — preflight + coverage WARN messages
// ---------------------------------------------------------------------------

/// Pick a python interpreter that exists on PATH but does **not** have
/// `pytest` importable. Used by the preflight-WARN test to deliberately
/// trip the missing-pytest path. Returns `None` if every candidate either
/// is absent or *does* have pytest (we don't want the test to falsely
/// pass when the env is fully provisioned).
fn python_without_pytest() -> Option<String> {
    let candidates = ["/usr/bin/python3", "/usr/bin/python", "python3", "python"];
    for c in candidates {
        // Probe existence + lack-of-pytest in one go. Two non-matching
        // outcomes ("pytest importable" and "interpreter not on PATH")
        // both mean "keep looking" — the only hit is `Ok(non-zero)`.
        let status = StdCommand::new(c).args(["-c", "import pytest"]).status();
        if let Ok(s) = status {
            if !s.success() {
                return Some(c.to_string());
            }
        }
    }
    None
}

#[test]
fn preflight_warns_when_pytest_unavailable_but_static_still_runs() {
    // The new preflight check must emit a structured WARN naming the
    // resolved python — and crucially must *not* abort: static analysis
    // (files, test_functions) must still populate.
    let Some(python) = python_without_pytest() else {
        eprintln!("SKIPPED: every probed interpreter has pytest importable");
        return;
    };

    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(fixture_root())
        .arg("--python")
        .arg(&python)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    let v: Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");

    // Static analysis still ran.
    let files = v["files"].as_array().expect("files must remain populated");
    assert!(!files.is_empty(), "static inventory must run even when pytest is unavailable");
    let test_functions = v["test_functions"].as_array().expect("test_functions array");
    assert!(
        !test_functions.is_empty(),
        "test_functions must populate from static parse even when pytest is missing"
    );

    // pytest-derived fields are null because the subprocess produced no
    // parseable output.
    assert_eq!(
        v["suite"]["test_count"],
        Value::Null,
        "test_count must be null when pytest is unavailable in the resolved Python"
    );
    assert_eq!(v["tool"]["ran_pytest"], Value::Bool(false));

    // The new preflight WARN fires and names both the interpreter and the
    // recovery hint. Match on the structured substrings, not the whole line.
    let stderr_lc = stderr.to_lowercase();
    assert!(
        stderr_lc.contains("warn"),
        "expected a warn-level log on stderr from the preflight, got: {stderr}"
    );
    assert!(
        stderr.contains("pytest unavailable in resolved python"),
        "expected preflight WARN to mention 'pytest unavailable in resolved python', got: {stderr}"
    );
    assert!(
        stderr.contains(&python),
        "expected preflight WARN to mention the resolved interpreter `{python}`, got: {stderr}"
    );
    assert!(
        stderr.contains("--python")
            && (stderr.contains("uv run python") || stderr.contains(".venv/bin/python")),
        "expected preflight WARN to include an actionable recovery hint, got: {stderr}"
    );
}

#[test]
fn coverage_warn_names_pytest_exit_code_when_report_missing() {
    // When the coverage subprocess produces no JSON report (empty file or
    // file written but empty), pycoati must surface a structured WARN
    // containing the pytest exit code + a stderr tail — *not* the raw
    // serde "EOF while parsing" string. We trigger the empty-report path
    // by scaffolding a project whose package name doesn't match a real
    // module, then running coverage against it: pytest exits non-zero,
    // writes nothing useful to the report path, and pycoati must degrade.
    let python = integration_python();
    if !pytest_available(&python) {
        eprintln!("SKIPPED: pytest not available via `{python}`");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    // Scaffold a project with a `mini` package, but lie in pyproject.toml
    // and claim the package name is `does_not_exist`. pycoati will pass
    // `--cov=does_not_exist`, pytest will be unable to find anything to
    // measure, and the JSON report path will end up empty / malformed.
    std::fs::write(tmp.path().join("pyproject.toml"), "[project]\nname = \"does_not_exist\"\n")
        .expect("write pyproject.toml");
    let pkg = tmp.path().join("mini");
    std::fs::create_dir(&pkg).expect("mkdir mini");
    std::fs::write(pkg.join("__init__.py"), "").expect("write __init__.py");
    std::fs::write(pkg.join("core.py"), "def greet():\n    return 'hi'\n").expect("write core.py");
    let tests = tmp.path().join("tests");
    std::fs::create_dir(&tests).expect("mkdir tests");
    std::fs::write(
        tests.join("test_greet.py"),
        "from mini.core import greet\n\n\
         def test_greet():\n    assert greet() == 'hi'\n",
    )
    .expect("write test_greet.py");

    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(tmp.path())
        .arg("--python")
        .arg(&python)
        .assert()
        .success();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let v: Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");

    // line_coverage_pct stays null because coverage failed.
    assert_eq!(v["suite"]["line_coverage_pct"], Value::Null);
    assert_eq!(v["tool"]["ran_coverage"], Value::Bool(false));

    // The new WARN must mention a pytest exit code and must NOT surface
    // the raw serde "EOF while parsing" string as the primary error.
    let stderr_lc = stderr.to_lowercase();
    assert!(
        stderr_lc.contains("warn"),
        "expected a warn-level log on stderr for the coverage failure, got: {stderr}"
    );
    assert!(
        stderr.contains("no coverage data produced") || stderr.contains("pytest exit"),
        "expected coverage WARN to surface `no coverage data produced` or `pytest exit`, got: {stderr}"
    );
    assert!(
        !stderr.contains("coverage JSON report was malformed"),
        "old serde-fronted WARN should be replaced by the new structured form, got: {stderr}"
    );
    assert!(
        !stderr.contains("EOF while parsing"),
        "raw serde 'EOF while parsing' must never appear as the primary coverage error, got: {stderr}"
    );
}

#[test]
fn collection_survives_repo_addopts_with_cov_flag() {
    // A `pytest.ini` with `addopts = --cov=foo` is the adversarial case
    // from the rollout report: when pycoati runs `pytest --collect-only`
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

    let assert = Command::cargo_bin("pycoati")
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

// ---------------------------------------------------------------------------
// Phase 5 — hyphen-to-underscore normalization for the auto-derived --cov=
// ---------------------------------------------------------------------------

/// When `[project].name` is hyphenated (`my-pkg`), the importable module is
/// conventionally the underscored form (`my_pkg`). Coati must normalize the
/// auto-derived default so `pytest --cov=my_pkg` finds the module — *without*
/// the normalization, pytest-cov emits `module-not-imported` and coverage
/// stays null. The override path is verified separately to remain verbatim.
#[test]
fn hyphenated_pyproject_name_produces_non_zero_coverage_via_default() {
    let python = integration_python();
    if !pytest_available(&python) {
        eprintln!("SKIPPED: pytest not available via `{python}`");
        return;
    }

    // Per-test staged copy: `pytest-cov` writes `.coverage` (and a
    // `.pytest_cache/`) into the project root. The sibling override test
    // also targets this fixture; under parallel `cargo test` execution
    // they would race on those files and the override test could see
    // coverage data leaked from this one. Staging in a tempdir per test
    // eliminates the race and keeps the source tree clean.
    let (_guard, fixture_root) = staged_hyphen_fixture();

    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&fixture_root)
        .arg("--python")
        .arg(&python)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    let v: Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");

    // The display name in the inventory keeps the original hyphenated form;
    // only the `--cov=` target gets normalized.
    assert_eq!(
        v["project"]["name"],
        Value::String("my-pkg".to_string()),
        "project.name must remain the original hyphenated distribution name"
    );

    // Coverage actually ran and reported a real percentage — *not* null.
    let cov = v["suite"]["line_coverage_pct"].as_f64();
    assert!(
        cov.is_some(),
        "line_coverage_pct must be populated when the hyphenated name is normalized to an importable module, got {:?} (stderr: {stderr})",
        v["suite"]["line_coverage_pct"]
    );
    assert!(
        cov.unwrap() > 0.0,
        "expected non-zero coverage from my_pkg.greet exercised by test_greet_says_hi, got {cov:?}"
    );
    assert_eq!(v["tool"]["ran_coverage"], Value::Bool(true));

    // Negative assertion: pytest-cov's `module-not-imported` warning must NOT
    // appear on stderr, since that's the exact regression this fix targets.
    assert!(
        !stderr.contains("module-not-imported"),
        "default-derived --cov= must resolve to an importable module, got: {stderr}"
    );
}

/// The verbatim-override contract: when the user passes
/// `--project-package <value>`, the value goes straight into `--cov=<value>`
/// with no normalization. We prove this *behaviourally* by overriding the
/// hyphen fixture with the hyphenated form `my-pkg` (the distribution name as
/// written in pyproject.toml). If the override were silently rewritten to
/// `my_pkg` it would actually succeed; we assert the opposite — that the
/// invalid hyphenated module name reaches pytest-cov and coverage degrades.
#[test]
fn cli_project_package_override_with_hyphen_is_passed_verbatim() {
    let python = integration_python();
    if !pytest_available(&python) {
        eprintln!("SKIPPED: pytest not available via `{python}`");
        return;
    }

    // Per-test staged copy — see the sibling default test for the race
    // this avoids. With both tests sharing the source-tree fixture, the
    // override test below would intermittently observe coverage > 0
    // leaked from the default test's `.coverage` write.
    let (_guard, fixture_root) = staged_hyphen_fixture();

    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&fixture_root)
        .arg("--python")
        .arg(&python)
        .arg("--project-package")
        .arg("my-pkg")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    let v: Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");

    // Verbatim hyphen reaches `--cov=` → pytest-cov can't import it →
    // coverage stays null. If normalization had been (incorrectly) applied
    // to the override path, this would have been > 0.
    assert_eq!(
        v["suite"]["line_coverage_pct"], Value::Null,
        "explicit --project-package override must be verbatim — a hyphenated override should NOT silently become an importable module"
    );
    assert_eq!(v["tool"]["ran_coverage"], Value::Bool(false));

    // Direct assertion on pycoati's own contract: when coverage produces no
    // data, pycoati emits a structured WARN naming the failure mode. This
    // tightens the test from "observe null coverage" (which depends on
    // pytest-cov's specific behaviour with an invalid `--cov=` value) to
    // "pycoati surfaced the no-data path with a non-zero pytest exit",
    // which is the message pycoati itself owns in `src/coverage.rs`.
    assert!(
        stderr.contains("no coverage data produced"),
        "expected pycoati to emit its 'no coverage data produced' WARN when the verbatim hyphenated override fails to resolve to a module, stderr was: {stderr}"
    );
}
