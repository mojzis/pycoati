//! File-system walker for Python test discovery.
//!
//! Uses [`ignore::WalkBuilder`] so `.gitignore` and friends are respected for
//! free. Names must match the standard pytest conventions: `test_*.py` or
//! `*_test.py`. Discovery is recursive under the supplied root.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ignore::WalkBuilder;

/// Discover Python test files under `tests_dir`.
///
/// Matches files whose **basename** is `test_*.py` or `*_test.py`. Returns
/// paths sorted lexicographically so callers — and the resulting inventory —
/// observe a deterministic order regardless of filesystem iteration order.
///
/// Symlinks are not followed (the `ignore` crate's default). Hidden files
/// and gitignored files are skipped, matching ripgrep's defaults; this also
/// keeps `target/` and similar build directories out of the scan.
pub fn discover_test_files(tests_dir: &Path) -> Result<Vec<PathBuf>> {
    if !tests_dir.exists() {
        anyhow::bail!("tests directory does not exist: {}", tests_dir.display());
    }
    if !tests_dir.is_dir() {
        anyhow::bail!("tests path is not a directory: {}", tests_dir.display());
    }

    let mut out = Vec::new();
    let walker = WalkBuilder::new(tests_dir).build();
    for entry in walker {
        let entry = entry.with_context(|| {
            format!("failed to read directory entry under {}", tests_dir.display())
        })?;
        // The first entry is the root itself; we only care about files.
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.into_path();
        if is_test_file(&path) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// `true` iff the file's basename matches `test_*.py` or `*_test.py`. Bare
/// `test.py` or `_test.py` (no stem) do not match — pytest itself only
/// collects names following the `test_*` / `*_test` shape.
fn is_test_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let Some(stem) = name.strip_suffix(".py") else {
        return false;
    };
    if stem.is_empty() {
        return false;
    }
    // Require a non-empty remainder after stripping the prefix/suffix so that
    // `test_.py` and `_test.py` are not matched (mirrors pytest's rules).
    if let Some(rest) = stem.strip_prefix("test_") {
        if !rest.is_empty() {
            return true;
        }
    }
    if let Some(rest) = stem.strip_suffix("_test") {
        if !rest.is_empty() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn recognises_test_prefix_and_suffix() {
        assert!(is_test_file(Path::new("/foo/test_x.py")));
        assert!(is_test_file(Path::new("/foo/other_test.py")));
        assert!(is_test_file(Path::new("test_basic.py")));
    }

    #[test]
    fn rejects_non_test_filenames() {
        assert!(!is_test_file(Path::new("/foo/helpers.py")));
        assert!(!is_test_file(Path::new("/foo/conftest.py")));
        assert!(!is_test_file(Path::new("/foo/README.md")));
        assert!(!is_test_file(Path::new("/foo/test_x.rs")));
        // Degenerate stems: no character on either side of the marker.
        assert!(!is_test_file(Path::new("/foo/test_.py")));
        assert!(!is_test_file(Path::new("/foo/_test.py")));
    }

    #[test]
    fn discovers_fixture_project_test_files() {
        let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        root.push("tests/fixtures/project/tests");
        let files = discover_test_files(&root).expect("discover");
        let basenames: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_name().and_then(|s| s.to_str()).map(str::to_string))
            .collect();
        assert_eq!(
            basenames,
            vec![
                "other_test.py".to_string(),
                "test_mixed.py".to_string(),
                "test_mock_only.py".to_string(),
                "test_no_asserts.py".to_string(),
                "test_overmocked.py".to_string(),
                "test_real.py".to_string(),
                "test_uses_repo.py".to_string(),
            ],
            "discovery must be sorted and exclude helpers.py"
        );
    }

    #[test]
    fn missing_directory_is_an_error() {
        let result = discover_test_files(Path::new("/definitely/does/not/exist/for/pycoati"));
        assert!(result.is_err());
    }
}
