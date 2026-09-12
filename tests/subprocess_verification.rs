//! Regression guard for issue #17: a test can do its verification in a child
//! process and propagate the failure through `subprocess.run(..., check=True)`.
//!
//! Such a test has zero locally visible assertions, but it is not
//! assertionless — a failing child fails the parent. The inventory records
//! that evidence in `external_verification_count` (a separate field, so
//! `assertion_count` stays a syntactic count) and the suspicion score drops
//! the zero-assert term for those tests only.
//!
//! One fixture per shape the issue asks to cover, with aliasing and
//! self-manufactured evidence split into files of their own: checked
//! child-failure propagation (`test_checked.py`, `test_aliases.py`),
//! unchecked subprocesses (`test_unchecked.py`), a checked subprocess test
//! carrying an independent problematic signal (`test_mixed_signal.py`), and
//! checked calls against a double the test installed (`test_shadowed.py`).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use pycoati::TestRecord;

fn fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/subprocess_verification");
    p.push(name);
    p
}

/// Records of one fixture file, keyed by bare test-function name.
fn records(name: &str) -> BTreeMap<String, TestRecord> {
    let path = fixture(name);
    let inv = pycoati::run_static(&path).expect("run_static on subprocess fixture");
    inv.test_functions
        .into_iter()
        .map(|t| {
            let bare = t.nodeid.rsplit("::").next().expect("nodeid has a test name").to_string();
            (bare, t)
        })
        .collect()
}

fn record(file: &str, test: &str) -> TestRecord {
    records(file).remove(test).unwrap_or_else(|| panic!("{file} has no test named {test}"))
}

/// `w_zero_asserts` from `WEIGHTS.md`. Duplicated here on purpose: the
/// weights are crate-private, and this test asserts on the score a consumer
/// of the JSON inventory actually sees.
const W_ZERO_ASSERTS: f64 = 0.20;

#[test]
fn checked_run_is_external_verification_evidence() {
    let t = record("test_checked.py", "test_child_process_contract");
    assert_eq!(t.assertion_count, 0, "assertion_count stays a syntactic count");
    assert_eq!(
        t.external_verification_count, 1,
        "`subprocess.run(..., check=True)` propagates a child failure"
    );
}

#[test]
fn check_call_and_check_output_are_verification_evidence() {
    let recs = records("test_checked.py");
    for name in ["test_check_call_contract", "test_check_output_contract"] {
        let t = &recs[name];
        assert_eq!(t.assertion_count, 0, "{name} asserts nothing locally");
        assert_eq!(
            t.external_verification_count, 1,
            "{name}: check_call / check_output raise on a non-zero exit"
        );
    }
}

#[test]
fn explicit_check_returncode_is_verification_evidence() {
    let t = record("test_checked.py", "test_explicit_returncode_check");
    assert_eq!(t.assertion_count, 0);
    assert_eq!(
        t.external_verification_count, 1,
        "`CompletedProcess.check_returncode()` raises on a non-zero exit"
    );
}

#[test]
fn aliased_and_imported_checked_calls_resolve() {
    let recs = records("test_aliases.py");
    for name in ["test_aliased_module", "test_aliased_function", "test_imported_check_call"] {
        let t = &recs[name];
        assert_eq!(
            t.external_verification_count, 1,
            "{name}: the import map should canonicalize the call head to `subprocess.*`"
        );
    }
}

#[test]
fn checked_subprocess_test_is_not_scored_as_assertionless() {
    let checked = record("test_checked.py", "test_child_process_contract");
    let unchecked = record("test_unchecked.py", "test_unchecked_run");

    assert!(
        checked.suspicion_score < W_ZERO_ASSERTS,
        "a checked-subprocess test must not carry the zero-assert weight, got {}",
        checked.suspicion_score
    );
    assert!(
        unchecked.suspicion_score > W_ZERO_ASSERTS,
        "an unchecked-subprocess test is still assertionless, got {}",
        unchecked.suspicion_score
    );
    assert!(
        unchecked.suspicion_score - checked.suspicion_score > 0.15,
        "the two must be separated by roughly the zero-assert weight: {} vs {}",
        unchecked.suspicion_score,
        checked.suspicion_score
    );
}

#[test]
fn unchecked_subprocess_is_not_verification_evidence() {
    let recs = records("test_unchecked.py");
    for name in ["test_unchecked_run", "test_explicit_check_false", "test_popen_without_check"] {
        let t = &recs[name];
        assert_eq!(
            t.external_verification_count, 0,
            "{name}: a discarded exit status proves nothing"
        );
        assert_eq!(t.assertion_count, 0, "{name} has no local assertion either");
        assert!(
            t.suspicion_score > W_ZERO_ASSERTS,
            "{name} must keep the assertionless contribution, got {}",
            t.suspicion_score
        );
    }
}

#[test]
fn asserted_return_code_counts_as_an_assertion_not_as_verification() {
    let t = record("test_unchecked.py", "test_asserted_return_code");
    assert_eq!(t.assertion_count, 1, "the `assert completed.returncode == 0` is the evidence");
    assert_eq!(
        t.external_verification_count, 0,
        "an unchecked `subprocess.run` is not itself evidence, even when its result is asserted on"
    );
    assert!(
        t.suspicion_score < W_ZERO_ASSERTS,
        "an asserting test never carried the zero-assert weight, got {}",
        t.suspicion_score
    );
}

#[test]
fn independent_signals_survive_external_verification() {
    let mixed = record("test_mixed_signal.py", "test_checked_subprocess_with_heavy_patching");
    let plain = record("test_checked.py", "test_child_process_contract");

    assert_eq!(mixed.external_verification_count, 1, "the checked run is still recognised");
    assert_eq!(mixed.patch_decorator_count, 3, "the patch decorators are still counted");
    assert!(
        mixed.smell_hits.iter().any(|h| h.category == "mock_overuse"),
        "mock_overuse must still fire: {:?}",
        mixed.smell_hits
    );
    assert!(
        mixed.suspicion_score > plain.suspicion_score,
        "recognising the subprocess must not suppress unrelated signals: {} vs {}",
        mixed.suspicion_score,
        plain.suspicion_score
    );
}

#[test]
fn a_checked_call_against_a_double_is_not_evidence() {
    // The test replaced the process boundary, so `check=True` checks the
    // double's exit status, which is never non-zero. Crediting that would
    // silence the assertionless signal on textbook mock theater.
    let recs = records("test_shadowed.py");
    for name in [
        "test_patched_subprocess_run",
        "test_fixture_shadows_the_module",
        "test_monkeypatched_helper",
        // The receiver of `check_returncode()` is a local, so the shape has
        // to be rejected through the boundary the test replaced.
        "test_mocked_completed_process",
        "test_magicmock_returncode",
        "test_qualified_magicmock_returncode",
        // `@unittest.mock.patch.object(subprocess, "run")` — the
        // fully-qualified form of the first case.
        "test_qualified_patch_object",
    ] {
        let t = &recs[name];
        assert_eq!(
            t.external_verification_count, 0,
            "{name}: the checked call runs against a test double"
        );
        assert!(
            t.suspicion_score > W_ZERO_ASSERTS,
            "{name} must keep the assertionless contribution, got {}",
            t.suspicion_score
        );
    }
}

#[test]
fn verification_inside_a_nested_helper_is_not_evidence_either_way() {
    // The called case is the deliberate false negative: pinned so a future
    // change that starts walking into nested defs has to acknowledge both
    // directions.
    let t = record("test_shadowed.py", "test_helper_defined_and_called");
    assert_eq!(t.external_verification_count, 0);
    assert!(t.suspicion_score > W_ZERO_ASSERTS, "got {}", t.suspicion_score);
}

#[test]
fn verification_inside_an_uncalled_nested_helper_is_not_evidence() {
    // Defining a helper is not running it. For a signal whose job is to
    // suppress another one, under-counting is the safe direction.
    let t = record("test_shadowed.py", "test_helper_defined_but_never_called");
    assert_eq!(t.external_verification_count, 0);
    assert!(
        t.suspicion_score > W_ZERO_ASSERTS,
        "expected the assertionless contribution, got {}",
        t.suspicion_score
    );
}

#[test]
fn a_checked_call_whose_error_is_swallowed_is_not_evidence() {
    // "A failing child fails the test" is the whole premise. An `except`
    // that catches `CalledProcessError` breaks it, and so does a call that
    // only runs on the error path.
    let recs = records("test_shadowed.py");
    for name in ["test_swallowed_called_process_error", "test_checked_only_in_except_branch"] {
        let t = &recs[name];
        assert_eq!(t.external_verification_count, 0, "{name}: the failure may not escape");
        assert!(
            t.suspicion_score > W_ZERO_ASSERTS,
            "{name} must keep the assertionless contribution, got {}",
            t.suspicion_score
        );
    }
}

#[test]
fn a_self_rooted_return_code_check_is_evidence() {
    // `self` binds the test instance, never a double — a `CompletedProcess`
    // stashed on it is as real as a local.
    let t = record("test_checked.py", "test_self_rooted_returncode");
    assert_eq!(t.external_verification_count, 1);
}

#[test]
fn a_real_assertion_alongside_a_checked_call_keeps_both_signals() {
    let t = record("test_checked.py", "test_asserts_and_checks");
    assert_eq!(t.assertion_count, 1, "the local assert is still counted syntactically");
    assert_eq!(t.external_verification_count, 1, "and the checked call is still evidence");
}

#[test]
fn a_double_installed_for_the_whole_class_is_not_evidence() {
    // The shadow's scope is the class, not the method: a class-level
    // `@patch` and a `setUp`-started patcher replace the boundary for every
    // test in the class.
    let recs = records("test_class_shadowed.py");
    for name in ["test_class_level_patch", "test_patcher_from_setup"] {
        let t = &recs[name];
        assert_eq!(t.external_verification_count, 0, "{name}: the class installed the double");
        assert!(
            t.suspicion_score > W_ZERO_ASSERTS,
            "{name} must keep the assertionless contribution, got {}",
            t.suspicion_score
        );
    }
}

#[test]
fn a_suppressed_error_is_not_evidence() {
    // `contextlib.suppress(CalledProcessError)` is the one-line spelling of
    // the try/except that already disqualifies a call.
    let t = record("test_class_shadowed.py", "test_suppressed_error");
    assert_eq!(t.external_verification_count, 0);
    assert!(t.suspicion_score > W_ZERO_ASSERTS, "got {}", t.suspicion_score);
}

#[test]
fn doubles_bound_by_with_walrus_or_tuple_are_tracked() {
    let recs = records("test_class_shadowed.py");
    for name in ["test_with_as_binding", "test_walrus_bound_double", "test_tuple_bound_double"] {
        let t = &recs[name];
        assert_eq!(
            t.external_verification_count, 0,
            "{name}: the receiver was bound to a double, whatever the binding form"
        );
    }
}
