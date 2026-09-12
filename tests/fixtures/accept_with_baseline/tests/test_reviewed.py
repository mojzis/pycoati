# A project whose legitimate exceptions have already been reviewed. The
# `.pycoati-accept.toml` beside this tree is committed alongside it, so the
# integration tests exercise a baseline a human actually wrote rather than
# one a test synthesised.
#
# CAREFUL: the baseline pins a fingerprint against
# `test_cli_entry_point_installs`. Editing that test's body is supposed to
# lapse its acceptance — `the_committed_fingerprint_still_matches_its_test`
# will fail and tell you how to refresh it.
from unittest.mock import Mock

import reviewed


def test_cli_entry_point_installs():
    # zero_asserts + high_setup_ratio, and reviewed as correct: every
    # assertion runs in the child interpreter, and the parent propagates
    # failure through `check=True`.
    script = "\n".join(
        [
            "import reviewed",
            "assert reviewed.normalize('  Ada  ') == 'ada'",
            "assert reviewed.normalize('ADA') == 'ada'",
            "assert callable(reviewed.run_in_child)",
            "print('ok')",
        ]
    )
    reviewed.run_in_child(script)


def test_import_does_not_raise():
    # zero_asserts only: a smoke contract whose whole value is that importing
    # and calling does not blow up.
    reviewed.normalize("Ada")


def test_retry_reports_through_the_mock():
    # mock_only_assertions, and deliberately NOT accepted — this is the test
    # that proves an unreviewed finding still reaches the shortlist in a
    # project that has a baseline.
    client = Mock()
    client.send(1)
    assert client.send.assert_called_once_with(1)


def test_normalize_strips_and_lowercases():
    # No signal at all.
    assert reviewed.normalize("  Ada  ") == "ada"
