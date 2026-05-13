//! Auto-detect which Python interpreter to invoke pytest under.
//!
//! Resolution order:
//! 1. Nearest ancestor `.venv/bin/python` (or `.venv\Scripts\python.exe` on
//!    Windows) starting from the project path.
//! 2. `uv run --no-sync python`, gated on a successful `uv --version` probe
//!    so a missing `uv` doesn't surface as an obscure subprocess error inside
//!    the pytest runner.
//! 3. Bare `python` on `PATH`.
//!
//! `--no-sync` is important: it tells `uv` not to mutate the project's lock
//! or env as a side effect of running pytest — coati is a read-only audit
//! tool and must not nudge the user's dependencies.
//!
//! The user can always bypass this with `--python <cmd>`. Detection logs the
//! chosen interpreter at `debug` (not `info`) so the default-verbosity run
//! stays quiet on the happy path. Run with `RUST_LOG=coati::python_detect=debug`
//! to see which interpreter was picked.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolve a Python command (program + extra args) suitable for `python -m pytest`.
///
/// `project` is the user-supplied path (file or directory); detection walks
/// up from the directory itself (or the file's parent) to find an ancestor
/// `.venv`.
pub fn detect_python_cmd(project: &Path) -> Vec<String> {
    let start = starting_dir(project);
    if let Some(venv_python) = find_venv_python(&start) {
        let path_str = venv_python.display().to_string();
        tracing::debug!(python = %path_str, "auto-detected .venv python");
        return vec![path_str];
    }
    if uv_available() {
        tracing::debug!("auto-detected uv; using `uv run --no-sync python`");
        return vec!["uv".into(), "run".into(), "--no-sync".into(), "python".into()];
    }
    tracing::debug!("no .venv or uv found; falling back to bare `python`");
    vec!["python".into()]
}

fn starting_dir(project: &Path) -> PathBuf {
    if project.is_dir() {
        project.to_path_buf()
    } else {
        project.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    }
}

/// Walk up from `start` looking for the platform-appropriate venv python.
/// Returns the first hit (nearest wins), or `None` if no ancestor has a
/// `.venv` at all.
fn find_venv_python(start: &Path) -> Option<PathBuf> {
    let rel = venv_python_rel();
    let mut current: Option<&Path> = Some(start);
    while let Some(dir) = current {
        let candidate = dir.join(&rel);
        if candidate.is_file() {
            return Some(candidate);
        }
        current = dir.parent();
    }
    None
}

#[cfg(windows)]
fn venv_python_rel() -> PathBuf {
    PathBuf::from(".venv").join("Scripts").join("python.exe")
}

#[cfg(not(windows))]
fn venv_python_rel() -> PathBuf {
    PathBuf::from(".venv").join("bin").join("python")
}

fn uv_available() -> bool {
    Command::new("uv").arg("--version").output().is_ok_and(|o| o.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_venv_python(dir: &Path) -> PathBuf {
        let py = dir.join(venv_python_rel());
        fs::create_dir_all(py.parent().expect("parent")).expect("mkdir");
        fs::write(&py, b"").expect("touch");
        py
    }

    #[test]
    fn finds_venv_in_starting_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let expected = create_venv_python(tmp.path());
        let found = find_venv_python(tmp.path()).expect("found");
        assert_eq!(found, expected);
    }

    #[test]
    fn walks_up_to_find_ancestor_venv() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let nested = tmp.path().join("a/b/c");
        fs::create_dir_all(&nested).expect("mkdir nested");
        let expected = create_venv_python(tmp.path());
        let found = find_venv_python(&nested).expect("found");
        assert_eq!(found, expected);
    }

    #[test]
    fn nearest_venv_wins_over_ancestor() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outer = create_venv_python(tmp.path());
        let inner_dir = tmp.path().join("sub");
        fs::create_dir_all(&inner_dir).expect("mkdir inner");
        let inner = create_venv_python(&inner_dir);
        let found = find_venv_python(&inner_dir).expect("found");
        assert_eq!(found, inner);
        assert!(outer.is_file(), "outer venv should still exist on disk");
    }

    #[test]
    fn returns_none_when_no_venv_along_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let nested = tmp.path().join("a/b");
        fs::create_dir_all(&nested).expect("mkdir nested");
        // Relies on no `.venv` existing anywhere above the system tempdir,
        // which is true on standard Linux/macOS/Windows CI runners.
        assert!(find_venv_python(&nested).is_none());
    }

    #[test]
    fn starting_dir_for_file_uses_parent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("foo.py");
        fs::write(&file, b"").expect("write");
        assert_eq!(starting_dir(&file), tmp.path());
    }

    #[test]
    fn starting_dir_for_directory_uses_itself() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(starting_dir(tmp.path()), tmp.path());
    }

    #[test]
    fn detect_falls_back_to_venv_when_present() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let expected = create_venv_python(tmp.path());
        let cmd = detect_python_cmd(tmp.path());
        assert_eq!(cmd, vec![expected.display().to_string()]);
    }
}
