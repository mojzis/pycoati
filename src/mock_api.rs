//! Single source of truth for the Python `unittest.mock` attribute set used
//! by the `only_asserts_on_mock` predicate.
//!
//! The literal attribute names live in this module only. No other source
//! file should reference these strings directly — go through
//! [`is_mock_api_attribute`] or the [`MOCK_API_ATTRIBUTES`] slice.

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

/// True iff `name` matches a Mock-API attribute (exact, case-sensitive).
#[inline]
pub fn is_mock_api_attribute(name: &str) -> bool {
    MOCK_API_ATTRIBUTES.contains(&name)
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
}
