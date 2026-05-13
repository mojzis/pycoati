# Suspicion-score weights

`coati`'s `suspicion_score` blends five static heuristics into a single
[0.0, ~0.9]-bounded number per test, plus a small `mock_construction`
bonus per file. The values below are the v1 defaults that ship in
`src/suspicion.rs::DEFAULT`; they are **heuristic starting points**, not
empirically tuned constants. Treat the formula as the contract and the
numbers as defaults you may want to revise after running coati on a real
project (see [How to revise](#how-to-revise)).

## Formula

Per-test score:

```text
score = w_mock_only      * (only_asserts_on_mock ? 1.0 : 0.0)
      + w_patch_count    * min(patch_decorator_count / 5.0, 1.0)
      + w_setup_ratio    * sigmoid((setup_to_assertion_ratio - 8.0) / 4.0)
      + w_zero_asserts   * (assertion_count == 0 ? 1.0 : 0.0)
      + w_smell_density  * min(smell_hits.len() / 3.0, 1.0)
```

`sigmoid(x) = 1.0 / (1.0 + e^-x)`.

## Weights

| Weight | Value | Rationale |
|---|---|---|
| `w_mock_only` | `0.35` | Strongest single signal — a test whose only assertions target the Mock API is exercising the test double itself, not production logic. |
| `w_patch_count` | `0.20` | High-density `@patch` decorators flag tests where the System Under Test has been replaced wholesale; saturates at 5 to avoid runaway scores on extreme outliers. |
| `w_setup_ratio` | `0.15` | Long setup before the first assertion correlates with brittle tests that drift from their intent; sigmoid keeps the term smooth and bounded. |
| `w_zero_asserts` | `0.20` | A test without any assertions cannot fail by intent — it survives only by not raising. Weight is high but slightly below `w_mock_only` because such tests are often vestigial smoke tests rather than mock-theater. |
| `w_smell_density` | `0.10` | Re-uses information already captured by other heuristics, so weighted lowest — but a test that trips multiple smells should rank above one that trips none. |

Sum of the weight values: **1.00** — a test that fires every term and
saturates each saturating sub-metric reaches a score of 1.0.

## Sub-metric definitions

- `setup_to_assertion_ratio` (locked, Run 3): when at least one
  `assert_statement` exists in the body,
  `(first_assert_line - def_line) / max(assertion_count, 1)` measured in
  tree-sitter `start_position().row` deltas. When the body has zero
  `assert_statement` nodes, the numerator becomes
  `(last_body_line - def_line)` and the denominator is `1` — the result
  is the body height. This naturally pushes setup-heavy zero-assert tests
  up the ranking, complementing the `w_zero_asserts` term.
- `patch_decorator_count` saturates at **5**: the term becomes `1.0`
  once a test has five or more `@patch`-shaped decorators. Five is the
  point past which incremental decorators add no new signal — the test
  is already maximally mock-mediated.
- `smell_hits.len()` saturates at **3**: three smell categories on a
  single test is enough to flag it; the term is the same at three and at
  ten.
- The setup-ratio sigmoid uses `(setup_to_assertion_ratio - 8.0) / 4.0`
  as its argument. The `-8.0` term places the **inflection point at 8
  setup lines** — the row delta at which a test is becoming suspect (eight
  lines of arrange-act-assert prelude before the first assertion is a lot
  for a unit test). The `/ 4.0` divisor controls the sigmoid's slope: a
  ratio of 4 lines on either side of the inflection produces ~0.27 / ~0.73,
  and ratios beyond 16 / 0 saturate cleanly.

## Mock-smell thresholds

Mock-smell detection (`src/smells.rs`) feeds the `w_smell_density` term.
Its v1 thresholds:

- `mock_overuse_floor = 2` — the minimum value used as the floor in the
  `mocks > max(asserts, floor)` predicate. Stops the smell firing on a
  test with zero or one assertion just because the mock count is small
  but non-zero.
- `mock_overuse_ratio = 2.0` — the `(mocks + patches) / max(asserts, 1)`
  ratio that must be exceeded for `mock_overuse` to fire.
- Both predicates use strict `>` semantics — a 2-mock-2-assert test sits
  on the boundary and does **not** fire.

## File-level score

```text
file_score = mean(test_scores_in_file) + bonus
where bonus = min(0.1, max(0.0, (mock_construction_count / max(assertion_count, 1)) - 1.0) * 0.05)
```

The bonus is zero when the ratio is at most 1.0; it grows linearly past
that threshold (one extra `0.05` per unit of ratio above 1.0) and is
capped at `0.1`. Files with no tests yield `0.0` (no panic on empty
`test_scores_in_file`).

Concrete shapes:

- 1 mock, 1 assert → ratio = 1.0 → bonus = 0.0.
- 2 mocks, 1 assert → ratio = 2.0 → bonus = 0.05.
- 3 mocks, 1 assert → ratio = 3.0 → bonus = 0.10 (cap).
- 100 mocks, 1 assert → ratio = 100.0 → bonus = 0.10 (still capped).

## Deferred extensions

These are intentionally **not** implemented in Run 3 / v1.

- **Top-called overlap bonus.** The original spec sketched an additional
  per-test term that boosts the score of tests that touch many of the
  top-called SUT entries (the assumption: tests calling popular SUT
  surface area are the most consequential, and so the highest-yield
  candidates to review). Implementing this requires the
  `sut_calls.top_called` list to be rank-stable, which it is in v1, but
  the term has not been calibrated against real-world traces and lands
  later.
- **Runtime-tunable weights.** Planned `--weights <path>` CLI override
  loading a TOML file that mirrors the `SuspicionWeights` struct. The
  shape is straightforward; we are deferring because v1 callers (the
  workflow's Phase 2 LLM and an early-adopter human) all want the same
  defaults, and shipping the knob without a story for how Phase 2 picks
  weight overrides is a vector for inconsistent outputs.

## How to revise

These weights are heuristic starting points. The intended workflow:

1. Run `coati` on a real project.
2. Skim `top_suspicious.test_functions` (default 20 entries).
3. For each entry, mentally label it useful / borderline / noise.
4. If a category of test consistently appears as noise (or fails to
   appear), revise the weights — bump the term that should have caught
   the missing case, or shrink the term that brought in the noise.
5. Update this file and `src/suspicion.rs::DEFAULT` together — they are
   the single source of truth.

The defaults will not stay defaults forever. Revising them is expected.
