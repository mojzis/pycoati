# Add a `guide` command to coati

coati is a Rust CLI that inventories a Python test suite and emits a structured `inventory.json` for downstream analysis by a coding agent. This task adds a `coati guide` command so the tool carries its own workflow instructions: the agent runs the scan, the output points at the guide, the guide tells it what to do next. The instructions ship with the binary, so they can never drift from the schema the binary actually emits.

Work in the staged order below. Each STOP is a hard gate: show the requested output and wait for my approval before continuing. Do not scaffold ahead.

## Step 1 — Explore and propose (STOP after this)

Read the repo first: README, CLAUDE.md, `src/`, the clap command structure, the serde structs that define the `inventory.json` schema, how config is loaded (`coati.toml` / `[tool.coati]` in `pyproject.toml` or whatever exists), and the default output path of the scan.

Then show me, as plain text, no code yet:

1. The proposed guide page list (working assumption: `setup`, `analyze`, `remediate` — adjust if the repo's actual shape argues for something else, e.g. a separate `tune` page if scoring thresholds are user-configurable).
2. Where the page source files will live (they must be plain `.md` files in the repo, embedded into the binary via `include_str!`, so a docs site can later serve the identical bytes).
3. The state-detection rule for bare `coati guide` (see spec below) mapped to the actual config/output paths you found.
4. Where the scan-output footer will be emitted, and confirmation of how to do it without ever contaminating machine-readable output.
5. The exact list of serialized field names in the inventory schema (I want to check it against what the analyze page will need to cover).

## Step 2 — Write the guide content (STOP after this)

Write the markdown pages only. No Rust changes yet. Show me the full text of every page.

Content rules:

- **Audience is a coding agent.** Imperative, terse, no marketing, no prose warm-up. Every sentence must be an instruction or a definition the agent needs.
- **Self-contained.** The guide must not mention, recommend, or depend on any other tool. It describes coati and the workflow around coati's own output, nothing else.
- **`setup`** — how to configure coati for a repo (config file, relevant keys with defaults), how to run the scan, where `inventory.json` lands. Ends with: `next: run \`coati scan\`` (adjust to the real command).
- **`analyze`** — the core page. Two parts:
  - A field reference for `inventory.json`, derived from the serde structs in the code — not from memory. Every serialized field gets a one-line definition (units, 0- vs 1-indexing, what "null/absent" means).
  - The anti-pattern taxonomy with concrete detection heuristics tied to actual fields and the actual threshold constants in the code. The eight patterns: mock-as-assertion, implementation coupling, tautology, setup-heavy, redundant, slow-without-reason, wrong-layer, dead-test. For each: definition, the inventory signals that flag it as a candidate (e.g. "mock count ≥ N and every assertion targets a Mock attribute → mock-as-assertion candidate"), and a note that candidacy is a lead to verify by reading the test, not a verdict.
  - Ends with: `next: run \`coati guide remediate\``
- **`remediate`** — two parts:
  - A remedy ladder: an ordered list where the agent takes the **first rule that applies** and does not skip ahead. Map each anti-pattern to its remedy (dead-test → delete; tautology → delete or fix the assertion to test behavior; redundant → parametrize or delete; mock-as-assertion → rewrite against observable behavior; implementation coupling, setup-heavy, wrong-layer → these are design judgements: stop, report file, test name, and the signals to a human). The ladder must contain an explicit human-escalation rung — it is a rung, not a footnote.
  - A hard approval gate, stated as a rule near the top of the page: **produce the full findings report and present it; edit no file until a human approves the report.** After approval, around every edit: run the affected tests before touching anything, make the change, run them again, re-scan, confirm the finding is gone and no new finding appeared; revert on any failure.
  - A "Do not" list: do not raise thresholds or edit config to make a finding disappear (repo-wide policy, human-owned); do not delete or skip a test to pass a gate; do not batch-edit across findings before the report is approved.
- Every page ends with a single `next:` line so the agent is always one command from the right instructions.

## Step 3 — Implement (show final output when done)

Only after the content is approved:

1. **`coati guide [PAGE]` subcommand.** With a page argument, print that page's markdown to stdout verbatim and exit 0. With no argument, pick the page by filesystem state:
   - no coati config found → `setup`
   - config found, no `inventory.json` at the resolved output path → `setup`
   - `inventory.json` present → `analyze`
   - `remediate` is never auto-selected; it is reached only via the breadcrumb or explicitly. (Detecting "analysis finished" from the filesystem would be guesswork; don't attempt it.)
2. **Scan footer.** When a scan completes and reports candidates in human-readable output, end with one line pointing at `coati guide analyze`. JSON / machine-readable output streams must remain byte-identical to before — the footer goes only to the human-format output (or stderr if the human format shares a stream with JSON).
3. **Anti-drift test.** A unit test that walks the inventory schema (every field name that serde will emit, including nested structs) and asserts each name appears verbatim in the `analyze` page. This is the mechanism that keeps the guide honest when the schema changes — treat a failure message that names the missing field as part of the deliverable.
4. **Dispatch tests.** Tests for the state-detection rules using temp-dir fixtures (no config / config only / config + inventory).
5. House rules: match the existing clap and module conventions, no new dependencies unless genuinely trivial, `cargo fmt` and `cargo clippy` clean, update README with the `guide` command in the same style as the existing command docs.

Finish by showing me the literal stdout of `coati guide` run in a fixture for each of the three states, and the scan footer as it appears after a real scan of the test fixture.
