# pycoati

pycoati audits a Python test suite for mock smells and suspicious tests. It
walks the project with tree-sitter, optionally runs `pytest` for collection,
durations and coverage, and emits a structured inventory: per-file and
per-test counts, mock-API smells, and a per-test suspicion score.

It is meant for an occasional audit, not every commit: the suite is green but
you don't trust it. For the per-commit gate, see
[zorilla](https://mojzis.github.io/aesop/zorilla/).

## Install

```sh
uvx pycoati@latest guide      # agent-oriented instructions
uvx pycoati@latest .          # audit the current project
```

## What it does

**Audit.** `pycoati <PATH>` scores every test for suspicion and lists the most
suspicious first, with a remediation ladder for the agent to work through.

```sh
pycoati . --output inventory.json   # structured inventory (JSON)
pycoati . --format pretty           # aligned-column terminal view
pycoati . --static-only             # no pytest subprocesses
```

Review decisions persist in `.pycoati-accept.toml`, one test and one signal at
a time, each with a reason. See [Set up and run an audit](guide/setup.md).

**Plan.** `pycoati plan <ROOT> [MEMBER]...` works out how to run a monorepo's
pytest suites in parallel: how many pytest processes are worth starting side
by side, and which subprojects share one. It runs no tests.

```sh
pycoati . --no-coverage --output inventory.json            # measure once
pycoati plan . api core web --timings inventory.json --max-jobs 8
pycoati plan . --format pretty                             # no timings: balance on test counts
```

The recommendation is the fewest lanes whose wall time is within `--tolerance`
percent of the best packing found, so a lane that buys nothing is never
suggested. See [Plan parallel pytest runs](guide/plan.md).

## The guide

`pycoati guide [PAGE]` prints the pages in the *Agent guide* section of this
site. They are embedded in the binary at compile time, so they always describe
the schema that exact build emits; a test fails the build when a serialized
field goes undocumented.

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
