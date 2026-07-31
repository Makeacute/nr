import tempfile
import unittest
from pathlib import Path

from nr.config import (
    CheckSettings,
    FlakeTarget,
    discover_target,
    find_flake,
    load_config,
    split_flake_reference,
    validate_flake_path,
)
from nr.errors import NrError


def make_flake(path: Path) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    (path / "flake.nix").write_text("{}\n", encoding="utf-8")
    return path


class ConfigTests(unittest.TestCase):
    def test_target_reference(self) -> None:
        target = FlakeTarget(Path("/etc/nixos"), "laptop")
        self.assertEqual(target.reference, "/etc/nixos#laptop")

    def test_find_flake_walks_parents(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            nested = root / "one" / "two"
            nested.mkdir(parents=True)
            make_flake(root)
            self.assertEqual(find_flake(nested), root)

    def test_split_flake_reference(self) -> None:
        self.assertEqual(split_flake_reference("/flake#host"), ("/flake", "host"))
        self.assertEqual(split_flake_reference("/flake"), ("/flake", None))
        with self.assertRaises(NrError):
            split_flake_reference("/flake#")

    def test_validate_flake_path(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(NrError, "No flake.nix"):
                validate_flake_path(root)
            make_flake(root)
            validate_flake_path(root)

    def test_load_config_precedence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cli_flake = make_flake(root / "cli")
            env_flake = make_flake(root / "env")
            nearest_flake = make_flake(root / "nearest")
            user_flake = make_flake(root / "user")
            nested = nearest_flake / "nested"
            nested.mkdir()
            xdg = root / "xdg"
            config_dir = xdg / "nr"
            config_dir.mkdir(parents=True)
            (config_dir / "config.toml").write_text(
                f"""
[target]
flake = "{user_flake}"
host = "user-host"

[check]
nixfmt = true
commands = [["echo", "ok"]]

[publish]
remote = "upstream"
""".strip(),
                encoding="utf-8",
            )

            config = load_config(
                flake=f"{cli_flake}#fragment-host",
                host="cli-host",
                cwd=nested,
                environ={
                    "XDG_CONFIG_HOME": str(xdg),
                    "NR_FLAKE": f"{env_flake}#env-fragment",
                    "NR_HOST": "env-host",
                },
            )
            self.assertEqual(config.target, FlakeTarget(cli_flake, "cli-host"))

            config = load_config(
                cwd=nested,
                environ={"XDG_CONFIG_HOME": str(xdg), "NR_HOST": "env-host"},
            )
            self.assertEqual(config.target, FlakeTarget(nearest_flake, "env-host"))

            config = load_config(cwd=root / "empty", environ={"XDG_CONFIG_HOME": str(xdg)})
            self.assertEqual(config.target, FlakeTarget(user_flake, "user-host"))
            self.assertEqual(
                config.check,
                CheckSettings(nixfmt=True, commands=(("echo", "ok"),)),
            )
            self.assertEqual(config.publish.remote, "upstream")

    def test_repo_config_overrides_user_config(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            flake = make_flake(root / "flake")
            xdg = root / "xdg"
            config_dir = xdg / "nr"
            config_dir.mkdir(parents=True)
            (config_dir / "config.toml").write_text(
                """
[target]
host = "user-host"

[check]
ruff = true

[publish]
remote = "upstream"
""".strip(),
                encoding="utf-8",
            )
            (flake / ".nr.toml").write_text(
                """
[target]
host = "repo-host"

[check]
statix = true

[publish]
remote = "origin"
""".strip(),
                encoding="utf-8",
            )

            config = load_config(flake=str(flake), environ={"XDG_CONFIG_HOME": str(xdg)})
            self.assertEqual(config.target, FlakeTarget(flake, "repo-host"))
            self.assertTrue(config.check.ruff)
            self.assertTrue(config.check.statix)
            self.assertEqual(config.publish.remote, "origin")

    def test_config_rejects_bad_keys_and_repo_flake(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            flake = make_flake(root / "flake")
            (flake / ".nr.toml").write_text("[target]\nflake = \"/etc/nixos\"\n", encoding="utf-8")
            with self.assertRaisesRegex(NrError, "cannot set"):
                load_config(flake=str(flake), environ={})
            (flake / ".nr.toml").unlink()

            xdg = root / "xdg"
            config_dir = xdg / "nr"
            config_dir.mkdir(parents=True)
            (config_dir / "config.toml").write_text("[check]\nunknown = true\n", encoding="utf-8")
            with self.assertRaisesRegex(NrError, "Unknown"):
                load_config(flake=str(flake), environ={"XDG_CONFIG_HOME": str(xdg)})

    def test_discover_target_wrapper(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = make_flake(Path(directory))
            self.assertEqual(
                discover_target(flake=f"{root}#test-host", environ={}),
                FlakeTarget(root, "test-host"),
            )


if __name__ == "__main__":
    unittest.main()
