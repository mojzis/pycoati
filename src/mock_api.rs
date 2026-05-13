//! Mock-API name sets.
//!
//! Single source of truth for the Python `unittest.mock` attribute set used
//! by the `only_asserts_on_mock` predicate, plus the constructor-name set
//! used to count Mock-API object constructions in test bodies.
//!
//! The literal attribute / constructor names live in this module only. No
//! other source file should reference these strings directly — go through
//! [`is_mock_api_attribute`] / [`is_mock_constructor`] or the two slices.

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
