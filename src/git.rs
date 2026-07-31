use std::path::{Path, PathBuf};

use crate::errors::{NrError, Result};
use crate::process::{CommandSpec, run_capture, shell_quote};

pub fn git_command(flake_path: &Path, arguments: &[&str]) -> CommandSpec {
    CommandSpec::new("git")
        .arg("-C")
        .arg(flake_path.display().to_string())
        .args(arguments.iter().map(|value| value.to_string()))
}

fn git_command_owned(flake_path: &Path, arguments: &[String]) -> CommandSpec {
    CommandSpec::new("git")
        .arg("-C")
        .arg(flake_path.display().to_string())
        .args(arguments.iter().cloned())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitStatusEntry {
    pub index: char,
    pub worktree: char,
    pub paths: Vec<String>,
}

impl GitStatusEntry {
    pub fn status(&self) -> String {
        format!("{}{}", self.index, self.worktree)
    }

    pub fn label(&self) -> String {
        if self.paths.len() == 2 {
            format!("{} -> {}", self.paths[1], self.paths[0])
        } else {
            self.paths.first().cloned().unwrap_or_default()
        }
    }

    pub fn is_staged_only(&self) -> bool {
        self.index != ' ' && self.index != '?' && self.worktree == ' '
    }

    pub fn has_worktree_change(&self) -> bool {
        self.status() == "??" || self.worktree != ' '
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitSummary {
    pub repository: bool,
    pub branch: Option<String>,
    pub dirty: bool,
    pub untracked: usize,
}

pub fn is_git_repository(flake_path: &Path) -> bool {
    let output = run_capture(
        &git_command(flake_path, &["rev-parse", "--is-inside-work-tree"]),
        false,
    );
    matches!(output, Ok(output) if output.code == 0 && output.stdout.trim() == "true")
}

pub fn ensure_git_repository(flake_path: &Path) -> Result<()> {
    if !is_git_repository(flake_path) {
        return Err(NrError::message(format!(
            "Not a Git repository: {}",
            flake_path.display()
        )));
    }
    Ok(())
}

pub fn untracked_files(flake_path: &Path) -> Result<Vec<String>> {
    let output = run_capture(
        &git_command(
            flake_path,
            &["ls-files", "--others", "--exclude-standard", "-z"],
        ),
        false,
    )?;
    Ok(output
        .stdout
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn ensure_git_flake_visible(flake_path: &Path) -> Result<()> {
    if !is_git_repository(flake_path) {
        return Ok(());
    }

    let untracked = untracked_files(flake_path)?;
    if untracked.is_empty() {
        return Ok(());
    }

    let formatted = untracked
        .iter()
        .map(|name| format!("  {name}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut suggestion = vec![
        "git".to_string(),
        "-C".to_string(),
        flake_path.display().to_string(),
        "add".to_string(),
        "--".to_string(),
    ];
    suggestion.extend(untracked.iter().cloned());
    let suggestion = suggestion
        .iter()
        .map(|part| shell_quote(part))
        .collect::<Vec<_>>()
        .join(" ");

    Err(NrError::message(format!(
        "Untracked files are invisible to Git flakes.\n{formatted}\n\nStage or remove them intentionally, then retry. Suggested command:\n  {suggestion}"
    )))
}

pub fn status_short(flake_path: &Path) -> Result<String> {
    ensure_git_repository(flake_path)?;
    Ok(run_capture(&git_command(flake_path, &["status", "--short"]), false)?.stdout)
}

pub fn status_entries(flake_path: &Path) -> Result<Vec<GitStatusEntry>> {
    ensure_git_repository(flake_path)?;
    let output = run_capture(
        &git_command(flake_path, &["status", "--porcelain=v1", "-z"]),
        false,
    )?;
    let records = output
        .stdout
        .split('\0')
        .filter(|record| !record.is_empty())
        .collect::<Vec<_>>();
    let mut entries = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        if record.len() < 4 {
            return Err(NrError::message(format!(
                "Unexpected Git status record: {record:?}"
            )));
        }
        let mut chars = record.chars();
        let index_status = chars.next().unwrap_or(' ');
        let worktree_status = chars.next().unwrap_or(' ');
        let path = record[3..].to_string();
        index += 1;

        if matches!(index_status, 'R' | 'C') || matches!(worktree_status, 'R' | 'C') {
            if index >= records.len() {
                return Err(NrError::message(format!(
                    "Unexpected Git rename/copy status record: {record:?}"
                )));
            }
            let old_path = records[index].to_string();
            index += 1;
            entries.push(GitStatusEntry {
                index: index_status,
                worktree: worktree_status,
                paths: vec![path, old_path],
            });
            continue;
        }

        entries.push(GitStatusEntry {
            index: index_status,
            worktree: worktree_status,
            paths: vec![path],
        });
    }
    Ok(entries)
}

pub fn staged_paths(flake_path: &Path) -> Result<Vec<String>> {
    let output = run_capture(
        &git_command(flake_path, &["diff", "--cached", "--name-only", "-z"]),
        false,
    )?;
    Ok(output
        .stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn current_branch(flake_path: &Path) -> Result<String> {
    let output = run_capture(
        &git_command(flake_path, &["branch", "--show-current"]),
        false,
    )?;
    let branch = output.stdout.trim();
    if branch.is_empty() {
        return Err(NrError::message("Cannot push from a detached HEAD."));
    }
    Ok(branch.to_string())
}

pub fn git_summary(flake_path: &Path) -> GitSummary {
    if !is_git_repository(flake_path) {
        return GitSummary {
            repository: false,
            branch: None,
            dirty: false,
            untracked: 0,
        };
    }
    let branch = current_branch(flake_path).ok();
    let status = status_short(flake_path).unwrap_or_default();
    let untracked = untracked_files(flake_path)
        .map(|items| items.len())
        .unwrap_or(0);
    GitSummary {
        repository: true,
        branch,
        dirty: !status.trim().is_empty(),
        untracked,
    }
}

pub fn add_all_command(flake_path: &Path) -> CommandSpec {
    git_command(flake_path, &["add", "-A"])
}

pub fn add_paths_command(flake_path: &Path, paths: &[String]) -> CommandSpec {
    let mut args = vec!["add".to_string(), "-A".to_string(), "--".to_string()];
    args.extend(paths.iter().cloned());
    git_command_owned(flake_path, &args)
}

pub fn commit_command(flake_path: &Path, message: &str) -> CommandSpec {
    git_command_owned(
        flake_path,
        &["commit".to_string(), "-m".to_string(), message.to_string()],
    )
}

pub fn push_command(flake_path: &Path, args: &[String]) -> CommandSpec {
    let mut all = vec!["push".to_string()];
    all.extend(args.iter().cloned());
    git_command_owned(flake_path, &all)
}

pub fn path_from_string(value: &str) -> PathBuf {
    PathBuf::from(value)
}
