use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::VERSION;
use crate::backend;
use crate::cli::{Cli, ShareArgs, ShareFormat};
use crate::config::NrConfig;
use crate::errors::{IoContext, NrError, Result};
use crate::generations::{current_generation, load_system_generations};
use crate::git::git_summary;
use crate::process::run_capture;
use crate::state;

pub fn run_share(_cli: &Cli, config: &NrConfig, args: &ShareArgs) -> Result<i32> {
    let summary = build_summary(config)?;
    let output = match args.format {
        ShareFormat::Text => summary.render_text(),
        ShareFormat::Markdown => format!("```text\n{}\n```", summary.render_text()),
    };

    if !args.no_clipboard && copy_to_clipboard(&output) {
        println!("Copied to clipboard.");
    } else {
        println!("{output}");
        println!("Paste the above.");
    }
    Ok(0)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SystemSummary {
    nixos_version: String,
    kernel_version: String,
    generation: String,
    generation_date: String,
    flake_host: String,
    git_branch: String,
    git_dirty: String,
    inputs: Vec<(String, String)>,
    build_stats: Option<BuildStats>,
    nr_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BuildStats {
    completed: u64,
    failed: u64,
    store: String,
}

impl SystemSummary {
    fn render_text(&self) -> String {
        let mut lines = vec![
            "nr system summary".to_string(),
            format!("NixOS: {}", self.nixos_version),
            format!("Kernel: {}", self.kernel_version),
            format!(
                "Current generation: {} ({})",
                self.generation, self.generation_date
            ),
            format!("Flake host: {}", self.flake_host),
            format!("Git: {} ({})", self.git_branch, self.git_dirty),
            "Flake inputs:".to_string(),
        ];
        if self.inputs.is_empty() {
            lines.push("  none found".to_string());
        } else {
            for (name, revision) in &self.inputs {
                lines.push(format!("  {name}: {revision}"));
            }
        }
        match &self.build_stats {
            Some(stats) => lines.push(format!(
                "Last build: completed {} failed {} store {}",
                stats.completed, stats.failed, stats.store
            )),
            None => lines.push("Last build: unavailable".to_string()),
        }
        lines.push(format!("nr: {}", self.nr_version));
        lines.join("\n")
    }
}

fn build_summary(config: &NrConfig) -> Result<SystemSummary> {
    let generations = load_system_generations().unwrap_or_default();
    let current = current_generation(&generations);
    let kernel_version = current
        .map(|generation| generation.kernel_version.clone())
        .filter(|value| !value.trim().is_empty())
        .or_else(read_kernel_version)
        .unwrap_or_else(|| "unknown".to_string());
    let git = git_summary(&config.target.path);

    Ok(SystemSummary {
        nixos_version: current
            .map(|generation| generation.nixos_version.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string()),
        kernel_version,
        generation: current
            .map(|generation| generation.generation.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        generation_date: current
            .map(|generation| generation.date.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string()),
        flake_host: config.target.host.clone(),
        git_branch: if git.repository {
            git.branch.unwrap_or_else(|| "detached".to_string())
        } else {
            "not a repository".to_string()
        },
        git_dirty: if git.dirty { "dirty" } else { "clean" }.to_string(),
        inputs: flake_inputs(&config.target.path)?,
        build_stats: last_build_stats()?,
        nr_version: VERSION.to_string(),
    })
}

fn read_kernel_version() -> Option<String> {
    let output = run_capture(&backend::uname_kernel_release_command(), false).ok()?;
    if output.code != 0 {
        return None;
    }
    let kernel = output.stdout.trim();
    if kernel.is_empty() {
        None
    } else {
        Some(kernel.to_string())
    }
}

fn flake_inputs(flake_path: &Path) -> Result<Vec<(String, String)>> {
    let path = flake_path.join("flake.lock");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text =
        fs::read_to_string(&path).with_context(format!("failed to read {}", path.display()))?;
    let lock: Value = serde_json::from_str(&text)
        .map_err(|error| NrError::message(format!("failed to parse flake.lock: {error}")))?;
    let Some(nodes) = lock.get("nodes").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let root = lock.get("root").and_then(Value::as_str).unwrap_or("root");
    let Some(inputs) = nodes
        .get(root)
        .and_then(|node| node.get("inputs"))
        .and_then(Value::as_object)
    else {
        return Ok(Vec::new());
    };

    let mut entries = Vec::new();
    for (name, reference) in inputs {
        let Some(node_name) = lock_reference_node(reference) else {
            continue;
        };
        let revision = nodes
            .get(node_name)
            .and_then(|node| node.get("locked"))
            .and_then(|locked| locked.get("rev"))
            .and_then(Value::as_str)
            .map(truncate_revision)
            .unwrap_or_else(|| "unlocked".to_string());
        entries.push((name.clone(), revision));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries)
}

fn lock_reference_node(value: &Value) -> Option<&str> {
    if let Some(node) = value.as_str() {
        return Some(node);
    }
    value
        .as_array()
        .and_then(|items| items.last())
        .and_then(Value::as_str)
}

fn truncate_revision(value: &str) -> String {
    value.chars().take(8).collect()
}

fn last_build_stats() -> Result<Option<BuildStats>> {
    let Ok(path) = state::latest_json(&state::reports_dir(), "report") else {
        return Ok(None);
    };
    let text =
        fs::read_to_string(&path).with_context(format!("failed to read {}", path.display()))?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        NrError::message(format!(
            "failed to parse report {}: {error}",
            path.display()
        ))
    })?;
    let Some(report) = value.get("report") else {
        return Ok(None);
    };
    let completed = report
        .get("build")
        .and_then(|build| build.get("completed"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let failed = report
        .get("build")
        .and_then(|build| build.get("failed"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let store = report
        .get("store_path")
        .and_then(Value::as_str)
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str())
        .unwrap_or("unknown")
        .to_string();
    Ok(Some(BuildStats {
        completed,
        failed,
        store,
    }))
}

fn copy_to_clipboard(text: &str) -> bool {
    for command in clipboard_commands() {
        if run_clipboard_command(command.program, command.args, text) {
            return true;
        }
    }
    false
}

struct ClipboardCommand {
    program: &'static str,
    args: &'static [&'static str],
}

fn clipboard_commands() -> [ClipboardCommand; 3] {
    [
        ClipboardCommand {
            program: "wl-copy",
            args: &[],
        },
        ClipboardCommand {
            program: "xclip",
            args: &["-selection", "clipboard"],
        },
        ClipboardCommand {
            program: "xsel",
            args: &["--clipboard", "--input"],
        },
    ]
}

fn run_clipboard_command(program: &str, args: &[&str], text: &str) -> bool {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let Some(mut stdin) = child.stdin.take() else {
        return false;
    };
    if stdin.write_all(text.as_bytes()).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    }
    drop(stdin);
    child.wait().map(|status| status.success()).unwrap_or(false)
}
