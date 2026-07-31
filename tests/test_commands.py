import unittest
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from unittest.mock import call, patch

from nr import checks, commands, lifecycle
from nr.config import CheckSettings, NrConfig
from tests.support import arguments, completed


class LifecycleCommandTests(unittest.TestCase):
    @patch.object(lifecycle, "run")
    @patch.object(lifecycle, "ensure_git_flake_visible")
    def test_build_uses_nh_without_git_mutation(self, mocked_visible, mocked_run) -> None:
        self.assertEqual(lifecycle.command_build(arguments()), 0)
        mocked_visible.assert_called_once()
        mocked_run.assert_called_once_with(["nh", "os", "build", "/etc/nixos#nixos"])

    @patch.object(lifecycle, "run")
    @patch.object(lifecycle, "ensure_git_flake_visible")
    def test_switch_does_not_call_sudo_directly(self, _mocked_visible, mocked_run) -> None:
        self.assertEqual(lifecycle.command_switch(arguments()), 0)
        mocked_run.assert_called_once_with(["nh", "os", "switch", "/etc/nixos#nixos"])

    @patch.object(lifecycle, "run")
    @patch.object(lifecycle, "ensure_git_flake_visible")
    def test_lifecycle_forwards_common_and_backend_args(self, _mocked_visible, mocked_run) -> None:
        args = arguments(
            dry=True,
            ask=True,
            offline=True,
            show_trace=True,
            specialisation="performance",
            backend_args=["--", "--no-nom"],
        )
        self.assertEqual(lifecycle.command_test(args), 0)
        mocked_run.assert_called_once_with(
            [
                "nh",
                "os",
                "test",
                "--dry",
                "--ask",
                "--offline",
                "--show-trace",
                "--specialisation",
                "performance",
                "/etc/nixos#nixos",
                "--no-nom",
            ]
        )

    @patch.object(lifecycle, "run_lifecycle", return_value=0)
    @patch.object(lifecycle, "run")
    def test_update_can_update_inputs_and_switch(self, mocked_run, mocked_lifecycle) -> None:
        args = arguments(inputs=["nixpkgs"], switch=True, offline=True)
        self.assertEqual(lifecycle.command_update(args), 0)
        mocked_run.assert_called_once_with(
            ["nix", "--offline", "flake", "update", "--flake", "/etc/nixos", "nixpkgs"]
        )
        mocked_lifecycle.assert_called_once_with("switch", args)

    @patch.object(lifecycle, "run")
    def test_rollback_and_generations(self, mocked_run) -> None:
        self.assertEqual(lifecycle.command_rollback(arguments(to=42, ask=True)), 0)
        self.assertEqual(
            mocked_run.call_args_list[-1],
            call(["nh", "os", "rollback", "--ask", "--to", "42"]),
        )

        self.assertEqual(
            lifecycle.command_generations(arguments(fields=["id", "date"], profile="/nix/profile")),
            0,
        )
        self.assertEqual(
            mocked_run.call_args_list[-1],
            call(["nh", "os", "info", "--profile", "/nix/profile", "--fields", "id,date"]),
        )


class CheckCommandTests(unittest.TestCase):
    def test_source_files_excludes_generated_directories(self) -> None:
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "module.nix").touch()
            ignored = root / ".git"
            ignored.mkdir()
            (ignored / "ignored.nix").touch()
            self.assertEqual(checks.source_files(root, ".nix"), [str(root / "module.nix")])

    @patch.object(checks, "run", return_value=completed())
    def test_check_defaults_to_flake_check_only(self, mocked_run) -> None:
        output = StringIO()
        with redirect_stdout(output):
            self.assertEqual(checks.command_check(arguments()), 0)
        mocked_run.assert_called_once_with(
            ["nix", "flake", "check", "path:/etc/nixos"],
            cwd=None,
            check=False,
        )
        self.assertIn("All checks passed", output.getvalue())

    @patch.object(checks, "source_files", side_effect=[["one.nix"], ["one.py"]])
    @patch.object(checks, "run")
    def test_check_all_runs_static_tools(self, mocked_run, _mocked_sources) -> None:
        mocked_run.side_effect = [
            completed(),
            completed(returncode=1),
            completed(),
            completed(),
        ]
        error = StringIO()
        with redirect_stdout(StringIO()), redirect_stderr(error):
            result = checks.command_check(arguments(all=True))
        self.assertEqual(result, 1)
        self.assertEqual(mocked_run.call_count, 4)
        self.assertIn("Nix static analysis", error.getvalue())

    @patch.object(checks, "run", return_value=completed())
    def test_custom_checks_run_in_flake_root(self, mocked_run) -> None:
        target = arguments().target
        args = arguments(
            config=NrConfig(
                target,
                check=CheckSettings(flake=False, commands=(("echo", "ok"),)),
            )
        )
        self.assertEqual(checks.command_check(args), 0)
        mocked_run.assert_called_once_with(["echo", "ok"], cwd=target.path, check=False)


class HelpCommandTests(unittest.TestCase):
    def test_commands_facade_exports_handlers(self) -> None:
        self.assertTrue(callable(commands.command_build))
        self.assertTrue(callable(commands.command_publish))
        self.assertTrue(callable(commands.command_doctor))

    def test_cheat(self) -> None:
        output = StringIO()
        with redirect_stdout(output):
            self.assertEqual(commands.command_cheat(arguments()), 0)
        self.assertIn("nr test", output.getvalue())
        self.assertIn("--flake PATH[#HOST]", output.getvalue())


if __name__ == "__main__":
    unittest.main()
