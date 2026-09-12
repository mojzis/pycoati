"""Checked subprocess calls: a child failure propagates into the parent test.

None of these tests assert anything locally. Verification happens in the
child process and reaches pytest as a `subprocess.CalledProcessError`, so
flipping the child assertion to a false one fails the parent test.
"""

import subprocess
import sys


def test_child_process_contract():
    subprocess.run(
        [sys.executable, "-c", "assert 2 + 2 == 4"],
        check=True,
        capture_output=True,
        text=True,
    )


def test_check_call_contract():
    subprocess.check_call([sys.executable, "-c", "assert 2 + 2 == 4"])


def test_check_output_contract():
    subprocess.check_output([sys.executable, "-c", "assert 2 + 2 == 4"])


def test_explicit_returncode_check():
    completed = subprocess.run([sys.executable, "-c", "assert 2 + 2 == 4"])
    completed.check_returncode()


def test_asserts_and_checks():
    completed = subprocess.run(
        [sys.executable, "-c", "print('ok')"],
        check=True,
        capture_output=True,
        text=True,
    )
    assert completed.stdout.strip() == "ok"


class TestClassBased:
    def test_self_rooted_returncode(self):
        self.completed = subprocess.run([sys.executable, "-c", "assert 2 + 2 == 4"])
        self.completed.check_returncode()
