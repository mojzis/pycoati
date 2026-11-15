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

#[test]
fn missing_testpaths_entries_are_skipped_and_all_missing_is_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tmp.path().join("pyproject.toml"),
        "[tool.pytest.ini_options]\ntestpaths = [\"gone\", \"here\"]\n",
    )
    .expect("write pyproject");
    std::fs::create_dir(tmp.path().join("here")).expect("mkdir");
    std::fs::write(tmp.path().join("here/test_x.py"), "def test_x():\n    assert True\n")
        .expect("write test");
    let inv = pycoati::run_static(tmp.path()).expect("one existing entry is enough");
    assert_eq!(inv.test_functions.len(), 1);

    std::fs::remove_dir_all(tmp.path().join("here")).expect("rm");
    let err = pycoati::run_static(tmp.path()).expect_err("no entry exists");
    let msg = format!("{err:#}");
    assert!(msg.contains("no tests directory"), "{msg}");
    assert!(msg.contains("gone") && msg.contains("here"), "names every entry: {msg}");
}
