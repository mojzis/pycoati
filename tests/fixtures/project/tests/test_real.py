# pycoati-expected: tests=1 asserts=2
import myproj


def add(a, b):
    return a + b


def test_adds():
    result = add(2, 3)
    # Side-effect call to exercise myproj.greet so `pytest --cov=myproj`
    # reports non-zero coverage in the Run 2 integration test. Doesn't add
    # an assertion — keeps the Run 1 expectations (tests=1 asserts=2) valid.
    myproj.greet("world")
    assert add(2, 3) == 5
    assert isinstance(result, int)
