//! Subprocess invocation of `pytest` and parsers for its stdout.
//!
//! Two flavours of invocation are exposed:
//!
//! * [`run_collection`] runs `pytest --collect-only -q` and parses the test
//!   count from the summary line (`<N> tests collected …`), falling back to
//!   nodeid line counting when the summary is missing.
//! * [`run_durations`] runs `pytest --durations=0 -q` and parses both the
//!   `slowest durations` section (for the per-test top-N) and the final
//!   `in <secs>s` summary (for `runtime_seconds`).
//!
//! Coverage runs live in [`crate::coverage`] because they own a tempfile and
//! a `serde_json::Value` shape that has shifted across `coverage.py`
//! versions.
//!
//! All callers degrade gracefully: a non-zero pytest exit, a failed
//! `Command::output()`, or unparseable stdout each produce `None`/empty data
//! and a `tracing::warn!` so the rest of the inventory still serializes.

use std::path::Path;
use std::process::Command;

use anyhow::Context;

use crate::SlowTest;

/// Top-N cap on `Suite.slowest_tests`. Hardcoded in Run 2; a future
/// `--slowest-tests` CLI flag is tracked in Run 3+.
pub const SLOWEST_TESTS_CAP: usize = 20;

/// Result of `pytest --collect-only -q`.
#[derive(Debug, Default)]
pub struct CollectionOutcome {
    pub test_count: Option<u64>,
}

/// Result of `pytest --durations=0 -q`.
#[derive(Debug, Default)]
pub struct DurationsOutcome {
    pub runtime_seconds: Option<f64>,
    pub slowest_tests: Vec<SlowTest>,
}

/// Invoke `pytest --collect-only -q <tests_dir>` and parse the test count.
///
/// Returns an outcome with `test_count = None` (rather than an `Err`) when
/// the subprocess fails or output is unparseable — the higher-level run
/// must keep emitting valid JSON.
pub fn run_collection(
    program: &str,
    extra_python_args: &[String],
    project_root: &Path,
    tests_dir: &Path,
    pytest_args: &[String],
) -> CollectionOutcome {
    let mut args: Vec<String> = extra_python_args.to_vec();
    args.extend(["-m".into(), "pytest".into(), "--collect-only".into(), "-q".into()]);
    args.push(tests_dir.display().to_string());
    args.extend(pytest_args.iter().cloned());

    let Some((stdout, _exit_code)) = run_pytest(program, &args, project_root, "collection") else {
        return CollectionOutcome::default();
    };

    let test_count = parse_collection_count(&stdout);
    if test_count.is_none() {
        tracing::warn!(
            "pytest --collect-only produced no parseable count; leaving test_count = null"
        );
    }
    CollectionOutcome { test_count }
}

/// Invoke `pytest --durations=0 -q <tests_dir>` and parse runtime + the
/// slowest-tests section.
pub fn run_durations(
    program: &str,
    extra_python_args: &[String],
    project_root: &Path,
    tests_dir: &Path,
    pytest_args: &[String],
) -> DurationsOutcome {
    let mut args: Vec<String> = extra_python_args.to_vec();
    // `--durations=0` asks pytest to report every test's duration. Recent
    // pytest (>=8) additionally hides any test whose duration is below
    // `--durations-min` (default 0.005s) even when `--durations=0` is set,
    // so we pin `--durations-min=0` to force all rows into the report. Both
    // flags are no-ops on older pytest versions that don't recognise the
    // min flag — pytest treats unknown CLI args as a config error and exits
    // 2; we already tolerate non-zero exit codes, but to be safe we use the
    // canonical post-7.x spelling that all currently-supported pytests
    // accept.
    args.extend([
        "-m".into(),
        "pytest".into(),
        "--durations=0".into(),
        "--durations-min=0".into(),
        "-q".into(),
    ]);
    args.push(tests_dir.display().to_string());
    args.extend(pytest_args.iter().cloned());

    let Some((stdout, _exit_code)) = run_pytest(program, &args, project_root, "durations") else {
        return DurationsOutcome::default();
    };

    let slowest_tests = parse_slowest_tests(&stdout, SLOWEST_TESTS_CAP);
    let runtime_seconds = parse_runtime_seconds(&stdout).or_else(|| {
        // Fall back to summing `call` durations when the summary line is
        // unparseable. We re-parse the full set (not capped) so the sum
        // reflects total wall time, not just the top-N.
        let calls = parse_slowest_tests(&stdout, usize::MAX);
        if calls.is_empty() {
            None
        } else {
            Some(calls.iter().map(|s| s.seconds).sum())
        }
    });

    if runtime_seconds.is_none() && slowest_tests.is_empty() {
        tracing::warn!(
            "pytest --durations=0 produced neither runtime nor slowest_tests; leaving null"
        );
    }
    DurationsOutcome { runtime_seconds, slowest_tests }
}

/// Spawn a pytest subprocess and capture stdout. Returns `None` on any
/// failure to launch or on a non-UTF-8 stdout. Non-zero exit codes are
/// **not** treated as failure here: pytest exits 5 when no tests collected
/// and 1/2 on test/collection failures, but the stdout still usually
/// contains the data we want to parse.
fn run_pytest(
    program: &str,
    args: &[String],
    project_root: &Path,
    label: &str,
) -> Option<(String, i32)> {
    let output = Command::new(program)
        .args(args)
        .current_dir(project_root)
        .output()
        .with_context(|| format!("failed to launch pytest for {label}"));

    let output = match output {
        Ok(o) => o,
        Err(err) => {
            tracing::warn!(error = %format!("{err:#}"), "pytest {label} subprocess failed to launch");
            return None;
        }
    };

    let stdout = match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(error = %err, "pytest {label} stdout was not valid UTF-8");
            return None;
        }
    };
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        tracing::debug!(
            label,
            exit_code = output.status.code().unwrap_or(-1),
            stderr = %stderr,
            "pytest subprocess stderr"
        );
    }
    Some((stdout, output.status.code().unwrap_or(-1)))
}

/// Parse the `<N> tests collected …` summary line emitted by
/// `pytest --collect-only -q`. Falls back to counting nodeid-shaped lines
/// (those containing `::` and not starting with whitespace or `=`) when the
/// summary line is absent.
fn parse_collection_count(stdout: &str) -> Option<u64> {
    for line in stdout.lines() {
        let line = line.trim();
        // Accept "<N> test collected" or "<N> tests collected", with optional trailing time.
        if let Some(rest) = line.strip_suffix(" collected") {
            if let Ok(n) = rest.split_whitespace().next()?.parse::<u64>() {
                return Some(n);
            }
        }
        if let Some(prefix) = line.split(" collected").next() {
            // Match "<N> tests collected in 0.05s" — second token after N is "test" or "tests".
            let mut toks = prefix.split_whitespace();
            if let (Some(num), Some(word)) = (toks.next(), toks.next()) {
                if (word == "test" || word == "tests") && toks.next().is_none() {
                    if let Ok(n) = num.parse::<u64>() {
                        return Some(n);
                    }
                }
            }
        }
    }
    // Fallback: count nodeid-shaped lines.
    let mut count: u64 = 0;
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        // Summary/separator/error lines start with `=`, `_`, or whitespace.
        let first = line.chars().next().unwrap_or(' ');
        if first.is_whitespace() || first == '=' || first == '_' {
            continue;
        }
        if line.contains("::") {
            count += 1;
        }
    }
    if count > 0 {
        Some(count)
    } else {
        None
    }
}

/// Parse the slowest-durations section of `pytest --durations=0 -q`,
/// extract `call`-stage lines only, sort descending by seconds, and cap.
fn parse_slowest_tests(stdout: &str, cap: usize) -> Vec<SlowTest> {
    let mut entries: Vec<SlowTest> = Vec::new();
    let mut in_section = false;
    for line in stdout.lines() {
        let lower = line.to_ascii_lowercase();
        if !in_section {
            if lower.contains("slowest durations") {
                in_section = true;
            }
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // A separator line of `=` or `-` characters marks the section's
        // closing fence (e.g. `========= N passed in 1.23s =========`).
        if trimmed.starts_with('=') {
            break;
        }
        if let Some(entry) = parse_duration_line(trimmed) {
            entries.push(entry);
        }
    }
    entries.sort_by(|a, b| b.seconds.partial_cmp(&a.seconds).unwrap_or(std::cmp::Ordering::Equal));
    entries.truncate(cap);
    entries
}

/// Parse a single duration line of the form `<float>s <stage>  <nodeid>`.
/// Only `call`-stage lines yield a `SlowTest`. Setup/teardown rows are
/// returned as `None` so the caller can ignore them without an extra pass.
fn parse_duration_line(line: &str) -> Option<SlowTest> {
    let mut toks = line.split_whitespace();
    let seconds_tok = toks.next()?;
    let seconds = seconds_tok.strip_suffix('s')?.parse::<f64>().ok()?;
    let stage = toks.next()?;
    if stage != "call" {
        return None;
    }
    let nodeid = toks.next()?.to_string();
    Some(SlowTest { nodeid, seconds })
}

/// Parse the final summary line for total runtime: `… in 12.30s` or
/// `… in 12.30s (0:00:12)`. Returns the first matching float.
fn parse_runtime_seconds(stdout: &str) -> Option<f64> {
    // Walk lines in reverse — the summary is at the end, and we want to
    // avoid accidentally matching an earlier `in 0.01s` from progress output.
    for line in stdout.lines().rev() {
        if let Some(secs) = extract_runtime_from_line(line) {
            return Some(secs);
        }
    }
    None
}

fn extract_runtime_from_line(line: &str) -> Option<f64> {
    // Find ` in ` and parse the following `<number>s` token.
    let mut search_from = 0usize;
    while let Some(pos) = line[search_from..].find(" in ") {
        let after = &line[search_from + pos + 4..];
        let tok = after.split_whitespace().next().unwrap_or("");
        if let Some(num) = tok.strip_suffix('s').and_then(|n| n.parse::<f64>().ok()) {
            return Some(num);
        }
        search_from += pos + 4;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLLECT_SAMPLE: &str = "tests/test_a.py::test_one
tests/test_a.py::test_two
tests/test_b.py::test_three

3 tests collected in 0.05s
";

    const DURATIONS_SAMPLE: &str =
        "============================= test session starts ==============================
collected 3 items

tests/test_a.py ..                                                       [ 66%]
tests/test_b.py .                                                        [100%]

============================= slowest durations =============================
0.50s call     tests/test_a.py::test_two
0.10s setup    tests/test_a.py::test_two
0.30s call     tests/test_a.py::test_one
0.05s teardown tests/test_a.py::test_one
0.20s call     tests/test_b.py::test_three
============================== 3 passed in 1.23s ==============================
";

    #[test]
    fn parses_count_from_summary_line() {
        let count = parse_collection_count(COLLECT_SAMPLE);
        assert_eq!(count, Some(3));
    }

    #[test]
    fn parses_count_when_summary_is_singular() {
        let stdout = "tests/test_a.py::test_only\n\n1 test collected in 0.01s\n";
        assert_eq!(parse_collection_count(stdout), Some(1));
    }

    #[test]
    fn falls_back_to_nodeid_line_count_when_summary_absent() {
        let stdout = "tests/test_a.py::test_one\ntests/test_a.py::test_two\n";
        assert_eq!(parse_collection_count(stdout), Some(2));
    }

    #[test]
    fn empty_output_yields_none() {
        assert_eq!(parse_collection_count(""), None);
    }

    #[test]
    fn parses_slowest_tests_in_descending_order_and_filters_to_call_stage() {
        let slowest = parse_slowest_tests(DURATIONS_SAMPLE, SLOWEST_TESTS_CAP);
        assert_eq!(slowest.len(), 3, "should keep only `call` rows");
        // Sorted descending by seconds: 0.50, 0.30, 0.20.
        assert!((slowest[0].seconds - 0.50).abs() < 1e-9);
        assert_eq!(slowest[0].nodeid, "tests/test_a.py::test_two");
        assert!((slowest[1].seconds - 0.30).abs() < 1e-9);
        assert_eq!(slowest[1].nodeid, "tests/test_a.py::test_one");
        assert!((slowest[2].seconds - 0.20).abs() < 1e-9);
        assert_eq!(slowest[2].nodeid, "tests/test_b.py::test_three");
    }

    #[test]
    fn cap_truncates_to_top_n() {
        let stdout = "============================= slowest durations =============================
0.50s call tests/a.py::t1
0.40s call tests/a.py::t2
0.30s call tests/a.py::t3
0.20s call tests/a.py::t4
========================= 4 passed in 1.40s =========================
";
        let slowest = parse_slowest_tests(stdout, 2);
        assert_eq!(slowest.len(), 2);
        assert_eq!(slowest[0].nodeid, "tests/a.py::t1");
        assert_eq!(slowest[1].nodeid, "tests/a.py::t2");
    }

    #[test]
    fn slowest_returns_empty_when_section_missing() {
        let stdout = "no slowest section here\n3 passed in 1.0s\n";
        assert_eq!(parse_slowest_tests(stdout, SLOWEST_TESTS_CAP).len(), 0);
    }

    #[test]
    fn parses_runtime_from_summary_line() {
        let secs = parse_runtime_seconds(DURATIONS_SAMPLE);
        assert_eq!(secs, Some(1.23));
    }

    #[test]
    fn parses_runtime_with_parenthesised_clock() {
        let stdout =
            "========================= 5 passed in 12.30s (0:00:12) =========================\n";
        assert_eq!(parse_runtime_seconds(stdout), Some(12.30));
    }

    #[test]
    fn parses_integer_runtime() {
        // pytest occasionally emits an integer when the run is sub-second
        // and durations are disabled; tolerate the trailing `s` only.
        let stdout = "========================= 1 passed in 0s =========================\n";
        assert_eq!(parse_runtime_seconds(stdout), Some(0.0));
    }

    #[test]
    fn runtime_falls_back_to_unparseable() {
        assert_eq!(parse_runtime_seconds("nothing matches here\n"), None);
    }
}
