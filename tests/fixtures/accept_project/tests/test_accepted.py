# Fixture for the accepted-findings baseline tests. Every test here exists to
# fire one exact set of signals; the comment on each names the set.
import subprocess
import sys
from unittest.mock import Mock

import acceptproj


def test_startup_sequence_smoke():
    # zero_asserts + high_setup_ratio. Nothing here verifies anything: the
    # test's whole contract is that a long startup sequence does not raise,
    # and the body is tall enough to push setup_to_assertion_ratio past 8.
    greeting = acceptproj.greet("world")
    shouted = greeting.upper()
    parts = shouted.split(" ")
    rejoined = " ".join(parts)
    doubled = rejoined * 2
    trimmed = doubled.strip()
    lowered = trimmed.lower()
    acceptproj.greet(lowered)


def test_checked_child_process():
    # No zero_asserts: the assertions run in a child interpreter and the
    # parent propagates failure through check=True, so this test can fail.
    # Present so the fixture carries the one shape acceptance must NOT be
    # offered for — an entry accepting `zero_asserts` here goes stale.
    script = "\n".join(
        [
            "import acceptproj",
            "value = acceptproj.greet('world')",
            "assert value == 'hello world'",
            "assert isinstance(value, str)",
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
