"""A checked subprocess test that is still suspicious for another reason.

The child failure propagates, so the assertionless signal is off — but the
test also patches three collaborators it never asserts on, which is an
independent problem that must survive.
"""

import subprocess
import sys
from unittest.mock import patch


@patch("shutil.which")
@patch("os.getcwd")
@patch("os.environ")
def test_checked_subprocess_with_heavy_patching(environ, getcwd, which):
    subprocess.run([sys.executable, "-c", "assert 2 + 2 == 4"], check=True)
