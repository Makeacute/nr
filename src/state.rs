use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::errors::{IoContext, NrError, Result};
use crate::process::state_dir;

pub fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn write_json<T: Serialize>(
    directory: &Path,
    prefix: &str,
    value: &T,
    keep: usize,
) -> Result<PathBuf> {
    fs::create_dir_all(directory)
        .with_context(format!("failed to create {}", directory.display()))?;
    let path = directory.join(format!(
        "{prefix}-{}-{}.json",
        timestamp(),
        std::process::id()
    ));
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| NrError::message(format!("failed to serialize state JSON: {error}")))?;
    fs::write(&path, text).with_context(format!("failed to write {}", path.display()))?;
    prune_json(directory, prefix, keep, Some(&path))?;
    Ok(path)
}

pub fn prune_json(
    directory: &Path,
    prefix: &str,
    keep: usize,
    preserve: Option<&Path>,
) -> Result<()> {
    if keep == 0 || !directory.is_dir() {
        return Ok(());
    }
    let mut files = sorted_json_files(directory, prefix)?;
    if let Some(preserve) = preserve {
        files.retain(|path| path != preserve);
    }
    let remove_count = files
        .len()
        .saturating_sub(keep.saturating_sub(preserve.is_some() as usize));
    for path in files.into_iter().take(remove_count) {
        fs::remove_file(&path).with_context(format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

pub fn sorted_json_files(directory: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
    sorted_files(directory, prefix, ".json")
}

pub fn sorted_log_files() -> Result<Vec<PathBuf>> {
    sorted_files(&logs_dir(), "nr-", ".log")
}

pub fn latest_json(directory: &Path, prefix: &str) -> Result<PathBuf> {
    sorted_json_files(directory, prefix)?
        .pop()
        .ok_or_else(|| NrError::message(format!("No {prefix} files found.")))
}

pub fn resolve_json_reference(directory: &Path, prefix: &str, reference: &str) -> Result<PathBuf> {
    if reference == "latest" {
        return latest_json(directory, prefix);
    }
    let direct = PathBuf::from(reference);
    if direct.is_file() {
        return Ok(direct);
    }
    for path in sorted_json_files(directory, prefix)? {
        if path
            .file_stem()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains(reference))
        {
            return Ok(path);
        }
    }
    Err(NrError::message(format!(
        "No {prefix} file matches reference: {reference}"
    )))
}

pub fn plans_dir() -> PathBuf {
    state_dir().join("plans")
}

pub fn reports_dir() -> PathBuf {
    state_dir().join("reports")
}

pub fn logs_dir() -> PathBuf {
    state_dir().join("logs")
}

pub fn history_path() -> PathBuf {
    state_dir().join("history.json")
}

fn sorted_files(directory: &Path, prefix: &str, suffix: &str) -> Result<Vec<PathBuf>> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in
        fs::read_dir(directory).with_context(format!("failed to read {}", directory.display()))?
    {
        let entry =
            entry.with_context(format!("failed to read entry in {}", directory.display()))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with(prefix) && name.ends_with(suffix) && path.is_file() {
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            files.push((modified, path));
        }
    }
    files.sort_by_key(|(modified, _)| *modified);
    Ok(files.into_iter().map(|(_, path)| path).collect())
}
