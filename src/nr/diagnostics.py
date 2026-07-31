import argparse
import shutil

from nr.git import git_command
from nr.process import run

REQUIRED_TOOLS = ("nix", "nh", "git")
OPTIONAL_TOOLS = ("gh", "nixfmt", "statix", "ruff")


def tool_version(name: str) -> str:
    result = run([name, "--version"], check=False, capture_output=True, announce=False)
    output = (result.stdout or result.stderr).strip()
    if result.returncode == 0 and output:
        return output.splitlines()[0]

    help_result = run([name, "--help"], check=False, capture_output=True, announce=False)
    help_output = (help_result.stdout or help_result.stderr).strip()
    if help_output:
        return f"installed ({help_output.splitlines()[0]})"
    return "installed"


def command_doctor(args: argparse.Namespace) -> int:
    config = args.config
    print("nr doctor")
    print(f"target: {config.target.reference}")
    print(f"user config: {config.user_config_path or 'not found'}")
    print(f"repo config: {config.repo_config_path or 'not found'}")

    missing_required: list[str] = []
    print("\ndependencies:")
    for name in REQUIRED_TOOLS:
        if shutil.which(name):
            print(f"  ok       {tool_version(name)}")
        else:
            missing_required.append(name)
            print(f"  missing  {name}")
    for name in OPTIONAL_TOOLS:
        if shutil.which(name):
            print(f"  optional {tool_version(name)}")
        else:
            print(f"  optional {name}: not installed")

    print("\ngit:")
    git_check = run(
        git_command(config.target.path, "rev-parse", "--is-inside-work-tree"),
        check=False,
        capture_output=True,
        announce=False,
    )
    if git_check.returncode == 0 and git_check.stdout.strip() == "true":
        status = run(
            git_command(config.target.path, "status", "--short"),
            capture_output=True,
            announce=False,
        ).stdout.strip()
        print("  repository: yes")
        print(f"  status: {'dirty' if status else 'clean'}")
        if status:
            for line in status.splitlines()[:20]:
                print(f"    {line}")
            if len(status.splitlines()) > 20:
                print("    ...")
    else:
        print("  repository: no")

    return 1 if missing_required else 0
