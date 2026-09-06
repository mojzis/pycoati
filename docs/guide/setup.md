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

## There is no config file

pycoati reads no configuration of its own. It has no `coati.toml`, and it
reads no `[tool.pycoati]` table. Every knob is a command-line flag, and the
defaults below are the values compiled into the binary.

It does read three tables that other tools own, and only these:

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
- `schema_version` is `"2"`.
- `test_functions` is non-empty. An empty array on a project with tests means
  discovery found nothing — check `--tests-dir` and the file naming convention
  above before continuing.

next: run `pycoati . --output inventory.json`, then `pycoati guide analyze`
