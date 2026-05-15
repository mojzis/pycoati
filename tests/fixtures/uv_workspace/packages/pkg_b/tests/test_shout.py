# pycoati-expected: tests=1 asserts=1
from pkg_b import shout


def test_shout_appends_bang():
    assert shout("hi") == "hi!"
