# pycoati — remediate

You have a list of confirmed candidates from the analyze page. This page tells
you what to do with them and in what order.

## The approval gate

**Produce the full findings report, present it, and edit no file until a human
approves it.**

This is the first rule on the page because it is the one that matters. Not a
draft edit, not a "small obvious one first", not a branch you plan to show
later. Zero file modifications exist between finishing analysis and receiving
approval.

The report must contain, for every finding:

- `nodeid`, `file`, and `line`.
- The anti-pattern, and the specific field values that flagged it.
- What you found when you read the test.
- The rung of the ladder it lands on, and the concrete action that implies.
- Anything that made you uncertain.
- The mutation check result for its test, when one ran (see below).

Group findings by rung, and put the escalation rung's findings first — those are
the ones a human has to decide, and they should not be at the bottom of a long
list.

Also state which patterns were unavailable. If `tool.ran_pytest` was `false`,
`slow-without-reason` was never checked, and the report must say so rather than
implying a clean result.

## Mutation check — evidence for the report

A signal says a test *looks* like it checks nothing. A small mutation run shows
whether it does: change the code the test calls, run only that test, and see if
it notices. Do this before you present the report, for the top of the
shortlist only. Never run the whole suite's mutation run — that takes hours;
this takes minutes.

Skip the check, and say so in the report, when `tool.ran_pytest` was `false`
or the project's tests cannot run. A mutation run needs a passing baseline.

### Pick the tests

1. Take confirmed candidates from `top_suspicious.test_functions`, top down,
   at most five.
2. Drop any whose `called_names` is empty — there is no project code to
   mutate, and the finding stands on the inventory alone.
3. Drop any with a current entry in `.pycoati-mutation.toml` (see below).
   Quote that entry's result in the report instead.

### Run it

Use [mutmut](https://github.com/boxed/mutmut) 3. It can be limited to single
functions and to a single test, which is what keeps the run small.

mutmut reads its settings only from `pyproject.toml` or `setup.cfg`, and
writes a `mutants/` folder. Neither may touch the working tree before approval,
so run it in a throwaway copy:

```sh
git worktree add --detach ../coati-mutation HEAD
cd ../coati-mutation
```

If the test or the code it calls has uncommitted changes, the copy does not
have them — check the committed version or skip the test.

In the copy, add to `pyproject.toml`, one test at a time:

```toml
[tool.mutmut]
source_paths = ["src"]                 # where the project's code lives
pytest_add_cli_args_test_selection = ["tests/test_repo.py::test_save"]
```

Then run only the mutants of the functions that test calls. Each
`called_names` entry maps to a mutant-name pattern:

- `myproj.repo.save` → `myproj.repo.x_<function>__mutmut_*`, here
  `myproj.repo.x_save__mutmut_*`.
- `myproj.repo.Repository.save` → `myproj.repo.xǁRepositoryǁsave__mutmut_*`.
- A class called as a constructor, `myproj.repo.Repository` →
  `myproj.repo.xǁRepositoryǁ__init____mutmut_*`.

The module part is the path under `source_paths`, so `src/myproj/repo.py`
with `source_paths = ["src"]` gives `myproj.repo`.

```sh
uv run --with mutmut mutmut run "myproj.repo.x_save__mutmut_*" "myproj.repo.xǁRepositoryǁsave__mutmut_*"
uv run --with mutmut mutmut results --all true
```

Run it in the project's own environment, so the tests can import the project.
`mutmut results --all true` lists every mutant name generated, also after a
run whose pattern matched nothing, so use it to fix a pattern. When done,
leave the copy and remove it: `git worktree remove --force ../coati-mutation`.

### Read the result

Each mutant ends as one of:

- `killed` — the test failed on the changed code. It checks that code.
- `survived` — the test passed on changed code. **This is the evidence.** A
  test that lets mutants of the code it calls survive is not checking that
  code. `mutmut show <name>` prints the diff; put one or two in the report.
- `no tests` — the test never reached the function. Its `called_names` entry
  is a call pycoati saw but the test does not execute (it was patched out, or
  sits on a branch never taken). That is evidence too, often of
  mock-as-assertion.

A survivor is a lead, like any signal. Some mutants change nothing a caller
can see (an equivalent mutant). Read the diff before you count it.

The check never changes a rung on its own. It confirms or weakens a finding
you already have. A test that killed every mutant is weaker as a candidate —
say so, and look again at why it was flagged.

### Remember what was checked

Results go in `.pycoati-mutation.toml` at the project root, one entry per
checked test, so a later run checks only new or changed tests:

```toml
[[check]]
test = "tests/test_repo.py::test_save"
fingerprint = "<test_functions[].fingerprint at check time>"
targets = ["myproj.repo.Repository.save"]
source_files = ["src/myproj/repo.py"]
commit = "<git rev-parse HEAD of the copy>"
killed = 3
survived = 4
no_tests = 0
date = "2026-10-28"
tool = "mutmut 3.8.1"
```

An entry is current only when both hold:

- its `fingerprint` equals the test's `fingerprint` in today's inventory, so
  the test was not edited; and
- `git diff --quiet <commit> -- <source_files>` exits 0, so the code it
  calls was not edited either.

Otherwise check the test again and replace the entry. Delete entries whose
test no longer exists.

This file is a measurement log, not a review decision, so you write it — unlike
`.pycoati-accept.toml`. But writing it is still a file change: put the new
entries in the report and write them as the first step after approval.

The record is for the next agent run only: pycoati never reads this file, and
the tool stays stateless.

## The remedy ladder

Take the **first rung that applies** to a finding. Do not skip ahead. A finding
that matches rungs 1 and 4 is a rung 1 finding.

**Rung 0 — already accepted → skip it.**
A finding covered by `accepted.findings` has been reviewed and signed off.
Quote the reason in your report and take no action. This rung exists so the
ladder never walks over a recorded decision. It applies only to the exact
(test, signal) pair listed, and never to an entry under `accepted.stale` — a
lapsed acceptance is an ordinary finding again, and a `content_changed` one
means the test was edited since review.

**Rung 1 — dead-test → delete.**
A test with no assertions that calls no project code protects nothing. Delete
it. Before deleting, confirm once more that it is not a smoke test whose value
is that it does not raise, that its assertions are not made through a helper
function, and that it does not verify in a child process
(`external_verification_count > 0` — a failing child fails the test through
`CalledProcessError`). If any is true, it is not a rung 1 finding — drop it from
the ladder entirely.

**Rung 2 — tautology → fix the assertion, or delete.**
Prefer fixing: replace the self-referential assertion with one about a value the
system under test produced. If no such value exists — the test calls nothing
that can be asserted on — delete it. Do not "fix" a tautology by adding an
assertion about a mock; that trades this finding for a rung 4 finding.

**Rung 3 — redundant → parametrize, or delete.**
When several tests differ only in input values, collapse them into one
parametrized test carrying every case. When the extra tests add no case at all,
delete them and keep one. Preserve every distinct input that reaches a distinct
branch — collapsing cases that actually differ is a coverage loss, not a
cleanup.

**Rung 4 — mock-as-assertion → rewrite against observable behaviour.**
Replace assertions about the double with assertions about what the code
produced: the return value, the state it changed, the output it wrote. Keep an
interaction assertion only when the interaction is the contract — a retry count,
a call that must not happen. If the behaviour genuinely has no observable
outcome, that is a design problem, not a test problem: escalate at rung 6
instead.

**Rung 5 — slow-without-reason → remove the cost, or escalate.**
When the cost is accidental — a real sleep, an unnecessarily large fixture, a
call that should have been stubbed at a boundary — remove it and keep the
assertions identical. When the time is inherent to what the test covers, do not
touch it; escalate at rung 6 so a human can decide whether the test belongs in
this suite at all.

**Rung 6 — implementation coupling, setup-heavy, wrong-layer → stop and
escalate to a human.**
These three are design judgements. Fixing implementation coupling means changing
what the code exposes as a seam. Fixing setup-heavy means deciding what the
scenario really is. Fixing wrong-layer means knowing the architecture. None of
these can be settled from the inventory, and a mechanical fix makes the suite
worse while making the score better.

Do not edit these. For each, report: `file`, test name, `line`, the signals that
flagged it, what you found reading the test, and the specific decision you need
from a human. Then move on to the next finding.

This rung is where a large share of real findings land. That is the correct
outcome, not a failure to finish.

**Rung 7 — correct as written, for a reason only the project knows → propose
an accept entry.**
Some tests trip a signal and are right anyway, in a way no amount of static
analysis will ever see: assertions made in a child interpreter, a smoke
contract whose value is that it does not raise, assertions made through a
helper. When you have read the test and concluded that, the finding is not a
code change — it is a review decision to record.

Put the proposed entry in your report, for a human to add to
`.pycoati-accept.toml`:

```toml
[[accept]]
test = "<nodeid>"
signals = ["<the signals you confirmed>"]
reason = "<what you found when you read the test>"
fingerprint = "<test_functions[].fingerprint>"
```

**Do not write this file yourself.** It records someone's judgement, and
signing it on their behalf is exactly the thing the approval gate exists to
prevent. Name only the signals you actually confirmed; a test you accept for
`zero_asserts` must still surface tomorrow if it grows a `mock_overuse` hit.

## After approval — the edit loop

Approval covers the report that was approved. Work one finding at a time, in
ladder order.

For each finding:

1. **Run the affected tests before touching anything.** Record the result. If
   they already fail, stop — you cannot verify a change against a broken
   baseline. Report it and move to the next finding.
2. Make the change for this one finding.
3. **Run the same tests again.** They must pass.
4. **Re-scan:** `pycoati . --output inventory.json`.
5. **Confirm two things in the new inventory:** the finding is gone, and no new
   finding appeared. A rising `suspicion_score` anywhere you did not touch, or a
   new `smell_hits` entry, counts as a new finding.
6. **On any failure at step 3 or step 5, revert this finding's change** and
   record what happened. Do not attempt a second approach without reporting the
   first one failed.

Deleting a test always reduces coverage. Before a rung 1 or rung 2 deletion,
check whether the behaviour is covered elsewhere. If it is not, the finding
becomes a rung 6 escalation: the test is bad *and* the behaviour is untested,
which is a decision for a human.

## Do not

- **Do not raise a threshold or edit configuration to make a finding
  disappear.** pycoati's thresholds are compiled into the binary and are
  deliberately not configurable per run. `--top-suspicious` changes list length
  only — shortening the list does not resolve anything, and using it to trim a
  report is falsifying the report.
- **Do not write or edit `.pycoati-accept.toml`.** Propose entries in the
  report (rung 7) and let a human add them. Adding an acceptance to make a
  finding go away, widening an existing entry to cover a new signal, or
  re-pinning a `content_changed` entry's fingerprint without reading the test
  are all the same move as raising a threshold, dressed as a review.
- **Do not delete or skip a test to make a gate pass.** Adding a skip marker,
  an expected-failure marker, or a narrowing condition to silence a failing test
  is out of scope for every rung on this ladder.
- **Do not weaken an assertion to make a test pass after your edit.** If your
  rewrite makes the test fail, the rewrite is wrong. Revert.
- **Do not batch edits across findings before the report is approved**, and do
  not batch them after approval either — one finding, one verification cycle. A
  batch you cannot revert individually is a batch you cannot verify.
- **Do not edit production code to satisfy a test finding.** Every rung on this
  ladder changes tests. A finding that can only be fixed by changing the system
  under test is a rung 6 escalation.
- **Do not report a score improvement as the outcome.** The score is a ranking
  device. Report what changed about the tests.

## Finishing

Report, in this order: findings resolved and how, findings escalated and what
decision each needs, accept entries proposed and for which signals, findings
reverted and why, the tests mutation-checked and the entries written to
`.pycoati-mutation.toml`, and the before/after of
`top_suspicious.test_functions`.
State plainly if any finding was left untouched.

Also list every `accepted.stale[]` entry, with what you did about it: worked
the finding, or proposed deleting a dead entry. A stale list that grows every
scan is a baseline nobody is reading.

next: run `pycoati . --output inventory.json` and `pycoati guide analyze` to
confirm the result
