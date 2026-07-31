import argparse
import subprocess
from pathlib import Path

from nr.config import FlakeTarget, NrConfig

REAL_SUBPROCESS_RUN = subprocess.run


def completed(stdout: str = "", returncode: int = 0) -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess([], returncode, stdout=stdout, stderr="")


def arguments(path: Path | None = None, **values: object) -> argparse.Namespace:
    target_path = Path("/etc/nixos") if path is None else path
    target = FlakeTarget(target_path, "nixos")
    defaults: dict[str, object] = {
        "target": target,
        "config": NrConfig(target),
        "message": None,
        "mode": None,
        "push": False,
        "remote": None,
        "switch": False,
        "inputs": [],
        "backend_args": [],
        "verbose": 0,
        "dry": False,
        "ask": False,
        "offline": False,
        "show_trace": False,
        "specialisation": None,
        "all": False,
        "no_flake": False,
        "nixfmt": False,
        "statix": False,
        "ruff": False,
        "to": None,
        "fields": None,
        "profile": None,
    }
    defaults.update(values)
    return argparse.Namespace(**defaults)


def git(root: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    return REAL_SUBPROCESS_RUN(
        ["git", "-C", str(root), *arguments],
        check=True,
        capture_output=True,
        text=True,
    )


def initialize_repository(root: Path) -> None:
    git(root, "init", "--quiet", "--initial-branch", "main")
    git(root, "config", "user.name", "NR Test")
    git(root, "config", "user.email", "nr@example.invalid")
    (root / "flake.nix").write_text("{}\n", encoding="utf-8")
    git(root, "add", "flake.nix")
    git(root, "commit", "--quiet", "-m", "initial")


def quiet_subprocess_run(*args, **kwargs):
    kwargs["capture_output"] = True
    return REAL_SUBPROCESS_RUN(*args, **kwargs)
