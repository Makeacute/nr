use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::cli::OpenArgs;
use crate::config::NrConfig;
use crate::errors::{IoContext, NrError, Result};
use crate::prompts::choose;

pub fn run_open(config: &NrConfig, args: &OpenArgs) -> Result<i32> {
    let relative = resolve_file(&config.target.path, &args.file)?;
    let path = config.target.path.join(&relative);
    open_editor(&path)
}

fn resolve_file(root: &Path, input: &str) -> Result<PathBuf> {
    let requested = Path::new(input);
    if requested.is_absolute()
        || requested
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err(NrError::message(
            "FILE must be a relative path inside the flake.",
        ));
    }

    if requested.extension().is_some() {
        let path = root.join(requested);
        validate_nix_file(root, &path)?;
        return Ok(requested.to_path_buf());
    }

    let direct = root.join(requested);
    if direct.is_file() && direct.extension().and_then(|value| value.to_str()) == Some("nix") {
        validate_nix_file(root, &direct)?;
        return Ok(requested.to_path_buf());
    }

    let matches = matching_nix_files(root, input)?;
    match matches.len() {
        0 => Err(NrError::message(format!("No .nix file matches: {input}"))),
        1 => matches
            .into_iter()
            .next()
            .ok_or_else(|| NrError::message(format!("No .nix file matches: {input}"))),
        _ => choose_match(matches),
    }
}

fn choose_match(matches: Vec<PathBuf>) -> Result<PathBuf> {
    let choices = matches
        .iter()
        .map(|path| {
            let text = path.display().to_string();
            (text.clone(), text)
        })
        .collect::<Vec<_>>();
    let borrowed = choices
        .iter()
        .map(|(key, label)| (key.as_str(), label.as_str()))
        .collect::<Vec<_>>();
    let selected =
        choose("Open file", &borrowed, None).ok_or_else(|| NrError::message("Open cancelled."))?;
    matches
        .into_iter()
        .find(|path| path.display().to_string() == selected)
        .ok_or_else(|| NrError::message("Selected file was not found."))
}

fn validate_nix_file(root: &Path, path: &Path) -> Result<()> {
    let canonical_root = root
        .canonicalize()
        .with_context(format!("failed to resolve {}", root.display()))?;
    let canonical_path = path
        .canonicalize()
        .with_context(format!("failed to resolve {}", path.display()))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(NrError::message("FILE must stay inside the flake."));
    }
    if canonical_path.extension().and_then(|value| value.to_str()) != Some("nix") {
        return Err(NrError::message("FILE must be a .nix file."));
    }
    Ok(())
}

fn matching_nix_files(root: &Path, query: &str) -> Result<Vec<PathBuf>> {
    let mut matches = Vec::new();
    collect_matches(root, root, query, &mut matches)?;
    matches.sort();
    Ok(matches)
}

fn collect_matches(
    root: &Path,
    directory: &Path,
    query: &str,
    matches: &mut Vec<PathBuf>,
) -> Result<()> {
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
            collect_matches(root, &path, query, matches)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("nix") {
            let relative = path.strip_prefix(root).map_err(|error| {
                NrError::message(format!(
                    "failed to make {} relative to {}: {error}",
                    path.display(),
                    root.display()
                ))
            })?;
            let label = relative.display().to_string();
            if label.contains(query) {
                matches.push(relative.to_path_buf());
            }
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

fn editor() -> String {
    env::var("EDITOR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("VISUAL")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".to_string())
}

#[cfg(unix)]
fn open_editor(path: &Path) -> Result<i32> {
    use std::os::unix::process::CommandExt;

    let error = Command::new(editor()).arg(path).exec();
    Err(error).with_context(format!("failed to exec editor for {}", path.display()))
}

#[cfg(not(unix))]
fn open_editor(path: &Path) -> Result<i32> {
    let status = Command::new(editor())
        .arg(path)
        .status()
        .with_context(format!("failed to run editor for {}", path.display()))?;
    Ok(status.code().unwrap_or(1))
}
