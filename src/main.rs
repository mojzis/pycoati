//! `coati` binary: runs an audit of a Python project (or single file)
//! and prints the resulting [`coati::Inventory`] as JSON.
//!
//! Run 2 makes `--static-only` load-bearing: by default `coati <path>`
//! now invokes `python -m pytest` three times against the project root
//! (collection, durations, coverage) and populates `suite.*`. Users opt
//! out with `--static-only`; `--no-coverage` skips only the coverage run.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "coati", version, about, long_about = None)]
struct Cli {
    /// Path to a Python project root (directory) or, for single-file mode,
    /// a `.py` file.
    path: PathBuf,

    /// Write the JSON inventory to this file instead of stdout.
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

    /// Accepted for forward compatibility; suspicion scoring lands in Run 3.
    #[arg(long, value_name = "N")]
    top_suspicious: Option<usize>,

    /// Project package name to pass to `pytest --cov=<NAME>`. Defaults to
    /// the discovered `[project].name` in `pyproject.toml`, falling back
    /// to the project directory's basename.
    #[arg(long, value_name = "NAME")]
    project_package: Option<String>,

    /// Python command to invoke pytest under. Whitespace-split into
    /// program + args. `--python "uv run python"` runs
    /// `uv run python -m pytest …` (no shell expansion).
    #[arg(long, value_name = "CMD", default_value = "python")]
    python: String,

    /// Extra arguments appended to every pytest invocation. Whitespace-
    /// split, no shell expansion.
    #[arg(long, value_name = "STR", default_value = "")]
    pytest_args: String,

    /// Skip only the coverage subprocess. Collection + durations still run.
    #[arg(long)]
    no_coverage: bool,
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
            // line: a `coati: <error>` line on stderr plus a non-zero exit
            // is the standard CLI contract and must not be affected by
            // `RUST_LOG` filtering.
            eprintln!("coati: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    // `--top-suspicious` is still a Run-3 stub.
    let _ = cli.top_suspicious;

    let inventory = if cli.static_only {
        coati::run_static_with_options(
            &cli.path,
            cli.tests_dir.as_deref(),
            cli.project_package.as_deref(),
        )?
    } else {
        let python_cmd: Vec<String> = cli.python.split_whitespace().map(str::to_string).collect();
        let pytest_args: Vec<String> =
            cli.pytest_args.split_whitespace().map(str::to_string).collect();
        coati::run_with_pytest(
            &cli.path,
            cli.tests_dir.as_deref(),
            &python_cmd,
            &pytest_args,
            cli.no_coverage,
            cli.project_package.as_deref(),
        )?
    };

    let json = serde_json::to_string_pretty(&inventory).context("failed to serialize inventory")?;

    if let Some(out_path) = cli.output.as_ref() {
        fs::write(out_path, &json)
            .with_context(|| format!("failed to write {}", out_path.display()))?;
    } else {
        let mut stdout = io::stdout().lock();
        stdout.write_all(json.as_bytes()).context("failed to write inventory to stdout")?;
        stdout.write_all(b"\n").context("failed to write trailing newline")?;
    }
    Ok(())
}
