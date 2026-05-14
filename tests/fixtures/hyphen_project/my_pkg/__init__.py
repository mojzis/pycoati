"""Hyphenated-distribution fixture for the auto-default `--cov=` normalization.

Distribution name in `pyproject.toml` is `my-pkg`; the importable module is
`my_pkg`. Coati's default-derivation must hyphens-to-underscores this so
`pytest --cov=my_pkg` actually finds and measures the module. `greet` is
exercised by `tests/test_greet.py` to produce non-zero coverage.
"""


def greet(name):
    return f"hi {name}"
