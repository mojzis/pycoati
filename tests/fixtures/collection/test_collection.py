"""Classes pytest collects beyond the plain `Test*` rule.

Every test passes under pytest, and `pytest --collect-only -q` on this file
lists exactly the nodeids `tests/collection.rs` expects from pycoati.
"""

import unittest
from unittest import IsolatedAsyncioTestCase


def total(items):
    return sum(items)


class CartChecks(unittest.TestCase):
    def test_total(self):
        self.assertEqual(total([1, 2]), 3)

    def testEmptyTotal(self):
        self.assertEqual(total([]), 0)

    def helper(self):
        return [1]

    class TestNestedInCase:
        def test_not_collected(self):
            raise AssertionError("pytest does not recurse into TestCase subclasses")


class BaseCase(unittest.TestCase):
    def setUp(self):
        self.items = [1, 2, 3]


class InheritedChecks(BaseCase):
    def test_inherited_setup(self):
        self.assertEqual(total(self.items), 6)


class AsyncChecks(IsolatedAsyncioTestCase):
    async def test_async_total(self):
        self.assertEqual(total([4]), 4)


class TestOuter:
    def test_outer(self):
        assert total([1]) == 1

    class TestInner:
        def test_inner(self):
            assert total([2]) == 2

    class Helpers:
        def test_not_collected(self):
            raise AssertionError("pytest does not collect non-Test nested classes")


class Plain:
    def test_not_collected(self):
        raise AssertionError("pytest does not collect non-Test classes")
