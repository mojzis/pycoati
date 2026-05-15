# pycoati ↔ zorilla integration (deferred)

> **Status:** undecided. Coati emits its own mock-related smells natively (see `pycoati-bootstrap.md` → "Native mock smells"). Zorilla is a separate Rust CLI shipping seven lint-style rules. It's not yet settled whether pycoati should ingest zorilla's findings or whether the Phase 2 LLM should just run zorilla itself.

## What zorilla does (current, v0.1)

Seven rules, all line/structural, emitted as `code + message + file:line:col + severity`:

| Code | Name | Catches |
|---|---|---|
| ZR001 | conditional-test-logic | `if` / `for` / `while` / `try` in test body |
| ZR002 | sleep-in-test | `time.sleep` / `asyncio.sleep` |
| ZR003 | no-assertion | test with no assertion or `pytest.raises` |
| ZR004 | assertion-roulette | too many bare (message-less) asserts |
| ZR005 | mystery-guest | absolute path / URL / `~`-path literal |
| ZR006 | patch-stack | more than N stacked `@patch` decorators |
| ZR007 | empty-test | empty body (`pass`, `...`, docstring-only) |

Output: `text` / `json` / `sarif`. CLI: `zorilla check <path>`. Exits `0` clean, `1` findings, `2` error.

Zorilla does **not** emit raw counts. It does not catch over-mocking, only patch stacking. Mock-API-tautology detection (`only_asserts_on_mock`) has no equivalent rule and is unlikely to fit zorilla's lint-rule model.

## Boundary

- **Coati owns**: `mock_only_assertions`, `mock_overuse` — derived from inventory counts, not parsed as separate lint rules. See `pycoati-bootstrap.md` → "Native mock smells".
- **Zorilla owns**: ZR001–ZR007 above. If integration happens, those flow into pycoati's `smell_hits` from zorilla, not from pycoati's own predicates.

Coati should never reimplement a ZR-something. Conversely, zorilla should never grow inventory-style counters — they're different shapes of tool.

## Possible integration shapes

### A. Subprocess + JSON ingest
Coati shells out to `zorilla check --format json <tests_dir>` and folds each finding into the relevant per-file / per-test `smell_hits` entry under `category: "zorilla:ZR00X"` (namespaced so they don't collide with pycoati's native categories).

- Pro: zero coupling, zorilla evolves independently, no shared tree-sitter state.
- Con: a second process per run (~tens of ms on a normal repo), zorilla must be installed.

### B. Library crate dependency
Coati pulls `zorilla-core` as a Cargo dep, hands it the already-parsed tree-sitter tree, gets back `Vec<Finding>`.

Suggested API (zorilla side):
```rust
pub fn analyze_file(path: &Path, source: &str, tree: &tree_sitter::Tree) -> Vec<Finding>;
```

- Pro: no second process, tree-sitter parse is reused.
- Con: tighter coupling, version pinning matters, requires zorilla-core to expose a library API the CLI does not currently need.

### C. No integration; the LLM runs both
Phase 2 LLM reads `inventory.json` from pycoati and runs `zorilla check` itself. Coati stays narrowly focused on counts + native mock smells.

- Pro: simplest, both tools stay self-contained.
- Con: the LLM has to cross-reference findings to inventory entries by `(file, line)` itself.

## Open questions

- Is option C good enough? The Phase 2 prompt is already going to read code; running zorilla in parallel is cheap.
- If we do integrate, is the subprocess approach (A) fine forever, or does the per-process overhead become a problem on large suites?
- Should pycoati's JSON namespace zorilla findings (`zorilla:ZR001`) or rebrand them as generic categories? Namespacing is honest; rebranding hides the provenance.
- Does zorilla's `pyproject.toml` / `zorilla.toml` config get respected when invoked via subprocess from pycoati? (Yes by default — zorilla searches upward — but worth noting.)
