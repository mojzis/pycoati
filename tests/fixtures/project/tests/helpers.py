# Not a test file: lacks both test_ prefix and _test suffix.
# Walker must not discover this file even though it contains a function
# named like a test.


def helper():
    return 1


def test_in_helper():
    # Should never appear in the inventory because the file itself
    # isn't matched by the walker's name globs.
    assert helper() == 1
