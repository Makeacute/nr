import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from nr import git as nr_git
from nr import process
from nr.errors import NrError
from tests.support import completed, git, initialize_repository, quiet_subprocess_run


class GitTests(unittest.TestCase):
    def test_git_command(self) -> None:
        self.assertEqual(
            nr_git.git_command(Path("/flake"), "status", "--short"),
            ["git", "-C", "/flake", "status", "--short"],
        )

    @patch.object(nr_git, "run")
    def test_repository_detection(self, mocked_run) -> None:
        mocked_run.return_value = completed("true\n")
        self.assertTrue(nr_git.is_git_repository(Path("/flake")))
        nr_git.ensure_git_repository(Path("/flake"))

        mocked_run.return_value = completed(returncode=128)
        self.assertFalse(nr_git.is_git_repository(Path("/flake")))
        with self.assertRaisesRegex(NrError, "Not a Git repository"):
            nr_git.ensure_git_repository(Path("/flake"))

    @patch.object(nr_git, "is_git_repository", return_value=False)
    @patch.object(nr_git, "run")
    def test_git_visibility_ignores_non_git_flake(self, mocked_run, _mocked_repo) -> None:
        nr_git.ensure_git_flake_visible(Path("/flake"))
        mocked_run.assert_not_called()

    @patch.object(nr_git, "is_git_repository", return_value=True)
    @patch.object(nr_git, "run", return_value=completed())
    def test_git_visibility_without_untracked_files(self, mocked_run, _mocked_repo) -> None:
        nr_git.ensure_git_flake_visible(Path("/flake"))
        mocked_run.assert_called_once()

    @patch.object(nr_git, "is_git_repository", return_value=True)
    @patch.object(nr_git, "run", return_value=completed("new.nix\0"))
    def test_git_visibility_rejects_untracked_files(self, _mocked_run, _mocked_repo) -> None:
        with self.assertRaisesRegex(NrError, "Untracked files"):
            nr_git.ensure_git_flake_visible(Path("/flake"))

    def test_git_visibility_does_not_stage_real_untracked_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initialize_repository(root)
            (root / "new.nix").write_text("{}\n", encoding="utf-8")

            with (
                patch.object(process.subprocess, "run", side_effect=quiet_subprocess_run),
                self.assertRaisesRegex(NrError, "Untracked files"),
            ):
                nr_git.ensure_git_flake_visible(root)

            self.assertEqual(git(root, "diff", "--cached", "--name-only").stdout.strip(), "")

    def test_status_entries_parse_rename(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initialize_repository(root)
            git(root, "mv", "flake.nix", "new-name.nix")

            with patch.object(process.subprocess, "run", side_effect=quiet_subprocess_run):
                entries = nr_git.status_entries(root)

            self.assertEqual(len(entries), 1)
            self.assertEqual(entries[0].paths, ("new-name.nix", "flake.nix"))
            self.assertEqual(entries[0].label, "flake.nix -> new-name.nix")


if __name__ == "__main__":
    unittest.main()
