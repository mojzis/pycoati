# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

coati is a new tool. Hybrid Rust/Python project: builds a Rust binary (`coati`) and packages it as a Python package via maturin for distribution through pip/uv.

## Common Commands

```bash
# Pre-commit checks (always run before committing)
cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features

# Run tests
cargo test

# Build and install locally for testing
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
