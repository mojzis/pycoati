"""Same-file base classes: inherited tests and inherited assert helpers.

pytest-valid; `pytest --collect-only` on this file lists exactly the
nodeids `tests/inheritance.rs` expects.
"""

from unittest import TestCase


class Base(TestCase):
    def check_one(self, x):
        self.assertEqual(x, 1)


class Checks(Base):
    def test_verified_by_base_helper(self):
        self.check_one(1)


class _Contract:
    def make(self):
        raise NotImplementedError

    def test_roundtrip(self):
        assert self.make() == 1


class TestA(_Contract):
    def make(self):
        return 1


class TestB(_Contract):
    def make(self):
        return 1

    def test_b_only(self):
        assert True
