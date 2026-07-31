import shlex
from dataclasses import dataclass
from pathlib import Path

from nr.errors import NrError
from nr.process import run


def git_command(flake_path: Path, *arguments: str) -> list[str]:
    return ["git", "-C", str(flake_path), *arguments]


@dataclass(frozen=True)
class GitStatusEntry:
    index: str
    worktree: str
    paths: tuple[str, ...]

    @property
    def status(self) -> str:
        return f"{self.index}{self.worktree}"

    @property
    def primary_path(self) -> str:
        return self.paths[0]

    @property
    def label(self) -> str:
        if len(self.paths) == 2:
            return f"{self.paths[1]} -> {self.paths[0]}"
        return self.paths[0]

    @property
    def is_staged_only(self) -> bool:
        return self.index not in {" ", "?"} and self.worktree == " "

    @property
    def has_index_change(self) -> bool:
        return self.index not in {" ", "?"}

    @property
    def has_worktree_change(self) -> bool:
        return self.status == "??" or self.worktree != " "


def is_git_repository(flake_path: Path) -> bool:
    result = run(
        git_command(flake_path, "rev-parse", "--is-inside-work-tree"),
        check=False,
        capture_output=True,
    )
    return result.returncode == 0 and result.stdout.strip() == "true"


def ensure_git_repository(flake_path: Path) -> None:
    if not is_git_repository(flake_path):
        raise NrError(f"Not a Git repository: {flake_path}")


def untracked_files(flake_path: Path) -> list[str]:
    result = run(
        git_command(
            flake_path,
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
        ),
        capture_output=True,
    )
    return [name for name in result.stdout.split("\0") if name]


def ensure_git_flake_visible(flake_path: Path) -> None:
    if not is_git_repository(flake_path):
        return

    untracked = untracked_files(flake_path)
    if not untracked:
        return

    formatted = "\n".join(f"  {name}" for name in untracked)
    suggestion = shlex.join(["git", "-C", str(flake_path), "add", "--", *untracked])
    raise NrError(
        "Untracked files are invisible to Git flakes.\n"
        f"{formatted}\n\n"
        "Stage or remove them intentionally, then retry. Suggested command:\n"
        f"  {suggestion}"
    )


def status_short(flake_path: Path) -> str:
    ensure_git_repository(flake_path)
    return run(
        git_command(flake_path, "status", "--short"),
        capture_output=True,
    ).stdout


def status_entries(flake_path: Path) -> list[GitStatusEntry]:
    ensure_git_repository(flake_path)
    result = run(
        git_command(flake_path, "status", "--porcelain=v1", "-z"),
        capture_output=True,
    )
    records = [record for record in result.stdout.split("\0") if record]
    entries: list[GitStatusEntry] = []
    index = 0
    while index < len(records):
        record = records[index]
        if len(record) < 4:
            raise NrError(f"Unexpected Git status record: {record!r}")
        index_status = record[0]
        worktree_status = record[1]
        path = record[3:]
        index += 1

        if index_status in {"R", "C"} or worktree_status in {"R", "C"}:
            if index >= len(records):
                raise NrError(f"Unexpected Git rename/copy status record: {record!r}")
            old_path = records[index]
            index += 1
            entries.append(GitStatusEntry(index_status, worktree_status, (path, old_path)))
            continue

        entries.append(GitStatusEntry(index_status, worktree_status, (path,)))

    return entries


def staged_paths(flake_path: Path) -> list[str]:
    result = run(
        git_command(flake_path, "diff", "--cached", "--name-only", "-z"),
        capture_output=True,
    )
    return [path for path in result.stdout.split("\0") if path]


def current_branch(flake_path: Path) -> str:
    result = run(
        git_command(flake_path, "branch", "--show-current"),
        capture_output=True,
    )
    branch = result.stdout.strip()
    if not branch:
        raise NrError("Cannot push from a detached HEAD.")
    return branch
