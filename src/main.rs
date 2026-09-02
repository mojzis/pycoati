//! `pycoati` binary: runs an audit of a Python project (or single file)
//! and prints the resulting [`pycoati::Inventory`] as JSON or as an aligned-
//! column plain-text view (`--format pretty`).
//!
//! Run 2 makes `--static-only` load-bearing: by default `pycoati <path>`
//! now invokes `python -m pytest` three times against the project root
//! (collection, durations, coverage) and populates `suite.*`. Users opt
//! out with `--static-only`; `--no-coverage` skips only the coverage run.
//!
//! Run 3 adds `--format <json|pretty>` (default `json`). `pretty` writes the
//! human-readable view; `json` is the structured `Inventory` payload.
//!
//! Run 4 adds `pycoati guide [PAGE]`, which prints the workflow instructions
//! embedded in the binary (see [`pycoati::guide`]). The audit itself keeps its
//! bare-positional form — `pycoati <path>` is unchanged — so the subcommand is
//! declared alongside the scan arguments rather than replacing them.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

/// Output format selector for `--format`.
///
/// `Json` produces the structured `Inventory` payload (pretty-printed). The
/// JSON path is the default and the contract every downstream consumer
/// (Phase 2 of the workflow, automation) reads. `Pretty` is the aligned-
/// column terminal view; it is a human convenience and not load-bearing.
#[derive(Copy, Clone, Debug, ValueEnum)]
enum Format {
    /// Structured `Inventory` JSON (pretty-printed). Default.
    Json,
    /// Aligned-column plain-text view for terminals.
    Pretty,
}

#[derive(Parser, Debug)]
#[command(
    name = "pycoati",
    version,
    about,
    args_conflicts_with_subcommands = true,
    long_about = "Audit Python test suites for mock smells and suspicious tests.\n\n\
        pycoati walks a Python project (or a single file), parses every test with \
        tree-sitter, and — unless --static-only is passed — runs `python -m pytest` \
        three times to capture collection, durations, and coverage. By default the \
        Python interpreter is auto-detected: an ancestor `.venv/bin/python` wins, \
        else `uv run --no-sync python` if uv is installed, else bare `python`. \
        Override with `--python <cmd>`.\n\n\
        It emits an Inventory describing test functions, assertion counts, mock-API \
        smells, and a per-test suspicion score that flags tests likely exercising \
        mocks instead of production code. Output is JSON by default; use \
        --format pretty for an aligned terminal view.\n\n\
        `pycoati guide` prints the workflow instructions that ship with this \
        binary; they describe the inventory schema this exact version emits."
)]
struct Cli {
    /// Subcommand. When absent, pycoati runs an audit of `<PATH>`.
    #[command(subcommand)]
    command: Option<Command>,

    /// Path to a Python project root (directory) or, for single-file mode,
    /// a `.py` file. Required unless a subcommand is given.
    path: Option<PathBuf>,

    /// Write the output to this file instead of stdout. The format is
    /// determined by `--format`.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Override the tests directory. Defaults to `<path>/tests`. Only
    /// meaningful when `<path>` is a directory.
    #[arg(long, value_name = "PATH")]
    tests_dir: Option<PathBuf>,

    /// Skip every pytest subprocess and emit the static-only inventory.
    /// The pre-Run-2 behaviour; useful when pytest isn't available or
    /// when the runtime numbers aren't needed.
    #[arg(long)]
    static_only: bool,

    /// Cap the `top_suspicious` test and file lists at N entries. Default
    /// `20`; setting to `0` returns empty lists.
    ///
    /// Kept as `Option<usize>` so the absence of the flag is distinguishable
    /// from an explicit `--top-suspicious 20`; absent → `DEFAULT_TOP_SUSPICIOUS`.
    #[arg(long, value_name = "N")]
    top_suspicious: Option<usize>,

    /// Project package name to pass to `pytest --cov=<NAME>`. Defaults to
    /// the discovered `[project].name` in `pyproject.toml`, falling back
    /// to the project directory's basename. Hyphens in the discovered name
    /// are converted to underscores for the default (since `--cov=` needs
    /// an importable Python module name, and hyphens are never valid in
    /// Python identifiers — e.g. `my-pkg` becomes `--cov=my_pkg`).
    ///
    /// For unusual layouts (monorepos with no top-level package, or names
    /// that diverge from the distribution name by more than hyphens), pass
    /// `--project-package <module-name>` explicitly. The override is used
    /// verbatim — no normalization is applied.
    #[arg(long, value_name = "NAME")]
    project_package: Option<String>,

    /// Python command to invoke pytest under. Whitespace-split into
    /// program + args. `--python "uv run python"` runs
    /// `uv run python -m pytest …` (no shell expansion).
    ///
    /// When omitted, pycoati auto-detects: an ancestor `.venv/bin/python`
    /// wins, then `uv run --no-sync python` if `uv --version` succeeds,
    /// else bare `python`.
    #[arg(long, value_name = "CMD")]
    python: Option<String>,

    /// Extra arguments appended to every pytest invocation. Whitespace-
    /// split, no shell expansion.
    #[arg(long, value_name = "STR", default_value = "")]
    pytest_args: String,

    /// Skip only the coverage subprocess. Collection + durations still run.
    #[arg(long)]
    no_coverage: bool,

    /// Output format. `json` (default) emits the structured `Inventory`
    /// payload; `pretty` emits the aligned-column plain-text view.
    #[arg(long, value_enum, default_value_t = Format::Json)]
    format: Format,

    /// Where to anchor pytest's subprocess cwd when auditing a uv
    /// workspace. `root` (default) runs every member's pytest from the
    /// workspace root and addresses tests via `<member>/tests`.
    /// `member` runs each member's pytest with cwd set to the member
    /// dir so member-local `conftest.py` / `pytest.ini` files apply.
    ///
    /// No-op outside workspace mode — accepted for ergonomic reasons
    /// (so scripts can pass it unconditionally) but silently ignored.
    #[arg(long, value_enum, default_value_t = MemberCwd::Root, value_name = "WHERE")]
    member_cwd: MemberCwd,
}

/// Subcommands. Deliberately kept to one: everything else pycoati does is
/// the bare-positional audit, and moving that behind a `scan` verb would be a
/// breaking change to every existing invocation for no gain.
#[derive(Subcommand, Debug)]
enum Command {
    /// Print the workflow guide that ships with this binary.
    ///
    /// With a page argument, prints that page. Without one, picks the page
    /// from the state of the current directory: `analyze` when an
    /// `inventory.json` is present there, otherwise `setup`.
    Guide {
        /// Page to print. Omit to select by filesystem state: `analyze` when
        /// an `inventory.json` is present, otherwise `setup`. `remediate` is
        /// never auto-selected — ask for it by name.
        #[arg(value_enum)]
        page: Option<GuidePage>,
    },
}

/// `pycoati guide <PAGE>` selector. Mirrors `pycoati::guide::Page` one-to-one.
#[derive(Copy, Clone, Debug, ValueEnum)]
enum GuidePage {
    /// Configure pycoati and produce `inventory.json`.
    Setup,
    /// Inventory field reference and the anti-pattern taxonomy.
    Analyze,
    /// The remedy ladder and the human-approval gate.
    Remediate,
}

impl From<GuidePage> for pycoati::guide::Page {
    fn from(value: GuidePage) -> Self {
        match value {
            GuidePage::Setup => Self::Setup,
            GuidePage::Analyze => Self::Analyze,
            GuidePage::Remediate => Self::Remediate,
        }
    }
}

/// `--member-cwd` selector. Mirrors `pycoati::MemberCwd` one-to-one.
#[derive(Copy, Clone, Debug, ValueEnum)]
enum MemberCwd {
    Root,
    Member,
}

impl From<MemberCwd> for pycoati::MemberCwd {
    fn from(value: MemberCwd) -> Self {
        match value {
            MemberCwd::Root => Self::Root,
            MemberCwd::Member => Self::Member,
        }
    }
}

fn main() -> ExitCode {
    // Log to stderr so warnings and other diagnostics never corrupt the
    // JSON inventory written to stdout. The default writer is stdout, which
    // would interleave `tracing` output with the structured payload.
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // Intentionally bypass `tracing` for the user-facing failure
            // line: a `pycoati: <error>` line on stderr plus a non-zero exit
            // is the standard CLI contract and must not be affected by
            // `RUST_LOG` filtering.
            eprintln!("pycoati: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    if let Some(Command::Guide { page }) = &cli.command {
        return run_guide(page.map(Into::into));
    }
    run_audit(cli)
}

/// Print one guide page to stdout, verbatim and unadorned.
///
/// `page` is `None` for the bare `pycoati guide`, in which case the page is
/// chosen from the current directory's state. Detection is infallible by
/// design — an unreadable cwd yields the `setup` page rather than an error.
fn run_guide(page: Option<pycoati::guide::Page>) -> Result<()> {
    let page = page.unwrap_or_else(|| {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        pycoati::guide::detect_page(&cwd)
    });
    let mut stdout = io::stdout().lock();
    stdout.write_all(page.text().as_bytes()).context("failed to write guide page to stdout")?;
    Ok(())
}

fn run_audit(cli: &Cli) -> Result<()> {
    let Some(path) = cli.path.as_deref() else {
        anyhow::bail!(
            "a PATH is required (run `pycoati --help` for usage, or `pycoati guide` for the workflow)"
        );
    };
    let top_n = cli.top_suspicious.unwrap_or(pycoati::DEFAULT_TOP_SUSPICIOUS);

    let result = if cli.static_only {
        pycoati::run_audit_static(
            path,
            cli.tests_dir.as_deref(),
            cli.project_package.as_deref(),
            top_n,
        )?
    } else {
        let python_cmd: Option<Vec<String>> =
            cli.python.as_deref().map(|s| s.split_whitespace().map(str::to_string).collect());
        let pytest_args: Vec<String> =
            cli.pytest_args.split_whitespace().map(str::to_string).collect();
        pycoati::run_audit_with_pytest(
            path,
            cli.tests_dir.as_deref(),
            python_cmd.as_deref(),
            &pytest_args,
            cli.no_coverage,
            cli.project_package.as_deref(),
            top_n,
            cli.member_cwd.into(),
        )?
    };

    let mut payload = match (&result, cli.format) {
        (pycoati::AuditResult::Single(inv), Format::Json) => {
            serde_json::to_string_pretty(inv).context("failed to serialize inventory")?
        }
        (pycoati::AuditResult::Single(inv), Format::Pretty) => pycoati::render_pretty(inv, top_n),
        (pycoati::AuditResult::Workspace(ws), Format::Json) => {
            serde_json::to_string_pretty(ws).context("failed to serialize workspace inventory")?
        }
        (pycoati::AuditResult::Workspace(ws), Format::Pretty) => {
            pycoati::render_pretty_workspace(ws, top_n)
        }
    };

    // Guide footer. Gated on `Format::Pretty` before anything else, so the
    // JSON payload — stdout or `--output` file — stays byte-identical to a
    // build without this feature. `pretty` is a human view by construction,
    // so the footer rides along with it wherever it is written.
    if matches!(cli.format, Format::Pretty) && reports_candidates(&result) {
        payload.push_str(SCAN_FOOTER);
    }

    if let Some(out_path) = cli.output.as_ref() {
        // File output: no trailing newline (matches the Run-2 contract — the
        // file contents round-trip cleanly through `serde_json::from_str`).
        fs::write(out_path, &payload)
            .with_context(|| format!("failed to write {}", out_path.display()))?;
    } else {
        let mut stdout = io::stdout().lock();
        stdout.write_all(payload.as_bytes()).context("failed to write payload to stdout")?;
        stdout.write_all(b"\n").context("failed to write trailing newline")?;
    }
    Ok(())
}

/// Breadcrumb appended to human-format output that has candidates to inspect.
///
/// The leading newline separates it from the last rendered section; the
/// caller supplies any trailing newline, matching how the payload itself is
/// written.
const SCAN_FOOTER: &str = "\nnext: run `pycoati guide analyze`";

/// Whether this audit surfaced anything worth pointing the reader at.
///
/// `top_suspicious.test_functions` is the shortlist the `analyze` page tells
/// an agent to start from, so an empty one means there is nothing to send it
/// to. A workspace qualifies when any member does.
fn reports_candidates(result: &pycoati::AuditResult) -> bool {
    match result {
        pycoati::AuditResult::Single(inv) => !inv.top_suspicious.test_functions.is_empty(),
        pycoati::AuditResult::Workspace(ws) => {
            ws.members.iter().any(|m| !m.top_suspicious.test_functions.is_empty())
        }
    }
}
