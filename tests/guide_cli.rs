//! CLI-level tests for `pycoati guide`: explicit page selection, the
//! filesystem-state dispatch for the bare form, and the scan footer.
//!
//! Dispatch fixtures are temp dirs so the tests never depend on whether the
//! repo itself happens to have an `inventory.json` lying around.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn fixture_path(rel: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(rel);
    p
}

/// Run `pycoati guide [page]` with `cwd` set to `dir`, returning stdout.
fn guide_in(dir: &std::path::Path, page: Option<&str>) -> String {
    let mut cmd = Command::cargo_bin("pycoati").expect("binary built");
    cmd.current_dir(dir).arg("guide");
    if let Some(p) = page {
        cmd.arg(p);
    }
    let assert = cmd.assert().success();
    String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be valid UTF-8")
}

/// The three page bodies, as embedded in the binary.
fn setup_page() -> &'static str {
    pycoati::guide::SETUP
}
fn analyze_page() -> &'static str {
    pycoati::guide::ANALYZE
}
fn remediate_page() -> &'static str {
    pycoati::guide::REMEDIATE
}

// --- explicit page selection ------------------------------------------------

#[test]
fn explicit_pages_print_verbatim_and_exit_zero() {
    let dir = TempDir::new().expect("tempdir");
    for (arg, expected) in
        [("setup", setup_page()), ("analyze", analyze_page()), ("remediate", remediate_page())]
    {
        let out = guide_in(dir.path(), Some(arg));
        assert_eq!(out, expected, "`pycoati guide {arg}` must print the page verbatim");
    }
}

#[test]
fn explicit_page_ignores_filesystem_state() {
    // An inventory is present, but an explicit `setup` still wins.
    let dir = TempDir::new().expect("tempdir");
    fs::write(dir.path().join("inventory.json"), r#"{"schema_version":"2"}"#).expect("write");
    assert_eq!(guide_in(dir.path(), Some("setup")), setup_page());
}

#[test]
fn unknown_page_is_rejected() {
    let dir = TempDir::new().expect("tempdir");
    Command::cargo_bin("pycoati")
        .expect("binary built")
        .current_dir(dir.path())
        .args(["guide", "nonsense"])
        .assert()
        .failure();
}

// --- bare-form dispatch -----------------------------------------------------

#[test]
fn bare_guide_with_no_inventory_selects_setup() {
    let dir = TempDir::new().expect("tempdir");
    assert_eq!(guide_in(dir.path(), None), setup_page());
}

#[test]
fn bare_guide_with_project_but_no_inventory_selects_setup() {
    let dir = TempDir::new().expect("tempdir");
    fs::write(dir.path().join("pyproject.toml"), "[project]\nname = \"demo\"\n").expect("write");
    fs::create_dir(dir.path().join("tests")).expect("mkdir");
    assert_eq!(guide_in(dir.path(), None), setup_page());
}

#[test]
fn bare_guide_with_inventory_selects_analyze() {
    let dir = TempDir::new().expect("tempdir");
    fs::write(dir.path().join("inventory.json"), r#"{"schema_version":"2","files":[]}"#)
        .expect("write");
    assert_eq!(guide_in(dir.path(), None), analyze_page());
}

#[test]
fn bare_guide_ignores_unparseable_inventory() {
    let dir = TempDir::new().expect("tempdir");
    fs::write(dir.path().join("inventory.json"), "not json {{{").expect("write");
    assert_eq!(
        guide_in(dir.path(), None),
        setup_page(),
        "a corrupt inventory must degrade to setup, not error"
    );
}

#[test]
fn bare_guide_ignores_foreign_json() {
    let dir = TempDir::new().expect("tempdir");
    fs::write(dir.path().join("inventory.json"), r#"{"totally":"unrelated"}"#).expect("write");
    assert_eq!(guide_in(dir.path(), None), setup_page());
}

#[test]
fn bare_guide_accepts_a_workspace_inventory() {
    let dir = TempDir::new().expect("tempdir");
    fs::write(
        dir.path().join("inventory.json"),
        r#"{"schema_version":"2","workspace_root":".","members":[]}"#,
    )
    .expect("write");
    assert_eq!(guide_in(dir.path(), None), analyze_page());
}

#[test]
fn remediate_is_never_auto_selected() {
    // Whatever the filesystem says, the bare form only ever yields setup or
    // analyze — `remediate` is reachable by breadcrumb or explicit arg only.
    let dir = TempDir::new().expect("tempdir");
    assert_ne!(guide_in(dir.path(), None), remediate_page());
    fs::write(dir.path().join("inventory.json"), r#"{"schema_version":"2"}"#).expect("write");
    assert_ne!(guide_in(dir.path(), None), remediate_page());
}

// --- page invariants --------------------------------------------------------

#[test]
fn every_page_ends_with_a_single_next_line() {
    for (name, page) in
        [("setup", setup_page()), ("analyze", analyze_page()), ("remediate", remediate_page())]
    {
        let trimmed = page.trim_end();
        let lines: Vec<&str> = trimmed.lines().collect();
        let breadcrumbs: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.starts_with("next:"))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(breadcrumbs.len(), 1, "{name} must have exactly one `next:` line");

        // The breadcrumb must be the last thing on the page. A `next:` line may
        // wrap (remediate.md's does), so trailing continuations are allowed --
        // but a blank line or a new paragraph after it is not.
        let start = breadcrumbs[0];
        for (offset, line) in lines[start + 1..].iter().enumerate() {
            assert!(
                !line.trim().is_empty(),
                "{name}: line {} after the `next:` breadcrumb is blank; the breadcrumb \
                 must be the last thing on the page",
                start + offset + 2
            );
        }
    }
}

/// Braindump todo 133: readers wired pycoati into the commit hook next to
/// zorilla. The setup page has to say which of the two is the gate.
#[test]
fn setup_page_says_it_is_periodic_and_names_the_per_commit_gate() {
    let text = setup_page();
    let fits = text.find("## Where this fits").expect("setup should have the section");
    let preconditions = text.find("## Preconditions").expect("setup keeps its preconditions");
    assert!(fits < preconditions, "the placement note comes before the mechanics");
    assert!(text.contains("not a commit hook"), "setup should rule out the hook");
    assert!(text.contains("uvx zorilla guide"), "setup should point at zorilla's guide");
    assert!(text.contains("madoqua"), "setup should name the hook runner it is not for");
}

#[test]
fn analyze_page_names_every_anti_pattern() {
    // Match the heading, not a passing mention: the sweep list in the taxonomy
    // preamble names several patterns in prose, so `contains("redundant")` would
    // survive deleting the whole `## 5. redundant` section.
    let page = analyze_page();
    for heading in [
        "## 1. mock-as-assertion",
        "## 2. implementation coupling",
        "## 3. tautology",
        "## 4. setup-heavy",
        "## 5. redundant",
        "## 6. slow-without-reason",
        "## 7. wrong-layer",
        "## 8. dead-test",
    ] {
        assert!(page.contains(heading), "analyze page is missing the `{heading}` section");
    }
}

// --- scan footer ------------------------------------------------------------

#[test]
fn pretty_scan_with_candidates_ends_with_the_guide_footer() {
    let project = fixture_path("tests/fixtures/project");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .args([project.to_str().expect("utf-8 path"), "--static-only", "--format", "pretty"])
        .assert()
        .success();
    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be UTF-8");
    assert!(
        stdout.trim_end().ends_with("next: run `pycoati guide analyze`"),
        "pretty output must end with the guide footer, got tail: {:?}",
        &stdout[stdout.len().saturating_sub(120)..]
    );
}

#[test]
fn json_scan_never_emits_the_footer() {
    let project = fixture_path("tests/fixtures/project");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .args([project.to_str().expect("utf-8 path"), "--static-only"])
        .assert()
        .success();
    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be UTF-8");
    // The strongest form of "uncontaminated": the whole stream is one JSON doc.
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("json stdout must parse in its entirety");
    assert_eq!(parsed["schema_version"], "2");
    assert!(!stdout.contains("pycoati guide"), "json output must not mention the guide");
}

#[test]
fn json_output_file_is_free_of_the_footer() {
    let dir = TempDir::new().expect("tempdir");
    let out = dir.path().join("inventory.json");
    let project = fixture_path("tests/fixtures/project");
    Command::cargo_bin("pycoati")
        .expect("binary built")
        .args([project.to_str().expect("utf-8 path"), "--static-only", "--output"])
        .arg(&out)
        .assert()
        .success();
    let written = fs::read_to_string(&out).expect("read output file");
    let _: serde_json::Value = serde_json::from_str(&written).expect("file must be valid JSON");
    assert!(!written.contains("guide"));
}

#[test]
fn pretty_scan_without_candidates_has_no_footer() {
    // `--top-suspicious 0` empties the candidate lists, so there is nothing to
    // point the agent at and the footer must stay off.
    let project = fixture_path("tests/fixtures/project");
    let assert = Command::cargo_bin("pycoati")
        .expect("binary built")
        .args([
            project.to_str().expect("utf-8 path"),
            "--static-only",
            "--format",
            "pretty",
            "--top-suspicious",
            "0",
        ])
        .assert()
        .success();
    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be UTF-8");
    assert!(!stdout.contains("pycoati guide analyze"));
}

// --- back-compat ------------------------------------------------------------

#[test]
fn adding_the_subcommand_did_not_break_the_positional_form() {
    let fixture = fixture_path("tests/fixtures/simple/test_basic.py");
    let assert =
        Command::cargo_bin("pycoati").expect("binary built").arg(&fixture).assert().success();
    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be UTF-8");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    assert_eq!(v["schema_version"], "2");
}

#[test]
fn no_arguments_at_all_fails_with_a_usage_hint() {
    let dir = TempDir::new().expect("tempdir");
    Command::cargo_bin("pycoati")
        .expect("binary built")
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("a PATH is required"))
        .stderr(predicate::str::contains("pycoati guide"));
}

#[test]
fn every_breadcrumb_points_at_a_real_page() {
    // A typo in a `pycoati guide <page>` reference ships silently and dead-ends
    // the agent mid-workflow, so pin every target named across all three pages.
    let mut found = 0;
    for (name, page) in
        [("setup", setup_page()), ("analyze", analyze_page()), ("remediate", remediate_page())]
    {
        for (index, _) in page.match_indices("pycoati guide ") {
            let rest = &page[index + "pycoati guide ".len()..];
            let target: String = rest.chars().take_while(char::is_ascii_alphabetic).collect();
            if target.is_empty() {
                continue; // a bare `pycoati guide` mention, which is valid
            }
            assert!(
                ["setup", "analyze", "remediate"].contains(&target.as_str()),
                "{name} references a page that does not exist: `pycoati guide {target}`"
            );
            found += 1;
        }
    }
    assert!(found >= 3, "expected the pages to cross-reference each other, found {found}");
}
