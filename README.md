# pycoati

Audits Python test suites for mock smells and suspicious tests. Walks a project
with tree-sitter, optionally runs `pytest` for collection, durations, and
coverage, and emits a structured inventory: per-file and per-test counts,
mock-API smells, and a per-test suspicion score.

📖 **[Documentation](https://mojzis.github.io/pycoati/)** · part of **[aesop](https://mojzis.github.io/aesop/)**, a small menagerie of tools for agentic Python

## Install

```bash
# From source (Rust)
cargo install --path .

# Or as a Python package (via maturin)
maturin develop
```

## Usage

```bash
pycoati --help
```

### `pycoati <PATH>`

Audit a project directory or a single `.py` file. Writes the inventory to
stdout as JSON unless `--output` is given.

```bash
# Structured inventory to a file
pycoati . --output inventory.json

# Aligned-column terminal view
pycoati . --format pretty

# Static analysis only, no pytest subprocesses
pycoati . --static-only
```

Key flags: `--format <json|pretty>`, `--output <PATH>`, `--tests-dir <PATH>`,
`--static-only`, `--no-coverage`, `--top-suspicious <N>`,
`--project-package <NAME>`, `--python <CMD>`, `--pytest-args <STR>`,
`--member-cwd <root|member>`, `--accept-file <PATH>`, `--no-accept`,
`--include-accepted`. See `pycoati --help` for the full list.

Weights behind `suspicion_score` are documented in [WEIGHTS.md](WEIGHTS.md).

### Accepted findings

An audit that runs on a schedule keeps re-surfacing the same legitimate tests.
Record the review decision once, with a reason, in `.pycoati-accept.toml` at
the project root (a single-file scan walks up from the file to find it):

```toml
schema_version = "1"

[[accept]]
test = "tests/test_packaging.py::test_wheel_installs"
signals = ["zero_asserts", "high_setup_ratio"]
reason = "assertions run in a child interpreter; the parent propagates failure via subprocess.run(check=True)"
reviewed = "2026-09-12"
fingerprint = "3f0a1c7d9b2e4a56"   # optional: lapses the entry if the test is edited
```

`reason` is required — an entry without one is a parse error. Acceptance is
scoped to one test and one named signal (`mock_only_assertions`,
`mock_overuse`, `zero_asserts`, `high_setup_ratio`); there is no wildcard, so
a new finding on an accepted test still surfaces.

The accepted test keeps running and keeps counting: pytest arguments are
untouched, so `suite.test_count`, runtime and coverage are unchanged, and every
count, smell hit and score stays in the inventory as measured. The only effect
is that a test whose *every* active signal is accepted is held back from
`top_suspicious.test_functions`. The decisions and their reasons are reported
under `accepted.findings`, entries that no longer apply under `accepted.stale`,
and `--include-accepted` puts everything back on the shortlist for a full
report.

Run `pycoati guide setup` for the full reference.

### `pycoati plan <ROOT> [MEMBER]...`

Plans how to run a monorepo's pytest suites in parallel: how many pytest
processes are worth starting side by side, and which subprojects share one.
It runs no tests — it packs the subprojects you name into lanes and reports
the trade-off.

```bash
# Measure once (the audit records each member's suite runtime)…
pycoati . --no-coverage --output inventory.json

# …then plan the subprojects you are about to run
pycoati plan . api core web --timings inventory.json --max-jobs 8

# No timings yet: balance on test counts instead
pycoati plan . --format pretty
```

Subprojects are the members of a uv workspace (all of them when none is
named), or — in any other monorepo — the directories you list. Each one stays
its own pytest invocation; a lane runs its subprojects one after another, and
the plan's wall time is its longest lane.

The recommendation is the **fewest** lanes whose wall time is within
`--tolerance` percent (default 10) of the best packing found up to
`--max-jobs`, so a lane that buys nothing is never suggested. The payload
carries the data behind it: per-subproject cost and where it came from
(`measured`, `estimated`, `test_count`), the wall time and speedup at every
lane count, the bottleneck subproject that caps the speedup, and for each lane
of the recommended plan the exact commands to run.

Key flags: `--timings <FILE>`, `--max-jobs <N>`, `--tolerance <PCT>`,
`--startup-seconds <SECS>`, `--member-cwd <root|member>`,
`--pytest-cmd <CMD>`, `--format <json|pretty>`, `--output <PATH>`. Run
`pycoati guide plan` for the field reference.

### `pycoati guide [PAGE]`

Prints the workflow instructions that ship inside the binary, aimed at a coding
agent working through an audit. Because the pages are embedded at compile time,
they always describe the inventory schema this exact build emits.

```bash
# Pick the page from the state of the current directory
pycoati guide

# Or ask for one by name
pycoati guide setup
pycoati guide analyze
pycoati guide remediate
pycoati guide plan
```

| Page | Contents |
|---|---|
| `setup` | Configuration surface, how to run the scan, where `inventory.json` lands. |
| `analyze` | Field reference for every key in the inventory, plus the anti-pattern taxonomy and its detection heuristics. |
| `remediate` | The remedy ladder, the human-approval gate, and the "do not" list. |
| `plan` | `pycoati plan`: its cost model, flags, the plan payload's fields, and how to execute a plan. |

With no argument, the page is chosen from the current directory: `analyze` when
an `inventory.json` is present there, otherwise `setup`. `remediate` is never
auto-selected — reach it via the `analyze` page's breadcrumb or by name — and
neither is `plan`, which sits outside the audit workflow.

The subcommands shadow directories of the same name: if you have a `./guide`
or `./plan` directory to audit, write `pycoati ./guide` or `pycoati ./plan`.

Each page ends with a single `next:` line, so an agent is always one command
away from the right instructions. A `--format pretty` scan that finds candidates
ends with the same breadcrumb; JSON output never carries it.

The page sources are plain markdown under [`docs/src/guide/`](docs/src/guide), embedded
via `include_str!`. `tests/guide_schema_drift.rs` asserts that every field name
serde emits is documented — inventory fields on the `analyze` page, plan fields
on the `plan` page — so a schema change cannot land without the guide being
updated alongside it.

## For your CLAUDE.md

<!-- BEGIN SHARED:claude-snippet -->
```markdown
### `pycoati` — test-suite audit

- Run `pycoati . --output inventory.json` now and then (not every commit), then
  `pycoati guide analyze`: it ranks the tests most likely to be weak (mock-only
  assertions, patch stacks, no assertions) and gives a remediation ladder.
- Ask before changing a test it flags. Keep a legitimate one with an entry in
  `.pycoati-accept.toml` (a `reason` is required).
- Monorepo: `pycoati plan . --timings inventory.json` says how many pytest
  processes to run side by side, and which subprojects go in each.
```
<!-- END SHARED:claude-snippet -->

## Development

```bash
# Pre-commit checks
cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features

# Full review (fmt, clippy, tests, audit, deny)
make review
```

## License

MIT
