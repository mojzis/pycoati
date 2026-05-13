# coati-expected: tests=1 asserts=2


def add(a, b):
    return a + b


def test_adds():
    result = add(2, 3)
    assert add(2, 3) == 5
    assert isinstance(result, int)
