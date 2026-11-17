//! Helper-based verification against `tests/fixtures/helper_verification/`,
//! a pytest-valid file. `test_verified_*` tests verify through a helper and
//! must not read as assertionless; `test_unverified_*` tests call a helper
//! that pycoati cannot credit and must keep the zero-assert signal.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use pycoati::TestRecord;

fn records() -> BTreeMap<String, TestRecord> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures/helper_verification/test_helpers.py");
    let inv = pycoati::run_static(&path).expect("run_static on helper_verification fixture");
    inv.test_functions
        .into_iter()
        .map(|t| (t.nodeid.rsplit("::").next().unwrap_or("").to_string(), t))
        .collect()
}

fn signals(t: &TestRecord) -> Vec<&'static str> {
    pycoati::accept::active_signals(t).into_iter().map(pycoati::accept::Signal::as_str).collect()
}

#[test]
fn helper_verified_tests_are_not_assertionless() {
    let recs = records();
    for name in [
        "test_verified_by_module_helper",
        "test_verified_by_warning_block",
        "test_verified_by_deprecated_call",
        "test_verified_by_pytest_fail",
        "test_verified_by_method_helper",
    ] {
        let t = recs.get(name).unwrap_or_else(|| panic!("{name} not collected"));
        assert_eq!(t.assertion_count, 0, "{name}: helpers stay out of assertion_count");
        assert_eq!(t.helper_assertion_count, 1, "{name}: helper_assertion_count");
        assert!(!t.verifies_nothing(), "{name}");
        assert!(!signals(t).contains(&"zero_asserts"), "{name}: {:?}", signals(t));
        assert!(!t.only_asserts_on_mock, "{name}");
    }
}

#[test]
fn uncreditable_helpers_leave_the_zero_assert_signal() {
    let recs = records();
    for name in ["test_unverified_mock_only_helper", "test_unverified_two_levels_deep"] {
        let t = recs.get(name).unwrap_or_else(|| panic!("{name} not collected"));
        assert_eq!(t.helper_assertion_count, 0, "{name}");
        assert!(t.verifies_nothing(), "{name}");
        assert!(signals(t).contains(&"zero_asserts"), "{name}: {:?}", signals(t));
    }
}
