# pycoati — setup

pycoati audits a Python test suite and emits a structured inventory: per-file
and per-test counts, mock-API smells, and a per-test suspicion score. It never
edits your code and never mutates your environment.

You are here to produce `inventory.json`. Do that, then move to the next page.

## Where this fits

pycoati is a periodic audit, not a commit hook. A scan runs the suite under
pytest for durations and coverage, so it is minutes, not milliseconds, and its
verdict is a ranked list to work through, not a pass/fail. Run it on a
schedule, before a refactor of the tests, or when the suite feels slow or
untrustworthy. Do not wire it into a pre-commit gate such as madoqua.

The per-commit gate for test quality is zorilla, which lints the changed test
files in milliseconds and exits non-zero on findings; `uvx zorilla guide`
explains how to wire it in. The two overlap on purpose: zorilla catches the
smell as it is written, pycoati finds the ones already in the suite.

## Preconditions

- `pycoati` is on PATH. Check with `pycoati --version`.
- The project has a test directory. The default is `<path>/tests`.
- For runtime metrics (test count, runtime, coverage), `pytest` and
  `pytest-cov` must be importable by the interpreter pycoati picks:
  `uv add --dev pytest pytest-cov`. Without pytest the scan still succeeds
  and every `suite.*` field is `null`. Without pytest-cov the coverage pass
  fails with `pytest exit=4`, `line_coverage_pct` is `null`, and stderr says
  so; pass `--no-coverage` if coverage is not wanted.

## Configuration

**No knob is configurable.** pycoati has no `coati.toml`, reads no
`[tool.pycoati]` table, and exposes no way to change a threshold, a weight, or
a scoring rule. Every switch is a command-line flag, and the defaults below
are the values compiled into the binary.

The one file of its own that pycoati reads is `.pycoati-accept.toml`, the
accepted-findings baseline. It changes no threshold and no score — it records
reviewed per-test exceptions. See "Accepted findings" below.

It also reads three tables that other tools own, and only these:

- `[project].name` — seeds `project.name` and the coverage package.
- `[tool.uv.workspace].members` — switches the scan into workspace mode.
- `[tool.hatch.build.targets.wheel].packages`, `[tool.setuptools].packages`,
  `[tool.setuptools.packages.find].include` — declare which import names count
  as project-internal for `sut_calls`.

If `sut_calls` comes back empty on a project that clearly calls its own code,
the cause is almost always that none of these package tables are present. Pass
`--project-package <module>` to fix it.

## Run the scan

From the project root:

```
pycoati . --output inventory.json
```

That is the invocation the rest of this guide assumes. `inventory.json` in the
current directory is a convention this guide establishes, not a default the
binary knows — pycoati writes to stdout unless you pass `--output`.

Single file instead of a project:

```
pycoati tests/test_thing.py --output inventory.json
```

Human-readable view for a terminal, no file written:

```
pycoati . --format pretty
```

## Flags

- `--output <PATH>` / `-o` — write to this file instead of stdout. Files are
  written with no trailing newline. Default: stdout.
- `--format <json|pretty>` — `json` is the structured inventory and the only
  format this guide can read. `pretty` is an aligned-column terminal view for
  humans. Default: `json`.
- `--tests-dir <PATH>` — override test discovery root. Default: `<path>/tests`.
  Directory input only; rejected for single-file input and for workspace roots.
- `--static-only` — skip every pytest subprocess. The scan becomes pure AST
  analysis: fast, no test execution, and every `suite.*` field stays `null`.
  Use when pytest cannot run or when the suite is too slow to execute.
- `--no-coverage` — skip only the coverage subprocess. Collection and durations
  still run.
- `--top-suspicious <N>` — cap the `top_suspicious.test_functions` and
  `top_suspicious.files` lists. Default: `20`. `0` returns empty lists. This
  changes list length only — it is not a threshold and it does not change any
  score.
- `--project-package <NAME>` — the importable module name used for coverage and
  for classifying calls as project-internal. Default: `[project].name` with
  hyphens converted to underscores, else the directory basename. The override is
  used verbatim, with no normalization.
- `--python <CMD>` — interpreter to run pytest under, whitespace-split, no shell
  expansion. `--python "uv run python"` runs `uv run python -m pytest ...`.
  Default: auto-detect, in order — nearest ancestor `.venv/bin/python`, then
  `uv run --no-sync python` if `uv --version` succeeds, then bare `python`.
- `--pytest-args <STR>` — extra arguments appended to every pytest invocation,
  whitespace-split, no shell expansion. Default: empty.
- `--member-cwd <root|member>` — workspace mode only. `root` runs every member's
  pytest from the workspace root; `member` runs each from its own directory so
  member-local `conftest.py` applies. Default: `root`. Silently ignored outside
  workspace mode.
- `--accept-file <PATH>` — accepted-findings baseline. Default for a project
  scan: `<project>/.pycoati-accept.toml` when that file exists, otherwise
  none. For a single-file scan: the nearest `.pycoati-accept.toml` at or above
  the file's own directory. A path passed explicitly must exist, or the scan
  fails. Rejected against a workspace root — each member reads its own.
- `--no-accept` — ignore the baseline and report the raw shortlist. Nothing is
  read, and `accepted.path` is `null`.
- `--include-accepted` — keep accepted findings on
  `top_suspicious.test_functions`. The full audit report rather than the
  default actionable shortlist. The `accepted` block is emitted either way.

## Accepted findings

A periodic audit re-surfaces the same legitimate tests every time it runs. The
baseline is where a reviewer records that judgement once, with a reason, so the
next scan does not make them reconstruct it.

Put `.pycoati-accept.toml` at the project root — a single-file scan looks
there too, walking up from the file's own directory to the nearest one:

```toml
schema_version = "1"

[[accept]]
test = "tests/test_packaging.py::test_wheel_installs"
signals = ["zero_asserts", "high_setup_ratio"]
reason = "assertions run in a child interpreter; the parent propagates failure via subprocess.run(check=True)"
reviewed = "2026-09-12"
fingerprint = "3f0a1c7d9b2e4a56"
```

- `test` — required. The nodeid exactly as it appears in
  `test_functions[].nodeid`. In a workspace, relative to the member.

  **Copy it from the inventory, and scan the same way every time.** In
  project mode the file prefix is relative to the project root, so
  `pycoati .` and `pycoati /abs/path/to/project` produce the same nodeid. In
  single-file mode the prefix is the path *exactly as typed*, so scanning the
  same file by a different path produces a different nodeid and the entry
  reports `unknown_test` instead of applying.
- `signal` / `signals` — required, exactly one of the two. One signal name, or
  a list of them. The closed set is `mock_only_assertions`, `mock_overuse`,
  `zero_asserts`, `high_setup_ratio`. There is no wildcard, on purpose:
  accepting this test's current finding must not accept a future one.
- `reason` — **required and non-empty.** A file with an entry that omits it
  does not parse. The reason is the point of the entry.
- `reviewed` — optional, free-form (a date, a name, a PR link). Never
  interpreted.
- `fingerprint` — optional. Copy `test_functions[].fingerprint` from the
  inventory. When set, editing the test lapses the acceptance until someone
  reviews it again. When unset, the acceptance survives edits.

`zero_asserts` means the test verifies nothing at all: no `assert`-shaped
construct **and** no `external_verification_count`. A test that checks a child
process's exit status never trips it, so it needs no entry.

An assertionless test usually trips `high_setup_ratio` too, because the ratio
degrades to the height of the whole body. Accept both signals in one entry if
that is the reviewed judgement — accepting only `zero_asserts` deliberately
leaves the other one visible.

### What acceptance does, and does not do

- The test is still discovered, still parsed, still counted, and still run by
  pytest. `suite.test_count`, `suite.runtime_seconds`, and
  `suite.line_coverage_pct` are identical with and without a baseline. This is
  not pytest deselection.
- Every count, every `smell_hits` entry, and every `suspicion_score` stays
  exactly as measured. Nothing is subtracted.
- The only effect is on `top_suspicious.test_functions`: a test whose **every**
  active signal is accepted is held back from it. One unreviewed signal and the
  test is back on the list.
- The accepted findings and their reasons are reported under `accepted`, and
  `--include-accepted` puts them back on the shortlist for a full report.

### Stale entries

Every scan re-checks each entry and reports the ones that no longer apply
under `accepted.stale[]`, with a warning on stderr:

- `unknown_test` — the nodeid is gone. Delete the entry.
- `signal_not_active` — the signal stopped firing. Delete the entry.
- `content_changed` — the test was edited since its recorded `fingerprint`.
  The finding is live again until a reviewer updates the entry.

A stale entry never suppresses anything.

## What gets scanned

Test files are discovered recursively under the tests directory. A file
qualifies when its basename matches `test_*.py` or `*_test.py`. Hidden files,
gitignored files, and symlinks are skipped.

Within a file, pycoati collects top-level `def test_*` and `async def test_*`,
plus `test_*` methods on classes whose name starts with `Test`. Functions nested
inside other functions are not collected.

## Scan behaviour you must not misread

- A failed pytest subprocess is not a failed scan. Collection, durations, and
  coverage each degrade to `null` independently, with a warning on stderr. Check
  `tool.ran_pytest` and `tool.ran_coverage` before trusting any `suite.*` value.
- Diagnostics go to stderr. stdout carries only the payload, so
  `pycoati . > inventory.json` is safe.
- A non-zero exit means the scan did not produce an inventory. The reason is on
  stderr, prefixed `pycoati:`.

## Workspaces

If the root `pyproject.toml` declares `[tool.uv.workspace]`, the output is a
workspace payload: a `members` array holding one complete inventory per member,
wrapped with a `workspace_root` key. Analyze each member independently — scores
and rankings are computed per member and are not comparable across members.
`--tests-dir` and `--project-package` are rejected against a workspace root.

## Verify before moving on

`inventory.json` is usable when all of these hold:

- It parses as JSON.
- `schema_version` is `"3"`.
- `test_functions` is non-empty. An empty array on a project with tests means
  discovery found nothing — check `--tests-dir` and the file naming convention
  above before continuing.

next: run `pycoati . --output inventory.json`, then `pycoati guide analyze`
