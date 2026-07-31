import argparse
import shutil
from pathlib import Path

from nr.errors import NrError
from nr.git import (
    GitStatusEntry,
    current_branch,
    ensure_git_repository,
    git_command,
    staged_paths,
    status_entries,
    status_short,
)
from nr.process import run
from nr.prompts import choose, confirm, read_line


def command_publish(args: argparse.Namespace) -> int:
    flake_path = args.target.path
    ensure_git_repository(flake_path)

    status = status_short(flake_path)
    if not status.strip():
        print("Nothing to publish.")
        return 0

    print(status, end="")
    mode = args.mode or prompt_commit_mode()
    if mode is None:
        print("Publish cancelled.")
        return 0

    if mode == "per-file" and args.message:
        raise NrError("--message can only be used with --mode single.")

    if mode == "single":
        committed = publish_single_commit(flake_path, args.message)
    else:
        committed = publish_per_file(flake_path)

    if not committed:
        print("No commits created.")
        return 0

    remote = args.remote or args.config.publish.remote
    if args.push or confirm("Push committed changes now?"):
        if push_with_remote_setup(flake_path, remote):
            print("Pushed.")
        else:
            print("Committed locally; push skipped.")
    else:
        print("Committed locally; push skipped.")

    return 0


def prompt_commit_mode() -> str | None:
    return choose(
        "Commit mode",
        [
            ("single", "one commit for all changes"),
            ("per-file", "one commit per file/logical change"),
        ],
        default="single",
    )


def publish_single_commit(flake_path: Path, message: str | None) -> bool:
    if not confirm("Stage all changes for one commit?", default=True):
        print("Publish cancelled.")
        return False

    run(git_command(flake_path, "add", "-A"))
    if not has_staged_changes(flake_path):
        return False

    review_staged_diff(flake_path)
    if not confirm("Create this commit?", default=True):
        print("Commit skipped; staged changes were left in place.")
        return False

    run(git_command(flake_path, "commit", "-m", commit_message(message)))
    return True


def publish_per_file(flake_path: Path) -> bool:
    staged = staged_paths(flake_path)
    if len(staged) > 1:
        raise NrError(
            "Per-file publish refuses pre-staged changes spanning multiple files. "
            "Commit or unstage them first."
        )

    committed = False
    if staged:
        print(f"Pre-staged change: {staged[0]}")
        if not confirm("Commit this staged change first?", default=True):
            print("Stopped before unstaged files so the existing index stays untouched.")
            return False
        review_staged_diff(flake_path)
        run(git_command(flake_path, "commit", "-m", commit_message(None)))
        committed = True

    skipped: set[tuple[str, ...]] = set()
    while True:
        change = next_unstaged_change(flake_path, skipped)
        if change is None:
            break

        print(f"{change.status} {change.label}")
        if not confirm("Commit this change?", default=True):
            skipped.add(change.paths)
            continue

        run(git_command(flake_path, "add", "-A", "--", *change.paths))
        review_staged_diff(flake_path)
        if not confirm("Create this commit?", default=True):
            run(git_command(flake_path, "restore", "--staged", "--", *change.paths), check=False)
            skipped.add(change.paths)
            print("Commit skipped; staged change was restored to the worktree.")
            continue

        run(git_command(flake_path, "commit", "-m", commit_message(None)))
        committed = True

    return committed


def next_unstaged_change(
    flake_path: Path,
    skipped: set[tuple[str, ...]],
) -> GitStatusEntry | None:
    for entry in status_entries(flake_path):
        if entry.paths in skipped:
            continue
        if entry.is_staged_only:
            continue
        if entry.has_worktree_change:
            return entry
    return None


def has_staged_changes(flake_path: Path) -> bool:
    return (
        run(
            git_command(flake_path, "diff", "--cached", "--quiet"),
            check=False,
        ).returncode
        != 0
    )


def review_staged_diff(flake_path: Path) -> None:
    run(git_command(flake_path, "diff", "--cached", "--stat"))
    if confirm("Show full staged diff?"):
        run(git_command(flake_path, "--no-pager", "diff", "--cached"))


def commit_message(message: str | None) -> str:
    if message is not None:
        message = message.strip()
        if not message:
            raise NrError("Commit message cannot be empty.")
        return message

    value = read_line("Commit message")
    if value is None:
        raise NrError("Commit message is required.")
    value = value.strip()
    if not value:
        raise NrError("Commit message cannot be empty.")
    return value


def push_with_remote_setup(flake_path: Path, remote: str) -> bool:
    if not remote_exists(flake_path, remote) and not configure_missing_remote(flake_path, remote):
        return False

    branch = current_branch(flake_path)
    upstream = run(
        git_command(flake_path, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"),
        check=False,
        capture_output=True,
    )
    if upstream.returncode == 0:
        run(git_command(flake_path, "push"))
    else:
        run(git_command(flake_path, "push", "--set-upstream", remote, branch))
    return True


def remote_exists(flake_path: Path, remote: str) -> bool:
    return (
        run(
            git_command(flake_path, "remote", "get-url", remote),
            check=False,
            capture_output=True,
        ).returncode
        == 0
    )


def configure_missing_remote(flake_path: Path, remote: str) -> bool:
    print(f"No Git remote named '{remote}'.")
    if not shutil.which("gh"):
        print_manual_remote_instructions(flake_path, remote)
        return False

    if not confirm("Configure a GitHub remote with gh now?"):
        print_manual_remote_instructions(flake_path, remote)
        return False

    auth = run(["gh", "auth", "status"], check=False, capture_output=True)
    if auth.returncode != 0:
        if confirm("Run 'gh auth login' now?"):
            run(["gh", "auth", "login"])
        else:
            print_manual_remote_instructions(flake_path, remote)
            return False

    action = choose(
        "GitHub remote setup",
        [
            ("existing", "connect an existing repository"),
            ("create", "create a new repository"),
        ],
        default="existing",
    )
    if action is None:
        return False
    if action == "existing":
        repo = read_line("Repository (owner/name or URL)")
        if repo is None or not repo.strip():
            raise NrError("Repository cannot be empty.")
        url = github_repo_url(repo.strip())
        run(git_command(flake_path, "remote", "add", remote, url))
        return True

    name = read_line("Repository name", default=flake_path.name)
    if name is None or not name.strip():
        raise NrError("Repository name cannot be empty.")
    visibility = choose(
        "Visibility",
        [("public", "public repository"), ("private", "private repository")],
        default="public",
    )
    if visibility is None:
        return False
    run(
        [
            "gh",
            "repo",
            "create",
            name.strip(),
            f"--{visibility}",
            "--source",
            str(flake_path),
            "--remote",
            remote,
        ]
    )
    return True


def github_repo_url(value: str) -> str:
    if value.startswith(("https://", "ssh://", "git@")):
        return value
    result = run(
        ["gh", "repo", "view", value, "--json", "url", "--jq", ".url"],
        capture_output=True,
    )
    return result.stdout.strip()


def print_manual_remote_instructions(flake_path: Path, remote: str) -> None:
    print("Add a remote manually, then run publish again or push directly:")
    print(f"  git -C {flake_path} remote add {remote} https://github.com/OWNER/REPO.git")
    print(
        f"  git -C {flake_path} push --set-upstream {remote} "
        f"$(git -C {flake_path} branch --show-current)"
    )
