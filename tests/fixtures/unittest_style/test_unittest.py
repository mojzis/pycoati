"""Unittest-style fixture for Phase 1: exercises `self.assertXxx` method
calls inside a `unittest.TestCase` subclass.

The parser must count `self.assertEqual`, `self.assertTrue`, and
`self.assertIn` calls as effective assertions, and a
`with self.assertRaises(ValueError):` block as an effective assertion too
(via the existing raises-block mechanism only if recognised — but here
the assertion comes from the unittest assert tally).

`test_camelcase_strictness` also exercises adversarial lookalikes
(`self.assert_called_with`, `self.assertion_count`, `self.assert_logged`)
that must NOT be counted as effective assertions.
"""

import unittest


class TestThing(unittest.TestCase):
    def test_unittest_asserts(self):
        x = 1 + 1
        self.assertEqual(x, 2)
        self.assertTrue(x > 0)
        self.assertIn(x, [1, 2, 3])

    def test_unittest_raises_block(self):
        with self.assertRaises(ValueError):
            int("not a number")

    def test_camelcase_strictness(self):
        # One real unittest assertion — must count.
        self.assertEqual(1, 1)
        # Lookalikes — none of these are unittest assertion methods.
        # `assert_called_with` is the Mock API; `assertion_count` and
        # `assert_logged` are imaginary user helpers. The strict
        # camelCase predicate must reject all three.
        self.assert_called_with("x")
        self.assertion_count()
        self.assert_logged("msg")
