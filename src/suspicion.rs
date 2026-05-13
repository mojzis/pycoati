//! Suspicion scoring (Run 3 step 10).
//!
//! Per-test score blends five heuristic signals — mock-only assertions,
//! patch-decorator density, setup-to-assertion ratio, zero-assert tests, and
//! smell density — each weighted by a tunable constant. File-level scores are
//! the mean of their tests' scores plus a small mock-overuse bonus.
//!
//! Weights, sub-metric definitions, and the bonus shape are documented in
//! `WEIGHTS.md` at the repo root; the in-code [`DEFAULT`] constant is the
//! single source of truth at runtime. Run 3 ships static defaults — there is
//! no TOML loader or `--weights` flag (planned v2 extension; see
//! `WEIGHTS.md`).

use crate::{FileRecord, TestRecord};

/// Weights for the per-test suspicion-score formula.
///
/// Lives in the crate-private `suspicion` module. v1 ships with [`DEFAULT`]
/// and no runtime override — exposing this surface is a deliberate next-step
/// design decision (see `WEIGHTS.md`).
///
/// Field names mirror the formula in `WEIGHTS.md` exactly (`w_mock_only`,
/// `w_patch_count`, …); clippy's `struct_field_names` warning is allowed
/// here so the names stay in sync with the docs.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::struct_field_names)]
pub struct SuspicionWeights {
    /// Bonus when every assertion in the test targets the Mock API.
    pub w_mock_only: f64,
    /// Density term over patch decorators (saturates at five).
    pub w_patch_count: f64,
    /// Sigmoid term over `setup_to_assertion_ratio` (inflection at 8 lines).
    pub w_setup_ratio: f64,
    /// Bonus when the test has zero `assert_statement` nodes.
    pub w_zero_asserts: f64,
    /// Density term over `smell_hits.len()` (saturates at three).
    pub w_smell_density: f64,
}

/// Locked v1 weights. See `WEIGHTS.md` for rationale and revision policy.
pub const DEFAULT: SuspicionWeights = SuspicionWeights {
    w_mock_only: 0.35,
    w_patch_count: 0.20,
    w_setup_ratio: 0.15,
    w_zero_asserts: 0.20,
    w_smell_density: 0.10,
};

/// Score one [`TestRecord`] against the weighted formula.
///
/// The formula (verbatim from `WEIGHTS.md`):
///
/// ```text
/// score = w_mock_only     * (only_asserts_on_mock ? 1.0 : 0.0)
///       + w_patch_count   * min(patch_decorator_count / 5.0, 1.0)
///       + w_setup_ratio   * sigmoid((setup_to_assertion_ratio - 8.0) / 4.0)
///       + w_zero_asserts  * (assertion_count == 0 ? 1.0 : 0.0)
///       + w_smell_density * min(smell_hits.len() / 3.0, 1.0)
/// ```
pub fn score_test(test: &TestRecord, weights: &SuspicionWeights) -> f64 {
    let mock_only_term = if test.only_asserts_on_mock { weights.w_mock_only } else { 0.0 };

    let patch_ratio = (test.patch_decorator_count as f64 / 5.0).min(1.0);
    let patch_term = weights.w_patch_count * patch_ratio;

    let setup_term = weights.w_setup_ratio * sigmoid((test.setup_to_assertion_ratio - 8.0) / 4.0);

    let zero_asserts_term = if test.assertion_count == 0 { weights.w_zero_asserts } else { 0.0 };

    let smell_ratio = (test.smell_hits.len() as f64 / 3.0).min(1.0);
    let smell_term = weights.w_smell_density * smell_ratio;

    mock_only_term + patch_term + setup_term + zero_asserts_term + smell_term
}

/// Score one [`FileRecord`] from its tests' already-computed scores.
///
/// File score = `mean(test_scores) + bonus`, where the bonus grows linearly
/// once `mock_construction_count / max(assertion_count, 1) > 1.0` and is
/// capped at `0.1`. Empty `test_scores` returns `0.0` without panicking.
pub fn score_file(file: &FileRecord, test_scores: &[f64]) -> f64 {
    if test_scores.is_empty() {
        return 0.0;
    }
    let mean = test_scores.iter().copied().sum::<f64>() / test_scores.len() as f64;
    let denom = file.assertion_count.max(1) as f64;
    let ratio = file.mock_construction_count as f64 / denom;
    let raw_bonus = (ratio - 1.0).max(0.0) * 0.05;
    let bonus = raw_bonus.min(0.1);
    mean + bonus
}

/// Return the top-N test nodeids by suspicion score (descending), tie-broken
/// by nodeid ascending for determinism. `n = 0` returns an empty vector;
/// `n > records.len()` returns every nodeid.
pub fn top_n_tests(records: &[TestRecord], n: usize) -> Vec<String> {
    if n == 0 || records.is_empty() {
        return Vec::new();
    }
    let mut ranked: Vec<(&str, f64)> =
        records.iter().map(|t| (t.nodeid.as_str(), t.suspicion_score)).collect();
    // `total_cmp` is a total order over f64 (NaN-aware), so the sort is
    // panic-free and deterministic even if a NaN somehow slips in.
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    ranked.into_iter().take(n).map(|(nodeid, _)| nodeid.to_string()).collect()
}

/// Return the top-N file paths by file score (descending), tie-broken by
/// path ascending. `files` and `scores` must align by index; mismatched
/// lengths yield an empty vector.
pub fn top_n_files(files: &[FileRecord], scores: &[f64], n: usize) -> Vec<String> {
    if n == 0 || files.is_empty() || files.len() != scores.len() {
        return Vec::new();
    }
    let mut ranked: Vec<(String, f64)> = files
        .iter()
        .zip(scores.iter().copied())
        .map(|(f, s)| (f.path.display().to_string(), s))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.into_iter().take(n).map(|(path, _)| path).collect()
}

/// Logistic sigmoid: `1 / (1 + e^-x)`. Centered at `x = 0` with value `0.5`,
/// saturating to `0` and `1` for large negative / positive inputs.
fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SmellHit;
    use std::path::PathBuf;

    fn make_test(nodeid: &str) -> TestRecord {
        TestRecord {
            nodeid: nodeid.to_string(),
            file: PathBuf::from("tests/test_x.py"),
            line: 1,
            assertion_count: 1,
            only_asserts_on_mock: false,
            patch_decorator_count: 0,
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
            fixture_count: 0,
            smell_hits: Vec::new(),
        }
    }

    fn smell_hit() -> SmellHit {
        SmellHit {
            category: "mock_overuse".to_string(),
            test: None,
            line: 0,
            evidence: String::new(),
        }
    }

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn mock_only_term_is_w_mock_only_when_true() {
        let mut t = make_test("a");
        t.only_asserts_on_mock = true;
        // setup_to_assertion_ratio defaults to 0 — sigmoid((0-8)/4) = sigmoid(-2),
        // a small positive constant. We isolate the mock-only term by checking
        // it appears alone among the on/off flags.
        let on = score_test(&t, &DEFAULT);
        t.only_asserts_on_mock = false;
        let off = score_test(&t, &DEFAULT);
        assert!(approx_eq(on - off, DEFAULT.w_mock_only));
    }

    #[test]
    fn mock_only_term_is_zero_when_false() {
        let mut t = make_test("a");
        t.only_asserts_on_mock = false;
        // The score should not depend on w_mock_only when the flag is off.
        let mut weights = DEFAULT;
        let s1 = score_test(&t, &weights);
        weights.w_mock_only = 999.0;
        let s2 = score_test(&t, &weights);
        assert!(approx_eq(s1, s2));
        // Sanity: also confirm sweeping the other terms still works.
        t.only_asserts_on_mock = false;
        let _ = score_test(&t, &weights);
    }

    #[test]
    fn patch_count_term_clamps_at_five() {
        let mut a = make_test("a");
        a.patch_decorator_count = 5;
        let mut b = make_test("b");
        b.patch_decorator_count = 10;
        let sa = score_test(&a, &DEFAULT);
        let sb = score_test(&b, &DEFAULT);
        assert!(approx_eq(sa, sb), "patch_count term must saturate at 5: {sa} vs {sb}");
    }

    #[test]
    fn setup_ratio_term_inflection_at_eight() {
        let mut t = make_test("a");
        t.setup_to_assertion_ratio = 8.0;
        // Score at ratio = 8 with w_setup_ratio contributing exactly 0.5 *
        // w_setup_ratio. Isolate by zeroing every other term:
        t.only_asserts_on_mock = false;
        t.patch_decorator_count = 0;
        t.assertion_count = 1; // non-zero → zero-asserts term off
        t.smell_hits.clear();
        let s = score_test(&t, &DEFAULT);
        assert!(
            approx_eq(s, 0.5 * DEFAULT.w_setup_ratio),
            "expected exactly 0.5 * w_setup_ratio at inflection, got {s}"
        );
    }

    #[test]
    fn setup_ratio_term_saturates_high_low() {
        let mut t = make_test("a");
        t.assertion_count = 1;
        t.setup_to_assertion_ratio = 0.0;
        let low = score_test(&t, &DEFAULT);
        t.setup_to_assertion_ratio = 30.0;
        let high = score_test(&t, &DEFAULT);
        // Low should be a small fraction of w_setup_ratio; high should approach
        // w_setup_ratio. Both must stay bounded inside [0, w_setup_ratio].
        assert!((0.0..0.05).contains(&low));
        assert!(high > 0.95 * DEFAULT.w_setup_ratio && high <= DEFAULT.w_setup_ratio + 1e-12);
    }

    #[test]
    fn zero_asserts_term_fires_only_when_zero() {
        let mut t = make_test("a");
        t.assertion_count = 0;
        let zero = score_test(&t, &DEFAULT);
        t.assertion_count = 1;
        let one = score_test(&t, &DEFAULT);
        assert!(approx_eq(zero - one, DEFAULT.w_zero_asserts));
    }

    #[test]
    fn smell_density_clamps_at_three() {
        let mut a = make_test("a");
        a.smell_hits = vec![smell_hit(), smell_hit(), smell_hit()];
        let mut b = make_test("b");
        b.smell_hits = (0..10).map(|_| smell_hit()).collect();
        let sa = score_test(&a, &DEFAULT);
        let sb = score_test(&b, &DEFAULT);
        assert!(approx_eq(sa, sb), "smell density must saturate at 3: {sa} vs {sb}");
    }

    #[test]
    fn score_test_sums_all_terms() {
        // Build a test with known values for every term.
        // - only_asserts_on_mock = true  → mock_only contributes 0.35
        // - patch_decorator_count = 3    → patch contributes 0.20 * 3/5 = 0.12
        // - setup_to_assertion_ratio = 8 → setup contributes 0.15 * 0.5 = 0.075
        // - assertion_count = 0          → zero_asserts contributes 0.20
        //   (since asserts == 0, zero_asserts_term fires)
        // - smell_hits.len() = 3         → smell contributes 0.10 * 1.0 = 0.10
        // Sum: 0.35 + 0.12 + 0.075 + 0.20 + 0.10 = 0.845
        let mut t = make_test("a");
        t.only_asserts_on_mock = true;
        t.patch_decorator_count = 3;
        t.setup_to_assertion_ratio = 8.0;
        t.assertion_count = 0;
        t.smell_hits = vec![smell_hit(), smell_hit(), smell_hit()];
        let s = score_test(&t, &DEFAULT);
        assert!(approx_eq(s, 0.845), "expected 0.845, got {s}");
    }

    #[test]
    fn score_file_is_mean_when_no_mock_bonus() {
        let f = make_file("tests/test_x.py");
        // No mock constructions → bonus = 0.
        let scores = [0.1_f64, 0.2, 0.3];
        let s = score_file(&f, &scores);
        assert!(approx_eq(s, 0.2));
    }

    #[test]
    fn score_file_bonus_capped_at_zero_point_one() {
        let mut f = make_file("tests/test_x.py");
        f.mock_construction_count = 100;
        f.assertion_count = 1;
        // ratio = 100, raw_bonus = (100 - 1) * 0.05 = 4.95, capped at 0.1.
        let scores = [0.2_f64];
        let s = score_file(&f, &scores);
        assert!(approx_eq(s, 0.3), "expected mean (0.2) + cap (0.1) = 0.3, got {s}");
    }

    #[test]
    fn score_file_bonus_zero_when_ratio_le_one() {
        let mut f = make_file("tests/test_x.py");
        f.mock_construction_count = 1;
        f.assertion_count = 1;
        let scores = [0.5_f64];
        let s = score_file(&f, &scores);
        assert!(approx_eq(s, 0.5), "expected mean (0.5) + bonus (0) = 0.5, got {s}");
    }

    #[test]
    fn score_file_empty_tests_returns_zero() {
        let f = make_file("tests/test_x.py");
        let s = score_file(&f, &[]);
        assert!(approx_eq(s, 0.0));
    }

    #[test]
    fn top_n_tests_sorted_desc_by_score_then_asc_by_nodeid() {
        let mut a = make_test("a");
        a.suspicion_score = 0.1;
        let mut b = make_test("b");
        b.suspicion_score = 0.1;
        let mut c = make_test("c");
        c.suspicion_score = 0.5;
        let records = vec![a, b, c];
        let top = top_n_tests(&records, 3);
        assert_eq!(top, vec!["c".to_string(), "a".to_string(), "b".to_string()]);
    }

    #[test]
    fn top_n_tests_caps_at_n() {
        let records: Vec<TestRecord> = (0..30)
            .map(|i| {
                let mut t = make_test(&format!("t{i:02}"));
                t.suspicion_score = f64::from(i) / 100.0;
                t
            })
            .collect();
        let top = top_n_tests(&records, 5);
        assert_eq!(top.len(), 5);
    }

    #[test]
    fn top_n_tests_n_zero_returns_empty() {
        let a = make_test("a");
        let top = top_n_tests(&[a], 0);
        assert!(top.is_empty());
    }

    #[test]
    fn top_n_files_sorted_desc_then_asc_by_path() {
        let f_a = make_file("a");
        let f_b = make_file("b");
        let f_c = make_file("c");
        let files = vec![f_a, f_b, f_c];
        let scores = vec![0.1, 0.1, 0.5];
        let top = top_n_files(&files, &scores, 3);
        assert_eq!(top, vec!["c".to_string(), "a".to_string(), "b".to_string()]);
    }

    #[test]
    fn top_n_files_caps_at_n() {
        let files: Vec<FileRecord> = (0..30).map(|i| make_file(&format!("f{i:02}"))).collect();
        let scores: Vec<f64> = (0..30).map(|i| f64::from(i) / 100.0).collect();
        let top = top_n_files(&files, &scores, 5);
        assert_eq!(top.len(), 5);
    }
}
