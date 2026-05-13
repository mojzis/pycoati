//! End-to-end test for tree-sitter assertion counting against the
//! `tests/fixtures/simple/test_basic.py` fixture.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

fn fixture_path(rel: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(rel);
    p
}

#[test]
fn counts_two_tests_with_five_total_asserts() {
    let path = fixture_path("tests/fixtures/simple/test_basic.py");
    let inv = coati::run_static(&path).expect("run_static on test_basic.py");

    assert_eq!(
        inv.tests.len(),
        2,
        "expected exactly 2 test records, got: {:?}",
        inv.tests.iter().map(|t| &t.nodeid).collect::<Vec<_>>()
    );

    let total: u64 = inv.tests.iter().map(|t| t.assertion_count).sum();
    assert_eq!(total, 5, "expected total assertion_count == 5");

    let by_name: std::collections::BTreeMap<&str, u64> = inv
        .tests
        .iter()
        .filter_map(|t| t.nodeid.split("::").nth(1).map(|n| (n, t.assertion_count)))
        .collect();
    assert_eq!(by_name.get("test_arithmetic"), Some(&3));
    assert_eq!(by_name.get("test_strings"), Some(&2));
}

#[test]
fn only_asserts_on_mock_is_false_for_every_phase_1_test() {
    let path = fixture_path("tests/fixtures/simple/test_basic.py");
    let inv = coati::run_static(&path).expect("run_static on test_basic.py");
    for t in &inv.tests {
        assert!(
            !t.only_asserts_on_mock,
            "Phase 1 must hardcode only_asserts_on_mock=false; offending test: {}",
            t.nodeid
        );
    }
}

#[test]
fn empty_file_yields_no_tests_and_no_file_record() {
    let path = fixture_path("tests/fixtures/simple/empty.py");
    let inv = coati::run_static(&path).expect("run_static on empty.py");
    assert!(inv.tests.is_empty(), "empty.py should produce no tests");
    assert!(inv.files.is_empty(), "empty.py should produce no file records");
}

#[test]
fn file_record_aggregates_match_per_test_totals() {
    let path = fixture_path("tests/fixtures/simple/test_basic.py");
    let inv = coati::run_static(&path).expect("run_static on test_basic.py");
    assert_eq!(inv.files.len(), 1);
    let file = &inv.files[0];
    assert_eq!(file.test_count, inv.tests.len() as u64);
    let expected_asserts: u64 = inv.tests.iter().map(|t| t.assertion_count).sum();
    assert_eq!(file.assertion_count, expected_asserts);
}
