"""The same doubles and patches, spelled the ways real suites spell them.

Every test passes under pytest. Each pair of tests installs the same
doubles, so pycoati must score each pair identically.
"""

import os
import unittest.mock
from unittest import mock
from unittest.mock import MagicMock as MM
from unittest.mock import patch


class Clock:
    def now(self):
        return 0


def test_constructors_imported_by_name():
    a = unittest.mock.Mock()
    b = MM()
    c = unittest.mock.AsyncMock()
    assert a is not b
    assert c is not None


def test_constructors_through_module_alias():
    a = mock.Mock()
    b = mock.MagicMock()
    c = mock.AsyncMock()
    assert a is not b
    assert c is not None


@patch.object(Clock, "now", return_value=5)
@patch.dict(os.environ, {"PYCOATI_FIXTURE": "1"})
def test_patches_as_decorators(now):
    assert Clock().now() == 5
    assert os.environ["PYCOATI_FIXTURE"] == "1"


def test_patches_as_context_managers():
    with mock.patch.object(Clock, "now", return_value=5), mock.patch.dict(
        os.environ, {"PYCOATI_FIXTURE": "1"}
    ):
        assert Clock().now() == 5
        assert os.environ["PYCOATI_FIXTURE"] == "1"


def test_unrelated_mock_attribute():
    factory = MM()
    built = factory.Mock()
    assert built is not None
