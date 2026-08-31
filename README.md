# pycoati

Audits Python test suites for mock smells and suspicious tests. Walks a project
with tree-sitter, optionally runs `pytest` for collection, durations, and
coverage, and emits a structured inventory: per-file and per-test counts,
mock-API smells, and a per-test suspicion score.

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
`--member-cwd <root|member>`. See `pycoati --help` for the full list.

Weights behind `suspicion_score` are documented in [WEIGHTS.md](WEIGHTS.md).

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
```

| Page | Contents |
|---|---|
| `setup` | Configuration surface, how to run the scan, where `inventory.json` lands. |
| `analyze` | Field reference for every key in the inventory, plus the anti-pattern taxonomy and its detection heuristics. |
| `remediate` | The remedy ladder, the human-approval gate, and the "do not" list. |

With no argument, the page is chosen from the current directory: `analyze` when
an `inventory.json` is present there, otherwise `setup`. `remediate` is never
auto-selected — reach it via the `analyze` page's breadcrumb or by name.

The subcommand shadows a directory of the same name: if you have a `./guide`
directory to audit, write `pycoati ./guide`.

Each page ends with a single `next:` line, so an agent is always one command
away from the right instructions. A `--format pretty` scan that finds candidates
ends with the same breadcrumb; JSON output never carries it.

The page sources are plain markdown under [`docs/guide/`](docs/guide), embedded
via `include_str!`. `tests/guide_schema_drift.rs` asserts that every field name
serde emits is documented on the `analyze` page, so a schema change cannot land
without the guide being updated alongside it.

## Development

```bash
# Pre-commit checks
cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features

# Full review (fmt, clippy, tests, audit, deny)
make review
```

## License

MIT
