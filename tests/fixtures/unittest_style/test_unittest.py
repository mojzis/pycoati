"""Unittest-style fixture for Phase 1: exercises `self.assertXxx` method
calls inside a `unittest.TestCase` subclass.

The parser must count `self.assertEqual`, `self.assertTrue`, and
`self.assertIn` calls as effective assertions, and a
`with self.assertRaises(ValueError):` block as an effective assertion too
(via the existing raises-block mechanism only if recognised — but here
the assertion comes from the unittest assert tally).
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
