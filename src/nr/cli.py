import argparse
import subprocess
import sys

from nr import __version__
from nr.commands import (
    command_boot,
    command_build,
    command_cheat,
    command_check,
    command_doctor,
    command_generations,
    command_publish,
    command_rollback,
    command_switch,
    command_test,
    command_update,
)
from nr.config import load_config
from nr.errors import NrError


def add_target_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--flake",
        metavar="PATH[#HOST]",
        default=argparse.SUPPRESS,
        help="NixOS flake path and optional configuration name",
    )
    parser.add_argument(
        "--host",
        default=argparse.SUPPRESS,
        help="NixOS configuration name",
    )


def add_runtime_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("-v", "--verbose", action="count", default=argparse.SUPPRESS)
    parser.add_argument("--dry", action="store_true", default=argparse.SUPPRESS)
    parser.add_argument("--ask", action="store_true", default=argparse.SUPPRESS)
    parser.add_argument("--offline", action="store_true", default=argparse.SUPPRESS)
    parser.add_argument("--show-trace", action="store_true", default=argparse.SUPPRESS)
    parser.add_argument("--specialisation", default=argparse.SUPPRESS)


def command_parser(
    commands,
    name: str,
    *,
    help: str,
) -> argparse.ArgumentParser:
    parser = commands.add_parser(name, help=help)
    add_target_options(parser)
    add_runtime_options(parser)
    return parser


def lifecycle_parser(commands, name: str, *, help: str) -> argparse.ArgumentParser:
    parser = command_parser(commands, name, help=help)
    parser.add_argument("backend_args", nargs=argparse.REMAINDER, help=argparse.SUPPRESS)
    parser.set_defaults(needs_config=True)
    return parser


def create_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="nr",
        description="Build, switch, update, check, and publish a NixOS flake.",
    )
    add_target_options(parser)
    parser.add_argument("--version", action="version", version=f"nr {__version__}")
    commands = parser.add_subparsers(dest="command")

    build = lifecycle_parser(commands, "build", help="Build without switching")
    build.set_defaults(handler=command_build)

    switch = lifecycle_parser(commands, "switch", help="Build and activate")
    switch.set_defaults(handler=command_switch)

    test = lifecycle_parser(commands, "test", help="Activate until the next reboot")
    test.set_defaults(handler=command_test)

    boot = lifecycle_parser(commands, "boot", help="Build and make next boot default")
    boot.set_defaults(handler=command_boot)

    update = command_parser(commands, "update", help="Update flake.lock")
    update.add_argument("inputs", nargs="*", help="Optional flake inputs to update")
    update.add_argument(
        "--switch",
        action="store_true",
        help="Build and activate after updating",
    )
    update.set_defaults(handler=command_update, needs_config=True)

    rollback = commands.add_parser("rollback", help="Roll back with nh")
    add_runtime_options(rollback)
    rollback.add_argument("--to", metavar="GENERATION", help="Roll back to a generation")
    rollback.add_argument("backend_args", nargs=argparse.REMAINDER, help=argparse.SUPPRESS)
    rollback.set_defaults(handler=command_rollback, needs_config=False)

    generations = commands.add_parser("generations", help="Show NixOS generations")
    generations.add_argument(
        "--fields",
        nargs="+",
        choices=("id", "date", "nver", "kernel", "confRev", "spec", "size"),
        help="Fields to show",
    )
    generations.add_argument("--profile", help="NixOS profile path")
    generations.add_argument("-v", "--verbose", action="count", default=argparse.SUPPRESS)
    generations.add_argument("backend_args", nargs=argparse.REMAINDER, help=argparse.SUPPRESS)
    generations.set_defaults(handler=command_generations, needs_config=False)

    publish = command_parser(
        commands,
        "publish",
        help="Review and commit changes, then optionally push",
    )
    publish.add_argument("-m", "--message", help="Commit message")
    publish.add_argument(
        "--push",
        action="store_true",
        help="Push without asking after committing",
    )
    publish.add_argument(
        "--mode",
        choices=("single", "per-file"),
        help="Commit all changes together or one file/logical change at a time",
    )
    publish.add_argument("--remote", help="Git remote to push to")
    publish.set_defaults(handler=command_publish, needs_config=True)

    check = command_parser(commands, "check", help="Run Flake and static checks")
    check.add_argument("--all", action="store_true", help="Run flake, nixfmt, statix, and ruff")
    check.add_argument("--nixfmt", action="store_true", help="Run nixfmt --check")
    check.add_argument("--statix", action="store_true", help="Run statix check")
    check.add_argument("--ruff", action="store_true", help="Run ruff check")
    check.add_argument("--no-flake", action="store_true", help="Skip nix flake check")
    check.set_defaults(handler=command_check, needs_config=True)

    doctor = command_parser(commands, "doctor", help="Show diagnostics")
    doctor.set_defaults(handler=command_doctor, needs_config=True)

    cheat = commands.add_parser("cheat", help="Show the command cheat sheet")
    cheat.set_defaults(handler=command_cheat, needs_config=False)

    return parser


def main() -> int:
    parser = create_parser()
    args = parser.parse_args()

    if not hasattr(args, "handler"):
        parser.print_help()
        return 0

    try:
        if getattr(args, "needs_config", False):
            args.config = load_config(
                flake=getattr(args, "flake", None),
                host=getattr(args, "host", None),
            )
            args.target = args.config.target
        return args.handler(args)
    except KeyboardInterrupt:
        print("\nCancelled.", file=sys.stderr)
        return 130
    except NrError as error:
        print(f"Error: {error}", file=sys.stderr)
        return 1
    except FileNotFoundError as error:
        print(f"Error: required command not found: {error.filename}", file=sys.stderr)
        return 127
    except subprocess.CalledProcessError as error:
        print(f"Command failed with exit code {error.returncode}.", file=sys.stderr)
        return error.returncode
