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
  `"3"`. Any other value means this guide and the inventory disagree; stop and
  re-run `pycoati guide analyze` from the binary that produced the file.
- `project` — object. Identity of the audited project.
- `suite` — object. Suite-level runtime metrics from pytest.
- `files` — array of file records, one per discovered test file, sorted by path.
  A file with zero tests still gets a record in directory mode.
- `test_functions` — array of test records, one per collected test function.
- `sut_calls` — object. Aggregated project-internal call frequency.
- `top_suspicious` — object. Pre-ranked shortlists.
- `accepted` — object. Reviewed suppressions in effect for this run.
- `tool` — object. Producer metadata.

## Top level, workspace

A workspace payload carries `schema_version`, `workspace_root`, `members`, and
`tool`, and nothing else. Each member carries its own `accepted` block —
acceptance is per member, because a nodeid is only unique within one.

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
- `assertion_count` — effective assertions across every test body in the file:
  the sum of `test_functions[].assertion_count` for this file's tests, with the
  same definition.
- `mock_construction_count` — sum of `test_functions[].mock_construction_count`
  for this file's tests, with the same definition.
- `patch_decorator_count` — sum of the per-test decorator counts in this file.
- `stubs_count` — fixture-driven patch call sites in this file. The matched set
  is closed and exact: `monkeypatch.setattr`, `monkeypatch.setenv`,
  `monkeypatch.delattr`, `monkeypatch.delenv`, `monkeypatch.context`,
  `monkeypatch.syspath_prepend`, `monkeypatch.chdir`, `mocker.patch`,
  `mocker.patch.object`, `mocker.patch.dict`, `mocker.patch.multiple`,
  `mocker.spy`, `mocker.stub`, `mocker.Mock`, `mocker.MagicMock`,
  `mocker.AsyncMock`, `mocker.create_autospec`.
- `fixture_count` — `@pytest.fixture` / `@fixture` decorators anywhere in the
  file, including on non-test functions. File scope only.
- `smell_hits` — array of smell hits at file scope. See `smell_hits[]` below.

## `test_functions[]`

One record per collected test function. This is the primary array; most of your
analysis reads from here.

- `nodeid` — `<file>::<test>`, or `<file>::<Class>::<method>` for class-based
  tests, with one `::<Class>` segment per level for nested classes
  (`<file>::TestOuter::TestInner::test_x`). Classes are collected when named
  `Test*` or when they subclass `unittest.TestCase` /
  `IsolatedAsyncioTestCase` (directly or through same-file base classes),
  matching what pytest collects; in a `TestCase` subclass every `test*`
  method counts, in other classes and at module level every `test_*`.
  **Parametrize is not expanded**, so there is never a `[...]` suffix.
- `file` — path to the file containing this test. Joins to `files[].path`.
- `line` — line of the `def`. **1-indexed**, matching what an editor shows.
- `assertion_count` — effective assertion sites in this test body, one each:
  `assert` statements, `with pytest.raises(...)` blocks, `self.assertXxx(...)`
  calls inside a test class, and bare mock-API assertion calls written as a
  statement on their own (`m.assert_called_once_with(1)`). Every one of these
  fails the test at the line it sits on. Verification that happens elsewhere
  is counted separately — see `external_verification_count`.
- `only_asserts_on_mock` — `true` when the test has at least one assertion and
  every one of them targets the mock API, i.e. `mock_assertion_count ==
  assertion_count > 0`. A raises block never targets the mock API, so one in
  the body makes this `false`. **`false` when the test has zero assertions** —
  this flag never fires on an assertionless test, so it does not overlap
  `dead-test`.
- `mock_assertion_count` — the part of `assertion_count` that inspects a test
  double rather than a value the code under test produced. The mock-API
  attribute set is closed and exact: `called`, `call_count`, `call_args`,
  `call_args_list`, `assert_called`, `assert_called_with`,
  `assert_called_once`, `assert_called_once_with`, `assert_not_called`,
  `assert_any_call`, `assert_has_calls`. A site counts here when it is:
  - a bare call to one of the `assert_*` names on any receiver except bare
    `self` — `m.assert_called_once_with(1)`, `self.client.send.assert_called()`;
  - an `assert` whose expression reads one of those attributes, directly or
    through `not`, parentheses, `len(...)`, an indexing or attribute access
    hanging off it (`m.call_args[0][0]`, `m.call_args.kwargs`), or either side
    of a comparison — `assert not m.called`, `assert m.call_count == 2`,
    `assert m.call_count == len(items)`. `assert result == m.return_value` does
    **not** count: `return_value` is configuration, not part of the set;
  - a `self.assertXxx(...)` call where at least one argument reads one of those
    attributes and every other argument is a literal, a mock assertion helper
    (`call(...)`, `call.save(...)`, `ANY`, imported from `unittest.mock` or
    `mock`) or a container of those — `self.assertEqual(m.call_count, 1)`,
    `self.assertFalse(m.called)`. `self.assertEqual(m.call_count, len(items))`
    does not count, because the argument list does not say which side is the
    value under test. A `msg=` argument is ignored.

  `and` / `or` counts only when every operand reads the mock API
  (`assert m.called and m.call_count == 1`); one real operand
  (`assert m.called and result == 3`) makes it real verification. So does an
  arbitrary call around the attribute (`helper(m.call_count)`).
- `patch_decorator_count` — `@patch`, `@mock.patch`, `@patch.object`, or any
  `@<something>.patch` decorator on this test.
- `patch_call_count` — patches this test installs *without* a decorator: calls
  in the body resolving to `patch`, `patch.object`, `patch.dict` or
  `patch.multiple` from `unittest.mock` or the `mock` backport (usually as
  `with patch(...):`), plus `mocker.patch`, `mocker.patch.object`,
  `mocker.patch.dict` and `mocker.patch.multiple`. Spelling does not matter:
  `with mock.patch(...)` under `from unittest import mock` counts. Read it
  beside `patch_decorator_count` — together they are every patch the test
  installs, and the score's patch term reads their sum. **Do not add it to
  `mock_construction_count` or `stubs_count`**: each site here is already in
  exactly one of those.
- `mock_construction_count` — Mock-API constructions in this test's body:
  calls resolving to `Mock`, `MagicMock`, `AsyncMock`, `create_autospec`,
  `patch`, `patch.object`, `patch.dict` or `patch.multiple` in `unittest.mock`
  or the `mock` backport. Call heads are resolved through the file's imports,
  so `mock.Mock()` under `from unittest import mock` and an aliased
  `MagicMock as MM` both count, while `factory.Mock()` or a `Mock` imported
  from your own package do not. A bare `Mock()` that no import binds (star
  import, conftest) still counts. `mocker.Mock()` and friends are stubs, not
  constructions.
- `stubs_count` — fixture-driven patch call sites in this test's body, from the
  closed list given under `files[].stubs_count`.
- `setup_to_assertion_ratio` — lines of setup per assertion, as a float.
  Computed as
  `(first_effective_assertion_line - def_line) / effective_assertion_count`,
  where an effective assertion is an `assert` statement, a
  `with pytest.raises(...)` block, a bare mock-API assertion call
  (`m.assert_called_once_with(1)`) or a `helper_assertion_count` site.
  `self.assertXxx(...)` calls raise
  `assertion_count` without feeding this denominator.
  When the body has no effective assertion but does carry external
  verification, the first such call site stands in for the first assertion:
  `(first_verification_line - def_line) / external_verification_count`. When it
  has neither, it degrades to `(last_body_line - def_line) / 1`, i.e. the height
  of the whole body — so a long unverified test scores high here by design.
  Units are source lines. `0.0` means the first assertion is on the `def` line
  itself, which in practice means a one-line body.
- `external_verification_count` — call sites in this test body that verify an
  outcome the body itself cannot see, because a failure raised outside this
  process still fails the test. The matched set is closed and exact: a call
  resolving to `subprocess.check_call` or `subprocess.check_output`; a call
  resolving to `subprocess.run` that passes the literal `check=True`; and
  `<receiver>.check_returncode()`. Import aliases are resolved first, so
  `import subprocess as sp` and `from subprocess import run as r` both match.
  **This is not part of `assertion_count`**, which stays a syntactic count of
  assertion sites in the body — read the two together. A non-zero value means
  zero assertions is not the absence of verification; it does **not** mean the
  child process checks anything worth checking. See `dead-test` below.

  Four shapes are deliberately **not** counted, so a `0` here is sometimes a
  known miss rather than a verdict: a call against a name the test replaced
  with a double (`@patch` on the test or its class, `monkeypatch.setattr`, a
  patcher started in `setUp`, a fixture of the same name, a local assigned
  from a `Mock()`); a call whose failure may never reach pytest — inside a
  nested `def`/`lambda` or a generator expression, inside a `try` that has an
  `except`, or inside `with contextlib.suppress(...)`; a call under
  `from subprocess import *`, which binds an unknown set of names; and every
  `check_returncode()` in a test that patched any `subprocess` name at all,
  including one on a real `CompletedProcess`.
- `helper_assertion_count` — verification this test delegates to a call
  rather than writing as an assertion site. Three closed sources, one each:
  - a call resolving through the file's imports to `numpy.testing.assert_*`,
    `pandas.testing.assert_*`, `polars.testing.assert_*` or `pytest.fail` —
    `np.testing.assert_allclose(...)` under `import numpy as np` counts;
  - a `with pytest.warns(...):` or `with pytest.deprecated_call():` block,
    one per block, like a raises block;
  - a call to a **same-file helper** whose own body holds a non-mock
    assertion: a module-level function called by bare name (and not shadowed
    by one of the test's parameters), or `self.<method>(...)` for a method of
    the test's own class. One level only: a helper that verifies by calling
    another helper does not count, nor does anything imported or defined in
    `conftest.py`. A helper whose only assertions target the mock API does
    not count either.

  **Not part of `assertion_count`** — read the two together, like
  `external_verification_count`. Each site counts as a non-mock assertion
  everywhere else: it switches off `zero_asserts`, makes
  `only_asserts_on_mock` `false`, and is an assertion site for
  `setup_to_assertion_ratio`. It does not enter the `mock_overuse`
  denominator, which stays `assertion_count`.
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
- `fingerprint` — 16 hex digits identifying this test's current source,
  decorators included. Trailing whitespace and blank lines are ignored;
  everything else, comments included, is part of it. Its only use is the
  `fingerprint` key of an accept entry: recording it there makes the
  acceptance lapse when the test is edited. Not a checksum of anything else,
  and not comparable across tests. `null` only when the source could not be
  read as UTF-8 — an entry pinned against such a test reports
  `content_changed` rather than silently matching.
- `accepted_signals` — sorted signal names a baseline entry accepted for this
  test, or `[]`. A signal listed here was reviewed and signed off; **it is
  still computed, still scored, and still present in `smell_hits`**. Read this
  alongside `accepted.findings`, which carries the reason for each.

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
  `suspicion_score`, ties broken by nodeid ascending. **A test whose every
  active signal was accepted is omitted** unless the scan ran with
  `--include-accepted`; `accepted.included_in_shortlist` tells you which. The
  test's own record in `test_functions[]` is there either way.
- `files` — paths of the highest-scoring files, descending by file score, ties
  broken by path ascending. Acceptance is per test and never filters this
  list: a file whose tests were all accepted keeps its score and its place.

## `accepted`

Reviewed per-test suppressions, read from the baseline file described on the
setup page. This block is the *only* place a suppression is visible — nothing
is removed from `files`, `test_functions`, `smell_hits`, or any count.

- `path` — the baseline file that was read, or `null` when none was found or
  the scan ran with `--no-accept`. `null` with a non-empty `findings` array is
  impossible.
- `included_in_shortlist` — `true` when the scan ran with
  `--include-accepted`, meaning accepted findings were **not** held back from
  `top_suspicious.test_functions`. Report this: it changes what the shortlist
  means.
- `findings` — array of acceptances that matched a live finding.
- `stale` — array of acceptances that matched nothing. Each one is a finding
  that is actionable again, or an entry to delete. Both are reported to stderr
  as warnings during the scan.

### `accepted.findings[]`

- `test` — nodeid of the accepted test. Joins to `test_functions[].nodeid`.
- `signal` — the one signal accepted for that test, from the closed set:
  `mock_only_assertions`, `mock_overuse`, `zero_asserts`, `high_setup_ratio`.
  The first two mirror `smell_hits[].category`; the last two are score-bearing
  signals that produce no smell hit. `zero_asserts` fires on the same
  condition as the score's term — `assertion_count == 0`,
  `external_verification_count == 0` **and** `helper_assertion_count == 0` —
  so a test that checks a child process or verifies through a helper never
  offers it, and an entry accepting it on such a test goes `stale`.
- `reason` — the reviewer's recorded justification. Always non-empty; the file
  will not parse without it. **Quote it in your report** — it is the whole
  content of the review decision, and re-deriving it is what this block
  exists to prevent.
- `reviewed` — free-form provenance note (a date, a name, a PR link), or
  `null`. Never interpreted.
- `fingerprint` — the `test_functions[].fingerprint` recorded at review time,
  or `null` when the entry did not pin one. When present it matched, or this
  entry would be under `stale` instead.

### `accepted.stale[]`

- `test`, `signal`, `reason` — copied verbatim from the entry that did not
  apply.
- `status` — why, from a closed set of three:
  - `unknown_test` — no test in this inventory carries that nodeid. The test
    was renamed, moved, or deleted; the entry is dead weight.
  - `signal_not_active` — the test is there and the signal no longer fires.
    The underlying problem was fixed, or the heuristic changed.
  - `content_changed` — the test's `fingerprint` differs from the one the
    entry pinned, or the test has no fingerprint at all. The acceptance is
    suspended until a human looks again. This is the only status that can
    hide a real, currently-firing finding, so treat it as an unreviewed
    finding, not as bookkeeping.
- `detail` — human-readable explanation, carrying the current fingerprint for
  the `content_changed` case. Display it; do not parse it.

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
                + 0.20 * min((patch_decorator_count + patch_call_count) / 5, 1)
                + 0.15 * sigmoid((setup_to_assertion_ratio - 8) / 4)
                + 0.20 * (unverified ? 1 : 0)
                + 0.10 * min(len(smell_hits) / 3, 1)

unverified = assertion_count == 0 and external_verification_count == 0
             and helper_assertion_count == 0
```

`sigmoid(x) = 1 / (1 + e^-x)`. Weights sum to `1.00`.

Consequences you need when reading scores:

- The setup term is never zero. Even a test with a ratio of `0` contributes
  `0.15 * sigmoid(-2)` ≈ `0.018`, so scores are never exactly `0.00`.
- Patches (`patch_decorator_count + patch_call_count`) saturate at **5** and
  `len(smell_hits)` at **3**. Ten patches score the same as five, and a `with
  patch(...)` scores the same as a `@patch`.
- The setup sigmoid inflects at a ratio of **8.0** with scale **4.0**: ratio 8
  contributes exactly half the setup weight.
- File scores are the mean of their tests' scores plus a bonus that grows at
  `0.05` per unit once `mock_construction_count / max(assertion_count, 1)`
  exceeds `1.0`, capped at `0.10`. File scores are therefore not directly
  comparable to test scores.

Two of the five terms fire on unverified tests (`w_zero_asserts` plus an
inflated setup ratio), so an empty test lands between about `0.22` for a short
body and `0.35` for a long one — `0.35` is the ceiling of those two terms
together and is approached, never reached. Do not read a score in that band as
mock abuse.

A test with `assertion_count == 0` and `external_verification_count > 0` or
`helper_assertion_count > 0` does not reach that band *for those reasons*: the `w_zero_asserts` term is off, and
the setup ratio is measured to the first verification call site instead of the
whole body height, so it is no longer inflated — though it still contributes,
exactly as it does for a test with a real assertion. The mock, patch and smell
terms score as they would on any other test, so such a test can still reach
`0.22`+ on those. When it does, that is a real signal about mock abuse, not
about the test being empty — read the field, not just the score.

## Smell thresholds

- `mock_only_assertions` fires per test when `only_asserts_on_mock` is `true`
  and `assertion_count > 0`. It fires per file when **every** test in the file
  that has at least one assertion has `only_asserts_on_mock` true; assertionless
  tests are excluded from that predicate.
- `mock_overuse` fires when `mocks > max(assertion_count, 2)` **and**
  `mocks / max(assertion_count, 1) > 2.0`. Both comparisons strict. `mocks` is
  `mock_construction_count + patch_decorator_count + stubs_count` at both test
  and file scope. Each patch lands in exactly one of those three, whatever its
  spelling: a decorator in `patch_decorator_count`, `with patch(...)` in
  `mock_construction_count`, `mocker.patch(...)` in `stubs_count`.

---

## Accepted findings and what they mean for your report

An acceptance is a **review decision recorded once**, not a rule change and
not a threshold. Read it as: a human looked at this exact test, agreed the
signal fires, and wrote down why it is correct anyway.

They live in `.pycoati-accept.toml` at the project root, one `[[accept]]`
block per (test, signal) pair, each with a required `reason`. `accepted.path`
names the file this run actually read. Run `pycoati guide setup` for the full
format — you do not need it to read an inventory, only to propose an entry.

Three consequences for how you read an inventory:

- **Do not re-litigate an accepted finding.** If `accepted.findings` covers a
  test and signal, that judgement has been made. Quote the reason and move on.
- **An acceptance covers one signal, never a test.** A test with
  `accepted_signals: ["zero_asserts"]` that also carries a `mock_overuse` hit
  is a live candidate for that hit, and it will be on the shortlist. Work it.
- **Every `stale` entry is work.** A `content_changed` entry in particular
  means the accepted test was edited since review and the finding is live
  again — report it as a candidate, and say the acceptance lapsed.

If you cannot confirm a finding by reading the test, and the reason it looks
suspicious is genuinely project context the AST cannot see, the right output
is a proposed accept entry in your report — test nodeid, signal, reason,
fingerprint — for a human to add to `.pycoati-accept.toml`. Not a threshold
change, and not a deleted test. The remediate page gives the exact block to
put in the report, and the rule that you propose it rather than write it.

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
  definitive signal; the flag is computed to mean exactly this pattern. It
  covers bare `m.assert_called_once_with(...)` statements, `assert
  m.call_count == 2`, `assert not m.called` and
  `self.assertEqual(m.call_count, 1)` alike.
- `mock_assertion_count > 0` with `only_asserts_on_mock == false` is the
  partial form: some assertions verify the double, some verify output. Not a
  candidate on its own.
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
- `patch_decorator_count + patch_call_count >= 3` on a single test — every
  patch, decorator or `with`.
- `stubs_count >= 3` on a single test.
- `mock_construction_count + patch_decorator_count + stubs_count` high relative
  to `assertion_count` — the `mock_overuse` hit at test scope encodes exactly
  this ratio.
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
- `assertion_count == 0`, `external_verification_count == 0` **and**
  `helper_assertion_count == 0`. Definitive for the assertionless variant. Note this contributes `0.20` to the score on
  its own and inflates `setup_to_assertion_ratio` to the full body height, so
  these tests cluster near the top of `top_suspicious.test_functions`.
- The same three zeros **and** `called_names` empty: the test neither asserts
  nor calls project code.
- The test's nodeid appears in no `sut_calls.by_name[].test_nodeids` list
  **and** `external_verification_count == 0` — a test that shells out calls no
  project code by construction, so this signal fires on it by default.

**Not this pattern: verification in a child process.** A test with
`assertion_count == 0` and `external_verification_count > 0` asserts in a
process the AST cannot see and propagates the failure — `subprocess.run(...,
check=True)` raises `CalledProcessError`, which fails the parent test. This is
how a packaging or CLI integration test checks something the source-tree
interpreter cannot: it installs a wheel into a temporary directory, then runs an
isolated interpreter that asserts on the installed artifact. Those tests are
correct as written; do not report them as assertionless, and do not ask for a
local assertion that would have to duplicate the child's.

The evidence is narrow on purpose. `check=True` proves the child's failure
reaches the parent — nothing more. If the child command asserts nothing (a bare
`--help`, a `true`), the test is still dead, and only reading it tells you which
you have. A subprocess call with no `check=True` proves nothing at all and does
not set this field: it needs its own evidence, such as an `assert
completed.returncode == 0`, which is a normal assertion and counts as one.

**Not this pattern: verification through a helper.** A test with
`assertion_count == 0` and `helper_assertion_count > 0` verifies through a call
that raises on failure — a `pandas.testing.assert_frame_equal`, a
`pytest.warns` block, or a same-file `check_*` helper that asserts. Do not
report it as assertionless.

**Verify by reading.** Three legitimate shapes still look identical here. A
smoke test that asserts nothing but would raise on failure is doing real work —
importing a module, constructing an object, running a migration. A test whose
assertions live in a helper pycoati cannot see — imported from another module,
defined in `conftest.py`, or reached through a second helper — shows all three
counts at zero, because helpers are followed one level deep within the file
only. And a test that verifies in a child process the parser could not prove,
as above. Check for all three before reporting.

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
