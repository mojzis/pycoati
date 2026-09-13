//! Helpers shared by the integration-test crates.
//!
//! Two things live here, both of which were duplicated before:
//!
//! - **Fixture staging.** Several tests need a writable copy of a checked-in
//!   fixture: `pytest-cov` writes `.coverage` and `.pytest_cache/` into the
//!   project root, so two tests pointed at the same source directory race on
//!   those files under a parallel `cargo test`, and the accept tests edit the
//!   fixture's Python source outright.
//! - **The pytest probe.** Tests that drive the pytest path self-skip when
//!   pytest is not importable. Keeping one probe means the skip condition and
//!   the interpreter override cannot drift apart between test crates.
//!
//! Cargo compiles this module separately into each test binary, so anything
//! one crate does not use is dead there. `#![allow(dead_code)]` at the top is
//! the standard way to keep that quiet.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

/// Absolute path to a repo-relative path.
pub fn fixture_path(rel: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(rel);
    p
}

/// Recursively copy a fixture tree into `dst`, skipping the build artifacts
/// pytest and coverage.py leave behind (a previous run may have left them in
/// the source tree).
pub fn copy_fixture_tree(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if matches!(name_str.as_ref(), ".coverage" | ".pytest_cache" | "__pycache__" | ".pycoati") {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        let ft = entry.file_type().expect("file type");
        if ft.is_dir() {
            std::fs::create_dir_all(&dst_path).expect("create dir in tempdir");
            copy_fixture_tree(&src_path, &dst_path);
        } else if ft.is_file() {
            std::fs::copy(&src_path, &dst_path).expect("copy fixture file");
        }
        // Symlinks / others: the fixture trees have none today; skip if met.
    }
}

/// Stage `tests/fixtures/<name>` in a fresh tempdir and return the
/// (`TempDir` guard, staged root) pair. Holding the guard alive keeps the
/// copy on disk; dropping it removes the copy plus anything pytest wrote
/// there.
pub fn stage_fixture(name: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join(name);
    std::fs::create_dir_all(&root).expect("create staged fixture root");
    copy_fixture_tree(&fixture_path(&format!("tests/fixtures/{name}")), &root);
    (tmp, root)
}

/// Whitespace-split a command-line string into program + args.
pub fn split_command(cmd: &str) -> Option<(String, Vec<String>)> {
    let mut tokens = cmd.split_whitespace();
    let prog = tokens.next()?.to_string();
    Some((prog, tokens.map(str::to_string).collect()))
}

/// Probe for pytest + pytest-cov availability using the given python command.
/// True iff `python -c 'import pytest, pytest_cov'` exits 0. We probe by
/// import rather than by `which pytest` because the command may be a
/// multi-token one like `uv run python`.
pub fn pytest_available(python_cmd: &str) -> bool {
    let Some((prog, args)) = split_command(python_cmd) else {
        return false;
    };
    StdCommand::new(&prog)
        .args(&args)
        .args(["-c", "import pytest, pytest_cov"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Python command for the integration tests. Honours `COATI_TEST_PYTHON`
/// (e.g. `"uv run python"`) so CI can wire in a venv; otherwise plain
/// `python`.
pub fn integration_python() -> String {
    std::env::var("COATI_TEST_PYTHON").unwrap_or_else(|_| "python".to_string())
}
