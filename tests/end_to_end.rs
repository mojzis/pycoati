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
fn walker_discovers_test_files_and_ignores_helpers() {
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
    assert_eq!(files.len(), 7, "expected 7 discovered test files, got {names:?}");
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
        "test_uses_repo.py",
        "test_overmocked.py",
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
            fr["test_function_count"].as_u64().expect("test_function_count u64"),
            exp.tests,
            "test_function_count mismatch for {fixture}"
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
    let test_functions = v["test_functions"].as_array().expect("test_functions must be array");

    let by_test_name: BTreeMap<String, &Value> = test_functions
        .iter()
        .map(|t| {
            let nodeid = t["nodeid"].as_str().expect("nodeid string");
            let name = nodeid.rsplit("::").next().unwrap_or("").to_string();
            (name, t)
        })
        .collect();

    // From the fixture annotations + Phase 2 additions:
    // - test_repo_save_called (existing fixture)
    // - test_three_mocks_one_assert (Phase 2 fixture: `assert a.called`)
    let true_names: std::collections::BTreeSet<&str> =
        ["test_repo_save_called", "test_three_mocks_one_assert"].into_iter().collect();

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
    let test_functions = v["test_functions"].as_array().expect("test_functions must be array");
    for t in test_functions {
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
    assert_eq!(v["schema_version"], Value::String("2".to_string()));
    assert_eq!(v["tool"]["ran_pytest"], Value::Bool(false));
    assert_eq!(v["tool"]["ran_coverage"], Value::Bool(false));
    assert_eq!(v["suite"]["test_count"], Value::Null);
    // sut_calls.by_name is now populated by Phase 2; just confirm the key
    // exists and is an array.
    assert!(v["sut_calls"]["by_name"].is_array());
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
    assert_eq!(v["files"].as_array().expect("files array").len(), 7);
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

/// A malformed Python file inside the tests directory must NOT abort the run.
/// The walker should emit a `tracing::warn!` and skip the file, while the
/// remaining well-formed files still appear in the inventory.
#[test]
fn malformed_python_file_is_skipped_with_warning_not_aborted() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path();
    std::fs::write(project.join("pyproject.toml"), "[project]\nname = \"badproj\"\n")
        .expect("write pyproject");
    let tests_dir = project.join("tests");
    std::fs::create_dir(&tests_dir).expect("mkdir tests");

    // Well-formed file with a single test + single assert.
    std::fs::write(tests_dir.join("test_ok.py"), "def test_ok():\n    assert 1 == 1\n")
        .expect("write ok fixture");

    // Malformed file: unterminated string literal — tree-sitter still
    // returns a tree (it's error-tolerant), so we additionally force a
    // hard failure by making the file unreadable as UTF-8.
    std::fs::write(tests_dir.join("test_bad.py"), b"\xff\xfe not valid utf-8")
        .expect("write bad fixture");

    let assert = Command::cargo_bin("coati").expect("binary built").arg(project).assert().success();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let v: Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");

    // The good file still produced a record.
    let files = v["files"].as_array().expect("files array");
    let basenames: Vec<&str> =
        files.iter().filter_map(|f| f["path"].as_str()?.rsplit('/').next()).collect();
    assert!(
        basenames.contains(&"test_ok.py"),
        "expected test_ok.py in inventory, got {basenames:?}"
    );
    assert!(
        !basenames.contains(&"test_bad.py"),
        "test_bad.py should have been skipped, got {basenames:?}"
    );

    // The skip emits a warning on stderr (tracing default writer).
    assert!(
        stderr.contains("test_bad.py") && stderr.to_lowercase().contains("warn"),
        "expected a warning mentioning test_bad.py on stderr, got: {stderr}"
    );
}

/// An empty `tests/` directory must emit a distinct WARN (separate from the
/// existing "tests directory not found" branch) and produce a valid, empty
/// JSON inventory. Without the WARN it's painfully easy to interpret a
/// zero-test inventory as a coati bug rather than a misconfigured project.
#[test]
fn empty_tests_directory_emits_warn_and_clean_inventory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path();
    std::fs::write(project.join("pyproject.toml"), "[project]\nname = \"emptyproj\"\n")
        .expect("write pyproject");
    let tests_dir = project.join("tests");
    std::fs::create_dir(&tests_dir).expect("mkdir tests");
    // No files at all under tests/.

    let assert = Command::cargo_bin("coati").expect("binary built").arg(project).assert().success();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let v: Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    // Inventory is clean and empty.
    assert_eq!(v["files"].as_array().expect("files array").len(), 0);
    assert_eq!(v["test_functions"].as_array().expect("test_functions array").len(), 0);
    assert_eq!(v["schema_version"], Value::String("2".to_string()));

    // A WARN must fire that names the test-discovery branch (not just the
    // pytest WARNs that fire on any empty inventory). The wording must be
    // distinct from the existing "tests directory not found" line.
    let lower = stderr.to_lowercase();
    assert!(
        lower.contains("no python test files"),
        "expected a WARN like 'no python test files under …' on stderr, got: {stderr}"
    );
    assert!(
        !stderr.contains("tests directory not found"),
        "the WARN must be distinct from the missing-dir WARN, got: {stderr}"
    );
    // The discovery WARN must be a `WARN` level line, not just an info.
    let discovery_line = stderr
        .lines()
        .find(|l| l.to_lowercase().contains("no python test files"))
        .expect("discovery line present");
    assert!(
        discovery_line.contains("WARN"),
        "expected WARN-level for the no-files line, got: {discovery_line}"
    );
}

// ---------------------------------------------------------------------------
// Phase 2 — sut_calls + mock smells
// ---------------------------------------------------------------------------

/// `tests/test_uses_repo.py::test_repo_save_and_load` imports `Repository`
/// via `from myproj.repository import Repository` and constructs it with
/// `Repository()`. The parser emits `Repository` as a raw called name; the
/// resolver looks it up in the import map and canonicalises to
/// `myproj.repository.Repository`. (`repo.save` / `repo.load` go via the
/// local variable `repo`, which is not in the import map and is therefore
/// not project-internally resolvable from static analysis alone.)
#[test]
fn sut_calls_resolves_repository_save_in_test_uses_repo() {
    let v = run_coati();
    let by_name = v["sut_calls"]["by_name"].as_array().expect("by_name array");
    let names: Vec<&str> = by_name.iter().filter_map(|e| e["name"].as_str()).collect();
    assert!(
        names.contains(&"myproj.repository.Repository"),
        "expected myproj.repository.Repository in sut_calls.by_name, got {names:?}"
    );
}

#[test]
fn sut_calls_top_called_non_empty_for_fixture_project() {
    let v = run_coati();
    let top = v["sut_calls"]["top_called"].as_array().expect("top_called array");
    assert!(!top.is_empty(), "expected at least one entry in top_called");
}

#[test]
fn test_overmocked_fires_mock_overuse_smell() {
    let v = run_coati();
    let test_functions = v["test_functions"].as_array().expect("test_functions array");
    let nodeid = "tests/test_overmocked.py::test_three_mocks_one_assert";
    let rec = test_functions
        .iter()
        .find(|t| t["nodeid"].as_str() == Some(nodeid))
        .unwrap_or_else(|| panic!("no test record for {nodeid}"));
    let hits = rec["smell_hits"].as_array().expect("smell_hits array");
    let categories: Vec<&str> = hits.iter().filter_map(|h| h["category"].as_str()).collect();
    // mock_overuse fires at file level (3 Mock() constructions, 1 assert);
    // at test level the only mock-construction proxy is patch decorators,
    // which this test has none of — so we assert the FILE-level smell here.
    let file = v["files"]
        .as_array()
        .expect("files array")
        .iter()
        .find(|f| f["path"].as_str() == Some("tests/test_overmocked.py"))
        .unwrap_or_else(|| panic!("no FileRecord for tests/test_overmocked.py"));
    let file_hits = file["smell_hits"].as_array().expect("file smell_hits array");
    let file_categories: Vec<&str> =
        file_hits.iter().filter_map(|h| h["category"].as_str()).collect();
    assert!(
        file_categories.contains(&"mock_overuse"),
        "expected mock_overuse on file tests/test_overmocked.py, got test-level={categories:?} file-level={file_categories:?}"
    );
}

#[test]
fn test_mock_only_fires_mock_only_assertions_smell() {
    let v = run_coati();
    let test_functions = v["test_functions"].as_array().expect("test_functions array");
    let nodeid = "tests/test_mock_only.py::test_repo_save_called";
    let rec = test_functions
        .iter()
        .find(|t| t["nodeid"].as_str() == Some(nodeid))
        .unwrap_or_else(|| panic!("no test record for {nodeid}"));
    let hits = rec["smell_hits"].as_array().expect("smell_hits array");
    let categories: Vec<&str> = hits.iter().filter_map(|h| h["category"].as_str()).collect();
    assert!(
        categories.contains(&"mock_only_assertions"),
        "expected mock_only_assertions on {nodeid}, got {categories:?}"
    );
}

#[test]
fn file_overmocked_has_file_level_mock_overuse_smell() {
    let v = run_coati();
    let files = v["files"].as_array().expect("files array");
    let file = files
        .iter()
        .find(|f| f["path"].as_str() == Some("tests/test_overmocked.py"))
        .unwrap_or_else(|| panic!("no FileRecord for tests/test_overmocked.py"));
    let hits = file["smell_hits"].as_array().expect("smell_hits array");
    let categories: Vec<&str> = hits.iter().filter_map(|h| h["category"].as_str()).collect();
    assert!(
        categories.contains(&"mock_overuse"),
        "expected file-level mock_overuse on tests/test_overmocked.py, got {categories:?}"
    );
}

#[test]
fn pyproject_project_package_detected_from_fixture() {
    let pkgs = coati::pyproject::read_project_packages(&fixture_root()).expect("read packages");
    assert!(pkgs.contains(&"myproj".to_string()), "expected myproj in {pkgs:?}");
}

#[test]
fn cli_project_package_override_replaces_detection() {
    // Pointing at a project whose pyproject says `myproj` but overriding
    // with `--project-package foo` should drop the myproj-resolved entries
    // from sut_calls. Since the fixture project's only resolved entries
    // come from myproj, the resulting by_name list should be empty.
    let assert = Command::cargo_bin("coati")
        .expect("binary built")
        .arg(fixture_root())
        .arg("--static-only")
        .arg("--project-package")
        .arg("foo")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8");
    let v: Value = serde_json::from_str(&stdout).expect("valid json");
    let by_name = v["sut_calls"]["by_name"].as_array().expect("by_name array");
    let names: Vec<&str> = by_name.iter().filter_map(|e| e["name"].as_str()).collect();
    assert!(
        !names.iter().any(|n| n.starts_with("myproj")),
        "override should have dropped myproj.* entries, got {names:?}"
    );
}
