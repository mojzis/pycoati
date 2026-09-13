//! External-verification name sets.
//!
//! Some tests do their verification outside the test process and let the
//! failure propagate back in. The canonical shape is a child process whose
//! non-zero exit status is checked:
//!
//! ```python
//! subprocess.run([sys.executable, "-c", "assert 2 + 2 == 4"], check=True)
//! ```
//!
//! That test has zero `assert_statement` nodes in its body, yet a failing
//! child fails it through `CalledProcessError`. Counting it as assertionless
//! is wrong, so the parser records each such call site in
//! [`crate::TestRecord::external_verification_count`].
//!
//! The inference is deliberately narrow. `check=True` proves only that a
//! child failure reaches the parent — it says nothing about whether the
//! child exercises production behaviour — so the count is evidence that
//! suppresses exactly one signal (the zero-assert term) and adjusts one
//! other (the setup ratio is measured to the verification call site rather
//! than to the end of the body), never a blanket exemption for subprocess
//! tests. A call whose exit status is discarded (`subprocess.run(...)` with
//! no `check`, `check=False`, `Popen`) is not evidence and needs its own,
//! such as an asserted return code.
//!
//! Three things the parser refuses to credit, listed here so the limits are
//! decisions rather than accidents:
//!
//! * a checked call against a name the test replaced with a double — a
//!   `@patch`ed, `monkeypatch.setattr`-ed, or fixture-shadowed target never
//!   exits non-zero, so checking it proves nothing;
//! * a checked call whose failure may never reach pytest — nested in a
//!   `def` or `lambda` the test may never invoke, or inside a `try` that
//!   has an `except` clause to swallow the `CalledProcessError`;
//! * a call head that does not resolve to `subprocess` — notably under
//!   `from subprocess import *`, which binds an unknown set of names.
//!
//! The last two under-count, which is the safe direction: a missed piece of
//! evidence leaves a correct test on the review list, while a false one
//! takes a bad test off it.
//!
//! Like [`crate::mock_api`], this module is the single source of truth for
//! the literal names; nothing else in the crate spells them out.

use crate::sut_calls::is_at_or_under;

/// Canonical dotted call-heads that raise on a non-zero child exit status
/// with no further arguments needed.
///
/// Matched against the *canonicalized* head chain — the parser rewrites the
/// head segment through the file's import map first, so
/// `import subprocess as sp; sp.check_call(...)` and
/// `from subprocess import check_call` both arrive here as
/// `subprocess.check_call`.
pub const CHECKED_SUBPROCESS_CALLS: &[&str] = &["subprocess.check_call", "subprocess.check_output"];

/// Canonical dotted call-heads that raise on a non-zero child exit status
/// only when the call passes `check=True`.
pub const CHECKABLE_SUBPROCESS_CALLS: &[&str] = &["subprocess.run"];

/// Keyword argument that turns a [`CHECKABLE_SUBPROCESS_CALLS`] entry into
/// verification evidence, and the literal its value must be.
pub const CHECK_KEYWORD: &str = "check";
/// Python's true literal — the only accepted value for [`CHECK_KEYWORD`].
/// A non-literal value (`check=strict`) is not evidence: its truth is not
/// statically known.
pub const TRUE_LITERAL: &str = "True";

/// Canonical dotted call-head of the context manager that discards an
/// exception outright. A checked call inside
/// `with contextlib.suppress(CalledProcessError):` cannot fail the test —
/// the one-line spelling of a `try` / `except` that swallows the error.
pub const SUPPRESS_CONTEXT_MANAGER: &str = "contextlib.suppress";

/// True iff `canonical` names the exception-suppressing context manager.
/// Exact: `with pytest.raises(...)` is an assertion, not a suppression, and
/// must keep counting.
#[inline]
pub fn is_suppress_context_manager(canonical: &str) -> bool {
    canonical == SUPPRESS_CONTEXT_MANAGER
}

/// Method on `subprocess.CompletedProcess` that raises `CalledProcessError`
/// for a non-zero exit status. Matched on the trailing segment of the call
/// head, because the receiver is a local variable the parser cannot type:
/// `completed.check_returncode()`. The name is idiosyncratic enough for the
/// false-positive risk to be negligible.
pub const RETURNCODE_CHECK_METHOD: &str = "check_returncode";

/// The module every recognised helper lives in. A test that replaced this
/// module, or any name under it, has replaced the process boundary itself.
pub const SUBPROCESS_MODULE: &str = "subprocess";

/// True iff `name` is the `subprocess` module or a name under it.
#[inline]
pub fn is_subprocess_name(name: &str) -> bool {
    is_at_or_under(name, SUBPROCESS_MODULE)
}

/// True iff `canonical` names a subprocess helper that always raises on a
/// non-zero child exit status.
#[inline]
pub fn is_checked_subprocess_call(canonical: &str) -> bool {
    CHECKED_SUBPROCESS_CALLS.contains(&canonical)
}

/// True iff `canonical` names a subprocess helper that raises on a non-zero
/// child exit status only when `check=True` is passed.
#[inline]
pub fn is_checkable_subprocess_call(canonical: &str) -> bool {
    CHECKABLE_SUBPROCESS_CALLS.contains(&canonical)
}

/// True iff the dotted call-head `chain` ends in an explicit return-code
/// check on a `CompletedProcess`. Requires a receiver — a bare
/// `check_returncode()` call has none and is not matched.
#[inline]
pub fn is_returncode_check(chain: &str) -> bool {
    chain
        .rsplit_once('.')
        .is_some_and(|(receiver, method)| !receiver.is_empty() && method == RETURNCODE_CHECK_METHOD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_helpers_match_exactly() {
        assert!(is_checked_subprocess_call("subprocess.check_call"));
        assert!(is_checked_subprocess_call("subprocess.check_output"));
        assert!(!is_checked_subprocess_call("subprocess.run"));
        assert!(
            !is_checked_subprocess_call("check_call"),
            "bare names must be canonicalized first"
        );
    }

    #[test]
    fn checkable_helpers_match_exactly() {
        assert!(is_checkable_subprocess_call("subprocess.run"));
        assert!(!is_checkable_subprocess_call("subprocess.Popen"));
        assert!(!is_checkable_subprocess_call("runner.run"));
    }

    #[test]
    fn subprocess_names_are_recognised_under_the_module() {
        assert!(is_subprocess_name("subprocess"));
        assert!(is_subprocess_name("subprocess.run"));
        assert!(!is_subprocess_name("subprocesses"));
        assert!(!is_subprocess_name("myproj.subprocess"));
    }

    #[test]
    fn only_suppress_is_a_suppressing_context_manager() {
        assert!(is_suppress_context_manager("contextlib.suppress"));
        assert!(!is_suppress_context_manager("pytest.raises"));
        assert!(!is_suppress_context_manager("suppress"), "canonicalized first");
    }

    #[test]
    fn returncode_check_needs_a_receiver() {
        assert!(is_returncode_check("completed.check_returncode"));
        assert!(is_returncode_check("self.result.check_returncode"));
        assert!(!is_returncode_check("check_returncode"));
        assert!(!is_returncode_check("completed.returncode"));
    }
}
