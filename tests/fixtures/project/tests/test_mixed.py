# pycoati-expected: tests=1 asserts=2
from unittest.mock import Mock


def test_partial_mock():
    mock = Mock()
    mock.do_thing()
    value = 42
    mock.do_thing.assert_called_once()
    assert value == 42
