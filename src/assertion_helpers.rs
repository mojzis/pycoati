//! Assertion-helper name sets.
//!
//! Plenty of tests verify through a call rather than an `assert` keyword:
//!
//! ```python
//! pandas.testing.assert_frame_equal(result, expected)
//! with pytest.warns(DeprecationWarning):
//!     old_api()
//! ```
//!
//! Such a test fails when the helper raises, so counting it as assertionless
//! is wrong. The parser records each such site in
//! [`crate::TestRecord::helper_assertion_count`], kept apart from the
//! syntactic `assertion_count` the way `external_verification_count` is.
//!
//! Two sources, both deliberately closed:
//!
//! * **known library helpers**, listed here and matched against call heads
//!   *canonicalized* through the file's import map — so
//!   `import numpy as np; np.testing.assert_allclose(...)` and
//!   `from pandas.testing import assert_frame_equal` both resolve;
//! * **same-file helpers** — a module-level function, or a method of the
//!   test's own class, whose body holds a non-mock effective assertion. The
//!   parser follows one level of indirection and no further: no imports, no
//!   `conftest.py`, no helper-calls-helper chains. A helper whose only
//!   assertions target the Mock API is not counted, which under-credits
//!   verification — the safe direction.
//!
//! Like [`crate::mock_api`] and [`crate::verification`], this module is the
//! single source of truth for the literal names.

/// Library modules whose `assert_*` functions raise `AssertionError` on a
/// mismatch. Matched as `<module>.assert_<anything>`, one segment deep.
pub const ASSERTION_HELPER_MODULES: &[&str] =
    &["numpy.testing", "pandas.testing", "polars.testing"];

/// Prefix a function in an [`ASSERTION_HELPER_MODULES`] module must carry to
/// be an assertion: `assert_frame_equal` is, `makeDataFrame` is not.
const ASSERTION_HELPER_PREFIX: &str = "assert_";

/// Canonical calls that fail the test outright when reached.
pub const FAILING_CALLS: &[&str] = &["pytest.fail"];

/// Canonical context managers that fail the test unless their block does
/// something specific — the warning-shaped siblings of `pytest.raises`,
/// counted per `with` block in the same way.
pub const CHECKING_CONTEXT_MANAGERS: &[&str] = &["pytest.warns", "pytest.deprecated_call"];

/// True iff a canonical call-head is a library assertion helper or a
/// failing call: `numpy.testing.assert_allclose`, `pytest.fail`.
pub fn is_library_assertion_call(canonical: &str) -> bool {
    FAILING_CALLS.contains(&canonical)
        || ASSERTION_HELPER_MODULES.iter().any(|module| {
            canonical.strip_prefix(module).and_then(|rest| rest.strip_prefix('.')).is_some_and(
                |name| name.starts_with(ASSERTION_HELPER_PREFIX) && !name.contains('.'),
            )
        })
}

/// True iff a canonical call-head is a checking context manager:
/// `pytest.warns`, `pytest.deprecated_call`.
pub fn is_checking_context_manager(canonical: &str) -> bool {
    CHECKING_CONTEXT_MANAGERS.contains(&canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_helpers_match_under_their_modules() {
        for c in [
            "numpy.testing.assert_allclose",
            "numpy.testing.assert_array_equal",
            "pandas.testing.assert_frame_equal",
            "polars.testing.assert_series_equal",
            "pytest.fail",
        ] {
            assert!(is_library_assertion_call(c), "{c}");
        }
        for c in [
            "assert_frame_equal",
            "numpy.allclose",
            "pandas.testing.makeDataFrame",
            "myproj.testing.assert_ok",
            "numpy.testingx.assert_allclose",
            "numpy.testing.assert_allclose.inner",
            "pytest.raises",
        ] {
            assert!(!is_library_assertion_call(c), "{c}");
        }
    }

    #[test]
    fn checking_context_managers_match_exactly() {
        assert!(is_checking_context_manager("pytest.warns"));
        assert!(is_checking_context_manager("pytest.deprecated_call"));
        assert!(!is_checking_context_manager("warns"), "canonicalized first");
        assert!(!is_checking_context_manager("pytest.raises"), "counted as an assertion");
    }
}
