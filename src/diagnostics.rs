use std::path::Path;
use std::process::Command;

use crate::config::NrConfig;
use crate::git::{git_command, git_summary};
use crate::process::{CommandSpec, run_capture, state_dir};

const REQUIRED_TOOLS: &[&str] = &["nix", "nix-store", "nixos-rebuild", "git"];
const OPTIONAL_TOOLS: &[&str] = &["gh", "nom", "nixfmt", "statix", "cargo"];

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
    println!(
        "state retention: logs={} reports={} history={} plans={}",
        config.state.keep_logs,
        config.state.keep_reports,
        config.state.keep_history,
        config.state.keep_plans
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
    if config.remote.target_host.is_some() || config.remote.build_host.is_some() {
        if command_exists("ssh") {
            println!("  optional {}", tool_version("ssh"));
        } else {
            missing_required.push("ssh");
            println!("  missing  ssh");
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

    println!("\nstate:");
    let state = state_dir();
    match std::fs::create_dir_all(&state)
        .and_then(|_| std::fs::write(state.join(".write-test"), "ok\n"))
        .and_then(|_| std::fs::remove_file(state.join(".write-test")))
    {
        Ok(()) => println!("  writable: {}", state.display()),
        Err(error) => println!("  not writable: {} ({error})", state.display()),
    }
    if let Ok(output) = run_capture(
        &CommandSpec::new("df")
            .arg("-Pk")
            .arg(state.display().to_string()),
        false,
    ) && output.code == 0
    {
        for line in output.stdout.lines().take(2) {
            println!("  {line}");
        }
    }

    println!("\nnix:");
    if let Ok(output) = run_capture(
        &CommandSpec::new("nix")
            .arg("show-config")
            .arg("experimental-features"),
        false,
    ) && output.code == 0
    {
        let text = output.stdout.trim();
        if text.contains("flakes") && text.contains("nix-command") {
            println!("  flakes: enabled");
        } else {
            println!("  flakes: not detected in experimental-features");
        }
    }
    if let Some(host) = &config.remote.target_host {
        println!("  remote target: {host}");
    }
    if let Some(host) = &config.remote.build_host {
        println!("  remote builder: {host}");
    }

    Ok(if missing_required.is_empty() { 0 } else { 1 })
}

fn command_exists(name: &str) -> bool {
    let paths = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&paths).any(|path| executable_exists(&path.join(name)))
}

fn executable_exists(path: &Path) -> bool {
    path.is_file() && has_execute_permission(path)
}

#[cfg(unix)]
fn has_execute_permission(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn has_execute_permission(_path: &Path) -> bool {
    true
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

#[cfg(test)]
mod tests {
    use super::executable_exists;

    #[cfg(unix)]
    #[test]
    fn executable_exists_requires_execute_permission() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("tool");
        fs::write(&path, "#!/bin/sh\n").expect("write tool");

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("remove execute permission");
        assert!(!executable_exists(&path));

        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("add execute permission");
        assert!(executable_exists(&path));
    }
}
