"""The same checked forms reached through aliased and direct imports."""

import subprocess as sp
from subprocess import check_call
from subprocess import run as run_child


def test_aliased_module():
    sp.run(["true"], check=True)


def test_aliased_function():
    run_child(["true"], check=True)


def test_imported_check_call():
    check_call(["true"])
