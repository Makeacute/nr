import argparse

from nr.config import FlakeTarget


def backend_args(args: argparse.Namespace) -> list[str]:
    values = list(getattr(args, "backend_args", []) or [])
    if values and values[0] == "--":
        values.pop(0)
    return values


def nh_common_args(args: argparse.Namespace, *, include_nix_flags: bool = True) -> list[str]:
    command: list[str] = []
    for _index in range(getattr(args, "verbose", 0) or 0):
        command.append("--verbose")
    if getattr(args, "dry", False):
        command.append("--dry")
    if getattr(args, "ask", False):
        command.append("--ask")
    if include_nix_flags and getattr(args, "offline", False):
        command.append("--offline")
    if include_nix_flags and getattr(args, "show_trace", False):
        command.append("--show-trace")
    if specialisation := getattr(args, "specialisation", None):
        command.extend(["--specialisation", specialisation])
    return command


def nix_common_args(args: argparse.Namespace) -> list[str]:
    command: list[str] = []
    if getattr(args, "offline", False):
        command.append("--offline")
    if getattr(args, "show_trace", False):
        command.append("--show-trace")
    return command


def nh_lifecycle_command(
    action: str,
    target: FlakeTarget,
    args: argparse.Namespace,
) -> list[str]:
    return [
        "nh",
        "os",
        action,
        *nh_common_args(args),
        target.reference,
        *backend_args(args),
    ]


def nh_rollback_command(args: argparse.Namespace) -> list[str]:
    command = [
        "nh",
        "os",
        "rollback",
        *nh_common_args(args, include_nix_flags=False),
    ]
    if generation := getattr(args, "to", None):
        command.extend(["--to", str(generation)])
    command.extend(backend_args(args))
    return command


def nh_generations_command(args: argparse.Namespace) -> list[str]:
    command = ["nh", "os", "info"]
    for _index in range(getattr(args, "verbose", 0) or 0):
        command.append("--verbose")
    if profile := getattr(args, "profile", None):
        command.extend(["--profile", profile])
    fields = getattr(args, "fields", None)
    if fields:
        command.extend(["--fields", ",".join(fields)])
    command.extend(backend_args(args))
    return command
