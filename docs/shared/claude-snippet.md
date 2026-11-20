### `pycoati` — test-suite audit

- Run `pycoati . --output inventory.json` now and then (not every commit), then
  `pycoati guide analyze`: it ranks the tests most likely to be weak (mock-only
  assertions, patch stacks, no assertions) and gives a remediation ladder.
- Ask before changing a test it flags. Keep a legitimate one with an entry in
  `.pycoati-accept.toml` (a `reason` is required).
- Monorepo: `pycoati plan . --timings inventory.json` says how many pytest
  processes to run side by side, and which subprojects go in each.
