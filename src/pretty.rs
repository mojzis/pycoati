//! Pretty-print an [`crate::Inventory`] to plain text for terminal use.
//!
//! Pure function: no I/O, no color, no tracing. The renderer reads the
//! already-populated inventory and emits aligned-column sections separated by
//! `-`-underlines (section headers) and a `=`-underline (document title). No
//! pipe characters, no markdown — terminal-friendly output users can pipe
//! into `less` or grep.
//!
//! Output shape (locked in Run 3 spec, step 11; Phase 2 adds `stubs`;
//! Run 4 phase 3 updates the title to surface the directory basename and
//! disambiguate against the pyproject `[project].name` only when they
//! differ):
//!
//! ```text
//! pycoati audit — <basename>                       # when basename == project.name
//! pycoati audit — <basename> (<project name>)      # when they differ
//! ============================
//!
//! Suite
//! -----
//!   tests           <count or —>
//!   runtime         <seconds or —>s
//!   coverage        <pct or —>%
//!
//! Top suspicious tests
//! --------------------
//!   score  nodeid                                    asserts  mocks  stubs  smells
//!    …
//!
//! Top suspicious files
//! --------------------
//!   score  path                  tests  asserts  mocks  stubs  smells
//!    …
//!
//! SUT calls (top 20)
//! ------------------
//!   count  name
//!    …
//! ```

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::{FileRecord, Inventory, Project, TestRecord};

/// Render the inventory as plain text using `top_n` for both the test and
/// file suspicious sections. Returns the formatted string; the caller writes
/// it to stdout or a file.
pub fn render(inv: &Inventory, top_n: usize) -> String {
    let mut out = String::new();
    render_title(&mut out, &inv.project);
    out.push('\n');
    render_suite(&mut out, inv);
    out.push('\n');
    render_top_tests(&mut out, inv, top_n);
    out.push('\n');
    render_top_files(&mut out, inv, top_n);
    out.push('\n');
    render_sut_calls(&mut out, inv);
    out
}

fn render_title(out: &mut String, project: &Project) {
    // Prefer the directory basename as the primary identifier — that's what
    // users typed on the command line and recognise — and surface the
    // pyproject `[project].name` as a secondary token only when it differs
    // (the two routinely disagree: `zazzy-thunder` checkout, `thunder_pkg`
    // package). When `project.path` has no `file_name()` (e.g. it's the
    // filesystem root), fall back to the project name as the primary.
    let basename = project.path.file_name().and_then(|n| n.to_str()).unwrap_or(&project.name);
    let title = if basename == project.name {
        format!("pycoati audit — {basename}")
    } else {
        format!("pycoati audit — {basename} ({})", project.name)
    };
    // The em-dash is a single Unicode char counted via `chars().count()` so
    // multi-byte chars do not over-extend the underline.
    let underline_len = title.chars().count();
    out.push_str(&title);
    out.push('\n');
    for _ in 0..underline_len {
        out.push('=');
    }
    out.push('\n');
}

fn render_section_header(out: &mut String, header: &str) {
    out.push_str(header);
    out.push('\n');
    for _ in 0..header.chars().count() {
        out.push('-');
    }
    out.push('\n');
}

fn render_suite(out: &mut String, inv: &Inventory) {
    render_section_header(out, "Suite");
    let tests = inv.suite.test_count.map_or_else(|| "—".to_string(), |n| n.to_string());
    let runtime =
        inv.suite.runtime_seconds.map_or_else(|| "—s".to_string(), |s| format!("{s:.2}s"));
    let coverage =
        inv.suite.line_coverage_pct.map_or_else(|| "—%".to_string(), |c| format!("{c:.2}%"));
    let _ = writeln!(out, "  tests           {tests}");
    let _ = writeln!(out, "  runtime         {runtime}");
    let _ = writeln!(out, "  coverage        {coverage}");
}

/// One row in the "Top suspicious tests" table — pre-rendered string cells
/// kept together so column widths can be computed across the full row set
/// in a single pass.
struct TestRow {
    score: String,
    nodeid: String,
    asserts: String,
    mocks: String,
    stubs: String,
    smells: String,
}

fn render_top_tests(out: &mut String, inv: &Inventory, top_n: usize) {
    render_section_header(out, "Top suspicious tests");
    if inv.top_suspicious.test_functions.is_empty() {
        out.push_str("  (none)\n");
        return;
    }

    // Index test records by nodeid for O(1) lookup.
    let by_nodeid: BTreeMap<&str, &TestRecord> =
        inv.test_functions.iter().map(|t| (t.nodeid.as_str(), t)).collect();

    let rows: Vec<TestRow> = inv
        .top_suspicious
        .test_functions
        .iter()
        .take(top_n)
        .filter_map(|nodeid| by_nodeid.get(nodeid.as_str()).map(|t| build_test_row(nodeid, t)))
        .collect();

    if rows.is_empty() {
        out.push_str("  (none)\n");
        return;
    }

    let headers = ["score", "nodeid", "asserts", "mocks", "stubs", "smells"];
    let mut widths = [0_usize; 6];
    for (i, h) in headers.iter().enumerate() {
        widths[i] = h.len();
    }
    for r in &rows {
        widths[0] = widths[0].max(r.score.chars().count());
        widths[1] = widths[1].max(r.nodeid.chars().count());
        widths[2] = widths[2].max(r.asserts.chars().count());
        widths[3] = widths[3].max(r.mocks.chars().count());
        widths[4] = widths[4].max(r.stubs.chars().count());
        widths[5] = widths[5].max(r.smells.chars().count());
    }

    // Header line: score / asserts / mocks / stubs right-aligned (numeric),
    // nodeid and smells left-aligned.
    let _ = writeln!(
        out,
        "  {sc:>w0$}  {nd:<w1$}  {as_:>w2$}  {mk:>w3$}  {st:>w4$}  {sm}",
        sc = headers[0],
        nd = headers[1],
        as_ = headers[2],
        mk = headers[3],
        st = headers[4],
        sm = headers[5],
        w0 = widths[0],
        w1 = widths[1],
        w2 = widths[2],
        w3 = widths[3],
        w4 = widths[4],
    );
    for r in &rows {
        let _ = writeln!(
            out,
            "  {sc:>w0$}  {nd:<w1$}  {as_:>w2$}  {mk:>w3$}  {st:>w4$}  {sm}",
            sc = r.score,
            nd = r.nodeid,
            as_ = r.asserts,
            mk = r.mocks,
            st = r.stubs,
            sm = r.smells,
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
            w4 = widths[4],
        );
    }
}

fn build_test_row(nodeid: &str, t: &TestRecord) -> TestRow {
    let smells = join_smell_categories(&t.smell_hits);
    TestRow {
        score: format!("{:.2}", t.suspicion_score),
        nodeid: nodeid.to_string(),
        asserts: t.assertion_count.to_string(),
        // Test records don't carry `mock_construction_count`; use the
        // patch-decorator count as the per-test mock proxy (matches the
        // mock_overuse smell logic in `smells.rs`). `stubs_count` renders
        // in its own column.
        mocks: t.patch_decorator_count.to_string(),
        stubs: t.stubs_count.to_string(),
        smells,
    }
}

fn join_smell_categories(hits: &[crate::SmellHit]) -> String {
    if hits.is_empty() {
        return "—".to_string();
    }
    let mut cats: Vec<&str> = hits.iter().map(|h| h.category.as_str()).collect();
    cats.sort_unstable();
    cats.dedup();
    cats.join(", ")
}

struct FileRow {
    score: String,
    path: String,
    tests: String,
    asserts: String,
    mocks: String,
    stubs: String,
    smells: String,
}

fn render_top_files(out: &mut String, inv: &Inventory, top_n: usize) {
    render_section_header(out, "Top suspicious files");
    if inv.top_suspicious.files.is_empty() {
        out.push_str("  (none)\n");
        return;
    }

    let by_path: BTreeMap<String, &FileRecord> =
        inv.files.iter().map(|f| (f.path.display().to_string(), f)).collect();

    // File scores live on `top_suspicious.files` as ordered paths; we don't
    // get the score back. Recompute display score by averaging that file's
    // tests (mirroring the JSON pipeline's ordering).
    let test_scores_by_file = group_test_scores_by_file(inv);

    let rows: Vec<FileRow> = inv
        .top_suspicious
        .files
        .iter()
        .take(top_n)
        .filter_map(|path| {
            by_path.get(path.as_str()).map(|f| {
                build_file_row(path, f, test_scores_by_file.get(path.as_str()).map(Vec::as_slice))
            })
        })
        .collect();

    if rows.is_empty() {
        out.push_str("  (none)\n");
        return;
    }

    let headers = ["score", "path", "tests", "asserts", "mocks", "stubs", "smells"];
    let mut widths = [0_usize; 7];
    for (i, h) in headers.iter().enumerate() {
        widths[i] = h.len();
    }
    for r in &rows {
        widths[0] = widths[0].max(r.score.chars().count());
        widths[1] = widths[1].max(r.path.chars().count());
        widths[2] = widths[2].max(r.tests.chars().count());
        widths[3] = widths[3].max(r.asserts.chars().count());
        widths[4] = widths[4].max(r.mocks.chars().count());
        widths[5] = widths[5].max(r.stubs.chars().count());
        widths[6] = widths[6].max(r.smells.chars().count());
    }

    let _ = writeln!(
        out,
        "  {sc:>w0$}  {pt:<w1$}  {ts:>w2$}  {as_:>w3$}  {mk:>w4$}  {st:>w5$}  {sm}",
        sc = headers[0],
        pt = headers[1],
        ts = headers[2],
        as_ = headers[3],
        mk = headers[4],
        st = headers[5],
        sm = headers[6],
        w0 = widths[0],
        w1 = widths[1],
        w2 = widths[2],
        w3 = widths[3],
        w4 = widths[4],
        w5 = widths[5],
    );
    for r in &rows {
        let _ = writeln!(
            out,
            "  {sc:>w0$}  {pt:<w1$}  {ts:>w2$}  {as_:>w3$}  {mk:>w4$}  {st:>w5$}  {sm}",
            sc = r.score,
            pt = r.path,
            ts = r.tests,
            as_ = r.asserts,
            mk = r.mocks,
            st = r.stubs,
            sm = r.smells,
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
            w4 = widths[4],
            w5 = widths[5],
        );
    }
}

fn build_file_row(path: &str, f: &FileRecord, scores: Option<&[f64]>) -> FileRow {
    // Delegate to `suspicion::score_file` — single source of truth for the
    // file-score formula. Without per-test scores the pretty output shows
    // 0.00 (no scores means nothing to average).
    let score = scores.map_or(0.0, |s| crate::suspicion::score_file(f, s));
    FileRow {
        score: format!("{score:.2}"),
        path: path.to_string(),
        tests: f.test_function_count.to_string(),
        asserts: f.assertion_count.to_string(),
        mocks: (f.mock_construction_count + f.patch_decorator_count).to_string(),
        stubs: f.stubs_count.to_string(),
        smells: f.smell_hits.len().to_string(),
    }
}

fn group_test_scores_by_file(inv: &Inventory) -> BTreeMap<String, Vec<f64>> {
    let mut grouped: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for t in &inv.test_functions {
        grouped.entry(t.file.display().to_string()).or_default().push(t.suspicion_score);
    }
    grouped
}

fn render_sut_calls(out: &mut String, inv: &Inventory) {
    render_section_header(out, "SUT calls (top 20)");
    if inv.sut_calls.top_called.is_empty() {
        out.push_str("  (none)\n");
        return;
    }

    let by_name: BTreeMap<&str, u64> =
        inv.sut_calls.by_name.iter().map(|e| (e.name.as_str(), e.test_function_count)).collect();

    let rows: Vec<(String, String)> = inv
        .sut_calls
        .top_called
        .iter()
        .map(|name| {
            let count = by_name.get(name.as_str()).copied().unwrap_or(0);
            (count.to_string(), name.clone())
        })
        .collect();

    let mut count_w = "count".len();
    for (c, _) in &rows {
        count_w = count_w.max(c.chars().count());
    }

    let header_count = "count";
    let _ = writeln!(out, "  {header_count:>count_w$}  name");
    for (c, n) in &rows {
        let _ = writeln!(out, "  {c:>count_w$}  {n}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Inventory, Project, SmellHit, Suite, SutCallEntry, SutCalls, TestRecord, ToolInfo,
        TopSuspicious,
    };
    use std::path::PathBuf;

    fn empty_inv() -> Inventory {
        Inventory {
            schema_version: "2".to_string(),
            // Pick a path whose basename matches the project name so the
            // title-rendering helper produces the single-token form
            // `pycoati audit — demo`. Tests that exercise the
            // basename-differs branch construct their own `Project`.
            project: Project { path: PathBuf::from("/demo"), name: "demo".to_string() },
            suite: Suite {
                test_count: None,
                runtime_seconds: None,
                line_coverage_pct: None,
                slowest_tests: Vec::new(),
            },
            files: Vec::new(),
            test_functions: Vec::new(),
            sut_calls: SutCalls { by_name: Vec::new(), top_called: Vec::new() },
            top_suspicious: TopSuspicious { test_functions: Vec::new(), files: Vec::new() },
            tool: ToolInfo::with_runtime(false, false),
        }
    }

    fn make_test(nodeid: &str, score: f64, asserts: u64, patches: u64) -> TestRecord {
        TestRecord {
            nodeid: nodeid.to_string(),
            file: PathBuf::from("tests/x.py"),
            line: 1,
            assertion_count: asserts,
            only_asserts_on_mock: false,
            patch_decorator_count: patches,
            stubs_count: 0,
            setup_to_assertion_ratio: 0.0,
            called_names: Vec::new(),
            smell_hits: Vec::new(),
            suspicion_score: score,
        }
    }

    #[test]
    fn render_handles_empty_inventory() {
        let inv = empty_inv();
        let out = render(&inv, 20);
        let lines: Vec<&str> = out.lines().collect();
        // Title line + matching `=` underline. `chars().count()` matters
        // because the title contains the em-dash, which is multi-byte.
        let title = "pycoati audit — demo";
        assert_eq!(lines[0], title);
        let underline = lines[1];
        assert_eq!(underline.chars().count(), title.chars().count());
        assert!(underline.chars().all(|c| c == '='), "underline not all '=': {underline:?}");
        // All four section headers must render even when their content is empty.
        for header in
            ["Suite", "Top suspicious tests", "Top suspicious files", "SUT calls (top 20)"]
        {
            assert!(out.contains(header), "missing section header '{header}' in:\n{out}");
        }
    }

    #[test]
    fn render_handles_static_only() {
        let inv = empty_inv();
        let out = render(&inv, 20);
        // Suite block uses `—` for None.
        assert!(out.contains("  tests           —"));
        assert!(out.contains("  runtime         —s"));
        assert!(out.contains("  coverage        —%"));
    }

    #[test]
    fn render_handles_empty_top_suspicious() {
        let inv = empty_inv();
        let out = render(&inv, 20);
        // Every empty section under its underline shows `(none)`.
        assert!(out.contains("Top suspicious tests\n--------------------\n  (none)"));
        assert!(out.contains("Top suspicious files\n--------------------\n  (none)"));
        assert!(out.contains("SUT calls (top 20)\n------------------\n  (none)"));
    }

    #[test]
    fn render_aligns_columns() {
        let mut inv = empty_inv();
        let t1 = make_test("tests/a.py::test_one", 0.5, 2, 4);
        let t2 = make_test("tests/longer/path/b.py::test_two_with_long_name", 0.4, 10, 1);
        inv.top_suspicious.test_functions = vec![t1.nodeid.clone(), t2.nodeid.clone()];
        inv.test_functions = vec![t1, t2];
        let out = render(&inv, 20);

        // Find the header line ("score  nodeid …") and the two data rows;
        // assert the score column starts at the same character offset on
        // each rendered row.
        let lines: Vec<&str> = out.lines().collect();
        let header_idx = lines.iter().position(|l| l.contains("score") && l.contains("nodeid"));
        let header_idx = header_idx.expect("found tests header");
        // `score` is right-aligned and `nodeid` left-aligned, so the column
        // that we can easily align-check is `nodeid`: same start offset on
        // both rows.
        let nodeid_col = lines[header_idx].find("nodeid").expect("header has 'nodeid'");
        for row in &lines[header_idx + 1..header_idx + 3] {
            assert!(row.len() > nodeid_col, "row too short: {row:?}");
            // Character at the nodeid column should NOT be space (the row's
            // nodeid begins there).
            let ch = row.chars().nth(nodeid_col);
            assert!(
                ch.is_some_and(|c| c != ' '),
                "row's nodeid column ({nodeid_col}) is not aligned: {row:?}"
            );
        }
    }

    #[test]
    fn render_score_format_two_decimals() {
        let mut inv = empty_inv();
        let t = make_test("tests/a.py::test_x", 0.123_456, 1, 0);
        inv.top_suspicious.test_functions = vec![t.nodeid.clone()];
        inv.test_functions = vec![t];
        let out = render(&inv, 20);
        assert!(out.contains(" 0.12 "), "expected score formatted as '0.12', got:\n{out}");
        assert!(!out.contains("0.123"), "two-decimal cap violated, got:\n{out}");
    }

    #[test]
    fn render_smell_categories_joined() {
        let mut inv = empty_inv();
        let mut t = make_test("tests/a.py::test_x", 0.5, 1, 0);
        let nodeid = t.nodeid.clone();
        t.smell_hits = vec![
            SmellHit {
                category: "mock_overuse".to_string(),
                test: Some(nodeid.clone()),
                line: 1,
                evidence: String::new(),
            },
            SmellHit {
                category: "mock_only_assertions".to_string(),
                test: Some(nodeid.clone()),
                line: 1,
                evidence: String::new(),
            },
        ];
        inv.top_suspicious.test_functions = vec![nodeid];
        inv.test_functions = vec![t];
        let out = render(&inv, 20);
        // Sorted form: "mock_only_assertions, mock_overuse".
        assert!(
            out.contains("mock_only_assertions, mock_overuse"),
            "expected sorted joined smells, got:\n{out}"
        );
    }

    #[test]
    fn render_no_markdown_syntax() {
        let mut inv = empty_inv();
        let t = make_test("tests/a.py::test_x", 0.5, 1, 0);
        let nodeid = t.nodeid.clone();
        inv.sut_calls.top_called = vec!["myproj.foo".to_string()];
        inv.sut_calls.by_name = vec![SutCallEntry {
            name: "myproj.foo".to_string(),
            test_function_count: 1,
            test_nodeids: vec![nodeid.clone()],
        }];
        inv.top_suspicious.test_functions = vec![nodeid];
        inv.test_functions = vec![t];
        let out = render(&inv, 20);
        assert!(!out.contains('|'), "pipe characters disallowed, got:\n{out}");
        assert!(!out.contains("---|"), "markdown table separators disallowed, got:\n{out}");
    }

    #[test]
    fn render_includes_stubs_column_in_test_table() {
        let mut inv = empty_inv();
        let mut t = make_test("tests/a.py::test_x", 0.5, 1, 0);
        t.stubs_count = 4;
        inv.top_suspicious.test_functions = vec![t.nodeid.clone()];
        inv.test_functions = vec![t];
        let out = render(&inv, 20);

        // Header line contains a `stubs` column between `mocks` and `smells`.
        let lines: Vec<&str> = out.lines().collect();
        let header = lines
            .iter()
            .find(|l| l.contains("score") && l.contains("nodeid") && l.contains("stubs"))
            .expect("tests header with stubs column");
        let mocks_at = header.find("mocks").expect("mocks header present");
        let stubs_at = header.find("stubs").expect("stubs header present");
        let smells_at = header.find("smells").expect("smells header present");
        assert!(
            mocks_at < stubs_at && stubs_at < smells_at,
            "expected column order mocks < stubs < smells, got header: {header:?}"
        );

        // The data row carries `4` in the stubs column.
        let row =
            lines.iter().find(|l| l.contains("::test_x")).expect("data row for test_x rendered");
        // The `4` should be rendered somewhere after `mocks` and before
        // `smells` — confirm the row contains the stubs value.
        assert!(row.contains(" 4 "), "expected stubs value 4 in row, got: {row:?}");
    }

    #[test]
    fn render_includes_stubs_column_in_file_table() {
        // Build an inventory whose only file has a non-zero stubs_count and
        // sits in `top_suspicious.files`.
        let mut inv = empty_inv();
        let t = make_test("tests/a.py::test_x", 0.5, 1, 0);
        let file_path = "tests/a.py".to_string();
        let file = FileRecord {
            path: PathBuf::from(&file_path),
            test_function_count: 1,
            assertion_count: 1,
            mock_construction_count: 0,
            patch_decorator_count: 0,
            stubs_count: 7,
            fixture_count: 0,
            smell_hits: Vec::new(),
        };
        inv.top_suspicious.test_functions = vec![t.nodeid.clone()];
        inv.test_functions = vec![t];
        inv.files = vec![file];
        inv.top_suspicious.files = vec![file_path];
        let out = render(&inv, 20);

        // Locate the files-table header (the one with `path` + `stubs`).
        let lines: Vec<&str> = out.lines().collect();
        let header = lines
            .iter()
            .find(|l| l.contains("score") && l.contains("path") && l.contains("stubs"))
            .expect("files header with stubs column");
        let mocks_at = header.find("mocks").expect("mocks present");
        let stubs_at = header.find("stubs").expect("stubs present");
        let smells_at = header.find("smells").expect("smells present");
        assert!(
            mocks_at < stubs_at && stubs_at < smells_at,
            "expected files column order mocks < stubs < smells, got header: {header:?}"
        );

        // The file-table data row carries `7` in the stubs column. (The
        // test-table also mentions `tests/a.py` via the nodeid, so search
        // for a row that does NOT carry `::test_x` to disambiguate.)
        let row = lines
            .iter()
            .find(|l| l.contains("tests/a.py") && !l.contains("::test_x") && !l.contains("score"))
            .expect("file-table data row for tests/a.py rendered");
        assert!(row.contains(" 7 "), "expected stubs value 7 in row, got: {row:?}");
    }

    #[test]
    fn render_title_uses_basename_alone_when_matches_project_name() {
        // basename(path) == project.name → single-token title.
        let project =
            Project { path: PathBuf::from("/home/u/widgets"), name: "widgets".to_string() };
        let mut out = String::new();
        render_title(&mut out, &project);
        let first_line = out.lines().next().expect("title line present");
        assert_eq!(first_line, "pycoati audit — widgets");
    }

    #[test]
    fn render_title_appends_project_name_when_basename_differs() {
        // basename(path) != project.name → "<basename> (<project.name>)".
        let project = Project {
            path: PathBuf::from("/home/u/zazzy-thunder"),
            name: "thunder_pkg".to_string(),
        };
        let mut out = String::new();
        render_title(&mut out, &project);
        let first_line = out.lines().next().expect("title line present");
        assert_eq!(first_line, "pycoati audit — zazzy-thunder (thunder_pkg)");
        // Underline matches the title's char-count.
        let underline = out.lines().nth(1).expect("underline line present");
        assert_eq!(underline.chars().count(), first_line.chars().count());
    }

    #[test]
    fn render_title_falls_back_to_project_name_when_path_has_no_basename() {
        // A root-like path with no file_name() falls back to the project name.
        let project = Project { path: PathBuf::from("/"), name: "demo".to_string() };
        let mut out = String::new();
        render_title(&mut out, &project);
        let first_line = out.lines().next().expect("title line present");
        assert_eq!(first_line, "pycoati audit — demo");
    }

    #[test]
    fn render_truncates_at_top_n() {
        let mut inv = empty_inv();
        let t1 = make_test("tests/a.py::t1", 0.9, 1, 0);
        let t2 = make_test("tests/a.py::t2", 0.8, 1, 0);
        let t3 = make_test("tests/a.py::t3", 0.7, 1, 0);
        // top_suspicious lists all three but caller asks for 2 — render
        // should only emit two data rows.
        inv.top_suspicious.test_functions =
            vec![t1.nodeid.clone(), t2.nodeid.clone(), t3.nodeid.clone()];
        inv.test_functions = vec![t1, t2, t3];
        let out = render(&inv, 2);
        assert!(out.contains("::t1"));
        assert!(out.contains("::t2"));
        assert!(!out.contains("::t3"), "row beyond top_n must not render, got:\n{out}");
    }
}
