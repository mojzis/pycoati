# Suspicion-score weights

`pycoati`'s `suspicion_score` blends five static heuristics into a single
number per test in `[0.0, 0.80)` (see [Reachable range](#reachable-range)),
plus a small `mock_construction` bonus per file. The values below are the v1 defaults that ship in
`src/suspicion.rs::DEFAULT`; they are **heuristic starting points**, not
empirically tuned constants. Treat the formula as the contract and the
numbers as defaults you may want to revise after running pycoati on a real
project (see [How to revise](#how-to-revise)).

## Formula

Per-test score:

```text
score = w_mock_only      * (only_asserts_on_mock ? 1.0 : 0.0)
      + w_patch_count    * min(patches / 5.0, 1.0)
      + w_setup_ratio    * sigmoid((setup_to_assertion_ratio - 8.0) / 4.0)
      + w_zero_asserts   * (unverified ? 1.0 : 0.0)
      + w_smell_density  * min(smell_hits.len() / 2.0, 1.0)

unverified = assertion_count == 0 && external_verification_count == 0
             && helper_assertion_count == 0
patches    = patch_decorator_count + patch_call_count
```

`sigmoid(x) = 1.0 / (1.0 + e^-x)`.

## Weights

| Weight | Value | Rationale |
|---|---|---|
| `w_mock_only` | `0.35` | Strongest single signal — a test whose only assertions target the Mock API is exercising the test double itself, not production logic. |
| `w_patch_count` | `0.20` | High patch density flags tests where the System Under Test has been replaced wholesale; saturates at 5 to avoid runaway scores on extreme outliers. Counts every patch the test installs — `@patch` decorators, `with patch(...)` / `patch.object(...)` in the body, `mocker.patch*` — so the spelling does not change the score. |
| `w_setup_ratio` | `0.15` | Long setup before the first assertion correlates with brittle tests that drift from their intent; sigmoid keeps the term smooth and bounded. |
| `w_zero_asserts` | `0.20` | A test that verifies nothing cannot fail by intent — it survives only by not raising. Weight is high but slightly below `w_mock_only` because such tests are often vestigial smoke tests rather than mock-theater. The term is gated on `unverified`, not on `assertion_count` alone: a test that checks a child process's exit status, or verifies through an assertion helper, does fail by intent. |
| `w_smell_density` | `0.10` | Re-uses information already captured by other heuristics, so weighted lowest — but a test that trips multiple smells should rank above one that trips none. |

Sum of the weight values: **1.00**. That sum is **not** a reachable
score — see the next section.

## Reachable range

Two of the five terms can never fire together. `w_mock_only` requires
`only_asserts_on_mock`, which requires `assertion_count > 0`; `w_zero_asserts`
requires `assertion_count == 0`. So the highest-scoring test is one of two
shapes:

| Branch | mock-only | patch | setup | zero-asserts | smells | Supremum |
|---|---|---|---|---|---|---|
| Mock-only (asserts only on the double) | 0.35 | 0.20 | → 0.15 | 0 | 0.10 (both smells) | **0.80** |
| Unverified (no verification at all) | 0 | 0.20 | → 0.15 | 0.20 | 0.05 (`mock_overuse` only) | 0.60 |

The unverified branch can carry only one smell: `mock_only_assertions`
needs an assertion. The setup term is a sigmoid, which approaches
`w_setup_ratio` without reaching it, so both bounds are approached and never
attained: **every test score lies in `[0.0, 0.80)`**, and in practice in
`(0.018, 0.80)`, because the setup term is never zero either. A file score
adds at most `0.10` to a mean of test scores, so it lies in `[0.0, 0.90)`.

Changing a weight changes these bounds; recompute them from the table.

## Sub-metric definitions

- `assertion_count` counts four kinds of assertion site, one each:
  `assert_statement` nodes, `with pytest.raises(...)` / `with raises(...)`
  context-manager blocks, `self.assertXxx(...)` calls inside a test class,
  and bare Mock-API assertion calls written as their own statement
  (`m.assert_called_once_with(1)`, any `assert_*` name in
  `MOCK_API_ATTRIBUTES`). All four raise at the line they sit on, which is
  what keeps the count syntactic. A raises block is always a non-mock
  assertion: it disqualifies `only_asserts_on_mock`. A bare Mock-API
  assertion call is always a mock assertion. Raises blocks and bare mock
  calls participate in `setup_to_assertion_ratio` like an
  `assert_statement`; `self.assertXxx(...)` calls do not.
- `mock_assertion_count` is the subset of `assertion_count` that inspects
  the test double: bare `m.assert_*(...)` calls; `assert` statements whose
  expression reads a Mock-API attribute directly, through `not`,
  parentheses, `len(...)`, an index or attribute hanging off it
  (`m.call_args[0][0]`), or on either side of a comparison
  (`m.call_count == len(items)` counts; `result == m.return_value` does
  not, since `return_value` is not in the set); and `self.assertXxx(...)`
  calls whose every argument is a Mock-API read, a literal, or a
  `unittest.mock` assertion helper (`call(...)`, `ANY`), with at least one
  Mock-API read. `only_asserts_on_mock` is exactly
  `assertion_count > 0 && mock_assertion_count == assertion_count`, which
  is what the `w_mock_only` term reads. `and` / `or` counts as a mock read
  only when every operand is one; `assert m.called and result == 3` is
  real verification. An arbitrary call wrapped around an attribute
  (`helper(m.call_count)`) is also classified as real verification: the
  helper may check anything, and only `len(...)` is known not to.
- `external_verification_count` counts call sites that propagate a
  failure from outside the test process: a call resolving to
  `subprocess.check_call` / `subprocess.check_output`, a call resolving
  to `subprocess.run` carrying the literal `check=True`, and
  `<receiver>.check_returncode()`. Import aliases are canonicalized
  first. It is deliberately **not** folded into `assertion_count`, which
  stays a syntactic count — the two answer different questions, and
  merging them would silently change a published field's meaning. Its
  effects on the score are two: the `w_zero_asserts` term switches off,
  and `setup_to_assertion_ratio` is measured to the first verification
  call site instead of the body height (see the next bullet). The mock,
  patch and smell terms are computed as usual, so a checked-subprocess
  test that overuses mocks still ranks for that.
  A checked call against a name the test itself replaced with a double
  (`@patch`, `monkeypatch.setattr`, a fixture of the same name, a local
  assigned from a `Mock()`, a patcher a `setUp` started for the whole
  class) is excluded, as is one whose failure may never reach pytest —
  nested in a `def`/`lambda` or a generator expression, inside a `try`
  that has an `except`, or inside `with contextlib.suppress(...)`. Evidence the test manufactured, or that cannot
  escape, is not evidence. `from subprocess import *` is never matched
  at all, which under-counts; that is the safe direction.
- `helper_assertion_count` counts verification delegated to a call:
  a call canonicalizing to `numpy.testing.assert_*` /
  `pandas.testing.assert_*` / `polars.testing.assert_*` / `pytest.fail`;
  a `with pytest.warns(...)` / `with pytest.deprecated_call()` block; and a
  call to a same-file helper (module-level function by bare name, or
  `self.<method>` on the test's class) whose body holds a non-mock
  effective assertion — one level deep, no imports, no `conftest.py`. The
  closed lists live in `src/assertion_helpers.rs`. Like
  `external_verification_count` it stays out of `assertion_count`; unlike
  it, each site is a full non-mock assertion: it switches off
  `w_zero_asserts`, disqualifies `only_asserts_on_mock` (so `w_mock_only`
  is off), and is an assertion site for `setup_to_assertion_ratio`. It is
  not part of the `mock_overuse` denominator.
- `setup_to_assertion_ratio` (locked, Run 3; extended to raises blocks
  and to external verification): when at least one effective assertion
  exists in the body, `(first_effective_line - def_line) /
  effective_count` measured in tree-sitter `start_position().row`
  deltas, where an effective assertion is an `assert_statement`, a
  `with pytest.raises(...)` block, a bare Mock-API assertion call or a
  `helper_assertion_count` site. When the body has no effective
  assertion but does carry external verification, the first verification
  call site takes the assertion's place: `(first_verification_line -
  def_line) / external_verification_count` — the body is not "setup with
  no payoff", so the fallback below would misread it. When it has
  neither, the numerator becomes `(last_body_line - def_line)` and the
  denominator is `1` — the result is the body height. This naturally
  pushes setup-heavy unverified tests up the ranking, complementing the
  `w_zero_asserts` term.
- `patches` saturates at **5**: the term becomes `1.0` once a test
  installs five or more patches, by decorator or in the body. Five is the
  point past which incremental patches add no new signal — the test is
  already maximally mock-mediated.
- `smell_hits.len()` saturates at **2**, the number of per-test smell
  categories (`mock_only_assertions`, `mock_overuse`). The saturation point
  is derived from that category list in `src/smells.rs`, so a test that
  trips every per-test smell gets the full `w_smell_density`; a third
  category would move it to 3 automatically. Before this was tied to the
  category count it sat at 3 and the term could only reach two thirds of
  its weight.
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
- `mock_overuse_ratio = 2.0` — the `mocks / max(asserts, 1)` ratio that
  must be exceeded for `mock_overuse` to fire, where `mocks` is the
  sum defined below.
- Both predicates use strict `>` semantics — a 2-mock-2-assert test sits
  on the boundary and does **not** fire.

### `mocks` decomposition

The numerator of `mock_overuse` is a sum of three **disjoint** counts. The
same sum is used at test scope and at file scope (where each count is the
sum over the file's tests).

- `mock_construction_count` — calls in a test body whose head resolves,
  through the file's imports, to `Mock` / `MagicMock` / `AsyncMock` /
  `create_autospec` / `patch` / `patch.object` / `patch.dict` /
  `patch.multiple` in `unittest.mock` or the `mock` backport. A bare name no
  import binds (star import, conftest) is assumed to be `unittest.mock`'s
  when it is itself one of those names; any other unbound head
  (`factory.Mock`, a local `mock`) is not counted.
- `patch_decorator_count` — `@patch`-shaped decorators on the test
  function: `@patch`, `@<x>.patch`, `@patch.<attr>`, or any decorator
  resolving to a `patch*` variant (`@mock.patch.object`).
- `stubs_count` — fixture-driven calls whose dotted call-head matches
  `STUB_HEADS` (`monkeypatch.setattr`, `monkeypatch.setenv`,
  `monkeypatch.delattr`, `monkeypatch.delenv`, `monkeypatch.context`,
  `monkeypatch.syspath_prepend`, `monkeypatch.chdir`, `mocker.patch`,
  `mocker.patch.object`, `mocker.patch.dict`, `mocker.patch.multiple`,
  `mocker.spy`, `mocker.stub`, `mocker.Mock`, `mocker.MagicMock`,
  `mocker.AsyncMock`, `mocker.create_autospec`). The `mocker` fixture is
  not an import, so these are matched as written and never overlap the
  construction count.

A fourth count, `patch_call_count`, feeds the **patch term** only. It is
the body-level patches — the `patch*` subset of `mock_construction_count`
plus the `mocker.patch*` subset of `stubs_count` — and is never added to
`mocks`. That gives every patch the same footprint whatever its spelling:

| Spelling | Patch term input | `mocks` term |
|---|---|---|
| `@patch("a.b")` | `patch_decorator_count` | `patch_decorator_count` |
| `with patch("a.b"):` / `patch.object(...)` | `patch_call_count` | `mock_construction_count` |
| `mocker.patch("a.b")` | `patch_call_count` | `stubs_count` |
| `Mock()` / `mock.MagicMock()` | — | `mock_construction_count` |
| `mocker.MagicMock()` / `monkeypatch.setattr(...)` | — | `stubs_count` |

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

1. Run `pycoati` on a real project.
2. Skim `top_suspicious.test_functions` (default 20 entries).
3. For each entry, mentally label it useful / borderline / noise.
4. If a category of test consistently appears as noise (or fails to
   appear), revise the weights — bump the term that should have caught
   the missing case, or shrink the term that brought in the noise.
5. Update this file and `src/suspicion.rs::DEFAULT` together — they are
   the single source of truth.

The defaults will not stay defaults forever. Revising them is expected.
