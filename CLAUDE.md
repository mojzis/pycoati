# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

pycoati audits Python test suites for mock smells and suspicious tests. Hybrid Rust/Python project: builds a Rust binary (`pycoati`) and packages it as a Python package via maturin for distribution through pip/uv.

## Common Commands

```bash
# Pre-commit checks (always run before committing)
cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features

# Run tests
cargo test

# Build and install locally for testing (puts `pycoati` on PATH at ~/.cargo/bin)
cargo install --path . --force

# Build a Python wheel (only needed when packaging for pip/uv distribution)
maturin develop

# Full review (fmt, clippy, tests, audit, deny)
make review
```

## Development Workflow

All features and bug fixes follow TDD (red-green-refactor). No implementation code without a failing test first. Bug fixes must include a regression test that fails without the fix.

## Test Changes Require Deliberation

When a test fails during implementation:

1. **Stop and diagnose.** Understand WHY it fails before changing anything.
2. **Default assumption: the test is right.** Fix the implementation first.
3. **If the test genuinely needs updating** (requirements changed, API evolved), explain what changed and why the old assertion is no longer correct before modifying it.
4. **Never weaken an assertion just to make it pass.**
5. **If uncertain, ask.**

## Branch Hygiene

**Always merge `main` into your feature branch before creating a PR.** Run:

```bash
git fetch origin main && git merge origin/main
```

Then re-run the full check suite to verify the merge didn't introduce breakage.

## When Stuck

If hitting a wall:
1. Do not silently work around it — state the problem explicitly.
2. Do not attempt more than 3 approaches without reporting what was tried and why each failed.
3. Do not modify unrelated code hoping it fixes the issue.
4. Revert to last known good state if changes made things worse.
5. When in doubt, ask.

## Review Before Completing Work

Before marking any task as complete:

1. **Automated checks** (run automatically via prek pre-commit hook on `git commit`):
   - `cargo fmt --all -- --check`
   - `cargo clippy --all-targets --all-features -- -D warnings`
   - `cargo test --all-features --bins`

2. **Deep review** (REQUIRED for all significant changes):
   - Run the `rust-review` skill (`/rust-review`) before marking work as complete
   - Address all Must Fix items before completing
   - Address Should Fix items unless there's a documented reason not to

3. **Full review** (run before pushing):
   - `make review`

### Code Rules
- No `.unwrap()` outside tests — use `.context()` when propagating errors with `?`
- No `MutexGuard` held across `.await`
- Prefer `&str`/`&[T]`/`&Path` over owned types in function parameters when ownership isn't needed
- Tests must assert on values, not just "runs without panic"
- Flaky tests need root-cause analysis, not looser assertions
- No new dependencies without surfacing the choice

## Docs

The mdBook site under `docs/` (https://mojzis.github.io/pycoati/) deploys to
GitHub Pages on push to `main` (`.github/workflows/docs.yml`), along with
`llms.txt` and `llms-full.txt`. Build locally with `make docs`.

- The guide pages in `docs/src/guide/` are what `pycoati guide` prints
  (`include_str!` in `src/guide.rs`): one source for the CLI and the site.
- `docs/src/weights.md` includes `WEIGHTS.md` from the repo root; edit the root file.
- Shared prose lives in `docs/shared/` and is injected into `README.md` and
  `docs/src/index.md` by `docs/inject-shared.sh`. Edit the shared file, then
  run the script — never edit between the `<!-- BEGIN SHARED:... -->` markers.
- `docs/custom.css`, `docs/aesop.js` and `docs/theme/head.hbs` are generated
  by mojzis/aesop `docs-theme/sync.py`; don't edit them here.
- `docs/gen-version.sh` syncs the docs version badge with `Cargo.toml`
  (`cargo release` also bumps it via `release.toml`).
- `docs/toolchain.sh` pins the `mdbook` / `mdbook-mermaid` pair used by both
  `make docs` and the workflow.
- Adding a page means adding it to `docs/src/SUMMARY.md`; the `llms.txt`
  generator reads that file.

## Release notes

A change a user would notice adds one file to `release-notes/` in the same PR
(`YYYY-MM-DD-short-slug.md`, frontmatter `kind: added|changed|fixed|removed`, one or two
plain sentences). Run `uvx --from git+https://github.com/mojzis/relnotes relnotes guide` before writing one: it has the wording rules.
Dependency bumps, CI, refactors and tests get no note. `uvx --from git+https://github.com/mojzis/relnotes relnotes check` validates them.
The release workflow turns them into the GitHub release, the docs build into
`docs/src/release-notes.md` (generated, gitignored), and the aesop site shows them.
