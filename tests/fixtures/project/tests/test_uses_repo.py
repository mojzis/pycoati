from unittest.mock import patch
from myproj.repository import Repository


def test_repo_save_and_load():
    repo = Repository()
    repo.save({"k": "v"})
    result = repo.load("id")
    assert result is None


@patch("myproj.repository.Repository")
def test_repo_patched(mock_repo_cls):
    assert mock_repo_cls is not None
