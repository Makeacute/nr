import argparse
import sys
import unittest
from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path
from unittest.mock import patch

from nr import cli
from nr.config import FlakeTarget, NrConfig
from nr.errors import NrError


class CliTests(unittest.TestCase):
    def test_parser_exposes_every_command(self) -> None:
        parser = cli.create_parser()
        for command in (
            "build",
            "switch",
            "test",
            "boot",
            "update",
            "rollback",
            "generations",
            "publish",
            "check",
            "doctor",
            "cheat",
        ):
            with self.subTest(command=command):
                args = parser.parse_args([command])
                self.assertEqual(args.command, command)
                self.assertTrue(callable(args.handler))

    def test_target_options_work_before_or_after_command(self) -> None:
        parser = cli.create_parser()
        before = parser.parse_args(["--flake", "/one#host", "build"])
        after = parser.parse_args(["build", "--flake", "/two#host"])
        self.assertEqual(before.flake, "/one#host")
        self.assertEqual(after.flake, "/two#host")

    def test_lifecycle_backend_args_after_separator(self) -> None:
        parser = cli.create_parser()
        args = parser.parse_args(["build", "--dry", "--", "--no-nom"])
        self.assertTrue(args.dry)
        self.assertEqual(args.backend_args, ["--", "--no-nom"])

    @patch.object(cli, "command_cheat", return_value=0)
    @patch.object(cli, "load_config")
    def test_main_runs_cheat_without_config(self, mocked_config, mocked_cheat) -> None:
        with patch.object(sys, "argv", ["nr", "cheat"]):
            self.assertEqual(cli.main(), 0)
        mocked_config.assert_not_called()
        mocked_cheat.assert_called_once()

    @patch.object(cli, "load_config")
    def test_main_loads_config_for_target_commands(self, mocked_config) -> None:
        target = FlakeTarget(Path("/flake"), "host")
        mocked_config.return_value = NrConfig(target)
        handler = unittest.mock.Mock(return_value=0)
        parser = unittest.mock.Mock()
        parser.parse_args.return_value = argparse.Namespace(
            command="build",
            handler=handler,
            needs_config=True,
            flake="/flake#host",
        )

        with patch.object(cli, "create_parser", return_value=parser):
            self.assertEqual(cli.main(), 0)

        mocked_config.assert_called_once_with(flake="/flake#host", host=None)
        passed_args = handler.call_args.args[0]
        self.assertEqual(passed_args.config, NrConfig(target))
        self.assertEqual(passed_args.target, target)

    @patch.object(cli, "load_config", side_effect=NrError("bad config"))
    def test_main_reports_config_error(self, _mocked_config) -> None:
        error = StringIO()
        with patch.object(sys, "argv", ["nr", "build"]), redirect_stderr(error):
            self.assertEqual(cli.main(), 1)
        self.assertIn("Error: bad config", error.getvalue())


if __name__ == "__main__":
    unittest.main()
