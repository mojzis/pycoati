//! `testpaths` discovery against `tests/fixtures/testpaths_project/`, whose
//! `pyproject.toml` lists two test roots and which also has a `tests/` dir
//! pytest would not collect from.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;

fn fixture_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/testpaths_project");
    p
}

fn nodeids(inv: &pycoati::Inventory) -> BTreeSet<String> {
    inv.test_functions.iter().map(|t| t.nodeid.clone()).collect()
}

#[test]
fn every_testpaths_entry_is_walked_and_tests_is_not() {
    let inv = pycoati::run_static(&fixture_root()).expect("run_static");
    let expected: BTreeSet<String> = [
        "checks/unit/test_unit.py::test_unit",
        "integration/test_integration.py::test_integration",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(nodeids(&inv), expected);
    assert_eq!(inv.files.len(), 2);
}

#[test]
fn an_explicit_tests_dir_wins_over_testpaths() {
    let root = fixture_root();
    let inv = pycoati::run_static_with_tests_dir(&root, Some(&root.join("tests"))).expect("run");
    let expected: BTreeSet<String> =
        std::iter::once("tests/test_ignored.py::test_ignored".to_string()).collect();
    assert_eq!(nodeids(&inv), expected);
}

fn project_with(pyproject: &str, test_dirs: &[&str]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(tmp.path().join("pyproject.toml"), pyproject).expect("write pyproject");
    for dir in test_dirs {
        let path = tmp.path().join(dir);
        std::fs::create_dir_all(&path).expect("mkdir");
        let name = dir.replace('/', "_");
        std::fs::write(
            path.join(format!("test_{name}.py")),
            format!("def test_{name}():\n    assert True\n"),
        )
        .expect("write test");
    }
    tmp
}

#[test]
fn missing_testpaths_entries_are_skipped() {
    let tmp =
        project_with("[tool.pytest.ini_options]\ntestpaths = [\"gone\", \"here\"]\n", &["here"]);
    let inv = pycoati::run_static(tmp.path()).expect("one existing entry is enough");
    assert_eq!(inv.test_functions.len(), 1);
}

#[test]
fn all_testpaths_missing_falls_back_to_tests() {
    // A stale `testpaths` must not turn a project that worked with the
    // `tests/` default into a hard error.
    let tmp = project_with("[tool.pytest.ini_options]\ntestpaths = [\"gone\"]\n", &["tests"]);
    let inv = pycoati::run_static(tmp.path()).expect("falls back to tests/");
    assert_eq!(inv.test_functions.len(), 1);
}

#[test]
fn nothing_to_scan_is_an_error_naming_every_entry() {
    let tmp = project_with("[tool.pytest.ini_options]\ntestpaths = [\"gone\", \"lost\"]\n", &[]);
    let err = pycoati::run_static(tmp.path()).expect_err("no entry exists");
    let msg = format!("{err:#}");
    assert!(msg.contains("no tests directory"), "{msg}");
    assert!(msg.contains("gone") && msg.contains("lost"), "names every entry: {msg}");
}

#[test]
fn overlapping_testpaths_parse_each_file_once() {
    let tmp = project_with(
        "[tool.pytest.ini_options]\ntestpaths = [\"tests\", \"tests/unit\"]\n",
        &["tests", "tests/unit"],
    );
    let inv = pycoati::run_static(tmp.path()).expect("run");
    assert_eq!(inv.test_functions.len(), 2);
    assert_eq!(inv.files.len(), 2);
}
