//! Best-effort `pyproject.toml` reader.
//!
//! Phase 2 consumes both `[project].name` and the union of declared package
//! roots from the most common build-backend tables (Hatch, setuptools). The
//! existing `read_project_name` is left untouched — `lib.rs::project_from_root`
//! still uses it to populate `Project.name`.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
struct PyProject {
    project: Option<Project>,
    tool: Option<Tool>,
}

#[derive(Debug, Default, Deserialize)]
struct Project {
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Tool {
    hatch: Option<Hatch>,
    setuptools: Option<Setuptools>,
}

#[derive(Debug, Default, Deserialize)]
struct Hatch {
    build: Option<HatchBuild>,
}

#[derive(Debug, Default, Deserialize)]
struct HatchBuild {
    targets: Option<HatchTargets>,
}

#[derive(Debug, Default, Deserialize)]
struct HatchTargets {
    wheel: Option<HatchWheel>,
}

#[derive(Debug, Default, Deserialize)]
struct HatchWheel {
    packages: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct Setuptools {
    packages: Option<SetuptoolsPackages>,
}

/// Setuptools accepts two shapes for `[tool.setuptools].packages`:
/// - an explicit list of dotted package names (legacy);
/// - a `find` directive with `include`/`exclude` glob-ish lists.
///
/// We only consume the values verbatim — no glob expansion. Anything else
/// (e.g. `find = {}` with no lists) yields no package names from this branch.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SetuptoolsPackages {
    List(Vec<String>),
    Find {
        #[serde(default)]
        find: SetuptoolsFind,
    },
}

#[derive(Debug, Default, Deserialize)]
struct SetuptoolsFind {
    include: Option<Vec<String>>,
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
    let parsed = read_pyproject_silent(project_root)?;
    parsed.project.and_then(|p| p.name)
}

/// Return the union (deduped, sorted) of project package names declared in
/// `<project_root>/pyproject.toml`. Used by sut-call resolution to decide
/// which `called_names` are project-internal.
///
/// Sources, in order:
/// - `[project].name`
/// - `[tool.hatch.build.targets.wheel].packages` — entries are relative
///   paths (e.g. `src/myproj`); the project package name is the basename
///   of each path.
/// - `[tool.setuptools.packages.find].include` — values verbatim. We do
///   not expand glob patterns; a glob like `myproj*` would be passed
///   through and almost certainly not match any import head, which is
///   harmless.
/// - `[tool.setuptools].packages` (legacy explicit list)
///
/// If none of the above produces any entries (e.g. no `pyproject.toml`,
/// or the file declares nothing useful), falls back to the project root's
/// directory basename.
///
/// The `Result` return is a forward-compat hook: today every failure mode is
/// folded into the basename fallback, but downstream callers should not have
/// to change their signatures if we later surface real errors (e.g. an
/// invalid `--project-package` value, an unreadable `pyproject.toml`).
#[allow(clippy::unnecessary_wraps)]
pub fn read_project_packages(root: &Path) -> Result<Vec<String>> {
    let parsed = read_pyproject_silent(root);
    let mut packages: Vec<String> = Vec::new();

    if let Some(p) = parsed {
        if let Some(name) = p.project.and_then(|proj| proj.name) {
            packages.push(name);
        }
        if let Some(tool) = p.tool {
            if let Some(paths) = tool
                .hatch
                .and_then(|h| h.build)
                .and_then(|b| b.targets)
                .and_then(|t| t.wheel)
                .and_then(|w| w.packages)
            {
                for path in paths {
                    if let Some(base) = package_basename(&path) {
                        packages.push(base);
                    }
                }
            }
            if let Some(setup) = tool.setuptools {
                match setup.packages {
                    Some(SetuptoolsPackages::List(list)) => packages.extend(list),
                    Some(SetuptoolsPackages::Find { find }) => {
                        if let Some(include) = find.include {
                            packages.extend(include);
                        }
                    }
                    None => {}
                }
            }
        }
    }

    if packages.is_empty() {
        if let Some(base) = root.file_name().and_then(|n| n.to_str()) {
            packages.push(base.to_string());
        }
    }

    packages.sort();
    packages.dedup();
    Ok(packages)
}

/// Read + parse `pyproject.toml` without surfacing errors. Used by the
/// `Option`-returning public helpers. Missing file is silent; every other
/// failure logs at `warn` with enough context for a user to find the
/// offending key:
///
/// - I/O errors (permission denied, EISDIR, etc.) include the OS error message.
/// - TOML *syntax* errors include line/column from the `toml` crate.
/// - TOML *shape* errors (wrong type, e.g. `[project].name = 123`, or
///   `[tool.hatch.build.targets.wheel].packages = "not-a-list"`) include the
///   field path produced by `serde`, surfaced via the alternate `{err:#}`
///   formatter on `toml::de::Error`.
///
/// In every failure case the function returns `None` and the caller falls
/// back to the directory basename.
fn read_pyproject_silent(project_root: &Path) -> Option<PyProject> {
    let path = project_root.join("pyproject.toml");
    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(err) => {
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
        Ok(parsed) => Some(parsed),
        Err(err) => {
            // `{err:#}` (alternate Display) on `toml::de::Error` includes the
            // serde field path and the source snippet — without it the user
            // sees only "invalid type" with no clue which key is at fault.
            tracing::warn!(
                path = %path.display(),
                error = %format!("{err:#}"),
                "failed to parse pyproject.toml; falling back to basename"
            );
            None
        }
    }
}

/// Extract the basename of a relative path string. Used for Hatch's
/// `packages = ["src/myproj"]` shape → `"myproj"`.
fn package_basename(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let base = trimmed.rsplit('/').next()?;
    if base.is_empty() {
        None
    } else {
        Some(base.to_string())
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

    #[test]
    fn read_project_packages_returns_project_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("pyproject.toml"), "[project]\nname = \"myproj\"\n")
            .expect("write fixture");
        let pkgs = read_project_packages(tmp.path()).expect("ok");
        assert_eq!(pkgs, vec!["myproj".to_string()]);
    }

    #[test]
    fn read_project_packages_includes_hatch_wheel_packages() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("pyproject.toml"),
            "[project]\nname = \"a\"\n\n[tool.hatch.build.targets.wheel]\npackages = [\"src/b\"]\n",
        )
        .expect("write fixture");
        let pkgs = read_project_packages(tmp.path()).expect("ok");
        assert_eq!(pkgs, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn read_project_packages_includes_setuptools_packages() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("pyproject.toml"),
            "[project]\nname = \"a\"\n\n[tool.setuptools]\npackages = [\"b\", \"c\"]\n",
        )
        .expect("write fixture");
        let pkgs = read_project_packages(tmp.path()).expect("ok");
        assert_eq!(pkgs, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn read_project_packages_includes_setuptools_find_include() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("pyproject.toml"),
            "[project]\nname = \"a\"\n\n[tool.setuptools.packages.find]\ninclude = [\"b\", \"c\"]\n",
        )
        .expect("write fixture");
        let pkgs = read_project_packages(tmp.path()).expect("ok");
        assert_eq!(pkgs, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn read_project_packages_fallback_to_basename_when_no_pyproject() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pkgs = read_project_packages(tmp.path()).expect("ok");
        // Tempdir basename varies but must be non-empty.
        assert_eq!(pkgs.len(), 1);
        let expected = tmp.path().file_name().and_then(|n| n.to_str()).map(String::from).unwrap();
        assert_eq!(pkgs[0], expected);
    }

    #[test]
    fn malformed_pyproject_wrong_type_for_name_falls_back_without_panic() {
        // `[project].name = 123` is a serde shape error. The reader must not
        // panic, must not surface the error, and must let the package list
        // fall back to the project-root basename.
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("pyproject.toml"), "[project]\nname = 123\n")
            .expect("write fixture");
        assert_eq!(read_project_name(tmp.path()), None);
        let pkgs = read_project_packages(tmp.path()).expect("ok");
        // Basename fallback fires because no packages were extracted.
        let expected = tmp.path().file_name().and_then(|n| n.to_str()).map(String::from).unwrap();
        assert_eq!(pkgs, vec![expected]);
    }

    #[test]
    fn malformed_pyproject_wrong_type_for_hatch_packages_falls_back() {
        // `packages = "src/myproj"` (string, not list) is a serde shape error.
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("pyproject.toml"),
            "[project]\nname = \"a\"\n[tool.hatch.build.targets.wheel]\npackages = \"src/b\"\n",
        )
        .expect("write fixture");
        let pkgs = read_project_packages(tmp.path()).expect("ok");
        let expected = tmp.path().file_name().and_then(|n| n.to_str()).map(String::from).unwrap();
        // The whole file fails to parse → no packages extracted → basename
        // fallback (NOT "a", since the file failed to parse as a whole).
        assert_eq!(pkgs, vec![expected]);
    }

    #[test]
    fn read_project_packages_deduped_and_sorted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("pyproject.toml"),
            "[project]\nname = \"a\"\n\n[tool.hatch.build.targets.wheel]\npackages = [\"src/a\", \"src/c\"]\n\n[tool.setuptools]\npackages = [\"b\", \"a\"]\n",
        )
        .expect("write fixture");
        let pkgs = read_project_packages(tmp.path()).expect("ok");
        assert_eq!(pkgs, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }
}
