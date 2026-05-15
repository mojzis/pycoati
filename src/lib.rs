//! pycoati library crate.
//!
//! Audits Python test suites: counts test functions and assertions per file,
//! and detects mock-API smells. This crate exposes the [`Inventory`] data
//! model and the [`run_static`] entry point used by the `pycoati` binary and
//! by integration tests.
//!
//! The output schema is at `schema_version = "2"` — `"1"` was the initial
//! Run-1/Run-2 shape; `"2"` renamed `tests[]` to `test_functions[]` and
//! `test_count` (per-file, per-sut-call, `top_suspicious`) to
//! `test_function_count` / `test_functions` to disambiguate the AST-level
//! function count from `suite.test_count` (pytest-collected, parametrize-
//! expanded). Every top-level field is always serialized; fields not yet
//! computed in the current run are populated with defaults (`null` / `0` /
//! `[]`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

pub mod coverage;
pub mod mock_api;
pub(crate) mod parser;
pub(crate) mod pretty;
pub mod pyproject;
pub mod pytest;
pub mod python_detect;
pub(crate) mod smells;
pub(crate) mod suspicion;
pub(crate) mod sut_calls;
pub mod walker;
pub mod workspace;

/// Top-level audit result for a CLI invocation.
///
/// Single-project inputs serialize as a bare [`Inventory`]; workspace
/// inputs serialize as a [`WorkspaceInventory`] wrapper carrying the
/// workspace-root path plus one inner `Inventory` per resolved member.
/// Downstream tooling discriminates by the presence of the
/// `workspace_root` key (a single-project payload never carries it).
// `AuditResult` is built once per CLI invocation, serialized, then dropped;
// it never sits in a hot collection. The `Inventory` variant being larger
// than the workspace wrapper is by design — boxing it would just trade a
// heap allocation for a noisier consumer API. Suppress the lint at the
// type definition rather than the call sites.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum AuditResult {
    Single(Inventory),
    Workspace(WorkspaceInventory),
}

/// Workspace-mode wrapper around a list of per-member [`Inventory`]s.
///
/// `schema_version` stays at `"2"` — the new shape is discriminated by
/// the presence of `workspace_root`, not by a version bump. `tool`
/// mirrors the single-project [`ToolInfo`] so consumers can read the
/// pycoati version and `ran_pytest` / `ran_coverage` flags off the
/// wrapper without descending into every member.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceInventory {
    pub schema_version: String,
    pub workspace_root: PathBuf,
    pub members: Vec<Inventory>,
    pub tool: ToolInfo,
}

/// Top-level audit result. Every field is always serialized (no
/// `skip_serializing_if`) so the on-the-wire shape is stable across runs.
#[derive(Debug, Clone, Serialize)]
pub struct Inventory {
    pub schema_version: String,
    pub project: Project,
    pub suite: Suite,
    pub files: Vec<FileRecord>,
    pub test_functions: Vec<TestRecord>,
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
    /// Number of `def test_*` functions parsed from this file via AST.
    /// Class-nested methods are counted; parametrize is **not** expanded —
    /// see `suite.test_count` for the pytest-collected item count.
    pub test_function_count: u64,
    pub assertion_count: u64,
    pub mock_construction_count: u64,
    pub patch_decorator_count: u64,
    /// Number of fixture-driven stub-API call sites (`monkeypatch.*`,
    /// `mocker.*` — see [`mock_api::STUB_HEADS`]) across every test body in
    /// this file. Feeds into the file-level `mock_overuse` smell alongside
    /// `mock_construction_count` and `patch_decorator_count`.
    pub stubs_count: u64,
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
    /// Number of fixture-driven stub-API call sites in this test's body.
    /// At per-test scope, `mock_overuse` consumes
    /// `(patch_decorator_count + stubs_count)` — body-level Mock-API
    /// constructions remain a file-scope signal only.
    pub stubs_count: u64,
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
    /// Number of test functions whose body invokes a call resolving to this name.
    pub test_function_count: u64,
    pub test_nodeids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopSuspicious {
    /// Nodeids of the top-N suspicious test functions (sorted by suspicion
    /// score). Each entry references one record in `test_functions[]`.
    pub test_functions: Vec<String>,
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
            name: "pycoati".to_string(),
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
            name: "pycoati".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            ran_pytest,
            ran_coverage,
        }
    }
}

/// Render the inventory as plain text. Thin pub wrapper over the crate-
/// private `pretty` module so binaries and integration tests can request the
/// pretty output without exposing the renderer internals.
pub fn render_pretty(inv: &Inventory, top_n: usize) -> String {
    pretty::render(inv, top_n)
}

/// Render a workspace audit as plain text. Mirrors [`render_pretty`] but
/// emits a workspace header followed by one per-member section.
pub fn render_pretty_workspace(ws: &WorkspaceInventory, top_n: usize) -> String {
    pretty::render_workspace(ws, top_n)
}

/// Build an empty file-record for a single .py file. Counts are filled in by
/// the caller after parsing.
fn empty_file_record(path: &Path) -> FileRecord {
    FileRecord {
        path: path.to_path_buf(),
        test_function_count: 0,
        assertion_count: 0,
        mock_construction_count: 0,
        patch_decorator_count: 0,
        stubs_count: 0,
        fixture_count: 0,
        smell_hits: Vec::new(),
    }
}

/// Build an empty inventory whose project metadata is already filled in.
/// Callers populate `files` and `test_functions`; everything else stays at
/// the default shape (every key present, dynamic fields null/empty).
fn empty_inventory(project: Project) -> Inventory {
    Inventory {
        schema_version: "2".to_string(),
        project,
        suite: Suite {
            test_count: None,
            runtime_seconds: None,
            line_coverage_pct: None,
            slowest_tests: Vec::new(),
        },
        files: Vec::new(),
        test_functions: Vec::new(),
        sut_calls: SutCalls { by_name: Vec::new(), top_called: Vec::new() },
        top_suspicious: TopSuspicious { test_functions: Vec::new(), files: Vec::new() },
        tool: ToolInfo::without_runtime(),
    }
}

/// Default `--top-suspicious N` when the CLI flag is not provided.
///
/// Mirrors the hardcoded `top_called` cap of 20 — both default lists are the
/// same length to make the JSON / pretty outputs easy to skim side-by-side.
pub const DEFAULT_TOP_SUSPICIOUS: usize = 20;

/// Public entry point.
///
/// Dispatches on whether the input path is a single Python file (Phase 1
/// single-file mode) or a project root directory (Phase 2 walker mode). The
/// default `<project_root>/tests` is used for test discovery; see
/// [`run_static_with_tests_dir`] to override.
pub fn run_static(path: &Path) -> Result<Inventory> {
    run_static_with_tests_dir(path, None)
}

/// Same as [`run_static_with_tests_dir`] but accepts an explicit project
/// package override. When `Some(name)`, the override replaces the
/// pyproject-detected package list for sut-call resolution.
///
/// Uses the [`DEFAULT_TOP_SUSPICIOUS`] cap for the `top_suspicious` lists.
/// Callers needing a custom cap use [`run_static_with_top_n`].
pub fn run_static_with_options(
    path: &Path,
    tests_dir_override: Option<&Path>,
    project_package_override: Option<&str>,
) -> Result<Inventory> {
    run_static_with_top_n(
        path,
        tests_dir_override,
        project_package_override,
        DEFAULT_TOP_SUSPICIOUS,
    )
}

/// Like [`run_static_with_options`] but with an explicit `top_n` cap.
///
/// Used by the CLI to honor `--top-suspicious N`; library callers normally
/// use the default-N variant.
///
/// **Bails when pointed at a uv-workspace root.** Callers that want
/// workspace-aware dispatch must go through [`run_audit_static`]; this
/// signature is reserved for the single-project / single-file paths so
/// existing library consumers (e.g. `tests/inventory_shape.rs`) keep
/// their `Inventory`-shaped return.
pub fn run_static_with_top_n(
    path: &Path,
    tests_dir_override: Option<&Path>,
    project_package_override: Option<&str>,
    top_n: usize,
) -> Result<Inventory> {
    if !path.exists() {
        anyhow::bail!("path does not exist: {}", path.display());
    }
    if path.is_dir() {
        // Workspace roots are off-limits to this entry point — the
        // return type can't represent a workspace wrapper.
        if let Some(_ws) = workspace::detect(path)? {
            anyhow::bail!(
                "{} is a uv workspace root; use run_audit_static for workspace-aware dispatch",
                path.display()
            );
        }
        run_static_dir_with_options(path, tests_dir_override, project_package_override, top_n, true)
    } else {
        if tests_dir_override.is_some() {
            anyhow::bail!(
                "--tests-dir is only meaningful with a project-directory input (got file: {})",
                path.display()
            );
        }
        run_static_file_with_options(path, project_package_override, top_n)
    }
}

/// Workspace-aware static entry point.
///
/// Dispatches on the input path:
/// - File → `Single(<existing single-file inventory>)`.
/// - Dir with `[tool.uv.workspace]` declared in its pyproject →
///   `Workspace(<wrapper with one Inventory per member>)`.
/// - Dir without `[tool.uv.workspace]` → `Single(<single-project inventory>)`,
///   with `strict_missing_tests = true` so a missing `tests/` produces a
///   hard error instead of a silent empty inventory.
///
/// `tests_dir_override` and `project_package_override` are incompatible
/// with workspace mode — passing them while pointing at a workspace
/// root surfaces a clear error before any work runs.
pub fn run_audit_static(
    path: &Path,
    tests_dir_override: Option<&Path>,
    project_package_override: Option<&str>,
    top_n: usize,
) -> Result<AuditResult> {
    if !path.exists() {
        anyhow::bail!("path does not exist: {}", path.display());
    }
    if !path.is_dir() {
        // File input: there is no workspace concept; reuse the existing
        // single-file path verbatim.
        if tests_dir_override.is_some() {
            anyhow::bail!(
                "--tests-dir is only meaningful with a project-directory input (got file: {})",
                path.display()
            );
        }
        let inv = run_static_file_with_options(path, project_package_override, top_n)?;
        return Ok(AuditResult::Single(inv));
    }

    if let Some(ws) = workspace::detect(path)? {
        if tests_dir_override.is_some() {
            anyhow::bail!(
                "--tests-dir is incompatible with a uv-workspace root ({}); set it per-member instead",
                path.display()
            );
        }
        if project_package_override.is_some() {
            anyhow::bail!(
                "--project-package is incompatible with a uv-workspace root ({}); each member's pyproject declares its own package",
                path.display()
            );
        }
        let wsi = run_workspace_static(&ws, top_n)?;
        return Ok(AuditResult::Workspace(wsi));
    }

    let inv = run_static_dir_with_options(
        path,
        tests_dir_override,
        project_package_override,
        top_n,
        true,
    )?;
    Ok(AuditResult::Single(inv))
}

/// Static-mode pass for every workspace member. Per-member, runs the
/// existing single-project pipeline with `strict_missing_tests = false`
/// so a member without `tests/` skips with a `warn!` rather than aborting
/// the whole workspace audit.
fn run_workspace_static(ws: &workspace::Workspace, top_n: usize) -> Result<WorkspaceInventory> {
    let mut members: Vec<Inventory> = Vec::with_capacity(ws.members.len());
    for member in &ws.members {
        let inv = run_static_dir_with_options(member, None, None, top_n, false)
            .with_context(|| format!("auditing workspace member {}", member.display()))?;
        members.push(inv);
    }
    Ok(WorkspaceInventory {
        schema_version: "2".to_string(),
        workspace_root: ws.root.clone(),
        members,
        tool: ToolInfo::without_runtime(),
    })
}

/// Full-fat entry point.
///
/// Runs the static pass, then invokes pytest for collection, durations,
/// and (optionally) coverage. The static pass is identical to
/// [`run_static_with_tests_dir`]; pytest invocations layer on top of its
/// output and degrade gracefully on subprocess failure.
///
/// `python_cmd` is the whitespace-split python command (`Some(["python"])`,
/// `Some(["uv", "run", "python"])` for `--python "uv run python"`, etc.).
/// The first token is the program; the rest are prepended to every
/// `-m pytest ...` invocation. Pass `None` to auto-detect the interpreter
/// (see [`python_detect::detect_python_cmd`]) — detection runs *after* the
/// static phase succeeds, so an invalid `project` path fails before any
/// auto-detect output is emitted.
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
    python_cmd: Option<&[String]>,
    pytest_args: &[String],
    no_coverage: bool,
    project_package_override: Option<&str>,
    top_n: usize,
) -> Result<Inventory> {
    // `run_static_with_top_n` already bails on workspace roots; this
    // propagates that contract to the pytest path.
    let mut inv =
        run_static_with_top_n(project, tests_dir_override, project_package_override, top_n)?;

    // Single-file input has no project-level pytest semantics; leave
    // `suite` and `tool` at their static defaults.
    if !project.is_dir() {
        return Ok(inv);
    }

    let detected;
    let resolved_cmd: &[String] = if let Some(cmd) = python_cmd {
        cmd
    } else {
        detected = python_detect::detect_python_cmd(&inv.project.path);
        &detected
    };
    let (program, extra_python_args) = split_python_cmd(resolved_cmd);
    let tests_dir =
        tests_dir_override.map_or_else(|| inv.project.path.join("tests"), Path::to_path_buf);
    let project_root = inv.project.path.clone();
    let pkg = project_package_override
        .map_or_else(|| default_cov_package_for(&inv.project.name), str::to_string);

    invoke_pytest_for_inventory(
        &mut inv,
        &program,
        &extra_python_args,
        &project_root,
        &tests_dir,
        &pkg,
        pytest_args,
        no_coverage,
    );
    Ok(inv)
}

/// Run the pytest collection/durations/coverage trio against an already-
/// resolved interpreter + cwd + tests dir, mutating `inv` with the
/// dynamic fields. Does NOT detect python — callers are expected to
/// resolve `program` / `extra_python_args` themselves (the single-
/// project path detects via `python_detect::detect_python_cmd`; the
/// workspace path detects once at the workspace root and reuses).
///
/// `project_package` is passed verbatim to `--cov=`; an empty string
/// suppresses the coverage subprocess with a `warn!`. Single-project
/// callers normalize their own default via [`default_cov_package_for`];
/// workspace callers read each member's pyproject.
#[allow(clippy::too_many_arguments)]
fn invoke_pytest_for_inventory(
    inv: &mut Inventory,
    program: &str,
    extra_python_args: &[String],
    subprocess_cwd: &Path,
    tests_dir: &Path,
    project_package: &str,
    pytest_args: &[String],
    no_coverage: bool,
) {
    // Preflight: probe `import pytest` against the resolved interpreter
    // before firing the three pytest subprocesses. When the import fails
    // we emit a single WARN naming the interpreter + a recovery hint and
    // proceed: static analysis still runs, and the dynamic fields stay
    // null in the emitted JSON. This is purely advisory — we do **not**
    // abort, because the static portion of the inventory is still useful.
    if !pytest::pytest_available(program, extra_python_args, subprocess_cwd) {
        let extras = extra_python_args.join(" ");
        let separator = if extras.is_empty() { "" } else { " " };
        tracing::warn!(
            "pytest unavailable in resolved python; try --python \"uv run python\" or .venv/bin/python (resolved: {program}{separator}{extras})"
        );
    }

    let collection =
        pytest::run_collection(program, extra_python_args, subprocess_cwd, tests_dir, pytest_args);
    let durations =
        pytest::run_durations(program, extra_python_args, subprocess_cwd, tests_dir, pytest_args);

    let ran_pytest = collection.test_count.is_some()
        || !durations.slowest_tests.is_empty()
        || durations.runtime_seconds.is_some();

    inv.suite.test_count = collection.test_count;
    inv.suite.runtime_seconds = durations.runtime_seconds;
    inv.suite.slowest_tests = durations.slowest_tests;

    let mut ran_coverage = false;
    if !no_coverage {
        if project_package.is_empty() {
            tracing::warn!("no project package name available; skipping coverage");
        } else {
            let cov = coverage::run_coverage(
                program,
                extra_python_args,
                subprocess_cwd,
                tests_dir,
                pytest_args,
                project_package,
            );
            inv.suite.line_coverage_pct = cov;
            ran_coverage = cov.is_some();
        }
    }

    inv.tool = ToolInfo::with_runtime(ran_pytest, ran_coverage);
}

/// Where to anchor pytest's subprocess cwd when auditing a uv workspace.
///
/// `Root` (the default) keeps cwd at the workspace root and passes the
/// tests dir + `--cov=<pkg>` relative to it. This works well when the
/// member packages are installed into the workspace's single `.venv`
/// and importable by their distribution name.
///
/// `Member` runs each member's pytest with cwd set to the member dir.
/// Use this when a member ships its own `conftest.py` / `pytest.ini`
/// that only makes sense when pytest is invoked from inside the member.
#[derive(Copy, Clone, Debug)]
pub enum MemberCwd {
    Root,
    Member,
}

/// Workspace-aware full-fat entry point.
///
/// Mirrors [`run_audit_static`] for the pytest-enabled path: dispatches
/// on file vs single-project-dir vs workspace-root, runs the three
/// pytest subprocesses once per member, and wraps the results in an
/// [`AuditResult`].
///
/// Python is detected **once** at the workspace root and reused for
/// every member — uv workspaces always share a single `.venv` at the
/// root, so per-member detection would waste work and risk picking up
/// an unrelated nested venv.
#[allow(clippy::too_many_arguments)]
pub fn run_audit_with_pytest(
    path: &Path,
    tests_dir_override: Option<&Path>,
    python_cmd: Option<&[String]>,
    pytest_args: &[String],
    no_coverage: bool,
    project_package_override: Option<&str>,
    top_n: usize,
    member_cwd: MemberCwd,
) -> Result<AuditResult> {
    if !path.exists() {
        anyhow::bail!("path does not exist: {}", path.display());
    }

    if !path.is_dir() {
        // File input: identical to the single-file pytest path (which is
        // a no-op past the static pass).
        let inv = run_with_pytest(
            path,
            tests_dir_override,
            python_cmd,
            pytest_args,
            no_coverage,
            project_package_override,
            top_n,
        )?;
        return Ok(AuditResult::Single(inv));
    }

    if let Some(ws) = workspace::detect(path)? {
        if tests_dir_override.is_some() {
            anyhow::bail!(
                "--tests-dir is incompatible with a uv-workspace root ({}); set it per-member instead",
                path.display()
            );
        }
        if project_package_override.is_some() {
            anyhow::bail!(
                "--project-package is incompatible with a uv-workspace root ({}); each member's pyproject declares its own package",
                path.display()
            );
        }
        let wsi = run_workspace_with_pytest(
            &ws,
            python_cmd,
            pytest_args,
            no_coverage,
            top_n,
            member_cwd,
        )?;
        return Ok(AuditResult::Workspace(wsi));
    }

    let inv = run_with_pytest(
        path,
        tests_dir_override,
        python_cmd,
        pytest_args,
        no_coverage,
        project_package_override,
        top_n,
    )?;
    Ok(AuditResult::Single(inv))
}

/// Run static + pytest for each workspace member, sharing one detected
/// python across the lot.
fn run_workspace_with_pytest(
    ws: &workspace::Workspace,
    python_cmd: Option<&[String]>,
    pytest_args: &[String],
    no_coverage: bool,
    top_n: usize,
    member_cwd: MemberCwd,
) -> Result<WorkspaceInventory> {
    // Detect python ONCE at the workspace root. The whole point of
    // workspace mode is that all members share the same interpreter.
    let detected;
    let resolved_cmd: &[String] = if let Some(cmd) = python_cmd {
        cmd
    } else {
        detected = python_detect::detect_python_cmd(&ws.root);
        &detected
    };
    let (program, extra_python_args) = split_python_cmd(resolved_cmd);

    let mut members: Vec<Inventory> = Vec::with_capacity(ws.members.len());
    let mut any_ran_pytest = false;
    let mut any_ran_coverage = false;
    for member in &ws.members {
        let mut inv = run_static_dir_with_options(member, None, None, top_n, false)
            .with_context(|| format!("auditing workspace member {}", member.display()))?;

        // Workspace member without a `tests/` dir was skipped with a warn
        // and a zero-test inventory. Don't fire pytest at a member that
        // has nothing to run.
        if !member.join("tests").is_dir() {
            members.push(inv);
            continue;
        }

        // Per-member cwd is either the workspace root (default) or the
        // member dir, per `--member-cwd`. The tests path is rendered
        // relative to whichever cwd we picked so pytest's
        // collection/durations/coverage all see the same paths.
        let (subprocess_cwd, tests_dir) = match member_cwd {
            MemberCwd::Root => (ws.root.clone(), member.join("tests")),
            MemberCwd::Member => (member.clone(), PathBuf::from("tests")),
        };

        // Each member declares its own importable package via its
        // pyproject; reuse the existing pyproject reader to find the
        // first declared package name.
        let pkg = member_cov_package(member);

        invoke_pytest_for_inventory(
            &mut inv,
            &program,
            &extra_python_args,
            &subprocess_cwd,
            &tests_dir,
            &pkg,
            pytest_args,
            no_coverage,
        );
        if inv.tool.ran_pytest {
            any_ran_pytest = true;
        }
        if inv.tool.ran_coverage {
            any_ran_coverage = true;
        }
        members.push(inv);
    }

    Ok(WorkspaceInventory {
        schema_version: "2".to_string(),
        workspace_root: ws.root.clone(),
        members,
        tool: ToolInfo::with_runtime(any_ran_pytest, any_ran_coverage),
    })
}

/// Pick the `--cov=<pkg>` argument for a workspace member from its
/// pyproject. Falls back to the member directory's basename when no
/// `[project].name` (or other declared package) is available, mirroring
/// the single-project default-derivation pattern.
fn member_cov_package(member: &Path) -> String {
    let packages = pyproject::read_project_packages(member).unwrap_or_default();
    if let Some(first) = packages.into_iter().next() {
        first
    } else {
        member.file_name().and_then(|n| n.to_str()).map_or_else(String::new, str::to_string)
    }
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
    run_static_with_options(path, tests_dir_override, None)
}

/// Single-file path: parse one .py file. Retained from Phase 1 so the
/// existing integration tests stay green and so callers can drive the
/// parser on synthetic inputs.
fn run_static_file_with_options(
    path: &Path,
    project_package_override: Option<&str>,
    top_n: usize,
) -> Result<Inventory> {
    let project = project_from_file(path);
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let parsed = parser::parse_python_file(&source, path)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let mut file_record = empty_file_record(path);
    file_record.test_function_count = parsed.test_functions.len() as u64;
    file_record.assertion_count = parsed.test_functions.iter().map(|t| t.assertion_count).sum();
    file_record.mock_construction_count = parsed.mock_construction_count;
    file_record.patch_decorator_count = parsed.patch_decorator_count;
    file_record.stubs_count = parsed.stubs_count;
    file_record.fixture_count = parsed.fixture_count;

    let mut inv = empty_inventory(project);
    let has_tests = !parsed.test_functions.is_empty();
    let mut test_functions = parsed.test_functions;

    // Build the single-file imports map keyed by the same path the parser
    // used to construct nodeids (so resolution finds it).
    let mut imports_per_file: std::collections::BTreeMap<PathBuf, sut_calls::ImportMap> =
        std::collections::BTreeMap::new();
    imports_per_file.insert(path.to_path_buf(), parsed.import_map);

    let project_packages = resolve_project_packages(&inv.project.path, project_package_override)?;
    let sut = sut_calls::aggregate(&mut test_functions, &imports_per_file, &project_packages);
    inv.sut_calls = sut;

    apply_smells(&mut file_record, &mut test_functions);

    if has_tests {
        inv.files.push(file_record);
    }
    inv.test_functions = test_functions;
    score_and_rank(&mut inv, top_n);
    Ok(inv)
}

/// Directory path: discover tests, parse each, aggregate. Per-file parse
/// failures are logged and the file is skipped — the run as a whole must
/// not abort just because one fixture is malformed.
///
/// `strict_missing_tests` toggles the missing-`tests/` behaviour:
/// - `true` (single-project default): a missing `tests_dir` is a hard
///   error. This is the new Run-4 contract — silently emitting an empty
///   inventory was masking misconfigured projects.
/// - `false` (workspace per-member): a missing `tests_dir` warns and
///   returns an empty inventory. The workspace wrapper still lists the
///   member; just with zero discovered tests.
fn run_static_dir_with_options(
    project_root: &Path,
    tests_dir_override: Option<&Path>,
    project_package_override: Option<&str>,
    top_n: usize,
    strict_missing_tests: bool,
) -> Result<Inventory> {
    let project = project_from_root(project_root);
    let mut inv = empty_inventory(project);

    let tests_dir =
        tests_dir_override.map_or_else(|| inv.project.path.join("tests"), Path::to_path_buf);

    if !tests_dir.exists() {
        if strict_missing_tests {
            anyhow::bail!(
                "no tests directory at {}; not a workspace either (no [tool.uv.workspace] in pyproject.toml)",
                tests_dir.display()
            );
        }
        tracing::warn!(
            tests_dir = %tests_dir.display(),
            "tests directory not found; emitting empty inventory"
        );
        return Ok(inv);
    }

    let files = walker::discover_test_files(&tests_dir)
        .with_context(|| format!("failed to walk {}", tests_dir.display()))?;

    // Symmetric counterpart to the "tests directory not found" WARN above:
    // when the dir exists but the walker returns zero `test_*.py` files,
    // emit a distinct WARN so the empty inventory is obviously a project
    // configuration issue rather than a pycoati bug. The non-empty path
    // logs an info-level breadcrumb for the same reason.
    if files.is_empty() {
        tracing::warn!(
            tests_dir = %tests_dir.display(),
            "no python test files under tests dir; emitting empty inventory"
        );
    } else {
        // Static message + structured fields, matching the WARN above —
        // the fmt subscriber renders both, so interpolating the same data
        // into the message would double-stamp it in user-visible output.
        tracing::info!(
            tests_dir = %tests_dir.display(),
            count = files.len(),
            "discovered python test files"
        );
    }

    // Map from the (relativised) parser file path to the file's import map,
    // matching the key used when resolving `TestRecord.file` later.
    let mut imports_per_file: std::collections::BTreeMap<PathBuf, sut_calls::ImportMap> =
        std::collections::BTreeMap::new();

    for file_path in &files {
        match parse_single_file(file_path, &inv.project.path) {
            Ok((file_record, tfs, import_map)) => {
                imports_per_file.insert(file_record.path.clone(), import_map);
                inv.files.push(file_record);
                inv.test_functions.extend(tfs);
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

    // Resolve called_names + aggregate into sut_calls.
    let project_packages = resolve_project_packages(&inv.project.path, project_package_override)?;
    inv.sut_calls =
        sut_calls::aggregate(&mut inv.test_functions, &imports_per_file, &project_packages);

    // Smells: derive per-test, then per-file (file pass uses already-
    // populated TestRecord references).
    let config = smells::MockSmellConfig::default();
    for t in &mut inv.test_functions {
        t.smell_hits.extend(smells::derive_test_smells(t, &config));
    }
    // Re-borrow tests by file path for file-level pass.
    for file in &mut inv.files {
        let tests_in_file: Vec<&TestRecord> =
            inv.test_functions.iter().filter(|t| t.file == file.path).collect();
        file.smell_hits.extend(smells::derive_file_smells(file, &tests_in_file, &config));
    }

    score_and_rank(&mut inv, top_n);
    Ok(inv)
}

/// Apply the suspicion-score pipeline: per-test score, per-file score, then
/// the top-N rankings. Shared between single-file and directory mode so both
/// paths produce a fully-populated `top_suspicious` block.
fn score_and_rank(inv: &mut Inventory, top_n: usize) {
    let weights = suspicion::DEFAULT;
    // Per-test scores: write back into the record so the JSON `suspicion_score`
    // field reflects the same number we sort on.
    for t in &mut inv.test_functions {
        t.suspicion_score = suspicion::score_test(t, &weights);
    }
    // Per-file scores: group test scores by file path (matching TestRecord.file),
    // then call `score_file` once per file.
    let mut file_scores: Vec<f64> = Vec::with_capacity(inv.files.len());
    for file in &inv.files {
        let scores: Vec<f64> = inv
            .test_functions
            .iter()
            .filter(|t| t.file == file.path)
            .map(|t| t.suspicion_score)
            .collect();
        file_scores.push(suspicion::score_file(file, &scores));
    }
    inv.top_suspicious.test_functions = suspicion::top_n_tests(&inv.test_functions, top_n);
    inv.top_suspicious.files = suspicion::top_n_files(&inv.files, &file_scores, top_n);
}

/// Determine the active project-package list. CLI override wins and skips
/// `pyproject.toml` reading entirely; otherwise we ask
/// [`pyproject::read_project_packages`].
fn resolve_project_packages(
    project_root: &Path,
    override_name: Option<&str>,
) -> Result<std::collections::BTreeSet<String>> {
    if let Some(name) = override_name {
        return Ok(std::iter::once(name.to_string()).collect());
    }
    let pkgs = pyproject::read_project_packages(project_root)?;
    Ok(pkgs.into_iter().collect())
}

/// Apply mock-smell derivation to a single file's records (used by the
/// single-file static entry point, which never has cross-file aggregation).
fn apply_smells(file: &mut FileRecord, tests: &mut [TestRecord]) {
    let config = smells::MockSmellConfig::default();
    for t in tests.iter_mut() {
        t.smell_hits.extend(smells::derive_test_smells(t, &config));
    }
    let tests_refs: Vec<&TestRecord> = tests.iter().collect();
    file.smell_hits.extend(smells::derive_file_smells(file, &tests_refs, &config));
}

/// Parse a single discovered file and produce its `FileRecord` plus
/// per-test records and the per-file [`sut_calls::ImportMap`] used by
/// Phase 2's sut-call resolver. Nodeids and `TestRecord.file` are
/// relativised against `project_root` so the output (and downstream map
/// keys) are portable across machines.
fn parse_single_file(
    file_path: &Path,
    project_root: &Path,
) -> Result<(FileRecord, Vec<TestRecord>, sut_calls::ImportMap)> {
    let source = std::fs::read_to_string(file_path)
        .with_context(|| format!("failed to read {}", file_path.display()))?;
    let rel = relativize(file_path, project_root);
    let parsed = parser::parse_python_file(&source, &rel)
        .with_context(|| format!("failed to parse {}", file_path.display()))?;

    // The parser builds nodeids using the path it was handed (here `rel`),
    // so no rewriting is needed.
    let assertion_count: u64 = parsed.test_functions.iter().map(|t| t.assertion_count).sum();
    let test_function_count = parsed.test_functions.len() as u64;

    let mut test_functions = parsed.test_functions;
    // Be tidy about line ordering inside a file so the aggregated
    // `test_functions` array is deterministic regardless of how the parser
    // walks the tree.
    test_functions.sort_by_key(|t| t.line);

    let mut record = empty_file_record(&rel);
    record.test_function_count = test_function_count;
    record.assertion_count = assertion_count;
    record.mock_construction_count = parsed.mock_construction_count;
    record.patch_decorator_count = parsed.patch_decorator_count;
    record.stubs_count = parsed.stubs_count;
    record.fixture_count = parsed.fixture_count;

    Ok((record, test_functions, parsed.import_map))
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

/// Normalize a project distribution name into the importable Python module
/// name that `pytest --cov=<NAME>` expects. Hyphens are never valid in Python
/// identifiers, so a hyphenated distribution name like `my-pkg` (which maps to
/// an importable `my_pkg` module by convention) would otherwise cause
/// `pytest-cov` to emit `module-not-imported`. This is **hyphens-only**: we
/// do not lowercase, strip dots, or perform full PEP 503 normalization —
/// YAGNI until a confirmed failing case demands it.
///
/// Only applied to the auto-derived default. An explicit
/// `--project-package <value>` override is passed verbatim, since the user
/// knows what they typed.
fn default_cov_package_for(project_name: &str) -> String {
    project_name.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::default_cov_package_for;

    #[test]
    fn default_cov_package_for_replaces_hyphens_with_underscores() {
        assert_eq!(default_cov_package_for("b3d-validate"), "b3d_validate");
        assert_eq!(default_cov_package_for("gh-project-monitor"), "gh_project_monitor");
    }

    #[test]
    fn default_cov_package_for_passes_through_already_valid_names() {
        assert_eq!(default_cov_package_for("already_fine"), "already_fine");
        assert_eq!(default_cov_package_for("myproj"), "myproj");
    }

    #[test]
    fn default_cov_package_for_does_not_lowercase_or_strip_dots() {
        // YAGNI guard: this helper is hyphens-only by design.
        assert_eq!(default_cov_package_for("MyPkg"), "MyPkg");
        assert_eq!(default_cov_package_for("ns.sub"), "ns.sub");
    }
}
