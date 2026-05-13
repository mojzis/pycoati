"""Stubs-style fixture for Phase 2: exercises `monkeypatch.*` and
`mocker.*` fixture-driven patching.

These calls don't construct a `Mock()` directly — the `monkeypatch` and
`mocker` fixtures rewire production code from the outside. The parser
counts each `STUB_HEADS`-matching call as a stub, aggregates them on
`stubs_count`, and the smells layer folds them into `mock_overuse` at
both test and file scope.

- `test_monkeypatch_basic` exercises one stub against one assert: no
  smell fires (we're not stub-heavy).
- `test_monkeypatch_heavy` patches four times against one assert: the
  per-test `mock_overuse` smell fires because `(0 + 4) > max(1, 2)` and
  `4/1 > 2.0`.
- `test_mocker_patch` exercises the `mocker` fixture with multiple
  patch helpers — also stub-heavy, also fires `mock_overuse`.
"""

import os


def test_monkeypatch_basic(monkeypatch):
    monkeypatch.setenv("FOO", "bar")
    assert os.environ["FOO"] == "bar"


def test_monkeypatch_heavy(monkeypatch):
    # Four monkeypatch calls, one assertion — `(stubs > 2 * asserts)`
    # trips the `mock_overuse` smell.
    monkeypatch.setattr(os, "getcwd", lambda: "/x")
    monkeypatch.setenv("FOO", "bar")
    monkeypatch.delenv("BAZ", raising=False)
    monkeypatch.chdir("/tmp")
    assert os.getcwd() == "/x"


def test_mocker_patch(mocker):
    # Three mocker calls, one assertion — also stub-heavy.
    mocker.patch("os.getcwd", return_value="/x")
    mocker.patch.object(os, "environ", {"FOO": "bar"})
    mocker.spy(os, "listdir")
    assert os.getcwd() == "/x"
