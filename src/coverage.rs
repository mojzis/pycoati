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
    args.extend([
        "-m".into(),
        "pytest".into(),
        format!("--cov={package}"),
        format!("--cov-report=json:{}", report_file.path().display()),
        "-q".into(),
    ]);
    args.push(tests_dir.display().to_string());
    args.extend(pytest_args.iter().cloned());

    let output = Command::new(program).args(&args).current_dir(project_root).output();

    match output {
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            if !stderr.is_empty() {
                tracing::debug!(
                    label = "coverage",
                    exit_code = o.status.code().unwrap_or(-1),
                    stderr = %stderr,
                    "pytest coverage subprocess stderr"
                );
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to launch pytest for coverage");
            return None;
        }
    }

    let raw = match std::fs::read_to_string(report_file.path()) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(error = %err, "coverage JSON report was not written");
            return None;
        }
    };
    let value: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "coverage JSON report was malformed");
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
}
