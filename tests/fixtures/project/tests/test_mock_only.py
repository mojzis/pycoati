# pycoati-expected: tests=1 asserts=4 only_mock=test_repo_save_called
from unittest.mock import Mock


def test_repo_save_called():
    mock = Mock()
    repo = Mock()
    mock.save(1)
    repo.save(2)
    mock.save.assert_called_once_with(1)
    repo.save.assert_called()
    assert repo.save.call_count == 1
    assert not repo.delete.called
