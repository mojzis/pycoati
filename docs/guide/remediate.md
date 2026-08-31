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

Group findings by rung, and put the escalation rung's findings first — those are
the ones a human has to decide, and they should not be at the bottom of a long
list.

Also state which patterns were unavailable. If `tool.ran_pytest` was `false`,
`slow-without-reason` was never checked, and the report must say so rather than
implying a clean result.

## The remedy ladder

Take the **first rung that applies** to a finding. Do not skip ahead. A finding
that matches rungs 1 and 4 is a rung 1 finding.

**Rung 1 — dead-test → delete.**
A test with no assertions that calls no project code protects nothing. Delete
it. Before deleting, confirm once more that it is not a smoke test whose value
is that it does not raise, and that its assertions are not made through a helper
function. If either is true, it is not a rung 1 finding — drop it from the
ladder entirely.

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
decision each needs, findings reverted and why, and the before/after of
`top_suspicious.test_functions`. State plainly if any finding was left
untouched.

next: run `pycoati . --output inventory.json` and `pycoati guide analyze` to
confirm the result
