import argparse
import sys
from dataclasses import replace
from pathlib import Path

from nr.backend import nix_common_args
from nr.config import CheckSettings
from nr.process import render_command, run

EXCLUDED_DIRECTORIES = {
    ".cache",
    ".direnv",
    ".git",
    ".ruff_cache",
    ".venv",
    "__pycache__",
    "result",
}


def source_files(root: Path, suffix: str) -> list[str]:
    return [
        str(path)
        for path in sorted(root.rglob(f"*{suffix}"))
        if not EXCLUDED_DIRECTORIES.intersection(path.parts)
    ]


def apply_check_overrides(settings: CheckSettings, args: argparse.Namespace) -> CheckSettings:
    if getattr(args, "all", False):
        settings = replace(settings, flake=True, nixfmt=True, statix=True, ruff=True)
    if getattr(args, "no_flake", False):
        settings = replace(settings, flake=False)
    for name in ("nixfmt", "statix", "ruff"):
        if getattr(args, name, False):
            settings = replace(settings, **{name: True})
    return settings


def configured_checks(
    flake_path: Path,
    settings: CheckSettings,
    args: argparse.Namespace,
) -> list[tuple[str, list[str], Path | None]]:
    checks: list[tuple[str, list[str], Path | None]] = []
    nix_files = source_files(flake_path, ".nix")
    python_files = source_files(flake_path, ".py")

    if settings.nixfmt:
        if nix_files:
            checks.append(("Nix formatting", ["nixfmt", "--check", *nix_files], None))
        else:
            print("No .nix files found; skipping nixfmt.")
    if settings.statix:
        checks.append(("Nix static analysis", ["statix", "check", str(flake_path)], None))
    if settings.ruff:
        if python_files:
            checks.append(("Python static analysis", ["ruff", "check", *python_files], None))
        else:
            print("No .py files found; skipping ruff.")
    if settings.flake:
        checks.append(
            (
                "Flake checks",
                ["nix", *nix_common_args(args), "flake", "check", f"path:{flake_path}"],
                None,
            )
        )
    for index, command in enumerate(settings.commands, start=1):
        command_list = list(command)
        checks.append(
            (f"Custom check {index}: {render_command(command_list)}", command_list, flake_path)
        )

    return checks


def command_check(args: argparse.Namespace) -> int:
    settings = apply_check_overrides(args.config.check, args)
    checks = configured_checks(args.target.path, settings, args)
    if not checks:
        print("No checks enabled.")
        return 0

    failed: list[str] = []
    for name, command, cwd in checks:
        print(f"\n[{name}]", flush=True)
        result = run(command, cwd=cwd, check=False)
        if result.returncode != 0:
            failed.append(name)

    if failed:
        print(f"\nFailed checks: {', '.join(failed)}", file=sys.stderr)
        return 1

    print("\nAll checks passed.", flush=True)
    return 0
