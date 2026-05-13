"""Minimal package used by the Run 2 pytest+coverage integration test.

`greet` is exercised by `tests/test_greet.py` so coverage reports a
non-zero `totals.percent_covered` against `--cov=myproj`.
"""


def greet(name):
    return f"hello {name}"
