# coati-expected: tests=1 asserts=2 only_mock=test_repo_save_called
from unittest.mock import Mock


def test_repo_save_called():
    mock = Mock()
    repo = Mock()
    mock.save(1)
    repo.save.return_value = 2
    assert mock.assert_called_once_with(1)
    assert repo.save.assert_called()
