# pycoati-expected: tests=2 asserts=2
from pkg_a import greet


def test_greet_returns_hello():
    assert greet("world") == "hello from pkg_a world"


def test_greet_is_truthy():
    assert greet("anyone")
