import tempfile
import unittest
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
from unittest.mock import patch

from nr import process, publish
from nr.errors import NrError
from tests.support import arguments, git, initialize_repository, quiet_subprocess_run


class PublishTests(unittest.TestCase):
    def test_publish_clean_repository(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initialize_repository(root)

            with (
                patch.object(process.subprocess, "run", side_effect=quiet_subprocess_run),
                redirect_stdout(StringIO()) as output,
            ):
                self.assertEqual(publish.command_publish(arguments(root)), 0)

            self.assertIn("Nothing to publish.", output.getvalue())

    def test_single_publish_creates_real_commit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initialize_repository(root)
            (root / "flake.nix").write_text('{ description = "test"; }\n', encoding="utf-8")

            with (
                patch.object(publish, "confirm", side_effect=[True, False, True, False]),
                patch.object(process.subprocess, "run", side_effect=quiet_subprocess_run),
                redirect_stdout(StringIO()),
            ):
                result = publish.command_publish(
                    arguments(root, mode="single", message="test: publish changes")
                )

            self.assertEqual(result, 0)
            self.assertEqual(
                git(root, "log", "-1", "--pretty=%s").stdout.strip(),
                "test: publish changes",
            )

    def test_per_file_publish_creates_one_commit_per_change(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initialize_repository(root)
            (root / "one.nix").write_text("{}\n", encoding="utf-8")
            (root / "two.nix").write_text("{}\n", encoding="utf-8")

            with (
                patch.object(
                    publish,
                    "confirm",
                    side_effect=[True, False, True, True, False, True, False],
                ),
                patch.object(publish, "read_line", side_effect=["add one", "add two"]),
                patch.object(process.subprocess, "run", side_effect=quiet_subprocess_run),
                redirect_stdout(StringIO()),
            ):
                result = publish.command_publish(arguments(root, mode="per-file"))

            self.assertEqual(result, 0)
            self.assertEqual(
                git(root, "log", "--pretty=%s", "-2").stdout.splitlines(),
                ["add two", "add one"],
            )

    def test_per_file_publish_rejects_multiple_prestaged_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initialize_repository(root)
            (root / "one.nix").write_text("{}\n", encoding="utf-8")
            (root / "two.nix").write_text("{}\n", encoding="utf-8")
            git(root, "add", "one.nix", "two.nix")

            with (
                patch.object(process.subprocess, "run", side_effect=quiet_subprocess_run),
                redirect_stdout(StringIO()),
                self.assertRaisesRegex(NrError, "pre-staged changes"),
            ):
                publish.command_publish(arguments(root, mode="per-file"))

    def test_rejects_blank_single_commit_message(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initialize_repository(root)
            (root / "flake.nix").write_text('{ description = "test"; }\n', encoding="utf-8")

            with (
                patch.object(publish, "confirm", side_effect=[True, False, True]),
                patch.object(process.subprocess, "run", side_effect=quiet_subprocess_run),
                redirect_stdout(StringIO()),
                self.assertRaisesRegex(NrError, "cannot be empty"),
            ):
                publish.command_publish(arguments(root, mode="single", message="   "))

    @patch.object(publish.shutil, "which", return_value=None)
    def test_missing_remote_without_gh_prints_manual_instructions(self, _mocked_which) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            initialize_repository(root)

            with (
                patch.object(process.subprocess, "run", side_effect=quiet_subprocess_run),
                redirect_stdout(StringIO()) as output,
            ):
                self.assertFalse(publish.push_with_remote_setup(root, "origin"))

            self.assertIn("remote add origin", output.getvalue())


if __name__ == "__main__":
    unittest.main()
