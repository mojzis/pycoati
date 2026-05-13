//! coati library crate.
//!
//! Audits Python test suites: counts test functions and assertions per file,
//! and detects mock-API smells. This crate exposes the [`Inventory`] data
//! model and the [`run_static`] entry point used by the `coati` binary and
//! by integration tests.
//!
//! The output schema is locked at `schema_version = "1"`. Every top-level
//! field is always serialized; fields not yet computed in the current run
//! are populated with their Run-1 defaults (`null` / `0` / `[]`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

pub mod coverage;
pub mod mock_api;
pub mod parser;
pub mod pyproject;
pub mod pytest;
pub mod walker;

/// Top-level audit result. Every field is always serialized (no
/// `skip_serializing_if`) so the on-the-wire shape is stable across runs.
#[derive(Debug, Clone, Serialize)]
pub struct Inventory {
    pub schema_version: String,
    pub project: Project,
    pub suite: Suite,
    pub files: Vec<FileRecord>,
    pub tests: Vec<TestRecord>,
    pub sut_calls: SutCalls,
    pub top_suspicious: TopSuspicious,
    pub tool: ToolInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub path: PathBuf,
    pub name: String,
}

/// Suite-level dynamic metrics. Run 1 leaves these at their defaults
/// (pytest invocation lives in Run 2).
#[derive(Debug, Clone, Serialize)]
pub struct Suite {
    pub test_count: Option<u64>,
    pub runtime_seconds: Option<f64>,
    pub line_coverage_pct: Option<f64>,
    pub slowest_tests: Vec<SlowTest>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SlowTest {
    pub nodeid: String,
    pub seconds: f64,
}

/// One Python test-bearing file.
///
/// In directory (walker) mode, one record is emitted per discovered test
/// file even if it contains zero `test_*` functions — the file was on disk
/// and matched the naming convention, so the inventory acknowledges it.
///
/// In single-file mode, zero-test files are skipped (Phase 1 behaviour
/// retained for backwards compatibility with the original `--static-only`
/// single-file invocation).
#[derive(Debug, Clone, Serialize)]
pub struct FileRecord {
    pub path: PathBuf,
    pub test_count: u64,
    pub assertion_count: u64,
    pub mock_construction_count: u64,
    pub patch_decorator_count: u64,
    pub fixture_count: u64,
    pub smell_hits: Vec<SmellHit>,
}

/// One test function (`def test_*`).
#[derive(Debug, Clone, Serialize)]
pub struct TestRecord {
    pub nodeid: String,
    pub file: PathBuf,
    pub line: u64,
    pub assertion_count: u64,
    pub only_asserts_on_mock: bool,
    pub patch_decorator_count: u64,
    pub setup_to_assertion_ratio: f64,
    pub called_names: Vec<String>,
    pub smell_hits: Vec<SmellHit>,
    pub suspicion_score: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SmellHit {
    pub category: String,
    pub test: Option<String>,
    pub line: u64,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SutCalls {
    pub by_name: Vec<SutCallEntry>,
    pub top_called: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SutCallEntry {
    pub name: String,
    pub test_count: u64,
    pub test_nodeids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopSuspicious {
    pub tests: Vec<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub version: String,
    pub ran_pytest: bool,
    pub ran_coverage: bool,
}

impl ToolInfo {
    /// Defaults for a static-only run (no pytest, no coverage). Used by
    /// the static path and as the starting point for runs that flip
    /// `ran_pytest` / `ran_coverage` to `true` after a successful invocation.
    fn without_runtime() -> Self {
        Self {
            name: "coati".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            ran_pytest: false,
            ran_coverage: false,
        }
    }

    /// Variant for runs that successfully invoked pytest and/or coverage.
    /// The `ran_*` flags are wired through verbatim from the caller's
    /// observations — a failed subprocess leaves the corresponding flag
    /// `false` even when the user asked for the run.
    pub fn with_runtime(ran_pytest: bool, ran_coverage: bool) -> Self {
        Self {
            name: "coati".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            ran_pytest,
            ran_coverage,
        }
    }
}

/// Build an empty file-record for a single .py file. Counts are filled in by
/// the caller after parsing.
fn empty_file_record(path: &Path) -> FileRecord {
    FileRecord {
        path: path.to_path_buf(),
        test_count: 0,
        assertion_count: 0,
        mock_construction_count: 0,
        patch_decorator_count: 0,
        fixture_count: 0,
        smell_hits: Vec::new(),
    }
}

/// Build an empty inventory whose project metadata is already filled in.
/// Callers populate `files` and `tests`; everything else stays at the
/// Run-1 default shape (every key present, dynamic fields null/empty).
fn empty_inventory(project: Project) -> Inventory {
    Inventory {
        schema_version: "1".to_string(),
        project,
        suite: Suite {
            test_count: None,
            runtime_seconds: None,
            line_coverage_pct: None,
            slowest_tests: Vec::new(),
        },
        files: Vec::new(),
        tests: Vec::new(),
        sut_calls: SutCalls { by_name: Vec::new(), top_called: Vec::new() },
        top_suspicious: TopSuspicious { tests: Vec::new(), files: Vec::new() },
        tool: ToolInfo::without_runtime(),
    }
}

/// Public entry point.
///
/// Dispatches on whether the input path is a single Python file (Phase 1
/// single-file mode) or a project root directory (Phase 2 walker mode). The
/// default `<project_root>/tests` is used for test discovery; see
/// [`run_static_with_tests_dir`] to override.
pub fn run_static(path: &Path) -> Result<Inventory> {
    run_static_with_tests_dir(path, None)
}

/// Full-fat entry point.
///
/// Runs the static pass, then invokes pytest for collection, durations,
/// and (optionally) coverage. The static pass is identical to
/// [`run_static_with_tests_dir`]; pytest invocations layer on top of its
/// output and degrade gracefully on subprocess failure.
///
/// `python_cmd` is the whitespace-split python command (`["python"]` by
/// default, `["uv", "run", "python"]` for `--python "uv run python"`).
/// The first token is the program; the rest are prepended to every
/// `-m pytest ...` invocation.
///
/// `pytest_args` is appended to every pytest invocation (same no-shell-
/// expansion rule).
///
/// When `no_coverage` is true, the coverage subprocess is skipped and
/// `tool.ran_coverage` stays `false`.
///
/// `project_package_override` wins over the discovered `Inventory.project.name`
/// when picking the `--cov=<pkg>` argument. This matches the CLI's
/// `--project-package` flag.
pub fn run_with_pytest(
    project: &Path,
    tests_dir_override: Option<&Path>,
    python_cmd: &[String],
    pytest_args: &[String],
    no_coverage: bool,
    project_package_override: Option<&str>,
) -> Result<Inventory> {
    let mut inv = run_static_with_tests_dir(project, tests_dir_override)?;

    // Single-file input has no project-level pytest semantics; leave
    // `suite` and `tool` at their static defaults.
    if !project.is_dir() {
        return Ok(inv);
    }

    let (program, extra_python_args) = split_python_cmd(python_cmd);
    let tests_dir =
        tests_dir_override.map_or_else(|| inv.project.path.join("tests"), Path::to_path_buf);
    let project_root = inv.project.path.clone();

    let collection = pytest::run_collection(
        &program,
        &extra_python_args,
        &project_root,
        &tests_dir,
        pytest_args,
    );
    let durations =
        pytest::run_durations(&program, &extra_python_args, &project_root, &tests_dir, pytest_args);

    let ran_pytest = collection.test_count.is_some()
        || !durations.slowest_tests.is_empty()
        || durations.runtime_seconds.is_some();

    inv.suite.test_count = collection.test_count;
    inv.suite.runtime_seconds = durations.runtime_seconds;
    inv.suite.slowest_tests = durations.slowest_tests;

    let mut ran_coverage = false;
    if !no_coverage {
        let pkg = project_package_override.map_or_else(|| inv.project.name.clone(), str::to_string);
        if pkg.is_empty() {
            tracing::warn!("no project package name available; skipping coverage");
        } else {
            let cov = coverage::run_coverage(
                &program,
                &extra_python_args,
                &project_root,
                &tests_dir,
                pytest_args,
                &pkg,
            );
            inv.suite.line_coverage_pct = cov;
            ran_coverage = cov.is_some();
        }
    }

    inv.tool = ToolInfo::with_runtime(ran_pytest, ran_coverage);
    Ok(inv)
}

/// Split a whitespace-tokenised python command into program + extra args.
/// The default of `["python"]` produces `("python", [])`.
fn split_python_cmd(python_cmd: &[String]) -> (String, Vec<String>) {
    if python_cmd.is_empty() {
        return ("python".to_string(), Vec::new());
    }
    let program = python_cmd[0].clone();
    let extras = python_cmd[1..].to_vec();
    (program, extras)
}

/// Same as [`run_static`] but lets the caller override the tests directory.
/// Only meaningful in directory mode; passing a tests-dir override alongside
/// a single-file input is an error.
pub fn run_static_with_tests_dir(
    path: &Path,
    tests_dir_override: Option<&Path>,
) -> Result<Inventory> {
    if !path.exists() {
        anyhow::bail!("path does not exist: {}", path.display());
    }
    if path.is_dir() {
        run_static_dir(path, tests_dir_override)
    } else {
        if tests_dir_override.is_some() {
            anyhow::bail!(
                "--tests-dir is only meaningful with a project-directory input (got file: {})",
                path.display()
            );
        }
        run_static_file(path)
    }
}

/// Single-file path: parse one .py file. Retained from Phase 1 so the
/// existing integration tests stay green and so callers can drive the
/// parser on synthetic inputs.
fn run_static_file(path: &Path) -> Result<Inventory> {
    let project = project_from_file(path);
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let tests = parser::parse_python_file(&source, path)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let mut file_record = empty_file_record(path);
    file_record.test_count = tests.len() as u64;
    file_record.assertion_count = tests.iter().map(|t| t.assertion_count).sum();

    let mut inv = empty_inventory(project);
    if !tests.is_empty() {
        inv.files.push(file_record);
    }
    inv.tests = tests;
    Ok(inv)
}

/// Directory path: discover tests, parse each, aggregate. Per-file parse
/// failures are logged and the file is skipped — the run as a whole must
/// not abort just because one fixture is malformed.
fn run_static_dir(project_root: &Path, tests_dir_override: Option<&Path>) -> Result<Inventory> {
    let project = project_from_root(project_root);
    let mut inv = empty_inventory(project);

    let tests_dir =
        tests_dir_override.map_or_else(|| inv.project.path.join("tests"), Path::to_path_buf);

    if !tests_dir.exists() {
        tracing::warn!(
            tests_dir = %tests_dir.display(),
            "tests directory not found; emitting empty inventory"
        );
        return Ok(inv);
    }

    let files = walker::discover_test_files(&tests_dir)
        .with_context(|| format!("failed to walk {}", tests_dir.display()))?;

    for file_path in &files {
        match parse_single_file(file_path, &inv.project.path) {
            Ok((file_record, mut tests)) => {
                inv.files.push(file_record);
                inv.tests.append(&mut tests);
            }
            Err(err) => {
                tracing::warn!(
                    file = %file_path.display(),
                    error = %format!("{err:#}"),
                    "skipping test file due to parse error"
                );
            }
        }
    }
    Ok(inv)
}

/// Parse a single discovered file and produce its `FileRecord` plus
/// per-test records, with nodeids made relative to `project_root` so the
/// output is portable across machines.
fn parse_single_file(
    file_path: &Path,
    project_root: &Path,
) -> Result<(FileRecord, Vec<TestRecord>)> {
    let source = std::fs::read_to_string(file_path)
        .with_context(|| format!("failed to read {}", file_path.display()))?;
    let rel = relativize(file_path, project_root);
    let mut tests = parser::parse_python_file(&source, &rel)
        .with_context(|| format!("failed to parse {}", file_path.display()))?;

    // The parser builds nodeids using the path it was handed (here `rel`),
    // so no rewriting is needed.
    let assertion_count: u64 = tests.iter().map(|t| t.assertion_count).sum();
    let test_count = tests.len() as u64;

    // Be tidy about line ordering inside a file so the aggregated `tests`
    // array is deterministic regardless of how the parser walks the tree.
    tests.sort_by_key(|t| t.line);

    let mut record = empty_file_record(&rel);
    record.test_count = test_count;
    record.assertion_count = assertion_count;

    Ok((record, tests))
}

/// Make `path` relative to `base` for output. Falls back to `path` unchanged
/// when stripping fails (e.g. the walker handed back a path the user passed
/// absolutely from somewhere outside the canonicalized root).
fn relativize(path: &Path, base: &Path) -> PathBuf {
    path.strip_prefix(base).map_or_else(|_| path.to_path_buf(), Path::to_path_buf)
}

/// Derive a [`Project`] from a single-file input. The project `path` is the
/// directory containing the file, and `name` is that directory's basename.
///
/// Canonicalization is best-effort: if it fails (broken symlink, permission
/// denied, etc.) we log a warning via `tracing` and fall back to the raw
/// input path so `run_static` can still complete.
fn project_from_file(path: &Path) -> Project {
    let abs = canonicalize_or_warn(path);
    let dir = abs.parent().map_or_else(|| abs.clone(), Path::to_path_buf);
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("unknown").to_string();
    Project { path: dir, name }
}

/// Derive a [`Project`] from a project-root directory input. The name is
/// preferred from `pyproject.toml`'s `[project].name`; if absent, fall
/// back to the canonical directory basename.
fn project_from_root(root: &Path) -> Project {
    let abs = canonicalize_or_warn(root);
    let basename = abs.file_name().and_then(|n| n.to_str()).unwrap_or("unknown").to_string();
    let name = pyproject::read_project_name(&abs).unwrap_or(basename);
    Project { path: abs, name }
}

fn canonicalize_or_warn(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(abs) => abs,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "failed to canonicalize input path; falling back to non-canonical path"
            );
            path.to_path_buf()
        }
    }
}
