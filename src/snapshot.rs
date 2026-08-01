use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::cli::{Cli, SnapshotArgs};
use crate::config::NrConfig;
use crate::errors::{IoContext, NrError, Result};
use crate::generations::{current_generation, load_system_generations};
use crate::git::{current_revision, git_command, git_summary};
use crate::process::{run_inherit, state_dir};
use crate::prompts::confirm;

pub fn run_snapshot(cli: &Cli, config: &NrConfig, args: &SnapshotArgs) -> Result<i32> {
    let actions = usize::from(args.list)
        + usize::from(args.restore.is_some())
        + usize::from(args.name.is_some());
    if actions != 1 {
        return Err(NrError::message(
            "Use exactly one of --name LABEL, --list, or --restore LABEL.",
        ));
    }
    if args.list {
        return list_snapshots();
    }
    if let Some(label) = &args.restore {
        return restore_snapshot(cli, config, label);
    }
    let label = args
        .name
        .as_deref()
        .ok_or_else(|| NrError::message("--name LABEL is required to create a snapshot."))?;
    create_snapshot(config, label)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SnapshotFile {
    snapshot: SnapshotData,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SnapshotData {
    label: String,
    created_at: u64,
    generation: u64,
    git_revision: String,
    flake_lock: String,
    nixos_version: String,
}

fn create_snapshot(config: &NrConfig, label: &str) -> Result<i32> {
    let label = label.trim();
    if label.is_empty() {
        return Err(NrError::message("snapshot label cannot be empty."));
    }
    let generations = load_system_generations()?;
    let current = current_generation(&generations)
        .ok_or_else(|| NrError::message("failed to determine current generation."))?;
    let revision = current_revision(&config.target.path)
        .ok_or_else(|| NrError::message("failed to determine current Git revision."))?;
    let lock_path = config.target.path.join("flake.lock");
    let flake_lock = fs::read_to_string(&lock_path)
        .with_context(format!("failed to read {}", lock_path.display()))?;
    let snapshot = SnapshotFile {
        snapshot: SnapshotData {
            label: label.to_string(),
            created_at: crate::state::timestamp(),
            generation: current.generation,
            git_revision: revision,
            flake_lock,
            nixos_version: current.nixos_version.clone(),
        },
    };
    let path = snapshot_path(label);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(format!("failed to create {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(&snapshot)
        .map_err(|error| NrError::message(format!("failed to serialize snapshot: {error}")))?;
    fs::write(&path, text).with_context(format!("failed to write {}", path.display()))?;
    println!("snapshot saved: {}", path.display());
    Ok(0)
}

fn list_snapshots() -> Result<i32> {
    let snapshots = read_snapshots()?;
    if snapshots.is_empty() {
        println!("No snapshots.");
        return Ok(0);
    }
    println!("{:<24}  {:<12}  {:<10}  GIT", "LABEL", "DATE", "GENERATION");
    for snapshot in snapshots {
        println!(
            "{:<24}  {:<12}  {:<10}  {}",
            snapshot.label,
            snapshot.created_at,
            snapshot.generation,
            truncate_revision(&snapshot.git_revision)
        );
    }
    Ok(0)
}

fn restore_snapshot(cli: &Cli, config: &NrConfig, label: &str) -> Result<i32> {
    let snapshot = read_snapshot(label)?;
    let summary = git_summary(&config.target.path);
    if summary.dirty {
        eprintln!("warning: working tree is dirty; restore may overwrite local changes.");
    }
    let checkout = git_command(&config.target.path, &["checkout", &snapshot.git_revision]);
    let code = run_inherit(&checkout, true)?;
    if code != 0 {
        return Err(NrError::CommandFailed {
            command: checkout.render(),
            code,
        });
    }
    let lock_path = config.target.path.join("flake.lock");
    fs::write(&lock_path, snapshot.flake_lock)
        .with_context(format!("failed to write {}", lock_path.display()))?;
    println!(
        "restored snapshot {} at generation {}",
        snapshot.label, snapshot.generation
    );
    if confirm("Run nr switch now?", false) {
        return crate::lifecycle::run_lifecycle("switch", cli, &[]);
    }
    Ok(0)
}

fn read_snapshot(label: &str) -> Result<SnapshotData> {
    let path = snapshot_path(label);
    let text =
        fs::read_to_string(&path).with_context(format!("failed to read {}", path.display()))?;
    let file = toml::from_str::<SnapshotFile>(&text).map_err(|error| {
        NrError::message(format!("failed to parse {}: {error}", path.display()))
    })?;
    Ok(file.snapshot)
}

fn read_snapshots() -> Result<Vec<SnapshotData>> {
    let directory = snapshots_dir();
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut snapshots = Vec::new();
    for entry in
        fs::read_dir(&directory).with_context(format!("failed to read {}", directory.display()))?
    {
        let entry =
            entry.with_context(format!("failed to read entry in {}", directory.display()))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("toml") {
            continue;
        }
        let text =
            fs::read_to_string(&path).with_context(format!("failed to read {}", path.display()))?;
        let file = toml::from_str::<SnapshotFile>(&text).map_err(|error| {
            NrError::message(format!("failed to parse {}: {error}", path.display()))
        })?;
        snapshots.push(file.snapshot);
    }
    snapshots.sort_by_key(|snapshot| snapshot.created_at);
    Ok(snapshots)
}

fn snapshots_dir() -> PathBuf {
    state_dir().join("snapshots")
}

fn snapshot_path(label: &str) -> PathBuf {
    snapshots_dir().join(format!("{}.toml", safe_label(label)))
}

fn safe_label(label: &str) -> String {
    let mut escaped = String::new();
    for byte in label.trim().bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            escaped.push(byte as char);
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    if escaped.is_empty() {
        "_empty".to_string()
    } else {
        escaped
    }
}

fn truncate_revision(value: &str) -> String {
    value.chars().take(8).collect()
}
