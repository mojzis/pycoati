//! coati library crate.
//!
//! Audits Python test suites: counts test functions and assertions per file,
//! and (in later phases) detects mock-API smells. This crate exposes the
//! [`Inventory`] data model and the [`run_static`] entry point used by the
//! `coati` binary and by integration tests.
//!
//! The output schema is locked at `schema_version = "1"`. Every top-level
//! field is always serialized; fields not yet computed in the current run
//! are populated with their Run-1 defaults (`null` / `0` / `[]`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

pub mod parser;

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
    fn run_1_default() -> Self {
        Self {
            name: "coati".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            ran_pytest: false,
            ran_coverage: false,
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

/// Public entry point. Parses a single Python file and returns an
/// [`Inventory`]. Directory inputs are rejected for now (Run 2 adds the
/// walker).
pub fn run_static(path: &Path) -> Result<Inventory> {
    if path.is_dir() {
        anyhow::bail!(
            "directory input not supported in this run; pass a single .py file (got {})",
            path.display()
        );
    }
    if !path.exists() {
        anyhow::bail!("path does not exist: {}", path.display());
    }

    let project = project_from_path(path);

    let source = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let tests = parser::parse_python_file(&source, path)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let mut file_record = empty_file_record(path);
    file_record.test_count = tests.len() as u64;
    file_record.assertion_count = tests.iter().map(|t| t.assertion_count).sum();

    let files = if tests.is_empty() { Vec::new() } else { vec![file_record] };

    Ok(Inventory {
        schema_version: "1".to_string(),
        project,
        suite: Suite {
            test_count: None,
            runtime_seconds: None,
            line_coverage_pct: None,
            slowest_tests: Vec::new(),
        },
        files,
        tests,
        sut_calls: SutCalls { by_name: Vec::new(), top_called: Vec::new() },
        top_suspicious: TopSuspicious { tests: Vec::new(), files: Vec::new() },
        tool: ToolInfo::run_1_default(),
    })
}

/// Derive a [`Project`] from a single-file input. The project `path` is the
/// directory containing the file, and `name` is that directory's basename.
/// Phase 2 will replace this with real `pyproject.toml` parsing on a project
/// root.
fn project_from_path(path: &Path) -> Project {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = abs.parent().map_or_else(|| abs.clone(), Path::to_path_buf);
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("unknown").to_string();
    Project { path: dir, name }
}
