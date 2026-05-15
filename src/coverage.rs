//! Coverage subprocess and JSON parsing.
//!
//! Invokes `pytest --cov=<pkg> --cov-report=json:<tmp>` against the project
//! root, then deserializes the resulting report into a `serde_json::Value`
//! and extracts `totals.percent_covered`. Coverage.py 6.x and 7.x both write
//! this key; older shapes are handled by a `percent_covered_display`
//! fallback (parsed as `f64`).
//!
//! Failures degrade to `None` plus a `tracing::warn!` so the rest of the
//! inventory still serializes.

use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::NamedTempFile;

/// Run pytest with coverage and return the extracted `totals.percent_covered`.
///
/// Returns `None` on any failure (subprocess launch, non-readable tempfile,
/// JSON parse, missing keys). Callers leave `Suite.line_coverage_pct = None`
/// and `ToolInfo.ran_coverage = false` in that case.
pub fn run_coverage(
    program: &str,
    extra_python_args: &[String],
    project_root: &Path,
    tests_dir: &Path,
    pytest_args: &[String],
    package: &str,
) -> Option<f64> {
    let report_file = match NamedTempFile::new() {
        Ok(f) => f,
        Err(err) => {
            tracing::warn!(error = %err, "failed to create tempfile for coverage report");
            return None;
        }
    };

    let mut args: Vec<String> = extra_python_args.to_vec();
    // `-o addopts=` neutralises any `addopts = …` line in the project's
    // pytest.ini / pyproject.toml for this invocation — see the same
    // override in `pytest::run_collection` for the rationale. The coverage
    // pass is especially sensitive to inherited addopts because the
    // project's own `--cov=…` would overwrite the `--cov-report=json:…`
    // path that pycoati relies on for the report file.
    args.extend([
        "-m".into(),
        "pytest".into(),
        "-o".into(),
        "addopts=".into(),
        format!("--cov={package}"),
        format!("--cov-report=json:{}", report_file.path().display()),
        "-q".into(),
    ]);
    args.push(tests_dir.display().to_string());
    args.extend(pytest_args.iter().cloned());

    let output = Command::new(program).args(&args).current_dir(project_root).output();

    // Hoist exit code + stderr tail out of the debug-only branch so the
    // post-parse WARNs can name *why* coverage failed. Without this, the
    // user sees only `serde_json: EOF while parsing` and has no thread to
    // pull on; the pytest stderr is where the actionable error lives
    // (`coverage.py warning: No data was collected`, `ModuleNotFoundError`,
    // `pytest: error: argument --cov ...`, etc).
    let (exit_code, stderr_tail) = match output {
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let code = o.status.code().unwrap_or(-1);
            if !stderr.is_empty() {
                tracing::debug!(
                    label = "coverage",
                    exit_code = code,
                    stderr = %stderr,
                    "pytest coverage subprocess stderr"
                );
            }
            (code, tail_of_stderr(&stderr))
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to launch pytest for coverage");
            return None;
        }
    };

    let raw = match std::fs::read_to_string(report_file.path()) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(error = %err, "coverage JSON report was not written");
            return None;
        }
    };
    let value = match classify_raw_report(&raw) {
        ReportOutcome::Parsed(v) => v,
        ReportOutcome::Empty => {
            // pytest didn't write anything to the report path (the most
            // common shape: coverage.py refused to write because no data
            // was collected, or pytest blew up before the cov plugin's
            // session-finish hook ran). Surface the exit code + stderr
            // tail directly; do **not** let serde_json speak first with
            // "EOF while parsing".
            tracing::warn!(
                pytest_exit_code = exit_code,
                stderr_tail = %stderr_tail,
                "no coverage data produced (pytest exit={exit_code}, stderr: {stderr_tail})"
            );
            return None;
        }
        ReportOutcome::Malformed(err) => {
            // The report file existed and had bytes, but those bytes were
            // not valid JSON — same root cause for the user (pytest /
            // coverage misconfiguration) but a different proximate cause.
            // Surface the exit code + stderr tail as the headline; ship
            // the serde error as a `caused_by` field so we don't lose it,
            // but never as the primary message.
            tracing::warn!(
                pytest_exit_code = exit_code,
                stderr_tail = %stderr_tail,
                caused_by = %err,
                "no coverage data produced (pytest exit={exit_code}, stderr: {stderr_tail})"
            );
            return None;
        }
    };

    extract_percent_covered(&value).or_else(|| {
        tracing::warn!(
            "coverage JSON missing both totals.percent_covered and percent_covered_display"
        );
        None
    })
}

/// Result of inspecting the raw bytes coverage.py wrote (or didn't) to the
/// JSON report path. Split into three branches so the caller can emit a
/// distinct, structured WARN per failure mode without letting serde's
/// "EOF while parsing" wording leak into the headline.
#[derive(Debug)]
enum ReportOutcome {
    /// Bytes parsed cleanly into a JSON value.
    Parsed(Value),
    /// The file was missing/empty/whitespace-only — coverage.py refused
    /// to write because no data was collected, or pytest crashed before
    /// the cov plugin's session-finish hook ran.
    Empty,
    /// Bytes were present but not valid JSON. The serde error is carried
    /// so the caller can surface it as a `caused_by` field, never as the
    /// primary message.
    Malformed(serde_json::Error),
}

/// Inspect the raw bytes of a coverage report and classify the outcome.
/// Pure function so the empty vs malformed branches can be unit-tested
/// without spawning a subprocess.
fn classify_raw_report(raw: &str) -> ReportOutcome {
    if raw.trim().is_empty() {
        return ReportOutcome::Empty;
    }
    match serde_json::from_str(raw) {
        Ok(v) => ReportOutcome::Parsed(v),
        Err(err) => ReportOutcome::Malformed(err),
    }
}

/// Truncate `stderr` to the trailing 8 lines or 800 chars (whichever bound
/// trims more aggressively), preserving the **most recent** output — that's
/// where pytest/coverage.py print the actionable error. Returns an empty
/// string for empty input. Bytes-vs-chars: this works on UTF-8 chars,
/// not raw bytes, so the result is never split mid-codepoint.
fn tail_of_stderr(stderr: &str) -> String {
    let trimmed = stderr.trim_end();
    if trimmed.is_empty() {
        return String::new();
    }
    // Last-8-lines bound.
    let by_lines: String = {
        let lines: Vec<&str> = trimmed.lines().collect();
        let start = lines.len().saturating_sub(8);
        lines[start..].join("\n")
    };
    // Last-800-chars bound, applied to the line-trimmed text so we never
    // re-expand past the line budget.
    let by_chars: String = if by_lines.chars().count() <= 800 {
        by_lines
    } else {
        by_lines.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect()
    };
    by_chars
}

/// Defensively pull the top-level coverage % out of a coverage.py JSON
/// report. Tries `totals.percent_covered` (f64) first, then falls back to
/// `totals.percent_covered_display` (string → f64).
fn extract_percent_covered(value: &Value) -> Option<f64> {
    let totals = value.get("totals")?;
    if let Some(num) = totals.get("percent_covered").and_then(Value::as_f64) {
        return Some(num);
    }
    if let Some(s) = totals.get("percent_covered_display").and_then(Value::as_str) {
        if let Ok(n) = s.parse::<f64>() {
            return Some(n);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_percent_covered_from_canonical_shape() {
        let v = json!({
            "totals": {
                "percent_covered": 87.5,
                "covered_lines": 35,
                "num_statements": 40
            }
        });
        assert_eq!(extract_percent_covered(&v), Some(87.5));
    }

    #[test]
    fn falls_back_to_percent_covered_display_string() {
        let v = json!({
            "totals": {
                "percent_covered_display": "42.0"
            }
        });
        assert_eq!(extract_percent_covered(&v), Some(42.0));
    }

    #[test]
    fn returns_none_when_totals_missing() {
        let v = json!({"meta": {"version": "7.0"}});
        assert_eq!(extract_percent_covered(&v), None);
    }

    #[test]
    fn returns_none_when_neither_key_present() {
        let v = json!({"totals": {"covered_lines": 10}});
        assert_eq!(extract_percent_covered(&v), None);
    }

    #[test]
    fn classify_raw_report_treats_empty_string_as_empty() {
        assert!(matches!(classify_raw_report(""), ReportOutcome::Empty));
    }

    #[test]
    fn classify_raw_report_treats_whitespace_only_as_empty() {
        // pytest sometimes leaves the tempfile with a trailing newline only —
        // semantically the same as never being written.
        assert!(matches!(classify_raw_report("\n  \t \n"), ReportOutcome::Empty));
    }

    #[test]
    fn classify_raw_report_flags_non_json_bytes_as_malformed() {
        // Non-JSON bytes (e.g. a stray pytest error stream redirected into
        // the report path, or a partial coverage write) must trip the
        // Malformed branch so the caller's WARN can carry serde's error as
        // a `caused_by` field — never as the headline.
        let outcome = classify_raw_report("not valid json at all");
        match outcome {
            ReportOutcome::Malformed(err) => {
                let msg = err.to_string();
                assert!(!msg.is_empty(), "serde error must carry a message");
            }
            other => panic!("expected Malformed branch, got {other:?}"),
        }
    }

    #[test]
    fn classify_raw_report_flags_truncated_json_as_malformed() {
        // Real-world variant of malformed: pytest started writing the
        // report but died mid-flush. The bytes look like JSON up to a
        // point and then end abruptly — still the Malformed branch, not
        // Empty.
        assert!(matches!(
            classify_raw_report("{\"totals\": {\"percent_covered\":"),
            ReportOutcome::Malformed(_)
        ));
    }

    #[test]
    fn classify_raw_report_returns_parsed_value_for_valid_json() {
        // Sanity: the happy path round-trips the value so the caller can
        // hand it to `extract_percent_covered`.
        let raw = "{\"totals\":{\"percent_covered\":50.0}}";
        match classify_raw_report(raw) {
            ReportOutcome::Parsed(v) => {
                assert_eq!(extract_percent_covered(&v), Some(50.0));
            }
            other => panic!("expected Parsed branch, got {other:?}"),
        }
    }

    #[test]
    fn tail_of_stderr_returns_empty_for_empty_input() {
        assert_eq!(tail_of_stderr(""), "");
        assert_eq!(tail_of_stderr("\n\n  \n"), "");
    }

    #[test]
    fn tail_of_stderr_keeps_last_eight_lines() {
        let stderr = (1..=20).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let tail = tail_of_stderr(&stderr);
        let kept: Vec<&str> = tail.lines().collect();
        assert_eq!(kept.len(), 8, "expected last 8 lines, got {kept:?}");
        assert_eq!(kept[0], "line 13");
        assert_eq!(kept[7], "line 20");
    }

    #[test]
    fn tail_of_stderr_caps_at_800_chars_even_within_eight_lines() {
        // One enormous line; the char-budget must clip it.
        let line: String = "x".repeat(2_000);
        let tail = tail_of_stderr(&line);
        assert!(tail.chars().count() <= 800, "tail must not exceed 800 chars");
        assert!(tail.ends_with('x'));
    }
}
