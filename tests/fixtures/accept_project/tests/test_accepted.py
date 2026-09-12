# Fixture for the accepted-findings baseline tests. Every test here exists to
# fire one exact set of signals; the comment on each names the set.
import subprocess
import sys
from unittest.mock import Mock

import acceptproj


def test_child_interpreter_smoke():
    # zero_asserts + high_setup_ratio. The assertions run in a child
    # interpreter and the parent propagates failure through check=True, so
    # there is no assert statement in this body — and the body is tall
    # enough to push setup_to_assertion_ratio past 8.
    script = "\n".join(
        [
            "import acceptproj",
            "value = acceptproj.greet('world')",
            "assert value == 'hello world'",
            "assert isinstance(value, str)",
            "print(value)",
        ]
    )
    subprocess.run([sys.executable, "-c", script], check=True)


def test_short_smoke():
    # zero_asserts only: a one-line body keeps the ratio well under 8.
    acceptproj.greet("world")


def test_mock_only_assertion():
    # mock_only_assertions: the sole assertion targets the Mock API.
    client = Mock()
    client.send(1)
    assert client.send.assert_called_once_with(1)


def test_clean():
    # No signal at all. Present so the suite has a test that never appears in
    # any accepted or stale list.
    assert acceptproj.greet("world") == "hello world"
