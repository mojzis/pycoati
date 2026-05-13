//! `coati` binary: runs a static audit of a Python test file and prints the
//! resulting [`coati::Inventory`] as JSON.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "coati", version, about, long_about = None)]
struct Cli {
    /// Path to a Python test file. Directory inputs are not yet supported.
    path: PathBuf,

    /// Write the JSON inventory to this file instead of stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Accepted for forward compatibility; Run 1 is implicitly static-only.
    #[arg(long)]
    static_only: bool,

    /// Accepted for forward compatibility; suspicion scoring lands in Run 3.
    #[arg(long, value_name = "N")]
    top_suspicious: Option<usize>,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
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
    // `--static-only` and `--top-suspicious` are accepted but have no effect
    // in Run 1. Silence dead-code warnings without dropping the fields.
    let _ = cli.static_only;
    let _ = cli.top_suspicious;

    let inventory = coati::run_static(&cli.path)?;
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
