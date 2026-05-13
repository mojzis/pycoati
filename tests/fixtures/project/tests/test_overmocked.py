from unittest.mock import Mock


def test_three_mocks_one_assert():
    a = Mock()
    b = Mock()
    c = Mock()
    a.do()
    b.do()
    c.do()
    assert a.called
