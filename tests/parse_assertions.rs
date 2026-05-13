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
        inv.test_functions.len(),
        2,
        "expected exactly 2 test records, got: {:?}",
        inv.test_functions.iter().map(|t| &t.nodeid).collect::<Vec<_>>()
    );

    let total: u64 = inv.test_functions.iter().map(|t| t.assertion_count).sum();
    assert_eq!(total, 5, "expected total assertion_count == 5");

    let by_name: std::collections::BTreeMap<&str, u64> = inv
        .test_functions
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
    for t in &inv.test_functions {
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
    assert!(inv.test_functions.is_empty(), "empty.py should produce no test functions");
    assert!(inv.files.is_empty(), "empty.py should produce no file records");
}

#[test]
fn file_record_aggregates_match_per_test_totals() {
    let path = fixture_path("tests/fixtures/simple/test_basic.py");
    let inv = coati::run_static(&path).expect("run_static on test_basic.py");
    assert_eq!(inv.files.len(), 1);
    let file = &inv.files[0];
    assert_eq!(file.test_function_count, inv.test_functions.len() as u64);
    let expected_asserts: u64 = inv.test_functions.iter().map(|t| t.assertion_count).sum();
    assert_eq!(file.assertion_count, expected_asserts);
}

#[test]
fn unittest_self_assert_methods_count_as_assertions() {
    // The fixture has one class-nested test with 3 `self.assertXxx` calls
    // plus one `self.assertRaises` block; the parser must count all three
    // method calls as effective assertions.
    let path = fixture_path("tests/fixtures/unittest_style/test_unittest.py");
    let inv = coati::run_static(&path).expect("run_static on unittest fixture");

    let by_name: std::collections::BTreeMap<&str, &coati::TestRecord> = inv
        .test_functions
        .iter()
        .filter_map(|t| t.nodeid.split("::").last().map(|n| (n, t)))
        .collect();

    let asserts_test =
        by_name.get("test_unittest_asserts").expect("test_unittest_asserts must be discovered");
    assert_eq!(
        asserts_test.assertion_count, 3,
        "expected 3 self.assertXxx calls counted, got {}",
        asserts_test.assertion_count
    );
}

#[test]
fn unittest_test_with_real_assertions_is_not_only_mock() {
    // The unittest fixture's `test_unittest_asserts` asserts on plain
    // values (self.assertEqual(x, 2), etc.). It must NOT be flagged as
    // `only_asserts_on_mock`.
    let path = fixture_path("tests/fixtures/unittest_style/test_unittest.py");
    let inv = coati::run_static(&path).expect("run_static on unittest fixture");

    let asserts_test = inv
        .test_functions
        .iter()
        .find(|t| t.nodeid.ends_with("::test_unittest_asserts"))
        .expect("test_unittest_asserts must be discovered");
    assert!(
        !asserts_test.only_asserts_on_mock,
        "unittest assertions on plain values must not be flagged as only_asserts_on_mock"
    );
}

#[test]
fn unittest_assert_raises_block_counts_as_assertion() {
    // `with self.assertRaises(ValueError):` exposes one effective assertion
    // via the `self.assertRaises` call inside the with-clause. The
    // raises-block branch is restricted to `pytest.raises(...)`/`raises(...)`,
    // but the unittest collector should still catch this call site.
    let path = fixture_path("tests/fixtures/unittest_style/test_unittest.py");
    let inv = coati::run_static(&path).expect("run_static on unittest fixture");
    let raises_test = inv
        .test_functions
        .iter()
        .find(|t| t.nodeid.ends_with("::test_unittest_raises_block"))
        .expect("test_unittest_raises_block must be discovered");
    assert_eq!(
        raises_test.assertion_count, 1,
        "expected `with self.assertRaises(...)` to count as one assertion, got {}",
        raises_test.assertion_count
    );
}

#[test]
fn unittest_class_nodeids_include_class_segment() {
    // Sanity check the existing class-prefix nodeid plumbing for the new
    // unittest fixture — every method should appear under `TestThing::`.
    let path = fixture_path("tests/fixtures/unittest_style/test_unittest.py");
    let inv = coati::run_static(&path).expect("run_static on unittest fixture");
    let nodeids: Vec<&str> = inv.test_functions.iter().map(|t| t.nodeid.as_str()).collect();
    assert_eq!(
        inv.test_functions.len(),
        3,
        "expected 3 unittest methods discovered, got nodeids: {nodeids:?}"
    );
    for n in &nodeids {
        assert!(n.contains("::TestThing::"), "expected TestThing class prefix in {n:?}");
    }
}

#[test]
fn unittest_assert_predicate_rejects_snake_case_and_prefix_collisions() {
    // `test_camelcase_strictness` in the fixture mixes one real
    // `self.assertEqual` with three lookalikes that must be rejected:
    //   - `self.assert_called_with` (Mock API, snake_case after `assert`)
    //   - `self.assertion_count`    (user helper, `assert<lowercase>` prefix)
    //   - `self.assert_logged`      (user helper, snake_case after `assert`)
    // Only the real assertion must count.
    let path = fixture_path("tests/fixtures/unittest_style/test_unittest.py");
    let inv = coati::run_static(&path).expect("run_static on unittest fixture");
    let strict_test = inv
        .test_functions
        .iter()
        .find(|t| t.nodeid.ends_with("::test_camelcase_strictness"))
        .expect("test_camelcase_strictness must be discovered");
    assert_eq!(
        strict_test.assertion_count, 1,
        "only the real `self.assertEqual` should count; got {} (lookalikes leaked through)",
        strict_test.assertion_count
    );
}
