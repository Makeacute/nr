use crate::cli::{PublishArgs, PublishMode};
use crate::config::NrConfig;
use crate::errors::{NrError, Result};
use crate::git::{
    add_all_command, add_paths_command, commit_command, current_branch, ensure_git_repository,
    git_command, push_command, staged_paths, status_entries, status_short,
};
use crate::process::{CommandSpec, run_capture, run_inherit};
use crate::prompts::{choose, confirm, read_line};

pub fn run_publish(config: &NrConfig, args: &PublishArgs) -> Result<i32> {
    let flake_path = &config.target.path;
    ensure_git_repository(flake_path)?;

    let status = status_short(flake_path)?;
    if !status.trim().is_empty() {
        print!("{status}");
    } else {
        println!("Nothing to publish.");
        return Ok(0);
    }

    let mode = match args.mode {
        Some(mode) => mode,
        None => match prompt_commit_mode() {
            Some("single") => PublishMode::Single,
            Some("per-file") => PublishMode::PerFile,
            _ => {
                println!("Publish cancelled.");
                return Ok(0);
            }
        },
    };

    if mode == PublishMode::PerFile && args.message.is_some() {
        return Err(NrError::message(
            "--message can only be used with --mode single.",
        ));
    }

    let committed = match mode {
        PublishMode::Single => publish_single_commit(config, args.message.as_deref())?,
        PublishMode::PerFile => publish_per_file(config)?,
    };

    if !committed {
        println!("No commits created.");
        return Ok(0);
    }

    let remote = args.remote.as_deref().unwrap_or(&config.publish.remote);
    if args.push || confirm("Push committed changes now?", false) {
        if push_with_remote_setup(config, remote)? {
            println!("Pushed.");
        } else {
            println!("Committed locally; push skipped.");
        }
    } else {
        println!("Committed locally; push skipped.");
    }

    Ok(0)
}

fn prompt_commit_mode() -> Option<&'static str> {
    let choice = choose(
        "Commit mode",
        &[
            ("single", "one commit for all changes"),
            ("per-file", "one commit per file/logical change"),
        ],
        Some("single"),
    )?;
    Some(if choice == "per-file" {
        "per-file"
    } else {
        "single"
    })
}

fn publish_single_commit(config: &NrConfig, message: Option<&str>) -> Result<bool> {
    let flake_path = &config.target.path;
    if !confirm("Stage all changes for one commit?", true) {
        println!("Publish cancelled.");
        return Ok(false);
    }

    checked(&add_all_command(flake_path))?;
    if !has_staged_changes(flake_path)? {
        return Ok(false);
    }

    review_staged_diff(flake_path)?;
    if !confirm("Create this commit?", true) {
        println!("Commit skipped; staged changes were left in place.");
        return Ok(false);
    }

    checked(&commit_command(flake_path, &commit_message(message)?))?;
    Ok(true)
}

fn publish_per_file(config: &NrConfig) -> Result<bool> {
    let flake_path = &config.target.path;
    let staged = staged_paths(flake_path)?;
    if staged.len() > 1 {
        return Err(NrError::message(
            "Per-file publish refuses pre-staged changes spanning multiple files. Commit or unstage them first.",
        ));
    }

    let mut committed = false;
    if let Some(path) = staged.first() {
        println!("Pre-staged change: {path}");
        if !confirm("Commit this staged change first?", true) {
            println!("Stopped before unstaged files so the existing index stays untouched.");
            return Ok(false);
        }
        review_staged_diff(flake_path)?;
        checked(&commit_command(flake_path, &commit_message(None)?))?;
        committed = true;
    }

    let mut skipped: Vec<Vec<String>> = Vec::new();
    loop {
        let change = status_entries(flake_path)?.into_iter().find(|entry| {
            !entry.is_staged_only()
                && entry.has_worktree_change()
                && !skipped.contains(&entry.paths)
        });
        let Some(change) = change else {
            break;
        };

        println!("{} {}", change.status(), change.label());
        if !confirm("Commit this change?", true) {
            skipped.push(change.paths);
            continue;
        }
        checked(&add_paths_command(flake_path, &change.paths))?;
        review_staged_diff(flake_path)?;
        if !confirm("Create this commit?", true) {
            let restore = {
                let mut args = vec![
                    "restore".to_string(),
                    "--staged".to_string(),
                    "--".to_string(),
                ];
                args.extend(change.paths.clone());
                CommandSpec::new("git")
                    .arg("-C")
                    .arg(flake_path.display().to_string())
                    .args(args)
            };
            let _ = run_inherit(&restore, true);
            skipped.push(change.paths);
            println!("Commit skipped; staged change was restored to the worktree.");
            continue;
        }
        checked(&commit_command(flake_path, &commit_message(None)?))?;
        committed = true;
    }

    Ok(committed)
}

fn has_staged_changes(flake_path: &std::path::Path) -> Result<bool> {
    let output = run_capture(
        &git_command(flake_path, &["diff", "--cached", "--quiet"]),
        false,
    )?;
    Ok(output.code != 0)
}

fn review_staged_diff(flake_path: &std::path::Path) -> Result<()> {
    checked(&git_command(flake_path, &["diff", "--cached", "--stat"]))?;
    if confirm("Show full staged diff?", false) {
        checked(&git_command(
            flake_path,
            &["--no-pager", "diff", "--cached"],
        ))?;
    }
    Ok(())
}

fn commit_message(message: Option<&str>) -> Result<String> {
    if let Some(message) = message {
        let message = message.trim();
        if message.is_empty() {
            return Err(NrError::message("Commit message cannot be empty."));
        }
        return Ok(message.to_string());
    }
    let value = read_line("Commit message", None)
        .ok_or_else(|| NrError::message("Commit message is required."))?;
    let value = value.trim();
    if value.is_empty() {
        return Err(NrError::message("Commit message cannot be empty."));
    }
    Ok(value.to_string())
}

fn push_with_remote_setup(config: &NrConfig, remote: &str) -> Result<bool> {
    let flake_path = &config.target.path;
    if !remote_exists(flake_path, remote)? && !configure_missing_remote(config, remote)? {
        return Ok(false);
    }

    let branch = current_branch(flake_path)?;
    let upstream = run_capture(
        &git_command(
            flake_path,
            &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        ),
        false,
    )?;
    if upstream.code == 0 {
        checked(&push_command(flake_path, &[]))?;
    } else {
        checked(&push_command(
            flake_path,
            &["--set-upstream".to_string(), remote.to_string(), branch],
        ))?;
    }
    Ok(true)
}

fn remote_exists(flake_path: &std::path::Path, remote: &str) -> Result<bool> {
    Ok(run_capture(
        &git_command(flake_path, &["remote", "get-url", remote]),
        false,
    )?
    .code
        == 0)
}

fn configure_missing_remote(config: &NrConfig, remote: &str) -> Result<bool> {
    let flake_path = &config.target.path;
    println!("No Git remote named '{remote}'.");
    if !command_exists("gh") {
        print_manual_remote_instructions(config, remote);
        return Ok(false);
    }

    if !confirm("Configure a GitHub remote with gh now?", false) {
        print_manual_remote_instructions(config, remote);
        return Ok(false);
    }

    let auth = CommandSpec::new("gh").arg("auth").arg("status");
    if run_capture(&auth, false)?.code != 0 {
        if confirm("Run 'gh auth login' now?", false) {
            checked(&CommandSpec::new("gh").arg("auth").arg("login"))?;
        } else {
            print_manual_remote_instructions(config, remote);
            return Ok(false);
        }
    }

    let action = choose(
        "GitHub remote setup",
        &[
            ("existing", "connect an existing repository"),
            ("create", "create a new repository"),
        ],
        Some("existing"),
    );
    match action.as_deref() {
        Some("existing") => {
            let repo = read_line("Repository (owner/name or URL)", None)
                .ok_or_else(|| NrError::message("Repository cannot be empty."))?;
            let repo = repo.trim();
            if repo.is_empty() {
                return Err(NrError::message("Repository cannot be empty."));
            }
            let url = github_repo_url(repo)?;
            checked(&git_command(flake_path, &["remote", "add", remote, &url]))?;
            Ok(true)
        }
        Some("create") => {
            let default_name = flake_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let name = read_line("Repository name", Some(&default_name))
                .ok_or_else(|| NrError::message("Repository name cannot be empty."))?;
            let visibility = choose(
                "Visibility",
                &[
                    ("public", "public repository"),
                    ("private", "private repository"),
                ],
                Some("public"),
            );
            let Some(visibility) = visibility else {
                return Ok(false);
            };
            checked(
                &CommandSpec::new("gh")
                    .arg("repo")
                    .arg("create")
                    .arg(name)
                    .arg(format!("--{visibility}"))
                    .arg("--source")
                    .arg(flake_path.display().to_string())
                    .arg("--remote")
                    .arg(remote),
            )?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn github_repo_url(value: &str) -> Result<String> {
    if value.starts_with("https://") || value.starts_with("ssh://") || value.starts_with("git@") {
        return Ok(value.to_string());
    }
    let output = run_capture(
        &CommandSpec::new("gh")
            .arg("repo")
            .arg("view")
            .arg(value)
            .arg("--json")
            .arg("url")
            .arg("--jq")
            .arg(".url"),
        true,
    )?;
    if output.code != 0 {
        return Err(NrError::CommandFailed {
            command: "gh repo view".to_string(),
            code: output.code,
        });
    }
    Ok(output.stdout.trim().to_string())
}

fn print_manual_remote_instructions(config: &NrConfig, remote: &str) {
    let flake_path = &config.target.path;
    println!("Add a remote manually, then run publish again or push directly:");
    println!(
        "  git -C {} remote add {} https://github.com/OWNER/REPO.git",
        flake_path.display(),
        remote
    );
    println!(
        "  git -C {} push --set-upstream {} $(git -C {} branch --show-current)",
        flake_path.display(),
        remote,
        flake_path.display()
    );
}

fn checked(command: &CommandSpec) -> Result<()> {
    let code = run_inherit(command, true)?;
    if code != 0 {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code,
        });
    }
    Ok(())
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|path| path.join(name).is_file()))
        .unwrap_or(false)
}
