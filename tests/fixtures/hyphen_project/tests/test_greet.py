# coati-expected: tests=1 asserts=1
import my_pkg


def test_greet_says_hi():
    assert my_pkg.greet("world") == "hi world"
