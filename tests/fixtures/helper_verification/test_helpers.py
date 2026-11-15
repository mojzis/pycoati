"""Tests that verify through helpers instead of `assert` statements.

Every test passes under pytest. The `verified_*` tests can fail for their
own reason; the `unverified_*` tests cannot, despite calling a helper.
"""

import warnings
from unittest.mock import Mock

import pytest


def total(items):
    return sum(items)


def old_total(items):
    warnings.warn("use total()", DeprecationWarning)
    return total(items)


def check_total(items, expected):
    assert total(items) == expected


def check_was_called(m):
    m.assert_called_once()


def build_items():
    return [1, 2, 3]


def assert_via(items, expected):
    check_total(items, expected)


def test_verified_by_module_helper():
    items = build_items()
    check_total(items, 6)


def test_verified_by_warning_block():
    with pytest.warns(DeprecationWarning):
        old_total([1])


def test_verified_by_deprecated_call():
    with pytest.deprecated_call():
        old_total([1])


def test_verified_by_pytest_fail():
    result = total([2, 2])
    if result != 4:
        pytest.fail(f"expected 4, got {result}")


def test_unverified_mock_only_helper():
    m = Mock()
    m()
    check_was_called(m)


def test_unverified_two_levels_deep():
    assert_via(build_items(), 6)


class TestCart:
    def _check(self, items, expected):
        assert total(items) == expected

    def _items(self):
        return [4, 5]

    def test_verified_by_method_helper(self):
        self._check(self._items(), 9)
