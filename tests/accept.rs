//! Integration tests for the accepted-findings baseline.
//!
//! Drives the `pycoati` binary against a staged copy of
//! `tests/fixtures/accept_project/`, whose four tests fire an exactly known
//! set of signals:
//!
//! | test                          | signals                            |
//! |-------------------------------|------------------------------------|
//! | `test_startup_sequence_smoke` | `zero_asserts`, `high_setup_ratio` |
//! | `test_short_smoke`            | `zero_asserts`                     |
//! | `test_mock_only_assertion`    | `mock_only_assertions`             |
//! | `test_checked_child_process`  | none — it verifies in a child      |
//! | `test_clean`                  | none                               |
//!
//! Two fixtures, deliberately paired:
//!
//! - `accept_project` ships **no** `.pycoati-accept.toml`. Tests that need
//!   one write it into a staged tempdir copy, so each can craft the exact
//!   entry it is about. `the_no_baseline_fixture_ships_no_baseline` guards
//!   the "no file" half, since every other test built on it assumes it.
//! - `accept_with_baseline` ships a committed `.pycoati-accept.toml` next to
//!   its `pyproject.toml`, with comments, a multi-signal entry, a pinned
//!   fingerprint, and one flagged test left deliberately unreviewed. That is
//!   the end-to-end case: a real file on disk in a real project layout,
//!   discovered by the binary with no flags.
//!
//! Tests that write a baseline or edit Python source stage their own tempdir
//! copy. `--static-only` runs write nothing, so those may read the source
//! fixture directly; anything invoking pytest must stage (pytest-cov drops
//! `.coverage` into the project root).
//!
//! Staging and the pytest probe live in `tests/common/mod.rs`, shared with
//! `pytest_integration.rs`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;

mod common;
use common::{fixture_path, integration_python, pytest_available, stage_fixture};

const SMOKE: &str = "tests/test_accepted.py::test_startup_sequence_smoke";
const CHILD: &str = "tests/test_accepted.py::test_checked_child_process";
const SHORT: &str = "tests/test_accepted.py::test_short_smoke";
const MOCK_ONLY: &str = "tests/test_accepted.py::test_mock_only_assertion";
const CLEAN: &str = "tests/test_accepted.py::test_clean";

fn write_baseline(root: &Path, toml: &str) {
    std::fs::write(root.join(".pycoati-accept.toml"), toml).expect("write baseline");
}

/// Path of the fixture's only test file inside a staged copy.
fn test_file(root: &Path) -> PathBuf {
    root.join("tests/test_accepted.py")
}

/// Run a static-only scan and parse the JSON inventory.
fn scan(root: &Path, extra: &[&str]) -> Value {
    let mut cmd = Command::cargo_bin("pycoati").expect("binary built");
    cmd.arg(root).arg("--static-only");
    for arg in extra {
        cmd.arg(arg);
    }
    let assert = cmd.assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    serde_json::from_str(&stdout).expect("stdout is valid JSON")
}

fn shortlist(v: &Value) -> Vec<String> {
    v["top_suspicious"]["test_functions"]
        .as_array()
        .expect("shortlist array")
        .iter()
        .map(|n| n.as_str().expect("nodeid string").to_string())
        .collect()
}

fn record<'a>(v: &'a Value, nodeid: &str) -> &'a Value {
    v["test_functions"]
        .as_array()
        .expect("test_functions array")
        .iter()
        .find(|t| t["nodeid"] == nodeid)
        .unwrap_or_else(|| panic!("no record for {nodeid}"))
}

fn fingerprint_of(v: &Value, nodeid: &str) -> String {
    record(v, nodeid)["fingerprint"].as_str().expect("fingerprint string").to_string()
}

fn signals(v: &Value, nodeid: &str) -> Vec<String> {
    record(v, nodeid)["accepted_signals"]
        .as_array()
        .expect("accepted_signals array")
        .iter()
        .map(|s| s.as_str().expect("signal string").to_string())
        .collect()
}

// --- the default, with no baseline -----------------------------------------

#[test]
fn without_a_baseline_every_flagged_test_is_on_the_shortlist() {
    let (_tmp, root) = stage_fixture("accept_project");
    let v = scan(&root, &[]);

    assert_eq!(v["accepted"]["path"], Value::Null);
    assert_eq!(v["accepted"]["findings"], Value::Array(vec![]));
    assert_eq!(v["accepted"]["stale"], Value::Array(vec![]));

    let list = shortlist(&v);
    for nodeid in [SMOKE, SHORT, MOCK_ONLY] {
        assert!(list.contains(&nodeid.to_string()), "{nodeid} should be listed: {list:?}");
    }
}

// --- suppression -----------------------------------------------------------

#[test]
fn an_accepted_test_leaves_the_shortlist_but_keeps_its_record() {
    let (_tmp, root) = stage_fixture("accept_project");
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SHORT}\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract: it must not raise\"\n"
        ),
    );
    let v = scan(&root, &[]);

    assert!(!shortlist(&v).contains(&SHORT.to_string()), "accepted test must leave the shortlist");
    // …but the record is still there, with its evidence.
    assert_eq!(record(&v, SHORT)["assertion_count"], 0);
    assert!(
        record(&v, SHORT)["suspicion_score"].as_f64().expect("score") > 0.0,
        "the score is still computed from raw evidence"
    );
    assert_eq!(signals(&v, SHORT), vec!["zero_asserts".to_string()]);

    let findings = v["accepted"]["findings"].as_array().expect("findings array");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["test"], SHORT);
    assert_eq!(findings[0]["signal"], "zero_asserts");
    assert_eq!(findings[0]["reason"], "smoke contract: it must not raise");
    assert_eq!(findings[0]["reviewed"], Value::Null);
    assert_eq!(findings[0]["fingerprint"], Value::Null);
    assert!(
        v["accepted"]["path"].as_str().expect("path").ends_with(".pycoati-accept.toml"),
        "the baseline that was read must be named"
    );
}

#[test]
fn acceptance_changes_nothing_but_the_shortlist() {
    let (_tmp, root) = stage_fixture("accept_project");
    let raw = scan(&root, &[]);
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SHORT}\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract\"\n"
        ),
    );
    let accepted = scan(&root, &[]);

    // Every file record — counts, smell hits, the lot — is byte-identical.
    assert_eq!(raw["files"], accepted["files"], "file records must not change");

    // Test records differ only in `accepted_signals`.
    let mut stripped = accepted["test_functions"].clone();
    for t in stripped.as_array_mut().expect("array") {
        t["accepted_signals"] = Value::Array(vec![]);
    }
    assert_eq!(raw["test_functions"], stripped, "no test record may lose evidence");

    // And the shortlist is the only thing that moved.
    assert_ne!(shortlist(&raw), shortlist(&accepted));
    assert_eq!(shortlist(&raw).len(), shortlist(&accepted).len() + 1);
}

#[test]
fn accepting_one_signal_of_two_keeps_the_test_actionable() {
    // `test_startup_sequence_smoke` fires both `zero_asserts` and
    // `high_setup_ratio`. Signing off only the first must leave it listed.
    let (_tmp, root) = stage_fixture("accept_project");
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SMOKE}\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract: the startup sequence must not raise\"\n"
        ),
    );
    let v = scan(&root, &[]);

    assert_eq!(signals(&v, SMOKE), vec!["zero_asserts".to_string()]);
    assert!(
        shortlist(&v).contains(&SMOKE.to_string()),
        "an unreviewed high_setup_ratio must keep the test listed"
    );
    assert!(v["accepted"]["stale"].as_array().expect("stale").is_empty());
}

#[test]
fn accepting_both_signals_of_a_test_drops_it() {
    let (_tmp, root) = stage_fixture("accept_project");
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SMOKE}\"\nsignals = [\"zero_asserts\", \"high_setup_ratio\"]\nreason = \"smoke contract: the startup sequence must not raise\"\n"
        ),
    );
    let v = scan(&root, &[]);

    assert_eq!(
        signals(&v, SMOKE),
        vec!["high_setup_ratio".to_string(), "zero_asserts".to_string()],
        "accepted_signals is sorted"
    );
    assert!(!shortlist(&v).contains(&SMOKE.to_string()));
    assert_eq!(v["accepted"]["findings"].as_array().expect("findings").len(), 2);
}

#[test]
fn an_acceptance_never_touches_a_test_it_does_not_name() {
    let (_tmp, root) = stage_fixture("accept_project");
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SHORT}\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract\"\n"
        ),
    );
    let v = scan(&root, &[]);

    assert!(
        shortlist(&v).contains(&MOCK_ONLY.to_string()),
        "an unrelated test keeps its place on the shortlist"
    );
    assert!(signals(&v, MOCK_ONLY).is_empty());
    assert!(signals(&v, CLEAN).is_empty());
}

#[test]
fn a_signal_introduced_after_review_brings_the_test_back() {
    // The reviewed judgement covered `zero_asserts` on a one-line smoke test.
    // Someone then wraps it in three patch decorators, which fires
    // `mock_overuse` — a signal nobody has looked at.
    let (_tmp, root) = stage_fixture("accept_project");
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SHORT}\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract\"\n"
        ),
    );
    assert!(!shortlist(&scan(&root, &[])).contains(&SHORT.to_string()));

    let source = std::fs::read_to_string(test_file(&root)).expect("read fixture");
    let patched = source.replace(
        "def test_short_smoke():",
        "@patch(\"acceptproj.greet\")\n@patch(\"subprocess.run\")\n@patch(\"sys.exit\")\ndef test_short_smoke(_exit, _run, _greet):",
    );
    assert_ne!(source, patched, "the fixture's test signature must have been rewritten");
    let patched =
        patched.replace("from unittest.mock import Mock", "from unittest.mock import Mock, patch");
    std::fs::write(test_file(&root), patched).expect("write patched fixture");

    let v = scan(&root, &[]);

    let categories: Vec<&str> = record(&v, SHORT)["smell_hits"]
        .as_array()
        .expect("smell hits")
        .iter()
        .map(|h| h["category"].as_str().expect("category"))
        .collect();
    assert!(categories.contains(&"mock_overuse"), "the new signal must fire: {categories:?}");
    assert_eq!(
        signals(&v, SHORT),
        vec!["zero_asserts".to_string()],
        "the old acceptance still holds for the signal it named"
    );
    assert!(
        shortlist(&v).contains(&SHORT.to_string()),
        "the unreviewed signal puts the test back on the shortlist"
    );
}

// --- the reporting switches ------------------------------------------------

#[test]
fn include_accepted_puts_the_finding_back_on_the_shortlist() {
    let (_tmp, root) = stage_fixture("accept_project");
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SHORT}\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract\"\n"
        ),
    );

    let v = scan(&root, &["--include-accepted"]);

    assert!(shortlist(&v).contains(&SHORT.to_string()));
    assert_eq!(v["accepted"]["included_in_shortlist"], Value::Bool(true));
    assert_eq!(
        v["accepted"]["findings"].as_array().expect("findings").len(),
        1,
        "the acceptance is still reported, it is just not held back"
    );
}

#[test]
fn no_accept_ignores_the_baseline_entirely() {
    let (_tmp, root) = stage_fixture("accept_project");
    let raw = scan(&root, &[]);
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SHORT}\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract\"\n"
        ),
    );

    let v = scan(&root, &["--no-accept"]);

    assert_eq!(v["accepted"]["path"], Value::Null, "no file is read at all");
    assert_eq!(v["accepted"]["findings"], Value::Array(vec![]));
    assert_eq!(shortlist(&v), shortlist(&raw));
    assert!(signals(&v, SHORT).is_empty());
}

#[test]
fn an_explicit_accept_file_is_read_from_outside_the_project() {
    let (_tmp, root) = stage_fixture("accept_project");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let path = elsewhere.path().join("reviewed.toml");
    std::fs::write(
        &path,
        format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SHORT}\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract\"\n"
        ),
    )
    .expect("write baseline");

    let v = scan(&root, &["--accept-file", path.to_str().expect("utf-8 path")]);

    assert!(!shortlist(&v).contains(&SHORT.to_string()));
    assert_eq!(v["accepted"]["path"], Value::String(path.display().to_string()));
}

// --- invalidation ----------------------------------------------------------

#[test]
fn an_entry_for_a_test_that_is_gone_is_reported_stale() {
    let (_tmp, root) = stage_fixture("accept_project");
    write_baseline(
        &root,
        "schema_version = \"1\"\n\n[[accept]]\ntest = \"tests/test_accepted.py::test_renamed_away\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract\"\n",
    );
    // Captured from the same run: the guide promises a stderr warning, so a
    // reviewer notices a dead entry without opening the JSON.
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&root)
        .arg("--static-only")
        .assert()
        .success();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    assert!(stderr.contains("stale acceptance"), "stderr: {stderr}");
    assert!(stderr.contains("test_renamed_away"), "stderr: {stderr}");
    assert!(stderr.contains("unknown_test"), "stderr: {stderr}");

    let v: Value = serde_json::from_str(
        &String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout"),
    )
    .expect("stdout is valid JSON");
    let stale = v["accepted"]["stale"].as_array().expect("stale array");
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0]["status"], "unknown_test");
    assert_eq!(stale[0]["test"], "tests/test_accepted.py::test_renamed_away");
    assert_eq!(stale[0]["reason"], "smoke contract");
    assert!(v["accepted"]["findings"].as_array().expect("findings").is_empty());
}

#[test]
fn a_test_that_verifies_in_a_child_process_is_never_offered_zero_asserts() {
    // The score stopped charging the zero-assert weight for a checked
    // subprocess (issue #17), so the signal set must agree: there is nothing
    // here to review, and an entry accepting it is stale rather than
    // suppressing.
    let (_tmp, root) = stage_fixture("accept_project");
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{CHILD}\"\nsignal = \"zero_asserts\"\nreason = \"child interpreter\"\n"
        ),
    );
    let v = scan(&root, &[]);

    assert!(signals(&v, CHILD).is_empty(), "nothing was suppressed");
    assert_eq!(record(&v, CHILD)["assertion_count"], 0);
    assert_eq!(record(&v, CHILD)["external_verification_count"], 1);
    let stale = v["accepted"]["stale"].as_array().expect("stale array");
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0]["status"], "signal_not_active");
    assert_eq!(stale[0]["test"], CHILD);
}

#[test]
fn an_entry_whose_signal_no_longer_fires_is_reported_stale() {
    // `test_clean` asserts on a real value, so `zero_asserts` cannot apply.
    let (_tmp, root) = stage_fixture("accept_project");
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{CLEAN}\"\nsignal = \"zero_asserts\"\nreason = \"was assertionless once\"\n"
        ),
    );
    let v = scan(&root, &[]);

    let stale = v["accepted"]["stale"].as_array().expect("stale array");
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0]["status"], "signal_not_active");
    assert!(signals(&v, CLEAN).is_empty());
}

#[test]
fn a_pinned_fingerprint_that_still_matches_keeps_the_acceptance() {
    let (_tmp, root) = stage_fixture("accept_project");
    let fp = fingerprint_of(&scan(&root, &[]), SHORT);
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SHORT}\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract\"\nreviewed = \"2026-09-12\"\nfingerprint = \"{fp}\"\n"
        ),
    );
    let v = scan(&root, &[]);

    assert!(!shortlist(&v).contains(&SHORT.to_string()));
    let findings = v["accepted"]["findings"].as_array().expect("findings");
    assert_eq!(findings[0]["fingerprint"], Value::String(fp));
    assert_eq!(findings[0]["reviewed"], "2026-09-12");
    assert!(v["accepted"]["stale"].as_array().expect("stale").is_empty());
}

#[test]
fn editing_an_accepted_test_lapses_its_pinned_acceptance() {
    let (_tmp, root) = stage_fixture("accept_project");
    let first = scan(&root, &[]);
    let fp = fingerprint_of(&first, SHORT);
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SHORT}\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract\"\nfingerprint = \"{fp}\"\n"
        ),
    );
    assert!(!shortlist(&scan(&root, &[])).contains(&SHORT.to_string()));

    // Edit the body of the accepted test. Nothing else changes.
    let source = std::fs::read_to_string(test_file(&root)).expect("read fixture");
    let edited = source.replace(
        "    acceptproj.greet(\"world\")",
        "    acceptproj.greet(\"somebody else entirely\")",
    );
    assert_ne!(source, edited, "the fixture's smoke body must have been rewritten");
    std::fs::write(test_file(&root), edited).expect("write edited fixture");

    let v = scan(&root, &[]);

    let stale = v["accepted"]["stale"].as_array().expect("stale array");
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0]["status"], "content_changed");
    let new_fp = fingerprint_of(&v, SHORT);
    assert_ne!(new_fp, fp, "the edit must move the fingerprint");
    assert!(
        stale[0]["detail"].as_str().expect("detail").contains(&new_fp),
        "the detail must carry the new fingerprint so it can be copied in: {:?}",
        stale[0]["detail"]
    );
    assert!(
        shortlist(&v).contains(&SHORT.to_string()),
        "a lapsed acceptance makes the finding actionable again"
    );
    assert!(signals(&v, SHORT).is_empty());
}

#[test]
fn editing_an_accepted_test_keeps_an_unpinned_acceptance() {
    // The counterpart to the test above: without a recorded fingerprint the
    // acceptance is not content-sensitive, which is the documented default.
    let (_tmp, root) = stage_fixture("accept_project");
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SHORT}\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract\"\n"
        ),
    );
    let source = std::fs::read_to_string(test_file(&root)).expect("read fixture");
    std::fs::write(
        test_file(&root),
        source.replace(
            "    acceptproj.greet(\"world\")",
            "    acceptproj.greet(\"somebody else entirely\")",
        ),
    )
    .expect("write edited fixture");

    let v = scan(&root, &[]);

    assert!(v["accepted"]["stale"].as_array().expect("stale").is_empty());
    assert!(!shortlist(&v).contains(&SHORT.to_string()));
}

// --- validation ------------------------------------------------------------

/// Write a baseline, scan, and return the failure's stderr.
fn scan_failure(root: &Path, extra: &[&str]) -> String {
    let mut cmd = Command::cargo_bin("pycoati").expect("binary built");
    cmd.arg(root).arg("--static-only");
    for arg in extra {
        cmd.arg(arg);
    }
    let assert = cmd.assert().failure();
    String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr")
}

#[test]
fn an_entry_without_a_reason_fails_the_scan() {
    let (_tmp, root) = stage_fixture("accept_project");
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SHORT}\"\nsignal = \"zero_asserts\"\n"
        ),
    );
    let stderr = scan_failure(&root, &[]);
    assert!(stderr.contains("missing `reason`"), "stderr: {stderr}");
}

#[test]
fn an_unknown_signal_fails_the_scan_and_lists_the_valid_ones() {
    let (_tmp, root) = stage_fixture("accept_project");
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SHORT}\"\nsignal = \"too_slow\"\nreason = \"r\"\n"
        ),
    );
    let stderr = scan_failure(&root, &[]);
    assert!(stderr.contains("unknown signal"), "stderr: {stderr}");
    assert!(stderr.contains("high_setup_ratio"), "stderr: {stderr}");
}

#[test]
fn an_explicit_accept_file_that_does_not_exist_fails_the_scan() {
    let (_tmp, root) = stage_fixture("accept_project");
    let stderr = scan_failure(&root, &["--accept-file", "no/such/file.toml"]);
    assert!(stderr.contains("accept file not found"), "stderr: {stderr}");
}

#[test]
fn accept_file_and_no_accept_cannot_be_combined() {
    let (_tmp, root) = stage_fixture("accept_project");
    let stderr = scan_failure(&root, &["--no-accept", "--accept-file", "whatever.toml"]);
    assert!(stderr.contains("cannot be used with"), "stderr: {stderr}");
}

#[test]
fn accept_file_is_rejected_against_a_workspace_root() {
    let ws = fixture_path("tests/fixtures/uv_workspace");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&ws)
        .args(["--static-only", "--accept-file", "whatever.toml"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf-8 stderr");
    assert!(stderr.contains("--accept-file is incompatible"), "stderr: {stderr}");
}

// --- the committed baseline ------------------------------------------------

const REVIEWED_SMOKE: &str = "tests/test_reviewed.py::test_cli_entry_point_installs";
const REVIEWED_IMPORT: &str = "tests/test_reviewed.py::test_import_does_not_raise";
const REVIEWED_MOCK: &str = "tests/test_reviewed.py::test_retry_reports_through_the_mock";
const REVIEWED_CLEAN: &str = "tests/test_reviewed.py::test_normalize_strips_and_lowercases";

/// Scan the committed-baseline fixture in place. Safe without staging: a
/// `--static-only` run reads and writes nothing in the project.
fn scan_committed() -> Value {
    scan(&fixture_path("tests/fixtures/accept_with_baseline"), &[])
}

#[test]
fn a_committed_baseline_is_found_and_applied_with_no_flags() {
    let v = scan_committed();

    assert!(
        v["accepted"]["path"].as_str().expect("path").ends_with(".pycoati-accept.toml"),
        "the committed baseline must be discovered without --accept-file: {:?}",
        v["accepted"]["path"]
    );

    // The multi-signal entry and the single-signal one, reasons verbatim.
    assert_eq!(
        signals(&v, REVIEWED_SMOKE),
        vec!["high_setup_ratio".to_string(), "zero_asserts".to_string()]
    );
    assert_eq!(signals(&v, REVIEWED_IMPORT), vec!["zero_asserts".to_string()]);

    let findings = v["accepted"]["findings"].as_array().expect("findings array");
    assert_eq!(findings.len(), 3, "two signals on one test plus one on another");
    let reason_for = |nodeid: &str, signal: &str| -> String {
        findings
            .iter()
            .find(|f| f["test"] == nodeid && f["signal"] == signal)
            .unwrap_or_else(|| panic!("no finding for {nodeid} / {signal}"))["reason"]
            .as_str()
            .expect("reason string")
            .to_string()
    };
    assert_eq!(
        reason_for(REVIEWED_SMOKE, "zero_asserts"),
        "assertions run in a child interpreter; the parent propagates failure via subprocess.run(check=True)"
    );
    assert_eq!(
        reason_for(REVIEWED_IMPORT, "zero_asserts"),
        "smoke contract: importing and calling must not raise"
    );
    // Provenance survives the round trip through the file.
    let smoke = findings
        .iter()
        .find(|f| f["test"] == REVIEWED_SMOKE && f["signal"] == "zero_asserts")
        .expect("smoke finding");
    assert_eq!(smoke["reviewed"], "2026-09-12 / packaging review");
    assert_eq!(smoke["fingerprint"], "f4e02e3172e2c958");

    let list = shortlist(&v);
    assert!(!list.contains(&REVIEWED_SMOKE.to_string()), "{list:?}");
    assert!(!list.contains(&REVIEWED_IMPORT.to_string()), "{list:?}");
}

#[test]
fn the_committed_baseline_leaves_the_unreviewed_finding_on_the_shortlist() {
    // Nothing in the committed file accepts this test's
    // `mock_only_assertions` hit. A project having a baseline at all must not
    // make its unreviewed findings quieter.
    let v = scan_committed();

    assert!(signals(&v, REVIEWED_MOCK).is_empty());
    assert_eq!(
        shortlist(&v),
        vec![REVIEWED_MOCK.to_string(), REVIEWED_CLEAN.to_string()],
        "only the two tests nobody signed off on remain, in score order"
    );
}

#[test]
fn the_committed_baseline_has_no_stale_entries() {
    // A health check on the checked-in file itself: rename a fixture test
    // without touching the baseline and this fails, which is the same signal
    // a real project gets on its own baseline.
    let v = scan_committed();
    let stale = v["accepted"]["stale"].as_array().expect("stale array");
    assert!(
        stale.is_empty(),
        "tests/fixtures/accept_with_baseline/.pycoati-accept.toml has entries that no longer \
         apply: {stale:#?}"
    );
}

#[test]
fn the_committed_fingerprint_still_matches_its_test() {
    // The committed baseline pins a fingerprint, so the TOML and the Python
    // beside it have to stay in sync. This asserts the documented workflow —
    // copy `test_functions[].fingerprint` into the entry — actually round
    // trips, and that the hash is stable across runs and platforms.
    let v = scan_committed();
    let current = record(&v, REVIEWED_SMOKE)["fingerprint"].as_str().expect("fingerprint");
    assert_eq!(
        current, "f4e02e3172e2c958",
        "the fingerprint of {REVIEWED_SMOKE} changed. If you edited that test on purpose, \
         put {current} in the `fingerprint` key of \
         tests/fixtures/accept_with_baseline/.pycoati-accept.toml and in this assertion."
    );
}

#[test]
fn the_no_baseline_fixture_ships_no_baseline() {
    // Every other test in this file writes its own baseline into a staged
    // copy of `accept_project` and assumes the fixture starts clean.
    // Committing a `.pycoati-accept.toml` there would silently change what
    // all of them mean.
    let stray = fixture_path("tests/fixtures/accept_project/.pycoati-accept.toml");
    assert!(
        !stray.exists(),
        "{} must not exist — accept_project is the no-baseline fixture; \
         accept_with_baseline is the one that ships a file",
        stray.display()
    );
    // And the binary agrees, unprompted.
    let v = scan(&fixture_path("tests/fixtures/accept_project"), &[]);
    assert_eq!(v["accepted"]["path"], Value::Null);
    assert_eq!(v["accepted"]["findings"], Value::Array(vec![]));
    assert_eq!(v["accepted"]["stale"], Value::Array(vec![]));
    assert!(v["test_functions"]
        .as_array()
        .expect("tests")
        .iter()
        .all(|t| { t["accepted_signals"].as_array().expect("accepted_signals array").is_empty() }));
}

// --- single-file mode ------------------------------------------------------

/// Single-file scans have no project root of their own, so the baseline is
/// the nearest one at or above the file's directory. This is what makes the
/// documented "put it at the project root" placement work for
/// `pycoati tests/test_x.py`.
#[test]
fn a_single_file_scan_reads_the_baseline_from_the_project_root() {
    let (_tmp, root) = stage_fixture("accept_project");
    // In single-file mode the nodeid is the path exactly as typed, so the
    // entry is written against the path this scan passes.
    let file = root.join("tests/test_accepted.py");
    let typed = file.to_str().expect("utf-8 path").to_string();
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{typed}::test_short_smoke\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract\"\n"
        ),
    );

    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&file)
        .arg("--static-only")
        .assert()
        .success();
    let v: Value = serde_json::from_str(
        &String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout"),
    )
    .expect("stdout is valid JSON");

    assert!(
        v["accepted"]["path"].as_str().expect("path").ends_with(".pycoati-accept.toml"),
        "the project-root baseline must be found from a file one directory down: {:?}",
        v["accepted"]["path"]
    );
    let nodeid = format!("{typed}::test_short_smoke");
    assert_eq!(v["accepted"]["findings"].as_array().expect("findings").len(), 1);
    assert!(!shortlist(&v).contains(&nodeid), "the accepted test must leave the shortlist");
}

// --- pretty output ---------------------------------------------------------

#[test]
fn pretty_output_names_every_accepted_finding_and_its_reason() {
    let (_tmp, root) = stage_fixture("accept_project");
    write_baseline(
        &root,
        &format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SHORT}\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract: it must not raise\"\n\n[[accept]]\ntest = \"tests/test_accepted.py::test_gone\"\nsignal = \"mock_overuse\"\nreason = \"boundary stubs only\"\n"
        ),
    );
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&root)
        .args(["--static-only", "--format", "pretty"])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");

    assert!(out.contains("Accepted findings (held back from the shortlist)"), "{out}");
    assert!(out.contains("smoke contract: it must not raise"), "{out}");
    assert!(out.contains("Stale acceptances (actionable again)"), "{out}");
    assert!(out.contains("unknown_test"), "{out}");
}

#[test]
fn pretty_output_omits_the_sections_when_nothing_was_accepted() {
    let (_tmp, root) = stage_fixture("accept_project");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .arg(&root)
        .args(["--static-only", "--format", "pretty"])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");

    assert!(!out.contains("Accepted findings"), "{out}");
    assert!(!out.contains("Stale acceptances"), "{out}");
}

// --- runtime metrics -------------------------------------------------------

/// The committed baseline, end to end through a real pytest run: the two
/// accepted tests are held back from the shortlist and still collected, run
/// and counted. Compares against `--no-accept` on the same staged copy, so
/// the only variable is whether the file was read.
#[test]
fn a_committed_baseline_does_not_change_what_pytest_runs() {
    let python = integration_python();
    if !pytest_available(&python) {
        eprintln!("SKIPPED: pytest not available via `{python}`");
        return;
    }

    let (_tmp, root) = stage_fixture("accept_with_baseline");
    assert!(
        root.join(".pycoati-accept.toml").is_file(),
        "staging must carry the committed baseline across"
    );

    let run = |extra: &[&str]| -> Value {
        let mut cmd = Command::cargo_bin("pycoati").expect("binary built");
        cmd.arg(&root).args(["--python", &python]);
        for arg in extra {
            cmd.arg(arg);
        }
        let assert = cmd.assert().success();
        serde_json::from_str(
            &String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout"),
        )
        .expect("stdout is valid JSON")
    };

    let with_baseline = run(&[]);
    let without = run(&["--no-accept"]);

    assert_eq!(with_baseline["tool"]["ran_pytest"], Value::Bool(true));
    assert_eq!(with_baseline["accepted"]["findings"].as_array().expect("findings").len(), 3);
    assert_eq!(without["accepted"]["path"], Value::Null);

    // The shortlist is the only thing the baseline moved…
    assert!(!shortlist(&with_baseline).contains(&REVIEWED_SMOKE.to_string()));
    assert!(shortlist(&without).contains(&REVIEWED_SMOKE.to_string()));

    // …and pytest saw exactly the same suite either way.
    assert_eq!(with_baseline["suite"]["test_count"], without["suite"]["test_count"]);
    assert_eq!(with_baseline["suite"]["test_count"].as_u64(), Some(4));
    assert_eq!(with_baseline["suite"]["line_coverage_pct"], without["suite"]["line_coverage_pct"]);
    assert!(with_baseline["suite"]["line_coverage_pct"].as_f64().expect("coverage") > 0.0);
    assert_eq!(with_baseline["files"], without["files"]);
}

/// The load-bearing guarantee: acceptance is an analysis decision, so the
/// test still runs and still counts. Self-skips when pytest is unavailable,
/// matching `pytest_integration.rs`.
#[test]
fn accepted_tests_are_still_collected_run_and_covered() {
    let python = integration_python();
    if !pytest_available(&python) {
        eprintln!("SKIPPED: pytest not available via `{python}`");
        return;
    }

    let run = |root: &Path, baseline: Option<&str>| -> Value {
        if let Some(toml) = baseline {
            write_baseline(root, toml);
        }
        let assert = Command::cargo_bin("pycoati")
            .expect("binary built")
            .arg(root)
            .args(["--python", &python])
            .assert()
            .success();
        let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
        serde_json::from_str(&stdout).expect("stdout is valid JSON")
    };

    let (_raw_tmp, raw_root) = stage_fixture("accept_project");
    let raw = run(&raw_root, None);

    let (_acc_tmp, acc_root) = stage_fixture("accept_project");
    let accepted = run(
        &acc_root,
        Some(&format!(
            "schema_version = \"1\"\n\n[[accept]]\ntest = \"{SHORT}\"\nsignal = \"zero_asserts\"\nreason = \"smoke contract\"\n\n[[accept]]\ntest = \"{SMOKE}\"\nsignals = [\"zero_asserts\", \"high_setup_ratio\"]\nreason = \"smoke contract: the startup sequence must not raise\"\n"
        )),
    );

    // Preconditions: pytest really ran, and the acceptances really applied.
    assert_eq!(raw["tool"]["ran_pytest"], Value::Bool(true));
    assert_eq!(accepted["tool"]["ran_pytest"], Value::Bool(true));
    assert_eq!(accepted["accepted"]["findings"].as_array().expect("findings").len(), 3);
    for nodeid in [SHORT, SMOKE] {
        assert!(!shortlist(&accepted).contains(&nodeid.to_string()));
    }

    // The accepted tests were still collected and still ran…
    assert_eq!(
        raw["suite"]["test_count"], accepted["suite"]["test_count"],
        "acceptance must not change how many tests pytest collects"
    );
    let raw_count = raw["suite"]["test_count"].as_u64().expect("test_count");
    assert_eq!(raw_count, 4, "all four fixture tests must be collected");
    assert!(raw["suite"]["runtime_seconds"].as_f64().expect("runtime") > 0.0);
    assert!(accepted["suite"]["runtime_seconds"].as_f64().expect("runtime") > 0.0);

    // …and still contributed to coverage.
    assert_eq!(
        raw["suite"]["line_coverage_pct"], accepted["suite"]["line_coverage_pct"],
        "acceptance must not change coverage"
    );
    assert!(accepted["suite"]["line_coverage_pct"].as_f64().expect("coverage") > 0.0);

    // And the per-file AST counts are untouched too.
    assert_eq!(raw["files"], accepted["files"]);
}
