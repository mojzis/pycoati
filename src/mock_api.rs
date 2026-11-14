//! Mock-API name sets.
//!
//! Single source of truth for the Python `unittest.mock` names pycoati
//! recognises:
//!
//! * [`MOCK_API_ATTRIBUTES`] — attributes whose use marks an assertion as
//!   targeting the test double (`mock_assertion_count`,
//!   `only_asserts_on_mock`); [`is_mock_assert_method`] selects the subset
//!   that is called as a bare assertion statement;
//! * [`MOCK_MODULES`] and [`MOCK_ASSERTION_HELPERS`] — the modules that
//!   export the API and the helpers (`call`, `ANY`) that build the expected
//!   side of a mock assertion, matched by [`is_mock_assertion_helper`];
//! * [`MOCK_CONSTRUCTORS`] — names counted as Mock-API constructions;
//! * [`STUB_HEADS`] — fixture-driven patching call-heads.
//!
//! The literal names live in this module only. No other source file should
//! spell them out — go through the predicates or the slices.

use crate::sut_calls::is_at_or_under;

/// Attribute names on `unittest.mock.Mock` / `MagicMock` instances that
/// indicate an assertion about the mock itself rather than about the system
/// under test.
///
/// Order is irrelevant to callers — membership is the only query.
pub const MOCK_API_ATTRIBUTES: &[&str] = &[
    "called",
    "call_count",
    "call_args",
    "call_args_list",
    "assert_called",
    "assert_called_with",
    "assert_called_once",
    "assert_called_once_with",
    "assert_not_called",
    "assert_any_call",
    "assert_has_calls",
];

/// Prefix that marks a [`MOCK_API_ATTRIBUTES`] entry as an assertion
/// *method*: called as a bare statement (`m.assert_called_once_with(1)`) it
/// raises `AssertionError` itself, so it is an assertion site with no
/// `assert` keyword in sight.
const MOCK_ASSERT_METHOD_PREFIX: &str = "assert_";

/// Modules that export the Mock API: the standard library's `unittest.mock`
/// and the `PyPI` backport `mock`. Canonical (import-map-resolved) names are
/// matched against these prefixes.
pub(crate) const MOCK_MODULES: &[&str] = &["unittest.mock", "mock"];

/// Names exported by a [`MOCK_MODULES`] module that build the *expected*
/// side of a Mock-API assertion.
///
/// `call(1, x)`, `call.save(2)`, `ANY` are not values the system under test
/// produced. An argument built from
/// these says nothing about production behaviour, so a unittest assertion
/// comparing a Mock-API attribute against one is still about the mock.
pub(crate) const MOCK_ASSERTION_HELPERS: &[&str] = &["call", "ANY"];

/// Mock-API constructor names.
///
/// Names whose call-expression head identifies a Mock-API object being
/// constructed inside a test body. `patch` is included because
/// `patch(...)` used as a context manager (rather than as a decorator) is
/// effectively a mock construction at the call site.
pub const MOCK_CONSTRUCTORS: &[&str] =
    &["Mock", "MagicMock", "AsyncMock", "create_autospec", "patch"];

/// Stub-API dotted call-heads.
///
/// Fixture-driven patching is structurally distinct from `unittest.mock`
/// construction / decoration: a test asks the `monkeypatch` or `mocker`
/// fixture to temporarily rewire the system under test, rather than
/// instantiating a `Mock()` directly. The `stubs_count` signal tracks how
/// many such fixture-driven patches a test (or file) performs, and feeds
/// into the `mock_overuse` smell alongside `mock_construction_count` and
/// `patch_decorator_count`.
///
/// Each entry is an exact dotted call-head matched against the
/// `call_head_chain` of a `call_expression`. The list is closed: anything
/// not enumerated here is not a stub for counting purposes.
pub(crate) const STUB_HEADS: &[&str] = &[
    // pytest's built-in `monkeypatch` fixture.
    "monkeypatch.setattr",
    "monkeypatch.setenv",
    "monkeypatch.delattr",
    "monkeypatch.delenv",
    "monkeypatch.context",
    "monkeypatch.syspath_prepend",
    "monkeypatch.chdir",
    // pytest-mock's `mocker` fixture wraps the `unittest.mock` API.
    "mocker.patch",
    "mocker.patch.object",
    "mocker.patch.dict",
    "mocker.patch.multiple",
    "mocker.spy",
    "mocker.stub",
    "mocker.MagicMock",
];

/// True iff `name` matches a Mock-API attribute (exact, case-sensitive).
#[inline]
pub fn is_mock_api_attribute(name: &str) -> bool {
    MOCK_API_ATTRIBUTES.contains(&name)
}

/// True iff `name` is a Mock-API assertion method (`assert_called`,
/// `assert_called_once_with`, …): a [`MOCK_API_ATTRIBUTES`] entry that is
/// called rather than read.
#[inline]
pub(crate) fn is_mock_assert_method(name: &str) -> bool {
    name.starts_with(MOCK_ASSERT_METHOD_PREFIX) && is_mock_api_attribute(name)
}

/// True iff `canonical` — a name already resolved through the file's
/// import map — is, or lives under, one of the [`MOCK_ASSERTION_HELPERS`]
/// in a [`MOCK_MODULES`] module: `unittest.mock.call`,
/// `unittest.mock.call.save`, `mock.ANY`. A bare `call` that never resolved
/// to the Mock API is an arbitrary function and does not match.
pub(crate) fn is_mock_assertion_helper(canonical: &str) -> bool {
    MOCK_MODULES.iter().any(|module| {
        canonical
            .strip_prefix(module)
            .and_then(|rest| rest.strip_prefix('.'))
            .is_some_and(|rest| MOCK_ASSERTION_HELPERS.iter().any(|h| is_at_or_under(rest, h)))
    })
}

/// True iff the **last segment** of a dotted call-head names a Mock-API
/// constructor: `MagicMock`, `mock.MagicMock`, `unittest.mock.MagicMock`.
///
/// Deliberately broader than [`is_mock_constructor`], which matches the
/// head segment only so that `mock_construction_count` — a published
/// count — cannot be inflated by an unrelated `x.Mock()`. This one feeds
/// the parser's test-double detection, where the unsafe direction is
/// inverted: missing a constructor there credits a double with being real
/// verification.
#[inline]
pub(crate) fn chain_constructs_a_mock(chain: &str) -> bool {
    chain.rsplit('.').next().is_some_and(is_mock_constructor)
}

/// True iff `name` matches a Mock-API constructor (exact, case-sensitive).
#[inline]
pub fn is_mock_constructor(name: &str) -> bool {
    MOCK_CONSTRUCTORS.contains(&name)
}

/// True iff `head` matches one of the fixture-driven stub call-heads in
/// [`STUB_HEADS`] (exact, case-sensitive). The input is the dotted
/// call-head emitted by the parser (e.g. `"monkeypatch.setattr"`,
/// `"mocker.patch.object"`).
#[inline]
pub(crate) fn is_stub_call_head(head: &str) -> bool {
    STUB_HEADS.contains(&head)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_assertion_methods_are_mock_api() {
        assert!(is_mock_api_attribute("assert_called_once_with"));
        assert!(is_mock_api_attribute("assert_called"));
        assert!(is_mock_api_attribute("called"));
        assert!(is_mock_api_attribute("call_count"));
    }

    #[test]
    fn arbitrary_names_are_not_mock_api() {
        assert!(!is_mock_api_attribute("save"));
        assert!(!is_mock_api_attribute("foo"));
        assert!(!is_mock_api_attribute(""));
        // Case-sensitive: PEP-8 says snake_case, no capitals expected.
        assert!(!is_mock_api_attribute("Assert_Called"));
    }

    #[test]
    fn assert_methods_are_the_called_subset_of_the_mock_api() {
        for name in [
            "assert_called",
            "assert_called_with",
            "assert_called_once",
            "assert_called_once_with",
            "assert_not_called",
            "assert_any_call",
            "assert_has_calls",
        ] {
            assert!(is_mock_assert_method(name), "{name} should be a mock assert method");
        }
        // Attributes that are read, not called, are not assertion methods.
        for name in ["called", "call_count", "call_args", "call_args_list"] {
            assert!(!is_mock_assert_method(name), "{name} is read, not called");
        }
        // The prefix alone is not enough: the name must be in the API.
        assert!(!is_mock_assert_method("assert_logged"));
        assert!(!is_mock_assert_method("assertEqual"));
    }

    #[test]
    fn assertion_helpers_match_only_under_a_mock_module() {
        assert!(is_mock_assertion_helper("unittest.mock.call"));
        assert!(is_mock_assertion_helper("unittest.mock.call.save"));
        assert!(is_mock_assertion_helper("unittest.mock.ANY"));
        assert!(is_mock_assertion_helper("mock.call"));
        assert!(!is_mock_assertion_helper("call"), "canonicalized first");
        assert!(!is_mock_assertion_helper("myproj.call"));
        assert!(!is_mock_assertion_helper("unittest.mock.caller"));
        assert!(!is_mock_assertion_helper("unittest.mock.Mock"));
        assert!(!is_mock_assertion_helper("mockery.call"));
    }

    #[test]
    fn attribute_set_has_eleven_entries() {
        assert_eq!(MOCK_API_ATTRIBUTES.len(), 11);
    }

    #[test]
    fn known_mock_constructors_are_recognised() {
        for name in ["Mock", "MagicMock", "AsyncMock", "create_autospec", "patch"] {
            assert!(is_mock_constructor(name), "{name} should be a mock constructor");
        }
    }

    #[test]
    fn arbitrary_names_are_not_mock_constructors() {
        assert!(!is_mock_constructor("Repository"));
        assert!(!is_mock_constructor("mock"));
        assert!(!is_mock_constructor(""));
        // Case-sensitive: `mock.patch` is matched at the decorator-name
        // level, not via this constructor predicate.
        assert!(!is_mock_constructor("MOCK"));
    }

    #[test]
    fn known_stub_heads_are_recognised() {
        for head in [
            "monkeypatch.setattr",
            "monkeypatch.setenv",
            "monkeypatch.delattr",
            "monkeypatch.delenv",
            "monkeypatch.context",
            "monkeypatch.syspath_prepend",
            "monkeypatch.chdir",
            "mocker.patch",
            "mocker.patch.object",
            "mocker.patch.dict",
            "mocker.patch.multiple",
            "mocker.spy",
            "mocker.stub",
            "mocker.MagicMock",
        ] {
            assert!(is_stub_call_head(head), "{head} should be a stub call-head");
        }
    }

    #[test]
    fn arbitrary_heads_are_not_stub_heads() {
        // Bare `patch` is a Mock-API constructor, not a stub call-head.
        assert!(!is_stub_call_head("patch"));
        assert!(!is_stub_call_head("mock.patch"));
        // `monkeypatch` alone (no method) is not a call.
        assert!(!is_stub_call_head("monkeypatch"));
        // `mocker` alone is the fixture parameter, not a call.
        assert!(!is_stub_call_head("mocker"));
        // Empty / unrelated.
        assert!(!is_stub_call_head(""));
        assert!(!is_stub_call_head("repo.save"));
        // Case-sensitive.
        assert!(!is_stub_call_head("MonkeyPatch.setattr"));
    }

    #[test]
    fn stub_heads_disjoint_from_mock_constructors() {
        // Constructors and stub heads must not overlap — they feed
        // separate counts that are summed by the smell layer.
        for h in STUB_HEADS {
            assert!(
                !MOCK_CONSTRUCTORS.contains(h),
                "{h} appears in both STUB_HEADS and MOCK_CONSTRUCTORS"
            );
        }
    }
}
