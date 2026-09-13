"""Trivial package so the fixture's tests have project code to call."""

import subprocess
import sys


def normalize(name):
    return name.strip().lower()


def run_in_child(script):
    """Run `script` in a child interpreter, raising on a non-zero exit."""
    subprocess.run([sys.executable, "-c", script], check=True)
