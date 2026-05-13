//! Mock-related smell detection.
//!
//! Two smell categories derive directly from the per-test / per-file mock
//! counts produced by Phase 1:
//!
//! - **`mock_only_assertions`** — fires when every assertion in the test (or
//!   the file's set of asserting tests) targets the Mock API.
//! - **`mock_overuse`** — fires when the test (or file) constructs / patches
//!   far more mocks than it asserts on.
//!
//! Thresholds live in [`MockSmellConfig`]; v1 uses defaults and does not
//! expose them as CLI flags.

use crate::{FileRecord, SmellHit, TestRecord};

/// Configurable thresholds for the mock-overuse smell.
///
/// Lives in the crate-private `smells` module — reachable only from inside
/// the crate. v1 does not expose these thresholds as CLI flags; every caller
/// today uses [`MockSmellConfig::default`]. Exposing the knob is a contract
/// change to land alongside whatever surface (CLI flag, env var, `--config`
/// file) the user-facing tuning ends up using.
#[derive(Debug, Clone, Copy)]
pub struct MockSmellConfig {
    /// Minimum mocks-vs-asserts spread before `mock_overuse` fires. Acts as
    /// a floor so tests with zero or one assertion don't trip the heuristic
    /// just because the mock count is small but non-zero.
    pub mock_overuse_floor: u64,
    /// Ratio of `(mocks + patches) / max(asserts, 1)` that must be exceeded
    /// (strict `>`) for `mock_overuse` to fire.
    pub mock_overuse_ratio: f64,
}

impl Default for MockSmellConfig {
    fn default() -> Self {
        Self { mock_overuse_floor: 2, mock_overuse_ratio: 2.0 }
    }
}

/// Derive the per-test smell hits for one [`TestRecord`].
///
/// Returns a fresh list; callers append it into `test.smell_hits`.
pub fn derive_test_smells(test: &TestRecord, config: &MockSmellConfig) -> Vec<SmellHit> {
    let mut hits = Vec::new();

    if test.only_asserts_on_mock && test.assertion_count > 0 {
        hits.push(SmellHit {
            category: "mock_only_assertions".to_string(),
            test: Some(test.nodeid.clone()),
            line: test.line,
            evidence: format!("all {} asserts on Mock API", test.assertion_count),
        });
    }

    // Per-test `mock_overuse` consumes `(patch_decorator_count + stubs_count)`.
    // Body-level `mock_construction_count` is deliberately a file-only signal
    // (the AST count lives on `FileRecord`, not `TestRecord`); the per-test
    // smell therefore covers decorator-driven and fixture-driven patching
    // and leaves bare `Mock()`/`MagicMock()` constructions to the file scope.
    let test_mocks = test.patch_decorator_count.saturating_add(test.stubs_count);
    if mock_overuse_fires(test_mocks, test.assertion_count, config) {
        hits.push(SmellHit {
            category: "mock_overuse".to_string(),
            test: Some(test.nodeid.clone()),
            line: test.line,
            evidence: format!("{} mocks, {} assertions", test_mocks, test.assertion_count),
        });
    }

    hits
}

/// Derive the per-file smell hits for one [`FileRecord`].
///
/// The file-level `mock_only_assertions` smell fires only when **every** test
/// in the file that has at least one assertion has `only_asserts_on_mock =
/// true`. Tests with zero asserts are excluded from the predicate. (A file
/// of all-zero-assert tests does not fire `mock_only_assertions`.)
///
/// `mock_overuse` at file level uses the per-file aggregates emitted by the
/// parser.
pub fn derive_file_smells(
    file: &FileRecord,
    tests_in_file: &[&TestRecord],
    config: &MockSmellConfig,
) -> Vec<SmellHit> {
    let mut hits = Vec::new();

    let asserting: Vec<&&TestRecord> =
        tests_in_file.iter().filter(|t| t.assertion_count > 0).collect();
    if !asserting.is_empty() && asserting.iter().all(|t| t.only_asserts_on_mock) {
        hits.push(SmellHit {
            category: "mock_only_assertions".to_string(),
            test: None,
            line: 0,
            evidence: "all asserting tests assert only on Mock API".to_string(),
        });
    }

    let file_mocks = file
        .mock_construction_count
        .saturating_add(file.patch_decorator_count)
        .saturating_add(file.stubs_count);
    if mock_overuse_fires(file_mocks, file.assertion_count, config) {
        hits.push(SmellHit {
            category: "mock_overuse".to_string(),
            test: None,
            line: 0,
            evidence: format!("{} mocks, {} assertions", file_mocks, file.assertion_count),
        });
    }

    hits
}

/// Shared `mock_overuse` predicate.
///
/// Fires when `mocks > max(asserts, floor)` AND `mocks / max(asserts, 1) >
/// ratio`. Both comparisons strict.
fn mock_overuse_fires(mocks: u64, asserts: u64, config: &MockSmellConfig) -> bool {
    let bound = asserts.max(config.mock_overuse_floor);
    if mocks <= bound {
        return false;
    }
    let denom = asserts.max(1) as f64;
    (mocks as f64) / denom > config.mock_overuse_ratio
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_test(nodeid: &str) -> TestRecord {
        TestRecord {
            nodeid: nodeid.to_string(),
            file: PathBuf::from("tests/test_x.py"),
            line: 1,
            assertion_count: 0,
            only_asserts_on_mock: false,
            patch_decorator_count: 0,
            stubs_count: 0,
            setup_to_assertion_ratio: 0.0,
            called_names: Vec::new(),
            smell_hits: Vec::new(),
            suspicion_score: 0.0,
        }
    }

    fn make_file(path: &str) -> FileRecord {
        FileRecord {
            path: PathBuf::from(path),
            test_function_count: 0,
            assertion_count: 0,
            mock_construction_count: 0,
            patch_decorator_count: 0,
            stubs_count: 0,
            fixture_count: 0,
            smell_hits: Vec::new(),
        }
    }

    #[test]
    fn mock_only_assertions_fires_when_only_asserts_on_mock_and_count_gt_zero() {
        let mut t = make_test("a::t");
        t.only_asserts_on_mock = true;
        t.assertion_count = 3;
        let hits = derive_test_smells(&t, &MockSmellConfig::default());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].category, "mock_only_assertions");
        assert_eq!(hits[0].evidence, "all 3 asserts on Mock API");
    }

    #[test]
    fn mock_only_assertions_does_not_fire_when_count_is_zero() {
        let mut t = make_test("a::t");
        t.only_asserts_on_mock = true;
        t.assertion_count = 0;
        let hits = derive_test_smells(&t, &MockSmellConfig::default());
        assert!(hits.iter().all(|h| h.category != "mock_only_assertions"));
    }

    #[test]
    fn mock_only_assertions_does_not_fire_when_only_asserts_on_mock_false() {
        let mut t = make_test("a::t");
        t.only_asserts_on_mock = false;
        t.assertion_count = 5;
        let hits = derive_test_smells(&t, &MockSmellConfig::default());
        assert!(hits.iter().all(|h| h.category != "mock_only_assertions"));
    }

    #[test]
    fn mock_overuse_fires_strict_inequality() {
        // 3 mocks, 1 assert. bound = max(1, 2) = 2. 3 > 2 ✓.
        // ratio = 3 / 1 = 3.0. 3.0 > 2.0 ✓.
        assert!(mock_overuse_fires(3, 1, &MockSmellConfig::default()));
    }

    #[test]
    fn mock_overuse_does_not_fire_on_boundary() {
        // 2 mocks, 2 asserts. bound = max(2, 2) = 2. 2 > 2 false → no fire.
        assert!(!mock_overuse_fires(2, 2, &MockSmellConfig::default()));
    }

    #[test]
    fn mock_overuse_respects_floor() {
        // 1 mock, 0 asserts. bound = max(0, 2) = 2. 1 > 2 false → no fire.
        assert!(!mock_overuse_fires(1, 0, &MockSmellConfig::default()));
    }

    #[test]
    fn mock_overuse_respects_ratio() {
        // 3 mocks, 2 asserts. bound = max(2, 2) = 2. 3 > 2 ✓.
        // ratio = 3 / 2 = 1.5. 1.5 > 2.0 false → no fire.
        assert!(!mock_overuse_fires(3, 2, &MockSmellConfig::default()));
    }

    #[test]
    fn file_level_mock_only_fires_when_all_asserting_tests_only_mock() {
        let mut t1 = make_test("a::t1");
        t1.only_asserts_on_mock = true;
        t1.assertion_count = 2;
        let mut t2 = make_test("a::t2");
        t2.only_asserts_on_mock = true;
        t2.assertion_count = 1;
        // Zero-assert test is excluded from the predicate.
        let t3 = make_test("a::t3");
        let f = make_file("a");
        let refs: Vec<&TestRecord> = vec![&t1, &t2, &t3];
        let hits = derive_file_smells(&f, &refs, &MockSmellConfig::default());
        assert!(hits.iter().any(|h| h.category == "mock_only_assertions"));
        // File-level hits set test = None, line = 0.
        let hit = hits.iter().find(|h| h.category == "mock_only_assertions").unwrap();
        assert_eq!(hit.test, None);
        assert_eq!(hit.line, 0);
    }

    #[test]
    fn file_level_mock_only_does_not_fire_when_no_asserting_tests() {
        let t = make_test("a::t");
        let f = make_file("a");
        let refs: Vec<&TestRecord> = vec![&t];
        let hits = derive_file_smells(&f, &refs, &MockSmellConfig::default());
        assert!(hits.iter().all(|h| h.category != "mock_only_assertions"));
    }

    #[test]
    fn file_level_mock_overuse_uses_file_aggregates() {
        let mut f = make_file("a");
        f.mock_construction_count = 5;
        f.patch_decorator_count = 0;
        f.assertion_count = 1;
        let refs: Vec<&TestRecord> = Vec::new();
        let hits = derive_file_smells(&f, &refs, &MockSmellConfig::default());
        assert!(hits.iter().any(|h| h.category == "mock_overuse"));
        let hit = hits.iter().find(|h| h.category == "mock_overuse").unwrap();
        assert_eq!(hit.evidence, "5 mocks, 1 assertions");
    }

    #[test]
    fn per_test_mock_overuse_consumes_stubs_count() {
        // No `@patch` decorators, but four fixture-driven stubs and one
        // assert — `(0 + 4)` should fire the smell on its own.
        let mut t = make_test("a::t");
        t.patch_decorator_count = 0;
        t.stubs_count = 4;
        t.assertion_count = 1;
        let hits = derive_test_smells(&t, &MockSmellConfig::default());
        let mo = hits.iter().find(|h| h.category == "mock_overuse").expect("mock_overuse hit");
        assert_eq!(mo.evidence, "4 mocks, 1 assertions");
    }

    #[test]
    fn per_test_mock_overuse_sums_patches_and_stubs() {
        // 2 patches + 2 stubs = 4 against 1 assert.
        let mut t = make_test("a::t");
        t.patch_decorator_count = 2;
        t.stubs_count = 2;
        t.assertion_count = 1;
        let hits = derive_test_smells(&t, &MockSmellConfig::default());
        let mo = hits.iter().find(|h| h.category == "mock_overuse").expect("mock_overuse hit");
        assert_eq!(mo.evidence, "4 mocks, 1 assertions");
    }

    #[test]
    fn file_level_mock_overuse_includes_stubs_count() {
        // No constructions / no decorators, but a stub-heavy file with one
        // assert — `(0 + 0 + 5)` fires `mock_overuse` at file scope.
        let mut f = make_file("a");
        f.mock_construction_count = 0;
        f.patch_decorator_count = 0;
        f.stubs_count = 5;
        f.assertion_count = 1;
        let refs: Vec<&TestRecord> = Vec::new();
        let hits = derive_file_smells(&f, &refs, &MockSmellConfig::default());
        let mo = hits.iter().find(|h| h.category == "mock_overuse").expect("mock_overuse hit");
        assert_eq!(mo.evidence, "5 mocks, 1 assertions");
    }
}
