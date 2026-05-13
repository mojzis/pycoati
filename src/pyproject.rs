//! Best-effort `pyproject.toml` reader.
//!
//! Phase 2 only consumes `[project].name`. Anything else — `[tool.*]`, build
//! backend metadata, package discovery — is deferred to later phases.

use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct PyProject {
    project: Option<Project>,
}

#[derive(Debug, Deserialize)]
struct Project {
    name: Option<String>,
}

/// Read `<project_root>/pyproject.toml` and return `[project].name` if
/// present.
///
/// This is intentionally `Option<String>` rather than `Result`: every failure
/// mode (file missing, malformed TOML, section/field absent, permission
/// denied) is treated as "name not available" so the caller can fall back to
/// the directory basename. Parse failures are logged at `warn` so genuine
/// breakage isn't silently swallowed.
pub fn read_project_name(project_root: &Path) -> Option<String> {
    let path = project_root.join("pyproject.toml");
    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(err) => {
            // ENOENT is the common case (no pyproject.toml at all) and not
            // worth a warning. Other errors are surfaced.
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "failed to read pyproject.toml; falling back"
                );
            }
            return None;
        }
    };

    match toml::from_str::<PyProject>(&source) {
        Ok(parsed) => parsed.project.and_then(|p| p.name),
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "failed to parse pyproject.toml; falling back"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_name_from_fixture_pyproject() {
        let mut root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        root.push("tests/fixtures/project");
        assert_eq!(read_project_name(&root), Some("myproj".to_string()));
    }

    #[test]
    fn missing_pyproject_returns_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(read_project_name(tmp.path()), None);
    }

    #[test]
    fn pyproject_without_project_section_returns_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("pyproject.toml"), "[build-system]\nrequires=[]\n")
            .expect("write fixture");
        assert_eq!(read_project_name(tmp.path()), None);
    }

    #[test]
    fn malformed_pyproject_returns_none_without_panicking() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("pyproject.toml"), "this is not = valid toml = [[[")
            .expect("write fixture");
        assert_eq!(read_project_name(tmp.path()), None);
    }
}
