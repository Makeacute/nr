import argparse

from nr.backend import (
    nh_generations_command,
    nh_lifecycle_command,
    nh_rollback_command,
    nix_common_args,
)
from nr.git import ensure_git_flake_visible
from nr.process import run


def run_lifecycle(action: str, args: argparse.Namespace) -> int:
    ensure_git_flake_visible(args.target.path)
    run(nh_lifecycle_command(action, args.target, args))
    return 0


def command_build(args: argparse.Namespace) -> int:
    return run_lifecycle("build", args)


def command_switch(args: argparse.Namespace) -> int:
    return run_lifecycle("switch", args)


def command_test(args: argparse.Namespace) -> int:
    return run_lifecycle("test", args)


def command_boot(args: argparse.Namespace) -> int:
    return run_lifecycle("boot", args)


def command_update(args: argparse.Namespace) -> int:
    command = [
        "nix",
        *nix_common_args(args),
        "flake",
        "update",
        "--flake",
        str(args.target.path),
        *getattr(args, "inputs", []),
    ]
    run(command)

    if args.switch:
        return run_lifecycle("switch", args)
    return 0


def command_rollback(args: argparse.Namespace) -> int:
    run(nh_rollback_command(args))
    return 0


def command_generations(args: argparse.Namespace) -> int:
    run(nh_generations_command(args))
    return 0
