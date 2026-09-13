"""Subprocess calls that prove nothing on their own.

A non-zero exit status from any of these is discarded, so the test still
passes when the child fails. They need their own evidence — the last test
here has it, as an asserted return code.
"""

import subprocess
import sys


def test_unchecked_run():
    subprocess.run([sys.executable, "-c", "assert 2 + 2 == 5"], capture_output=True)


def test_explicit_check_false():
    subprocess.run([sys.executable, "-c", "raise SystemExit(1)"], check=False)


def test_popen_without_check():
    proc = subprocess.Popen([sys.executable, "-c", "assert 2 + 2 == 5"])
    proc.communicate()


def test_asserted_return_code():
    completed = subprocess.run([sys.executable, "-c", "assert 2 + 2 == 4"])
    assert completed.returncode == 0
