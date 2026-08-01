use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::cli::LintArgs;
use crate::config::NrConfig;
use crate::errors::{IoContext, NrError, Result};
use crate::git::{git_command, is_git_repository};
use crate::process::{CommandSpec, run_capture};

pub fn run_lint(config: &NrConfig, _args: &LintArgs) -> Result<i32> {
    let files = changed_nix_files(&config.target.path)?;
    if files.is_empty() {
        println!("No changed .nix files.");
        return Ok(0);
    }

    let mut failed = false;
    for file in &files {
        let nixfmt = run_tool(
            CommandSpec::new("nixfmt")
                .arg("--check")
                .arg(config.target.path.join(file).display().to_string()),
        )?;
        let deadnix = run_tool(
            CommandSpec::new("deadnix")
                .arg("--fail")
                .arg(config.target.path.join(file).display().to_string()),
        )?;
        if nixfmt && deadnix {
            println!("✓ {}", file.display());
        } else {
            failed = true;
            println!("✗ {}", file.display());
        }
    }
    Ok(if failed { 1 } else { 0 })
}

fn run_tool(command: CommandSpec) -> Result<bool> {
    let output = run_capture(&command, false)?;
    if output.code == 0 {
        return Ok(true);
    }
    for line in output.stdout.lines().chain(output.stderr.lines()) {
        eprintln!("{line}");
    }
    Ok(false)
}

fn changed_nix_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = if is_git_repository(root) {
        git_changed_files(root)?
    } else {
        all_nix_files(root)?
    };
    files.retain(|path| path.extension().and_then(|value| value.to_str()) == Some("nix"));
    files.sort();
    Ok(files)
}

fn git_changed_files(root: &Path) -> Result<Vec<PathBuf>> {
    let output = run_capture(&git_command(root, &["diff", "--name-only", "HEAD"]), false)?;
    if output.code != 0 {
        return all_nix_files(root);
    }
    Ok(output
        .stdout
        .lines()
        .map(PathBuf::from)
        .filter(|path| root.join(path).is_file())
        .collect())
}

fn all_nix_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_nix_files(root, root, &mut files)?;
    Ok(files)
}

fn collect_nix_files(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(directory).with_context(format!("failed to read {}", directory.display()))?
    {
        let entry =
            entry.with_context(format!("failed to read entry in {}", directory.display()))?;
        let path = entry.path();
        if skip_path(&path) {
            continue;
        }
        let metadata = entry
            .metadata()
            .with_context(format!("failed to stat {}", path.display()))?;
        if metadata.is_dir() {
            collect_nix_files(root, &path, files)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("nix") {
            files.push(
                path.strip_prefix(root)
                    .map_err(|error| {
                        NrError::message(format!(
                            "failed to make {} relative to {}: {error}",
                            path.display(),
                            root.display()
                        ))
                    })?
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}

fn skip_path(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::Normal(value)
                if matches!(
                    value.to_str(),
                    Some(".git" | "target" | ".direnv" | "__pycache__" | "node_modules")
                )
        )
    })
}
