# pycoati - measuring quality of pytest tests


---

## Goal

Build a Rust CLI that performs **Phase 1 — Inventory** of the `test-quality-review` workflow: a fast, static, deterministic audit pass over a single Python project's test suite, emitting `inventory.json` for downstream consumption by an LLM that runs Phase 2 (deep analysis) and beyond.

The tool answers exactly one question per run: **"What does this test suite look like, in numbers?"**

It does not form opinions. It does not propose changes. It does not edit files. It does not invoke an LLM. Those are explicitly other tools' jobs.


`pycoati` is the **orchestration and aggregation** layer that turns per-test signals plus runtime signals (from pytest invocation) into a prioritized inventory.

## Scope — what this tool does

For a single Python project (one `pyproject.toml`, one test directory), produce a JSON inventory with:

**Suite-level metrics**
- Total test count (from `pytest --collect-only -q`)
- Total runtime in seconds (from `pytest --durations=0`, or a dedicated timing run)
- Line coverage percentage (from `pytest --cov --cov-report=json`)
- Slowest N tests with durations (default N=20)

**Per-file metrics** (test files only)
- Test count
- Total assertion count (AST: count `Assert` nodes inside `test_*` functions)
- Mock construction count (AST: count calls to `Mock`, `MagicMock`, `AsyncMock`, `create_autospec`, `patch` and its variants)
- Fixture count and fixture-use density
- Native mock-smell hits (see "Native mock smells" below)

**Per-test metrics** (each `test_*` function)
- Assertion count
- `only_asserts_on_mock` boolean — true if every `Assert` node in the body has a target that is `.called`, `.call_count`, `.call_args`, `.call_args_list`, `assert_called*`, or any other Mock-API attribute
- Patch decorator count (`@patch`, `@mock.patch`, `@patch.object`)
- Setup-to-assertion line ratio
- Native mock-smell hits (see "Native mock smells" below)

**Suspicion score** (the bridge to Phase 2)
- Computed per test and per file via a tunable formula (see "Suspicion scoring" below)
- Top N most-suspicious tests and files are surfaced in a dedicated section of the output, so the Phase 2 LLM can focus its attention without re-ranking from raw counts

**Call-site frequency** (the consolidation signal)
- For each `test_*` function, the list of qualified call names invoked in its body
- Aggregated across the suite into a name → tests map: which tests touch each callee
- Top-N most-called names surfaced as a dedicated section
- Best-effort qualification: for `obj.method()`, emit `obj.method`; for `Foo.bar()`, emit `Foo.bar`; for bare `func()`, emit `func`. No symbol resolution in v1 — the Phase 2 LLM disambiguates by reading the test code. Conservative is fine; the goal is to surface "23 tests touch something called `save`" so the human or LLM can investigate, not to produce a typecheck-grade call graph.
- **Scope by imports, not by skiplist.** Read the project's package name(s) from `pyproject.toml` (`[project].name` + `[tool.hatch.build.targets.wheel].packages` or equivalent; fall back to the directory name). For each call, resolve its root name back to where it was imported from. Include only calls whose root binding traces to a project-internal package. This automatically excludes builtins (never imported), stdlib, pytest/mock infrastructure, and third-party deps — no enumeration of any of them needed. Star imports: treat unresolved bare names as potentially project-internal and include them (false positives cheaper than false negatives here). Same-file helpers (not imported) are correctly excluded as test plumbing.

## Scope — what this tool deliberately does not do

- No LLM calls.
- No proposals, no diffs, no edits to test files.
- No mutation testing. That is a Phase 2 sampling activity, driven by the LLM after reading `inventory.json`.
- **No structural *shape-similarity* detection** between test bodies — that's biston's job (LSH + anti-unification). However, **SUT call-site frequency** (how many tests invoke each function under test) **is** in scope: different algorithm, different signal, specifically valuable in the test domain. See "Call-site frequency" below.
- No multi-project orchestration in v1. One project, one invocation. Monorepo dispatch is a thin shell layer on top, added later.
- No coverage parsing beyond the top-level percentage. Per-line coverage attribution is out of scope for Phase 1.
- No detection of *uncalled* production functions (the "this code has no test" inverse signal). That requires walking production source and matching call names against definitions — ty-reach territory. Out of scope for v1.

## Hard constraints

- Rust, edition 2021.
- `tree-sitter` + `tree-sitter-python` for AST work. Use tree-sitter queries where possible rather than manual node-type matching.
- `serde` + `serde_json` for output. No other serialization formats.
- `clap` for the CLI.
- `walkdir` or `ignore` for file traversal.
- `toml` for `pyproject.toml` parsing.
- No tokio runtime unless a concrete need emerges. Phase 1 is naturally synchronous: walk files, run subprocesses, emit JSON.
- No PyO3, no Python embedding. The tool observes Python from outside.
- The binary must be invocable without any Python interpreter being available on PATH **for the AST and inventory work**. Pytest-invocation steps (runtime, coverage, collection) obviously require the project's Python; treat those as opt-in flags so the static analysis runs in any environment.

## CLI shape

```
pycoati <project-path>
  --tests-dir <path>            (default: discovered from pyproject.toml or "tests/")
  --project-package <name>      (override; default: discovered from pyproject.toml)
  --python <command>            (default: "python", e.g. "uv run python")
  --pytest-args <string>        (passed through to pytest invocations)
  --static-only                 (skip all pytest invocations; AST work only)
  --no-coverage                 (skip the coverage run)
  --top-suspicious <N>          (default: 20)
  --output <path>               (default: stdout)
  --format json|pretty          (default: json)
```

The default invocation `pycoati .` should do the right thing in a project root with a standard layout.

## Output schema

This schema is the contract with the Phase 2 LLM and with the `test-quality-review` SKILL.md. Lock it in early. Use `serde` derive on the structs; the JSON shape below is normative.

```json
{
  "schema_version": "2",
  "project": {
    "path": "/abs/path",
    "name": "from pyproject.toml or directory name"
  },
  "suite": {
    "test_count": 423,
    "runtime_seconds": 12.3,
    "line_coverage_pct": 87.2,
    "slowest_tests": [
      {"nodeid": "tests/foo.py::test_bar", "seconds": 4.2}
    ]
  },
  "files": [
    {
      "path": "tests/foo.py",
      "test_function_count": 18,
      "assertion_count": 42,
      "mock_construction_count": 31,
      "patch_decorator_count": 12,
      "fixture_count": 3,
      "smell_hits": [
        {"category": "mock_overuse", "test": "test_bar", "line": 45, "evidence": "8 mocks, 1 assertion"}
      ]
    }
  ],
  "test_functions": [
    {
      "nodeid": "tests/foo.py::test_bar",
      "file": "tests/foo.py",
      "line": 45,
      "assertion_count": 1,
      "only_asserts_on_mock": true,
      "patch_decorator_count": 4,
      "setup_to_assertion_ratio": 18.0,
      "called_names": ["repository.save", "Repository", "uuid.uuid4"],
      "smell_hits": [
        {"category": "mock_only_assertions", "evidence": "all 1 asserts on Mock API"}
      ],
      "suspicion_score": 0.87
    }
  ],
  "sut_calls": {
    "by_name": [
      {
        "name": "repository.save",
        "test_function_count": 23,
        "test_nodeids": ["tests/foo.py::test_bar", "..."]
      }
    ],
    "top_called": ["repository.save", "process_order", "..."]
  },
  "top_suspicious": {
    "test_functions": ["tests/foo.py::test_bar"],
    "files": ["tests/foo.py"]
  },
  "tool": {
    "name": "pycoati",
    "version": "x.y.z",
    "ran_pytest": true,
    "ran_coverage": true
  }
}
```

**Naming convention** — `suite.test_count` is the pytest-collected item count (parametrize-expanded; one per `--collect-only -q` line). `files[].test_function_count`, `sut_calls.by_name[].test_function_count`, and the `test_functions[]` / `top_suspicious.test_functions[]` arrays are AST-level: one entry per `def test_*` in source, with class-nested methods counted, but parametrize **not** expanded. The two numbers diverge in any suite that uses `@pytest.mark.parametrize` — the gap is meaningful, not a bug.

If `--static-only` or `--no-coverage` is used, set the corresponding `suite.*` fields to `null` and the `tool.ran_*` flags to `false`. Do not omit the keys.

## Suspicion scoring

Start with a simple linear formula and put the weights in a `SuspicionWeights` struct so they can be tuned without recompiling logic:

```
score = w_mock_only      * (only_asserts_on_mock ? 1.0 : 0.0)
      + w_patch_count    * min(patch_decorator_count / 5.0, 1.0)
      + w_setup_ratio    * sigmoid((setup_to_assertion_ratio - 8.0) / 4.0)
      + w_zero_asserts   * (assertion_count == 0 ? 1.0 : 0.0)
      + w_smell_density  * min(smell_hits.len() / 3.0, 1.0)
```

Initial weights: 0.35, 0.20, 0.15, 0.20, 0.10. Document them in a `WEIGHTS.md` so the rationale is visible. These are heuristic starting points — expect to revise once you run the tool against a real project and look at what it flags.

File-level suspicion is the mean of its tests' scores, plus a small bonus for high mock-construction-to-assertion ratio at the file level.

## Native mock smells

Coati emits categorized findings for mock-related anti-patterns that the inventory counts can already see. These live in `pycoati` (not zorilla) because zorilla's rule set is line/structural and has no equivalent — see [`zorilla-integration.md`](./zorilla-integration.md) for the boundary.

Categories owned by pycoati:

- **`mock_only_assertions`** (per-test). Fires when `only_asserts_on_mock == true` AND `assertion_count > 0`. The test verifies the mock, not the SUT. Evidence: `"all N asserts on Mock API"`. (When `assertion_count == 0`, zorilla's `ZR003 no-assertion` handles it — do not double-fire.)
- **`mock_overuse`** (per-test and per-file). Fires when `mock_construction_count + patch_decorator_count > max(assertion_count, mock_overuse_floor)` and the ratio exceeds `mock_overuse_ratio`. Evidence: `"N mocks, M assertions"`. Defaults: `mock_overuse_floor = 2`, `mock_overuse_ratio = 2.0`. Tunable via a `MockSmellConfig` struct, exposed later as TOML config; not as a CLI flag in v1.

Both categories are derived from inventory counts pycoati already computes — no extra AST passes. Thresholds live in one place; do not sprinkle them through the codebase.

Categories **not** owned by pycoati (deferred to zorilla if integration happens): conditional logic in tests, `sleep` calls, mystery-guest path literals, patch-stack `>3` (overlaps with `mock_overuse` but zorilla owns the rule), empty tests, assertion-roulette. See [`zorilla-integration.md`](./zorilla-integration.md).

## Implementation order

Do not implement the whole thing in one shot. The order matters:

1. **Skeleton + CLI parsing.** `clap` setup, `Project` struct, `Inventory` output struct, JSON serialization, empty inner methods. Run it end-to-end with stubbed values, confirm JSON shape is right.
2. **Tree-sitter setup.** Parse one Python file, walk it, count `Assert` nodes in functions named `test_*`. Smallest possible AST walk. Confirm counts match a hand-checked file.
3. **Mock-API detection.** The `only_asserts_on_mock` predicate. Use a tree-sitter query for `attribute` nodes whose attribute name is in a known set (`called`, `call_count`, `call_args`, `call_args_list`, `assert_called`, `assert_called_with`, `assert_called_once`, `assert_called_once_with`, `assert_not_called`, `assert_any_call`, `assert_has_calls`, etc.). Maintain this set in one place.
4. **File walker.** `ignore` crate, discover test files by convention (`test_*.py`, `*_test.py`, anywhere under the test dir). Run steps 2–3 across all files.
5. **Pytest invocation: collection.** `pytest --collect-only -q`, parse the output for the test count. Robust against pytest versions — prefer the `--collect-only --quiet` line-counting approach, fall back to JSON output if available.
6. **Pytest invocation: durations.** `pytest --durations=0 -q` or a single timing run. Parse the durations report. This run also gives you total runtime.
7. **Coverage.** `pytest --cov=<pkg> --cov-report=json:<tmp>`, read the JSON, extract the top-level percentage. Be defensive — coverage.py JSON has shifted shape between versions.
8. **Native mock smells.** Derive `mock_only_assertions` and `mock_overuse` from the counts already in the inventory (no new AST passes). Merge hits into per-file and per-test records. Keep the predicates in one module so thresholds are tweakable in one place.
9. **Call-site frequency.** First, parse the project's package name(s) from `pyproject.toml`. Per file: walk imports (`import_statement`, `import_from_statement`) and build a `name → source_module` map. Then, while walking each `test_*` function's body, collect call-expression head names, resolve each against the import map, and keep only those rooted in a project-internal package. Aggregate at the end of the run into the `sut_calls` section. Add a `--project-package <name>` CLI flag as override for non-standard layouts.
10. **Suspicion scoring + top-N ranking.** Apply the formula, sort, populate `top_suspicious`. Optionally bump the suspicion score of tests whose `called_names` overlap heavily with the suite's top-called set *and* whose `only_asserts_on_mock` is true — those are the strongest consolidation candidates.
11. **Pretty output format.** Optional `--format pretty` for human eyes during development. Markdown table per file.

After step 4 you have a useful static-only tool. After step 7 you have the full inventory. Steps 8–11 are polish. Do not optimize, parallelize, or generalize before all 11 steps work on one real project.

## Zorilla integration (deferred)

Zorilla covers lint-style test smells (ZR001–ZR007: conditional logic, `sleep`, no-assertion, assertion-roulette, mystery-guest, patch-stack, empty test). Whether to surface zorilla findings inside `pycoati`'s `inventory.json` — versus letting the Phase 2 LLM run zorilla itself — is undecided. See [`zorilla-integration.md`](./zorilla-integration.md). Do not wire it in as part of v1.

Coati's native mock smells (see above) are intentionally separate from zorilla — they're derived from inventory counts, not parsed as lint rules.

## Test the tool against itself

A useful smoke target: run `pycoati` against the test suite of one of your own Python projects — `dh`, `ajina`, or the Prague commuter tool — and read the JSON. The first interesting bug surfaces within minutes. The threshold defaults are almost certainly wrong; the JSON shape is almost certainly missing a field you'll want.

Also useful: write a fixture project under `tests/fixtures/` in the `pycoati` repo itself, containing hand-crafted test files that exemplify each category (one mock-only test, one tautology, one setup-heavy test, one good test) and assert against the expected inventory. This is a small integration test set that catches regressions in the heuristics.

## Done criteria for v1

- `pycoati .` on a real project completes in under 30 seconds for a suite of ~500 tests, including pytest invocations.
- `pycoati . --static-only` completes in under 3 seconds for the same suite.
- The emitted JSON validates against the schema above.
- At least 70% of the entries in `top_suspicious.tests` are, on inspection, genuinely worth a human reviewer's attention.
- The skill's Phase 2 prompt can consume `inventory.json` and produce a useful report without needing to re-derive any of the Phase 1 counts.

## Out of scope for v1, candidate for v2

- Multi-project monorepo dispatch (one binary call per project, results merged). The shell layer that does this is trivial; design the JSON so concatenation works.
- **Symbol resolution for `called_names`** via tyf — replaces best-effort textual matching with real qualified names. Disambiguates `save` across multiple classes. Adds a runtime dep on tyf.
- **Uncalled production functions** via ty-reach — the inverse signal: "this function in the SUT is not called by any test." Requires production-source walking and definition matching.
- biston subprocess integration: ingest biston's structural-duplication JSON and merge "test A and test B are shape-duplicates" hits into the inventory as a separate smell category.
- Trend tracking: timestamped inventories under `.pycoati/history/` so you can diff this run against last quarter's.
- Watch mode for editor integration.
- Caching of tree-sitter trees and pytest results.

Do not let any of these creep into v1. The Phase 1 / Phase 2 split exists precisely so that Phase 1 stays mechanical, fast, and boring.

---

## First instruction to Claude Code

Start by scaffolding the crate with `cargo new pycoati --bin`, add the workspace entry if relevant, set up the `Inventory` and `TestRecord` structs with `serde::Serialize`, wire up `clap`, and produce the trivial end-to-end run where every numeric field is zero and the JSON shape is correct. Stop there and show me the output before writing any tree-sitter code.
