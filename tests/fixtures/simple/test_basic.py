# coati-expected: tests=2 asserts=5


def helper():
    return 1 + 1


def test_arithmetic():
    x = 1 + 1
    assert x == 2
    assert x != 3
    assert x > 0


def test_strings():
    s = "hello"
    assert s == "hello"
    assert len(s) == 5
