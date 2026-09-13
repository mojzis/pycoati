"""Doubles a `unittest.TestCase` installs for the whole class.

A class-level `@patch` and a `setUp`-started patcher both replace the
process boundary for every method, so a checked call inside one of those
methods checks a double, exactly as a method-level `@patch` would.
"""

import contextlib
import subprocess
import unittest
from unittest.mock import MagicMock, patch


@patch("subprocess.run")
class TestClassDecorated(unittest.TestCase):
    def test_class_level_patch(self, run):
        subprocess.run(["prog"], check=True)


class TestSetUpPatched(unittest.TestCase):
    def setUp(self):
        self.patcher = patch("subprocess.check_call")
        self.patcher.start()
        self.addCleanup(self.patcher.stop)

    def test_patcher_from_setup(self):
        subprocess.check_call(["prog"])


def test_suppressed_error():
    with contextlib.suppress(subprocess.CalledProcessError):
        subprocess.check_call(["prog"])


def test_with_as_binding():
    with patch("myproj.thing") as completed:
        completed.check_returncode()


def test_walrus_bound_double():
    if (completed := MagicMock()) is not None:
        completed.check_returncode()


def test_tuple_bound_double():
    marker, completed = 1, MagicMock()
    completed.check_returncode()
