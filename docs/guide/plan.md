# pycoati — plan

`pycoati plan` answers one question about a monorepo: given the subprojects
you want to test, how many pytest processes are worth running side by side,
and which subprojects go in which one. It runs no tests, edits nothing, and
is separate from the audit workflow on the other pages.

You are here to produce a run plan and execute it. Read `recommended`, start
one worker per lane, run each lane's commands in order.

## The model

- Every subproject is **one pytest invocation** with one **cost**: its
  runtime plus a fixed start-up allowance for the process itself.
- A **lane** is one parallel slot. The subprojects in a lane run one after
  another, so a lane costs the sum of its subprojects.
- The plan's **wall cost** is its most expensive lane. That is how long the
  whole thing takes when every lane starts together.
- For each lane count from 1 to `--max-jobs`, pycoati packs the subprojects
  to minimise the wall cost. The recommendation is the **smallest** lane count
  whose wall cost is within `--tolerance` percent of the best one found. A
  lane that buys nothing is never recommended.

Subprojects are never merged into one pytest process. Two subprojects'
`conftest.py` files, rootdirs and same-named test modules routinely clash in
a single process, so each keeps its own invocation whatever lane it lands in.

## Run it

Costs are only as good as their source, so measure first. The audit already
records each member's suite runtime; `--no-coverage` keeps that scan to two
pytest passes per member:

```
pycoati . --no-coverage --output inventory.json
pycoati plan . --timings inventory.json --output plan.json
```

Plan only the subprojects you are about to run by naming them after the root:

```
pycoati plan . api core --timings inventory.json
```

Without `--timings` the plan still works, balancing on test counts:

```
pycoati plan . --format pretty
```

## Which subprojects

- **uv workspace** (`[tool.uv.workspace]` in the root `pyproject.toml`): with
  no names, every member. A name matches a member by its path relative to the
  root (`packages/api`), else by directory name or `[project].name` (`api`).
  A name that matches nothing, or more than one member, is an error that
  lists the member paths.
- **Any other monorepo**: name the subproject directories relative to the
  root (`pycoati plan . services/api libs/core`). With none named the command
  fails — pycoati does not guess where the subprojects are.

Each subproject's tests are found as in the audit: every `testpaths` entry of
its own `pyproject.toml`, else `<subproject>/tests`.

## Where costs come from

- `measured` — `suite.runtime_seconds` for that subproject in a `--timings`
  inventory. Matched by project path, else by `[project].name` when exactly
  one entry carries it, so an inventory produced in CI works locally. When
  several `--timings` files measure the same project, the last one wins.
- `estimated` — the subproject has no measured runtime, but others do: its
  test count times the measured seconds-per-test rate. A rough figure;
  re-measure when it matters.
- `test_count` — nothing usable was measured. Every cost in the plan is a
  test count, and `cost_unit` is `tests`. Seconds and counts are never mixed.

Every seconds cost then gets `--startup-seconds` added. pytest's own runtime
figure starts counting after the interpreter, pytest and its plugins have
loaded, so a suite it reports as `0.02s` still occupies a lane for most of a
second. In a monorepo of many small suites that start-up is the bulk of the
work, and leaving it out would make every plan look like "one lane is enough".
A `test_count` plan carries no allowance.

Test counts come from the timings inventory's `suite.test_count` when it is
above zero, else from static analysis, which counts a parametrised test once.

A plan in `tests` units still balances the lanes, but treats every test as
equally slow. Do not report its `wall_cost` as a time. Every scheduled
subproject costs at least `1` in such a plan.

## Flags

- `--max-jobs <N>` — upper bound on lanes. Default: the CPUs available to
  the process. Set it to what the machine or CI runner pool can really give
  you; more lanes than subprojects are never used.
- `--tolerance <PCT>` — how much slower than the fastest plan is acceptable
  in exchange for fewer lanes. Default: `10`. `0` insists on the fastest
  wall cost (still with the fewest lanes that reach it).
- `--startup-seconds <SECS>` — added to every subproject's cost when costs
  are in seconds. Default: `0.5`. Raise it when each run pays for something
  heavier before the first test — `uv run` resolving an environment, a
  container start, a database fixture; set `0` to plan on runtimes alone.
- `--timings <FILE>` — inventory to read measured runtimes from. Repeatable;
  accepts a workspace inventory or a single-project one.
- `--member-cwd <root|member>` — where the emitted commands run from. `root`
  addresses each subproject's tests from the monorepo root; `member` runs
  from inside the subproject so its own pytest configuration and
  `conftest.py` apply. Default: `root`. Use the value your suite already
  passes with.
- `--pytest-cmd <CMD>` — prefix of the emitted commands; test paths are
  appended. Default: `python -m pytest`. For a uv workspace,
  `--pytest-cmd "uv run pytest"` is typical.
- `--format <json|pretty>` — `json` is the plan payload described below;
  `pretty` is a terminal view. Default: `json`.
- `--output <PATH>` / `-o` — write to a file instead of stdout.

## Plan fields

- `schema_version` — `"1"`. Versioned separately from the inventory.
- `root` — absolute path of the monorepo root. Every `path` and `cwd` below
  is relative to it.
- `cost_unit` — `seconds` or `tests`: the unit of every `cost`, `serial_cost`
  and `wall_cost` in this plan.
- `max_jobs` — the lane cap the plan was computed under.
- `tolerance_pct` — the tolerance the recommendation was chosen with.
- `startup_seconds` — the allowance included in every `cost`, or `null` in a
  `tests` plan, where none applies.
- `members` — the subprojects that will run, in the order asked for.
- `name` — the subproject's `[project].name`, else its directory name.
- `path` — the subproject's directory, `/`-separated; `.` for the root.
- `test_count` — collected count from the timings inventory, else the static
  count.
- `runtime_seconds` — the measured runtime as pytest reported it, without
  the start-up allowance, or `null` when not measured.
- `cost` — what the packing used for this subproject, lane, or run. In a
  `seconds` plan a subproject's cost is its runtime (measured or estimated)
  plus `startup_seconds`.
- `cost_source` — `measured`, `estimated` or `test_count`. See above.
- `skipped` — selected subprojects with nothing to run. They appear in no
  lane.
- `reason` — why: `no tests directory` or `no tests`.
- `serial_cost` — the sum of all member costs: one lane, no parallelism.
- `bottleneck` — the most expensive member. No lane count finishes sooner
  than it.
- `member` — a subproject's `name`, in `bottleneck` and in each run.
- `limits_wall_time` — `true` when the recommended plan is already down to the
  bottleneck's own cost. Adding lanes cannot help any further; only making
  that subproject faster can — for example running it under `pytest-xdist`
  (`-n`), or splitting it into two subprojects.
- `options` — one entry per lane count from 1 to `max_jobs` (never more than
  there are members): the trade-off the recommendation was picked from.
- `jobs` — number of lanes, i.e. pytest processes running at once.
- `wall_cost` — cost of the most expensive lane at that lane count.
- `speedup` — `serial_cost / wall_cost`.
- `recommended` — the chosen option, plus its `lanes`.
- `lanes` — what to run, most expensive lane first.
- `lane` — 1-based lane number.
- `runs` — the lane's pytest invocations. Run them **in order, one at a
  time**; start the next only when the previous exits.
- `cwd` — directory to run the command from, relative to `root`.
- `command` — the command line: `--pytest-cmd` followed by the test paths.
  A path containing a space or another shell-special character is
  single-quoted for a POSIX shell; `path` and `cwd` are always raw.

## Executing a plan

- Start `recommended.jobs` workers at once, one per lane. In CI that is a job
  matrix over `recommended.lanes`; locally, one shell per lane.
- Within a lane, run every `command` from its `cwd` even if an earlier one
  failed, and fail the lane if any did. Stopping at the first failure hides
  the results of the subprojects behind it.
- Add your own pytest flags through `--pytest-cmd`, not by editing commands
  afterwards, so the plan stays the record of what ran.
- If `bottleneck.limits_wall_time` is `true` and the wall cost is still too
  high, more lanes are the wrong lever. Speed up that one subproject.
- If `options` shows the wall cost barely moving between lane counts, the
  recommendation already took the smaller one. Raise `--tolerance` to trade
  more time for fewer workers, lower it for the reverse.

## What a plan cannot tell you

- Costs are sequential runtimes measured on one machine. Lanes that compete
  for the same CPUs, database or ports will run slower than the plan says.
  `--max-jobs` is where you account for that.
- It assumes subprojects are independent. Two suites that share a fixed
  port, a temp path or a database schema must not be in different lanes at
  the same time; the plan does not know about them.
- Timings go stale. Regenerate `inventory.json` when suites change shape.

next: run `pycoati plan . --timings inventory.json --output plan.json`
