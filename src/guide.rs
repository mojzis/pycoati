//! The embedded workflow guide.
//!
//! pycoati ships its own instructions so they cannot drift from the schema the
//! binary emits: the pages are plain markdown under `docs/guide/`, pulled in
//! with [`include_str!`] at compile time. The same bytes serve the CLI and any
//! docs site that later renders the files from the repo.
//!
//! Three pages, each ending in a single `next:` breadcrumb so an agent is
//! always one command away from the right instructions:
//!
//! - [`SETUP`] — configure and run the scan.
//! - [`ANALYZE`] — the inventory field reference and the anti-pattern taxonomy.
//! - [`REMEDIATE`] — the remedy ladder and the human-approval gate.
//!
//! `tests/guide_schema_drift.rs` asserts every serialized inventory field is
//! documented on the `analyze` page; that test is what keeps this honest.

use std::path::Path;

/// Setup page: configuration surface, scan invocation, output location.
pub const SETUP: &str = include_str!("../docs/guide/setup.md");

/// Analyze page: `inventory.json` field reference plus the anti-pattern
/// taxonomy and its detection heuristics.
pub const ANALYZE: &str = include_str!("../docs/guide/analyze.md");

/// Remediate page: the remedy ladder, the approval gate, and the "do not" list.
pub const REMEDIATE: &str = include_str!("../docs/guide/remediate.md");

/// Inventory filename the guide tells the agent to write, and the one
/// [`detect_page`] looks for. pycoati itself has no default output path —
/// this is the convention the `setup` page establishes.
pub const INVENTORY_FILENAME: &str = "inventory.json";

/// One page of the guide.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Page {
    Setup,
    Analyze,
    Remediate,
}

impl Page {
    /// The page's markdown, verbatim.
    pub const fn text(self) -> &'static str {
        match self {
            Self::Setup => SETUP,
            Self::Analyze => ANALYZE,
            Self::Remediate => REMEDIATE,
        }
    }
}

/// Pick the page to show for a bare `pycoati guide`, from the state of `dir`.
///
/// - A readable `inventory.json` that parses as a pycoati payload →
///   [`Page::Analyze`].
/// - Anything else — no file, unreadable file, invalid JSON, or JSON that
///   isn't ours → [`Page::Setup`].
///
/// [`Page::Remediate`] is never auto-selected. Deciding from the filesystem
/// that analysis has finished would be guesswork, so the page is reachable
/// only via the `analyze` breadcrumb or an explicit argument.
///
/// Detection never fails: every error path degrades to [`Page::Setup`], which
/// is the page that explains how to produce the missing inventory.
pub fn detect_page(dir: &Path) -> Page {
    if is_inventory(&dir.join(INVENTORY_FILENAME)) {
        Page::Analyze
    } else {
        Page::Setup
    }
}

/// True when `path` holds a JSON object carrying a `schema_version` key.
///
/// Deliberately loose: the key's *value* is not checked, so an inventory from
/// a future schema still routes to `analyze` — where the page's own
/// `schema_version` note tells the agent what to do about the mismatch.
fn is_inventory(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value.get("schema_version").is_some()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn dir_with(name: &str, contents: &str) -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        fs::write(dir.path().join(name), contents).expect("write fixture");
        dir
    }

    #[test]
    fn empty_dir_selects_setup() {
        let dir = TempDir::new().expect("tempdir");
        assert_eq!(detect_page(dir.path()), Page::Setup);
    }

    #[test]
    fn inventory_selects_analyze() {
        let dir = dir_with(INVENTORY_FILENAME, r#"{"schema_version":"2","files":[]}"#);
        assert_eq!(detect_page(dir.path()), Page::Analyze);
    }

    #[test]
    fn workspace_inventory_selects_analyze() {
        let dir = dir_with(
            INVENTORY_FILENAME,
            r#"{"schema_version":"2","workspace_root":".","members":[]}"#,
        );
        assert_eq!(detect_page(dir.path()), Page::Analyze);
    }

    #[test]
    fn future_schema_version_still_selects_analyze() {
        let dir = dir_with(INVENTORY_FILENAME, r#"{"schema_version":"99"}"#);
        assert_eq!(
            detect_page(dir.path()),
            Page::Analyze,
            "a newer schema routes to analyze, where the page flags the mismatch"
        );
    }

    #[test]
    fn unparseable_inventory_degrades_to_setup() {
        let dir = dir_with(INVENTORY_FILENAME, "definitely not json");
        assert_eq!(detect_page(dir.path()), Page::Setup);
    }

    #[test]
    fn foreign_json_degrades_to_setup() {
        let dir = dir_with(INVENTORY_FILENAME, r#"{"name":"some other tool"}"#);
        assert_eq!(detect_page(dir.path()), Page::Setup);
    }

    #[test]
    fn json_scalar_degrades_to_setup() {
        let dir = dir_with(INVENTORY_FILENAME, "42");
        assert_eq!(detect_page(dir.path()), Page::Setup);
    }

    #[test]
    fn a_directory_named_inventory_json_degrades_to_setup() {
        let dir = TempDir::new().expect("tempdir");
        fs::create_dir(dir.path().join(INVENTORY_FILENAME)).expect("mkdir");
        assert_eq!(detect_page(dir.path()), Page::Setup);
    }

    #[test]
    fn detection_never_looks_at_ancestors() {
        // The rule is cwd-local: an inventory one level up must not promote a
        // child directory to `analyze`.
        let parent = dir_with(INVENTORY_FILENAME, r#"{"schema_version":"2"}"#);
        let child = parent.path().join("sub");
        fs::create_dir(&child).expect("mkdir");
        assert_eq!(detect_page(&child), Page::Setup);
    }

    #[test]
    fn detect_page_never_returns_remediate() {
        let empty = TempDir::new().expect("tempdir");
        let full = dir_with(INVENTORY_FILENAME, r#"{"schema_version":"2"}"#);
        for dir in [empty.path(), full.path()] {
            assert_ne!(detect_page(dir), Page::Remediate);
        }
    }

    #[test]
    fn each_page_text_is_distinct_and_non_empty() {
        let pages = [Page::Setup, Page::Analyze, Page::Remediate];
        for page in pages {
            assert!(!page.text().is_empty(), "{page:?} page is empty");
        }
        assert_ne!(Page::Setup.text(), Page::Analyze.text());
        assert_ne!(Page::Analyze.text(), Page::Remediate.text());
    }
}
