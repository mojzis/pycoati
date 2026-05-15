# pycoati-expected: tests=1 asserts=1
from pkg_a import greet


def test_greet_says_hi():
    assert greet("world") == "hi world"
