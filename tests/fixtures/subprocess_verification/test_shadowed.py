"""Checked calls against something the test itself replaced.

A double never exits non-zero, so a `check=True` against one proves nothing:
the evidence was manufactured by the test. These keep the assertionless
signal — and the first of them is the mock theater pycoati exists to catch.
"""

import subprocess
import sys
import unittest.mock
from unittest import mock
from unittest.mock import MagicMock, patch


@patch("subprocess.run")
def test_patched_subprocess_run(run):
    subprocess.run([sys.executable, "-c", "assert 2 + 2 == 5"], check=True)


def test_fixture_shadows_the_module(subprocess):
    subprocess.run(["prog"], check=True)


def test_monkeypatched_helper(monkeypatch):
    monkeypatch.setattr(subprocess, "check_call", lambda *a, **k: None)
    subprocess.check_call(["prog"])


def test_helper_defined_but_never_called():
    def run_child():
        subprocess.check_call(["prog"])


@patch("subprocess.run")
def test_mocked_completed_process(run):
    completed = subprocess.run([sys.executable, "-c", "assert 2 + 2 == 5"])
    completed.check_returncode()


def test_magicmock_returncode():
    completed = MagicMock()
    completed.check_returncode()


@unittest.mock.patch.object(subprocess, "run")
def test_qualified_patch_object(run):
    subprocess.run(["prog"], check=True)


def test_helper_defined_and_called():
    def run_child():
        subprocess.check_call(["prog"])

    run_child()


def test_qualified_magicmock_returncode():
    completed = mock.MagicMock()
    completed.check_returncode()


def test_swallowed_called_process_error():
    try:
        subprocess.run(["prog"], check=True)
    except subprocess.CalledProcessError:
        pass


def test_checked_only_in_except_branch():
    try:
        pass
    except ValueError:
        subprocess.check_call(["prog"])
