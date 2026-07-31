use std::path::Path;
use std::process::Command;

use crate::config::NrConfig;
use crate::git::{git_command, git_summary};
use crate::process::run_capture;

const REQUIRED_TOOLS: &[&str] = &["nix", "nix-store", "nixos-rebuild", "git"];
const OPTIONAL_TOOLS: &[&str] = &["gh", "nixfmt", "statix", "cargo"];

pub fn run_doctor(config: &NrConfig) -> crate::errors::Result<i32> {
    println!("nr doctor");
    println!("target: {}", config.target.reference());
    println!(
        "user config: {}",
        config
            .user_config_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not found".to_string())
    );
    println!(
        "repo config: {}",
        config
            .repo_config_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not found".to_string())
    );

    let mut missing_required = Vec::new();
    println!("\ndependencies:");
    for tool in REQUIRED_TOOLS {
        if command_exists(tool) {
            println!("  ok       {}", tool_version(tool));
        } else {
            missing_required.push(*tool);
            println!("  missing  {tool}");
        }
    }
    for tool in OPTIONAL_TOOLS {
        if command_exists(tool) {
            println!("  optional {}", tool_version(tool));
        } else {
            println!("  optional {tool}: not installed");
        }
    }

    println!("\ngit:");
    let summary = git_summary(&config.target.path);
    if summary.repository {
        println!("  repository: yes");
        println!(
            "  branch: {}",
            summary.branch.as_deref().unwrap_or("detached")
        );
        println!(
            "  status: {}",
            if summary.dirty { "dirty" } else { "clean" }
        );
        if let Ok(status) = run_capture(
            &git_command(&config.target.path, &["status", "--short"]),
            false,
        ) {
            for line in status.stdout.lines().take(20) {
                println!("    {line}");
            }
        }
    } else {
        println!("  repository: no");
    }

    Ok(if missing_required.is_empty() { 0 } else { 1 })
}

fn command_exists(name: &str) -> bool {
    let paths = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&paths).any(|path| executable_exists(&path.join(name)))
}

fn executable_exists(path: &Path) -> bool {
    path.is_file()
}

fn tool_version(name: &str) -> String {
    let output = Command::new(name).arg("--version").output();
    if let Ok(output) = output {
        let text = String::from_utf8_lossy(if output.stdout.is_empty() {
            &output.stderr
        } else {
            &output.stdout
        });
        if output.status.success() && !text.trim().is_empty() {
            return text.lines().next().unwrap_or(name).to_string();
        }
    }
    format!("{name} installed")
}
