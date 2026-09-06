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

    // Hoist exit code + stderr out of the debug-only branch so the
    // post-parse WARNs can name *why* coverage failed. Without this, the
    // user sees only `serde_json: EOF while parsing` and has no thread to
    // pull on; the pytest stderr is where the actionable error lives
    // (`coverage.py warning: No data was collected`, `ModuleNotFoundError`,
    // `pytest: error: unrecognized arguments: --cov ...`, etc). The full
    // stderr goes to the debug log; the WARN gets one line of it.
    let (exit_code, stderr) = match output {
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
            (code, stderr.into_owned())
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
            // was collected, pytest-cov is not installed, or pytest blew up
            // before the cov plugin's session-finish hook ran). Surface the
            // exit code + one line of stderr; do **not** let serde_json
            // speak first with "EOF while parsing".
            tracing::warn!("{}", coverage_failure_line(exit_code, &stderr));
            return None;
        }
        ReportOutcome::Malformed(err) => {
            // The report file existed and had bytes, but those bytes were
            // not valid JSON — same root cause for the user (pytest /
            // coverage misconfiguration) but a different proximate cause.
            // Surface the exit code + one line of stderr as the headline;
            // ship the serde error as a `caused_by` field so we don't lose
            // it, but never as the primary message.
            tracing::warn!(caused_by = %err, "{}", coverage_failure_line(exit_code, &stderr));
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

/// The one WARN line for a coverage pass that produced no report.
///
/// One line, because this fires on every scan of a repo without the plugin
/// and a multi-line argparse dump repeated per run trains people to stop
/// reading stderr. The full pytest stderr is already on the debug log.
/// The missing-plugin case is the one a reader can act on immediately, so
/// it gets its own wording rather than argparse's.
fn coverage_failure_line(exit_code: i32, stderr: &str) -> String {
    if stderr.contains("unrecognized arguments") && stderr.contains("--cov") {
        return format!(
            "no coverage data produced (pytest exit={exit_code}): pytest does not recognise \
             --cov; install pytest-cov (`uv add --dev pytest-cov`) or pass --no-coverage"
        );
    }
    match last_stderr_line(stderr) {
        Some(line) => format!("no coverage data produced (pytest exit={exit_code}): {line}"),
        None => format!("no coverage data produced (pytest exit={exit_code})"),
    }
}

/// The last non-blank line of `stderr`, clipped to 300 chars — that is where
/// pytest and coverage.py put the actionable error. `None` when there is
/// nothing to show.
fn last_stderr_line(stderr: &str) -> Option<String> {
    let line = stderr.lines().rev().map(str::trim).find(|l| !l.is_empty())?;
    let clipped: String = line.chars().take(300).collect();
    Some(clipped)
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
    fn missing_cov_plugin_is_named_in_one_line() {
        let stderr = "ERROR: usage: python -m pytest [options] [file_or_dir] [...]\n\
            python -m pytest: error: unrecognized arguments: --cov=demo --cov-report=json:/tmp/x\n\
            \x20 inifile: /p/pyproject.toml\n  rootdir: /p";
        let line = coverage_failure_line(4, stderr);
        assert_eq!(line.lines().count(), 1, "one line, got: {line}");
        assert!(line.contains("pytest-cov"), "should name the plugin to install: {line}");
        assert!(line.contains("--no-coverage"), "and the flag that skips coverage: {line}");
        assert!(!line.contains("inifile"), "no argparse dump: {line}");
    }

    #[test]
    fn other_failures_keep_the_exit_code_and_the_last_stderr_line() {
        let stderr = "some earlier noise\ncoverage.py warning: No data was collected\n\n";
        let line = coverage_failure_line(1, stderr);
        assert_eq!(line.lines().count(), 1, "one line, got: {line}");
        assert!(line.contains("no coverage data produced"), "keeps the headline: {line}");
        assert!(line.contains("pytest exit=1"), "names the exit code: {line}");
        assert!(line.contains("No data was collected"), "ends with the last line: {line}");
        assert!(!line.contains("earlier noise"), "only the last line: {line}");
    }

    #[test]
    fn empty_stderr_gives_just_the_headline() {
        let line = coverage_failure_line(2, "\n  \n");
        assert_eq!(line, "no coverage data produced (pytest exit=2)");
    }

    #[test]
    fn a_long_last_line_is_clipped() {
        let stderr = "x".repeat(2_000);
        let line = coverage_failure_line(1, &stderr);
        assert!(line.chars().count() <= 400, "must be clipped: {}", line.len());
    }
}
