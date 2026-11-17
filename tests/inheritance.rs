//! Same-file base classes against `tests/fixtures/inheritance/`, a
//! pytest-valid file: test methods inherited from a same-file base are
//! collected on each subclass under pytest's nodeids, and an assert helper
//! on a base class credits the subclass test that calls it.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

use pycoati::Inventory;

fn inventory() -> Inventory {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/inheritance/test_inheritance.py");
    pycoati::run_static(&path).expect("run_static on inheritance fixture")
}

#[test]
fn inherited_tests_are_collected_under_pytest_nodeids() {
    let inv = inventory();
    // Single-file mode keeps the path it was given; compare from the file
    // name on, which is what `pytest --collect-only` prints in its dir.
    let nodeids: Vec<&str> = inv
        .test_functions
        .iter()
        .map(|t| t.nodeid.rsplit_once('/').map_or(t.nodeid.as_str(), |(_, tail)| tail))
        .collect();
    // `lib.rs` orders records by line; the two inherited copies share the
    // base's line and keep emission order.
    assert_eq!(
        nodeids,
        [
            "test_inheritance.py::Checks::test_verified_by_base_helper",
            "test_inheritance.py::TestA::test_roundtrip",
            "test_inheritance.py::TestB::test_roundtrip",
            "test_inheritance.py::TestB::test_b_only",
        ]
    );
    let roundtrips: Vec<u64> = inv
        .test_functions
        .iter()
        .filter(|t| t.nodeid.ends_with("::test_roundtrip"))
        .map(|t| t.line)
        .collect();
    assert_eq!(roundtrips, [24, 24], "inherited tests report the base's def line");
    assert_eq!(inv.files.len(), 1);
    assert_eq!(inv.files[0].test_function_count, 4);
}

#[test]
fn a_base_class_assert_helper_credits_the_subclass_test() {
    let inv = inventory();
    let t = inv
        .test_functions
        .iter()
        .find(|t| t.nodeid.ends_with("::test_verified_by_base_helper"))
        .expect("collected");
    assert_eq!(t.assertion_count, 0);
    assert_eq!(t.helper_assertion_count, 1);
    assert!(!t.verifies_nothing());
    let signals: Vec<&str> = pycoati::accept::active_signals(t)
        .into_iter()
        .map(pycoati::accept::Signal::as_str)
        .collect();
    assert!(!signals.contains(&"zero_asserts"), "{signals:?}");
}
