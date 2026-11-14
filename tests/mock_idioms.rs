//! Mock-assertion idioms against `tests/fixtures/mock_idioms/`, a file of
//! tests that all pass under real pytest. Each `test_mock_only_*` test
//! verifies nothing but its test double and must be flagged as such; each
//! `test_real_*` test verifies a produced value and must not be.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use pycoati::TestRecord;

fn records() -> BTreeMap<String, TestRecord> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/mock_idioms/test_mock_idioms.py");
    let inv = pycoati::run_static(&path).expect("run_static on mock_idioms fixture");
    inv.test_functions
        .into_iter()
        .map(|t| (t.nodeid.rsplit("::").next().unwrap_or("").to_string(), t))
        .collect()
}

fn categories(t: &TestRecord) -> Vec<&str> {
    t.smell_hits.iter().map(|h| h.category.as_str()).collect()
}

#[test]
fn every_fixture_test_is_collected() {
    assert_eq!(records().len(), 7);
}

#[test]
fn mock_only_tests_are_flagged_whatever_the_idiom() {
    let recs = records();
    for (name, expected_asserts) in [
        ("test_mock_only_bare_assert_methods", 4),
        ("test_mock_only_comparisons", 6),
        ("test_mock_only_negation", 1),
        ("test_mock_only_unittest_forms", 4),
    ] {
        let t = recs.get(name).unwrap_or_else(|| panic!("{name} not collected"));
        assert_eq!(t.assertion_count, expected_asserts, "{name}: assertion_count");
        assert_eq!(t.mock_assertion_count, expected_asserts, "{name}: mock_assertion_count");
        assert!(t.only_asserts_on_mock, "{name}: only_asserts_on_mock");
        assert!(!t.verifies_nothing(), "{name}: must not be treated as assertionless");
        assert!(
            categories(t).contains(&"mock_only_assertions"),
            "{name}: expected mock_only_assertions, got {:?}",
            categories(t)
        );
        let signals: Vec<String> =
            pycoati::accept::active_signals(t).iter().map(|s| s.as_str().to_string()).collect();
        assert!(
            !signals.iter().any(|s| s == "zero_asserts"),
            "{name}: zero_asserts must not fire, got {signals:?}"
        );
    }
}

#[test]
fn tests_that_verify_a_produced_value_are_not_mock_only() {
    let recs = records();
    for (name, asserts, mock_asserts) in [
        ("test_real_result_beside_mock_assert", 2, 1),
        ("test_real_return_value_comparison", 1, 0),
        ("test_real_unittest_forms", 2, 0),
    ] {
        let t = recs.get(name).unwrap_or_else(|| panic!("{name} not collected"));
        assert_eq!(t.assertion_count, asserts, "{name}: assertion_count");
        assert_eq!(t.mock_assertion_count, mock_asserts, "{name}: mock_assertion_count");
        assert!(!t.only_asserts_on_mock, "{name}: only_asserts_on_mock");
        assert!(
            !categories(t).contains(&"mock_only_assertions"),
            "{name}: unexpected mock_only_assertions"
        );
    }
}

#[test]
fn bare_mock_assertions_score_the_mock_only_term_not_the_zero_assert_term() {
    // The two terms are mutually exclusive; a test verifying only through
    // bare `assert_*` calls lands on the mock-only side.
    let recs = records();
    let bare = recs.get("test_mock_only_bare_assert_methods").expect("collected");
    assert!(!bare.verifies_nothing(), "the zero-assert term must be off");
    assert_eq!(bare.patch_decorator_count, 0);
    assert_eq!(bare.smell_hits.len(), 1, "only mock_only_assertions: {:?}", categories(bare));
    // `def` on row 18, first assertion on row 21, four sites → 3 / 4.
    assert!(
        (bare.setup_to_assertion_ratio - 0.75).abs() < 1e-9,
        "ratio {}",
        bare.setup_to_assertion_ratio
    );
    // Weights from WEIGHTS.md, duplicated on purpose: this asserts on the
    // score a consumer of the inventory sees. Terms: mock-only (0.35),
    // patch (0), setup sigmoid, zero-asserts (0), one smell of three.
    let setup = 0.15 / (1.0 + (-(0.75_f64 - 8.0) / 4.0).exp());
    let expected = 0.35 + setup + 0.10 / 3.0;
    assert!(
        (bare.suspicion_score - expected).abs() < 1e-9,
        "expected {expected}, got {}",
        bare.suspicion_score
    );
}
