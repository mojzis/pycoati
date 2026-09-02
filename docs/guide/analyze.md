# pycoati — analyze

You have `inventory.json`. This page defines every field in it, then defines the
anti-patterns you are looking for and the signals that flag each one.

Read this whole page before opening the inventory. Produce no edits from this
page — analysis only.

## The one rule for this phase

**A signal is a lead, not a verdict.** Every heuristic below is computed from
the AST without understanding what the test means. A test can trip three
signals and still be correct, and a test can trip none and still be worthless.
Every candidate you identify must be confirmed by reading the test source
before it appears in your report. If reading the test contradicts the signal,
the test wins — drop the candidate.

---

# Part 1 — field reference

Every field is always present. `null` never means zero; it means not measured.
The payload has two shapes, discriminated by one key.

## Top level, single project

- `schema_version` — schema contract version, as a string. This page describes
  `"2"`. Any other value means this guide and the inventory disagree; stop and
  re-run `pycoati guide analyze` from the binary that produced the file.
- `project` — object. Identity of the audited project.
- `suite` — object. Suite-level runtime metrics from pytest.
- `files` — array of file records, one per discovered test file, sorted by path.
  A file with zero tests still gets a record in directory mode.
- `test_functions` — array of test records, one per collected test function.
- `sut_calls` — object. Aggregated project-internal call frequency.
- `top_suspicious` — object. Pre-ranked shortlists.
- `tool` — object. Producer metadata.

## Top level, workspace

A workspace payload carries `schema_version`, `workspace_root`, `members`, and
`tool`, and nothing else.

- `workspace_root` — path of the workspace root. **The presence of this key is
  the only discriminator between the two shapes.** Check for it first.
- `members` — array of complete single-project inventories, one per workspace
  member. Analyze each independently; scores are computed per member and are not
  comparable across members.

## `project`

- `path` — absolute, canonicalized path of the project root. In single-file
  mode this is the directory *containing* the file, not the file itself. Falls
  back to the path as given only if canonicalization fails, which logs a
  warning to stderr.
- `name` — `[project].name` from `pyproject.toml`, else the directory basename.

## `suite`

Every field here is `null` under `--static-only` and on any pytest failure.
Check `tool.ran_pytest` and `tool.ran_coverage` before using any of them.

- `test_count` — count of items pytest collected. **Parametrize is expanded**,
  so this legitimately exceeds the sum of `test_function_count` across `files`.
  `null` = not measured.
- `runtime_seconds` — wall-clock seconds for the whole durations run, as
  reported by pytest's summary line. `null` = not measured.
- `line_coverage_pct` — line coverage as a percentage in `0`–`100`, not a
  fraction. Suite-wide. There is no per-test or per-file coverage attribution
  anywhere in this schema. `null` = not measured.
- `slowest_tests` — array of at most 20 entries, descending by duration.

### `suite.slowest_tests[]`

- `nodeid` — pytest's nodeid. **Parametrize is expanded**, so this looks like
  `tests/test_x.py::test_y[case-3]`. This is a different string space from
  `test_functions[].nodeid` — see the joining rule under `slow-without-reason`.
- `seconds` — wall-clock duration of that test item.

## `files[]`

One record per discovered test file.

**What paths are relative to, which differs by mode.** In directory mode,
`files[].path`, `test_functions[].file`, and the file prefix of every `nodeid`
are relative to `project.path`. In single-file mode they are the path *exactly
as it was typed on the command line* — absolute if you passed an absolute path,
cwd-relative if you passed a relative one — and are therefore not relative to
`project.path`, which is canonicalized in both modes. Resolve a path before
opening a file at `file`:`line`.

- `path` — path to the test file.
- `test_function_count` — number of `def test_*` functions parsed from this file
  via AST. Class-nested test methods are counted. **Parametrize is not
  expanded.**
- `assertion_count` — effective assertions across every test body in the file.
  An effective assertion is an `assert` statement or a `with pytest.raises(...)`
  block; the raises block counts as one.
- `mock_construction_count` — call sites in test bodies whose callee is `Mock`,
  `MagicMock`, `AsyncMock`, `create_autospec`, or `patch`. `patch` is counted
  here because `with patch(...)` is a construction at the call site. This is a
  file-scope signal only; there is no per-test equivalent.
- `patch_decorator_count` — sum of the per-test decorator counts in this file.
- `stubs_count` — fixture-driven patch call sites in this file. The matched set
  is closed and exact: `monkeypatch.setattr`, `monkeypatch.setenv`,
  `monkeypatch.delattr`, `monkeypatch.delenv`, `monkeypatch.context`,
  `monkeypatch.syspath_prepend`, `monkeypatch.chdir`, `mocker.patch`,
  `mocker.patch.object`, `mocker.patch.dict`, `mocker.patch.multiple`,
  `mocker.spy`, `mocker.stub`, `mocker.MagicMock`.
- `fixture_count` — `@pytest.fixture` / `@fixture` decorators anywhere in the
  file, including on non-test functions. File scope only.
- `smell_hits` — array of smell hits at file scope. See `smell_hits[]` below.

## `test_functions[]`

One record per collected test function. This is the primary array; most of your
analysis reads from here.

- `nodeid` — `<file>::<test>`, or `<file>::<Class>::<method>` for class-based
  tests. **Parametrize is not expanded**, so there is never a `[...]` suffix.
- `file` — path to the file containing this test. Joins to `files[].path`.
- `line` — line of the `def`. **1-indexed**, matching what an editor shows.
- `assertion_count` — effective assertions in this test body: `assert`
  statements plus `with pytest.raises(...)` blocks, one each.
- `only_asserts_on_mock` — `true` when every `assert` statement in the body
  targets a mock-API attribute *and* the body contains no raises block. The
  matched attribute set is exact: `called`, `call_count`, `call_args`,
  `call_args_list`, `assert_called`, `assert_called_with`, `assert_called_once`,
  `assert_called_once_with`, `assert_not_called`, `assert_any_call`,
  `assert_has_calls`. **`false` when the test has zero assertions** — this flag
  never fires on an assertionless test, so it does not overlap `dead-test`.
- `patch_decorator_count` — `@patch`, `@mock.patch`, `@patch.object`, or any
  `@<something>.patch` decorator on this test.
- `stubs_count` — fixture-driven patch call sites in this test's body, from the
  closed list given under `files[].stubs_count`.
- `setup_to_assertion_ratio` — lines of setup per assertion, as a float.
  Computed as `(first_effective_assertion_line - def_line) / assertion_count`.
  When the body has no effective assertion, it degrades to
  `(last_body_line - def_line) / 1`, i.e. the height of the whole body — so a
  long assertionless test scores high here by design. Units are source lines.
  `0.0` means the first assertion is on the `def` line itself, which in practice
  means a one-line body.
- `called_names` — sorted, deduplicated, dot-joined names of calls in this test
  body, **filtered to project-internal names only**. Calls into the standard
  library, into third-party packages, and `self.*` calls are all excluded. An
  empty array means this test calls none of your own code by a name pycoati
  could resolve. Resolution goes through the file's imports, so aliases are
  canonicalized: `from myproj.repo import Repository` used as `Repository.save`
  appears as `myproj.repo.Repository.save`.
- `smell_hits` — array of smell hits at test scope. See below.
- `suspicion_score` — float in `0.0`–`1.0`. Composite; see the formula below.
  Higher is more suspicious. This is a ranking device, not a measurement — the
  absolute value has no meaning beyond ordering.

## `smell_hits[]`

Appears on both `files[]` and `test_functions[]`.

- `category` — a closed set of exactly two strings: `mock_only_assertions` and
  `mock_overuse`. Any other value means the schema changed under you.
- `test` — the nodeid this hit belongs to, or `null` for a file-scope hit. Use
  `null` here as the file-vs-test scope discriminator.
- `line` — 1-indexed line for test-scope hits; **`0` for file-scope hits**, which
  is a sentinel and not line 1.
- `evidence` — short human-readable string naming the counts that fired the
  rule, e.g. `7 mocks, 2 assertions`. Display it in your report; do not parse it.

## `sut_calls`

- `by_name` — array of entries, one per resolved project-internal name, sorted
  by name ascending.
- `top_called` — at most 20 names, descending by number of calling tests, ties
  broken by name ascending. A shortlist over `by_name`; carries no extra data.

### `sut_calls.by_name[]`

- `name` — the canonical dotted project-internal name.
- `test_function_count` — number of **distinct test functions** whose body calls
  this name. Not the number of call sites: a test calling it five times counts
  once.
- `test_nodeids` — sorted nodeids of those tests. Joins to
  `test_functions[].nodeid`.

## `top_suspicious`

Convenience shortlists, capped by `--top-suspicious` (default 20). They contain
no information that is not already in `test_functions` and `files`.

- `test_functions` — nodeids of the highest-scoring tests, descending by
  `suspicion_score`, ties broken by nodeid ascending.
- `files` — paths of the highest-scoring files, descending by file score, ties
  broken by path ascending.

## `tool`

- `name` — always `pycoati`.
- `version` — the binary's version. Record it in your report.
- `ran_pytest` — `true` only if the collection/durations subprocesses actually
  succeeded. **When `false`, every `suite.*` field is `null` and no
  timing-based finding is available.**
- `ran_coverage` — `true` only if the coverage subprocess succeeded. When
  `false`, `line_coverage_pct` is `null`.

## The suspicion score

```
suspicion_score = 0.35 * (only_asserts_on_mock ? 1 : 0)
                + 0.20 * min(patch_decorator_count / 5, 1)
                + 0.15 * sigmoid((setup_to_assertion_ratio - 8) / 4)
                + 0.20 * (assertion_count == 0 ? 1 : 0)
                + 0.10 * min(len(smell_hits) / 3, 1)
```

`sigmoid(x) = 1 / (1 + e^-x)`. Weights sum to `1.00`.

Consequences you need when reading scores:

- The setup term is never zero. Even a test with a ratio of `0` contributes
  `0.15 * sigmoid(-2)` ≈ `0.018`, so scores are never exactly `0.00`.
- `patch_decorator_count` saturates at **5** and `len(smell_hits)` at **3**. Ten
  patches score the same as five.
- The setup sigmoid inflects at a ratio of **8.0** with scale **4.0**: ratio 8
  contributes exactly half the setup weight.
- File scores are the mean of their tests' scores plus a bonus that grows at
  `0.05` per unit once `mock_construction_count / max(assertion_count, 1)`
  exceeds `1.0`, capped at `0.10`. File scores are therefore not directly
  comparable to test scores.

Two of the five terms fire on assertionless tests (`w_zero_asserts` plus an
inflated setup ratio), so an empty test lands between about `0.22` for a short
body and `0.35` for a long one — `0.35` is the ceiling of those two terms
together and is approached, never reached. Do not read a score in that band as
mock abuse.

## Smell thresholds

- `mock_only_assertions` fires per test when `only_asserts_on_mock` is `true`
  and `assertion_count > 0`. It fires per file when **every** test in the file
  that has at least one assertion has `only_asserts_on_mock` true; assertionless
  tests are excluded from that predicate.
- `mock_overuse` fires when `mocks > max(assertion_count, 2)` **and**
  `mocks / max(assertion_count, 1) > 2.0`. Both comparisons strict. `mocks` is
  `patch_decorator_count + stubs_count` at test scope, and
  `mock_construction_count + patch_decorator_count + stubs_count` at file scope.

---

# Part 2 — anti-pattern taxonomy

Eight patterns. For each: what it is, what flags it, and what to check by hand.

Work the list top to bottom against `top_suspicious.test_functions` first, then
sweep the full `test_functions` array for the patterns whose signals are not
score-bearing (`redundant`, `slow-without-reason`, `wrong-layer`).

A test may match more than one pattern. Record every match; the remedy page
decides precedence.

## 1. mock-as-assertion

**Definition.** The test's assertions are about the test double, not about the
system. It verifies that a mock was called, and nothing about what the code
produced. Such a test passes as long as the call shape is unchanged, including
when the underlying behaviour is broken or entirely absent.

**Signals.**
- `only_asserts_on_mock == true` and `assertion_count > 0`. This is the
  definitive signal; the flag is computed to mean exactly this pattern.
- Equivalently, a `smell_hits[]` entry with `category == "mock_only_assertions"`.
- Strength scales with `assertion_count`: a test with six mock-API assertions and
  nothing else is a stronger candidate than one with a single such assertion.

**Verify by reading.** Confirm the test has no observable outcome to assert on.
Legitimate exceptions exist: a test whose entire contract *is* the interaction —
verifying a retry fired three times, verifying a destructive call was *not* made
(`assert_not_called`) — is correctly written this way. Check whether the callee
returns a value or mutates state that the test could have asserted on instead.
If it does not, this is not a finding.

## 2. implementation coupling

**Definition.** The test is bound to how the code is structured rather than what
it does. It patches internal collaborators by dotted path, so any rename, move,
or refactor breaks the test even though behaviour is unchanged.

**Signals.**
- `patch_decorator_count >= 3` on a single test.
- `stubs_count >= 3` on a single test.
- `patch_decorator_count + stubs_count` high relative to `assertion_count` — the
  `mock_overuse` hit at test scope encodes exactly this ratio.
- `called_names` is short or empty while patch and stub counts are high: the
  test replaced more of your code than it calls.

**Verify by reading.** Look at what the patch targets name. Patching an external
boundary (network, clock, filesystem, subprocess) is appropriate and is not this
pattern. Patching internal functions of the module under test, or reaching
through several dotted segments into another module's internals, is. The
distinction is not in the inventory — you must read the target paths.

## 3. tautology

**Definition.** The assertion cannot fail, or can only fail if the test itself
is broken. It compares a value to itself, asserts a literal, or asserts a
property of a mock the test just configured.

**Signals.**
- `only_asserts_on_mock == true` combined with `patch_decorator_count > 0` and
  `called_names` empty — the test configured a double and then asserted on the
  double, never reaching project code.
- `assertion_count > 0` with `called_names` empty: the test asserts, but calls
  none of your code by any resolvable name.
- A high `suspicion_score` with a very low `setup_to_assertion_ratio`: assertions
  packed together with almost no setup preceding them.

**Verify by reading.** This pattern is only weakly visible in the inventory —
pycoati does not compare the two sides of an assertion. You must read the
assertion expressions. Confirm that the asserted value does not depend on any
project code path. Note that `called_names` is empty for tests that only
exercise pure standard-library or third-party behaviour, which may be a
deliberate contract test rather than a tautology.

## 4. setup-heavy

**Definition.** Arrangement dwarfs verification. The test spends dozens of lines
constructing state to check one thing, which makes it slow to read, expensive to
change, and unclear about what it protects.

**Signals.**
- `setup_to_assertion_ratio >= 8.0`. This is the sigmoid inflection point in the
  score — at 8 the term is at half strength, and it is near saturation by 20.
- `setup_to_assertion_ratio >= 15.0` is a strong candidate on its own.
- High `fixture_count` on the containing `files[]` record alongside a high
  ratio: setup that was extracted into fixtures and still leaves the test long.
- `assertion_count == 1` with a high ratio: maximum setup for minimum
  verification.

**Verify by reading.** The ratio is a line count and knows nothing about what
those lines do. A test built around a large but genuinely necessary data
fixture is not this pattern. Check whether the setup is incidental (could move
to a fixture or a builder) or essential (it *is* the scenario). Also check
whether the numerator is inflated by a long docstring or a block comment before
the first assertion — those count as lines.

## 5. redundant

**Definition.** Several tests exercise the same path with trivially different
inputs, so they fail together, are maintained together, and carry the
information of one test.

**Signals.**
- Two or more nodeids in the same `sut_calls.by_name[].test_nodeids` list whose
  `called_names` arrays are identical, and whose `assertion_count` values match.
- A `sut_calls.by_name[]` entry with a high `test_function_count` where the named
  callee is narrow — many tests converging on one function.
- Groups of `test_functions[]` entries in the same `file` with matching
  `assertion_count`, matching `patch_decorator_count`, and identical
  `called_names`.

**Verify by reading.** Identical signals do not prove identical coverage — two
tests can call the same function with inputs that take different branches, which
is exactly what good boundary tests do. Read the inputs and the expected values.
This is a candidate only when the inputs differ in a way the code does not
distinguish.

## 6. slow-without-reason

**Definition.** A test consumes disproportionate wall-clock time without
exercising anything that justifies it — sleeping, hitting a real service, or
building far more than it needs.

**Signals.**
- The test appears in `suite.slowest_tests[]` while its `test_functions[]` record
  shows a low `assertion_count` and short `called_names`.
- A `slowest_tests[]` entry whose `seconds` is an order of magnitude above the
  median of that array.
- High `patch_decorator_count` *and* slow: the test mocks heavily yet still takes
  real time, so the time is going somewhere the mocks do not cover.

**Joining rule — read this before using these signals.** `slowest_tests[].nodeid`
is parametrize-expanded and `test_functions[].nodeid` is not. To join, strip a
trailing `[...]` suffix from the `slowest_tests` nodeid and match the remainder
against `test_functions[].nodeid`. This is best-effort: several parametrized
cases collapse onto one test record, and a nodeid containing a literal `[` in a
parameter value can strip incorrectly. Treat an unmatched entry as unjoinable
and say so in the report rather than guessing.

**Precondition.** `tool.ran_pytest` must be `true`. When it is `false`,
`slowest_tests` is empty and **this entire pattern is unavailable** — state that
in your report rather than omitting the pattern silently.

**Verify by reading.** Some tests are legitimately slow: they are integration
tests, or the scenario needs volume. Confirm the time is accidental before
reporting.

## 7. wrong-layer

**Definition.** The test verifies behaviour at a level that cannot see it, or
duplicates a check that belongs elsewhere — a unit test asserting on end-to-end
wiring, or an integration test re-checking a pure function's arithmetic.

**Signals.**
- `called_names` spans several distinct top-level modules while the test lives in
  a file named for one of them.
- High `patch_decorator_count` and a long `called_names` list at once: the test
  reaches broadly and then stubs out most of what it reached.
- A test in a file whose `files[].fixture_count` is high while its own
  `called_names` names a single pure function — heavy machinery around a
  narrow check.
- The test's `file` path and the module prefix of its `called_names` entries
  disagree.

**Verify by reading.** The inventory has no model of your architecture. It does
not know which directories are unit tests and which are integration tests, and
it cannot tell a layering violation from a deliberate seam. This pattern is
always a judgement call — treat every signal here as weak.

## 8. dead-test

**Definition.** The test cannot fail for the reason it exists. It has no
assertions, or it exercises no project code, so it survives on the absence of an
exception.

**Signals.**
- `assertion_count == 0`. Definitive for the assertionless variant. Note this
  contributes `0.20` to the score on its own and inflates
  `setup_to_assertion_ratio` to the full body height, so these tests cluster near
  the top of `top_suspicious.test_functions`.
- `assertion_count == 0` **and** `called_names` empty: the test neither asserts
  nor calls project code.
- The test's nodeid appears in no `sut_calls.by_name[].test_nodeids` list.

**Verify by reading.** Two legitimate shapes look identical here. A smoke test
that asserts nothing but would raise on failure is doing real work — importing a
module, constructing an object, running a migration. A test whose assertions are
made through a helper function will show `assertion_count == 0` because pycoati
counts `assert` statements syntactically in the test body and does not follow
calls into helpers. Check for both before reporting.

There is no per-test coverage in this schema, so "this test covers nothing" is
not something the inventory can tell you. Do not claim it.

---

# What to produce

For each confirmed candidate, record: `nodeid`, `file`, `line`, the pattern, the
specific field values that flagged it, and one sentence on what you found when
you read the test. Also record `tool.version`, `tool.ran_pytest`, and
`tool.ran_coverage` — they determine which patterns were available to you.

Do not edit anything yet.

next: run `pycoati guide remediate`
