import shlex
import subprocess
from collections.abc import Sequence
from pathlib import Path


def render_command(command: Sequence[str]) -> str:
    rendered = shlex.join(command)
    if len(rendered) > 180:
        rendered = f"{shlex.join(command[:3])} ... ({len(command) - 3} more arguments)"
    return rendered


def run(
    command: Sequence[str],
    *,
    cwd: Path | None = None,
    check: bool = True,
    capture_output: bool = False,
    input: str | None = None,
    announce: bool = True,
) -> subprocess.CompletedProcess[str]:
    if announce:
        print(f"-> {render_command(command)}", flush=True)

    return subprocess.run(
        command,
        cwd=cwd,
        check=check,
        capture_output=capture_output,
        input=input,
        text=True,
    )
