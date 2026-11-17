"""Mock-assertion idioms as real suites write them.

Every test here passes under pytest. The `mock_only_*` tests verify nothing
but the test double; the `real_*` tests verify a value the code produced, even
where they also read the mock.
"""

import unittest
from unittest import mock
from unittest.mock import ANY, Mock, call


def notify(client, items):
    for item in items:
        client.send(item, priority=1)
    return len(items)


def test_mock_only_bare_assert_methods():
    client = Mock()
    notify(client, ["a", "b"])
    client.send.assert_called_with("b", priority=1)
    client.send.assert_any_call("a", priority=1)
    client.send.assert_has_calls([call("a", priority=1), call("b", priority=1)])
    client.close.assert_not_called()


def test_mock_only_comparisons():
    client = Mock()
    items = ["a", "b"]
    notify(client, items)
    assert client.send.call_count == 2
    assert client.send.call_count == len(items)
    assert client.send.call_args == call("b", priority=1)
    assert client.send.call_args_list == [call("a", priority=1), call("b", priority=1)]
    assert len(client.send.call_args_list) == 2
    assert client.send.call_args.kwargs["priority"] == 1


def test_mock_only_negation():
    client = Mock()
    notify(client, [])
    assert not client.send.called


def test_real_result_beside_mock_assert():
    client = Mock()
    sent = notify(client, ["a"])
    client.send.assert_called_once_with("a", priority=1)
    assert sent == 1


def test_real_return_value_comparison():
    client = Mock()
    client.send.return_value = "ok"
    result = client.send("a")
    assert result == client.send.return_value


class TestNotifyUnittest(unittest.TestCase):
    def test_mock_only_unittest_forms(self):
        client = mock.Mock()
        notify(client, ["a"])
        self.assertEqual(client.send.call_count, 1)
        self.assertTrue(client.send.called)
        self.assertEqual(client.send.call_args, call("a", priority=ANY))
        self.assertEqual(client.send.call_args_list, [mock.call("a", priority=1)])

    def test_real_unittest_forms(self):
        client = mock.Mock()
        items = ["a"]
        sent = notify(client, items)
        self.assertEqual(client.send.call_count, len(items))
        self.assertEqual(sent, 1)
