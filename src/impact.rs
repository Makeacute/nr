use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::backend::{self, BackendOptions};
use crate::errors::{IoContext, Result};
use crate::process::{LogFile, run_capture};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClosureDiff {
    pub raw: String,
    pub additions: Vec<String>,
    pub removals: Vec<String>,
    pub upgrades: Vec<String>,
    pub downgrades: Vec<String>,
    pub changes: Vec<String>,
    pub important: Vec<String>,
    pub size_delta: Option<String>,
    pub unavailable: Option<String>,
}

impl ClosureDiff {
    pub fn changed(&self) -> bool {
        !(self.additions.is_empty()
            && self.removals.is_empty()
            && self.upgrades.is_empty()
            && self.downgrades.is_empty()
            && self.changes.is_empty())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActivationImpact {
    pub raw: String,
    pub stopped: Vec<String>,
    pub started: Vec<String>,
    pub restarted: Vec<String>,
    pub reloaded: Vec<String>,
    pub skipped: Vec<String>,
    pub failed: Vec<String>,
    pub caveats: Vec<String>,
    pub unavailable: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GenerationInfo {
    pub generation: Option<u64>,
    pub nixos_version: Option<String>,
    pub kernel_version: Option<String>,
}

pub fn current_generation_info() -> GenerationInfo {
    GenerationInfo {
        generation: current_generation(),
        nixos_version: read_trimmed("/run/current-system/nixos-version"),
        kernel_version: kernel_version(),
    }
}

pub fn current_generation_info_for_options(options: &BackendOptions) -> GenerationInfo {
    let Some(host) = options.target_host.as_deref() else {
        return current_generation_info();
    };
    GenerationInfo {
        generation: remote_current_generation(host),
        nixos_version: remote_read_trimmed(host, "/run/current-system/nixos-version"),
        kernel_version: remote_kernel_version(host),
    }
}

pub fn current_system_path_for_options(options: &BackendOptions) -> Option<PathBuf> {
    if let Some(host) = options.target_host.as_deref() {
        return remote_readlink(host, "/run/current-system").map(PathBuf::from);
    }
    Some(PathBuf::from("/run/current-system"))
}

pub fn current_generation() -> Option<u64> {
    let link = fs::read_link("/nix/var/nix/profiles/system").ok()?;
    parse_generation_from_path(&link)
}

pub fn parse_generation_from_path(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_string_lossy();
    let rest = name.strip_prefix("system-")?;
    let value = rest.strip_suffix("-link")?;
    value.parse().ok()
}

pub fn resolve_result_link(cwd: &Path) -> Result<PathBuf> {
    let link = cwd.join("result");
    let path = fs::read_link(&link)
        .or_else(|_| link.canonicalize())
        .with_context(format!("failed to resolve {}", link.display()))?;
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(cwd.join(path))
    }
}

pub fn diff_current_to_new(
    new_path: &Path,
    options: &BackendOptions,
    log: &mut LogFile,
) -> Result<ClosureDiff> {
    let current = if let Some(host) = options.target_host.as_deref() {
        let Some(path) = current_system_path_for_options(options) else {
            return Ok(ClosureDiff {
                unavailable: Some(format!(
                    "failed to inspect /run/current-system on remote target {host}"
                )),
                ..ClosureDiff::default()
            });
        };
        path
    } else {
        PathBuf::from("/run/current-system")
    };
    if options.target_host.is_none() && !current.exists() {
        return Ok(ClosureDiff {
            unavailable: Some("/run/current-system does not exist".to_string()),
            ..ClosureDiff::default()
        });
    }

    let command = backend::nix_store_diff_closures_command(&current, new_path, options);
    log.write_command(&command)?;
    let output = run_capture(&command, false)?;
    log.write_output(&output)?;
    if output.code != 0 {
        return Ok(ClosureDiff {
            raw: output.stderr.clone(),
            unavailable: Some(format!(
                "nix store diff-closures exited with {}",
                output.code
            )),
            ..ClosureDiff::default()
        });
    }
    Ok(parse_closure_diff(&output.stdout))
}

fn remote_current_generation(host: &str) -> Option<u64> {
    remote_readlink(host, "/nix/var/nix/profiles/system")
        .as_deref()
        .and_then(|path| parse_generation_from_path(Path::new(path)))
}

fn remote_kernel_version(host: &str) -> Option<String> {
    remote_command_trimmed(host, "uname -r")
}

fn remote_readlink(host: &str, path: &str) -> Option<String> {
    let output = run_capture(
        &backend::ssh_command(host, &format!("readlink -f {}", shell_quote(path))),
        false,
    )
    .ok()?;
    if output.code == 0 {
        Some(output.stdout.trim().to_string()).filter(|value| !value.is_empty())
    } else {
        None
    }
}

fn remote_read_trimmed(host: &str, path: &str) -> Option<String> {
    remote_command_trimmed(host, &format!("cat {}", shell_quote(path)))
}

fn remote_command_trimmed(host: &str, command: &str) -> Option<String> {
    let output = run_capture(&backend::ssh_command(host, command), false).ok()?;
    if output.code == 0 {
        Some(output.stdout.trim().to_string()).filter(|value| !value.is_empty())
    } else {
        None
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub fn parse_closure_diff(output: &str) -> ClosureDiff {
    let mut diff = ClosureDiff {
        raw: output.to_string(),
        ..ClosureDiff::default()
    };

    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let lower = line.to_lowercase();
        if lower.starts_with("closure size:") {
            diff.size_delta = Some(line.to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix('+') {
            diff.additions.push(rest.trim().to_string());
            remember_important(&mut diff, rest.trim());
            continue;
        }
        if let Some(rest) = line.strip_prefix('-') {
            diff.removals.push(rest.trim().to_string());
            remember_important(&mut diff, rest.trim());
            continue;
        }
        if line.contains("->") || line.contains('→') {
            let is_downgrade = lower.contains("downgrade") || lower.contains("↓");
            let is_upgrade = lower.contains("upgrade") || lower.contains("↑");
            if is_downgrade {
                diff.downgrades.push(line.to_string());
            } else if is_upgrade || !lower.contains("downgrade") {
                diff.upgrades.push(line.to_string());
            } else {
                diff.changes.push(line.to_string());
            }
            remember_important(&mut diff, line);
            continue;
        }
        diff.changes.push(line.to_string());
        remember_important(&mut diff, line);
    }

    diff
}

pub fn parse_activation_impact(output: &str) -> ActivationImpact {
    let mut impact = ActivationImpact {
        raw: output.to_string(),
        ..ActivationImpact::default()
    };
    let mut section: Option<&str> = None;

    for original in output.lines() {
        let line = original.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_lowercase();
        if lower.contains("would restart") || lower.contains("restart the following") {
            section = Some("restarted");
            extend_units(&mut impact.restarted, units_after_colon(line));
        } else if lower.contains("would reload") || lower.contains("reload the following") {
            section = Some("reloaded");
            extend_units(&mut impact.reloaded, units_after_colon(line));
        } else if lower.contains("would stop") || lower.contains("stop the following") {
            section = Some("stopped");
            extend_units(&mut impact.stopped, units_after_colon(line));
        } else if lower.contains("would start") || lower.contains("start the following") {
            section = Some("started");
            extend_units(&mut impact.started, units_after_colon(line));
        } else if lower.contains("skip") {
            section = Some("skipped");
            extend_units(&mut impact.skipped, units_after_colon(line));
            if units_after_colon(line).is_empty() {
                impact.skipped.push(line.to_string());
            }
        } else if lower.contains("failed") || lower.contains("error") {
            section = Some("failed");
            impact.failed.push(line.to_string());
        } else if lower.contains("user") || lower.contains("warning") || lower.contains("caveat") {
            section = None;
            impact.caveats.push(line.to_string());
        } else if original.chars().next().is_some_and(char::is_whitespace) {
            match section {
                Some("stopped") => extend_units(&mut impact.stopped, split_units(line)),
                Some("started") => extend_units(&mut impact.started, split_units(line)),
                Some("restarted") => extend_units(&mut impact.restarted, split_units(line)),
                Some("reloaded") => extend_units(&mut impact.reloaded, split_units(line)),
                Some("skipped") => extend_units(&mut impact.skipped, split_units(line)),
                Some("failed") => impact.failed.push(line.to_string()),
                _ => {}
            }
        }
    }

    dedup(&mut impact.stopped);
    dedup(&mut impact.started);
    dedup(&mut impact.restarted);
    dedup(&mut impact.reloaded);
    dedup(&mut impact.skipped);
    dedup(&mut impact.failed);
    dedup(&mut impact.caveats);
    impact
}

pub fn reboot_recommendation(action: &str, diff: &ClosureDiff) -> String {
    if action == "boot" {
        return "reboot required to use the new boot generation".to_string();
    }
    if diff.important.iter().any(|item| {
        contains_any(
            &item.to_lowercase(),
            &["kernel", "linux", "systemd", "mesa", "nvidia"],
        )
    }) {
        "reboot recommended because core system components changed".to_string()
    } else {
        "no reboot requirement detected".to_string()
    }
}

fn read_trimmed(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn kernel_version() -> Option<String> {
    let output = run_capture(&backend::uname_kernel_release_command(), false).ok()?;
    if output.code == 0 {
        Some(output.stdout.trim().to_string()).filter(|value| !value.is_empty())
    } else {
        None
    }
}

fn remember_important(diff: &mut ClosureDiff, item: &str) {
    let lower = item.to_lowercase();
    if contains_any(
        &lower,
        &[
            "kernel",
            "linux",
            "systemd",
            "bootloader",
            "grub",
            "systemd-boot",
            "gdm",
            "sddm",
            "display-manager",
            "mesa",
            "nvidia",
            "networkmanager",
            "openssh",
            "openssl",
            "sudo",
            "pam",
            "polkit",
            "zfs",
            "btrfs",
            "bash",
            "fish",
            "zsh",
        ],
    ) && !diff.important.iter().any(|existing| existing == item)
    {
        diff.important.push(item.to_string());
    }
}

fn units_after_colon(line: &str) -> Vec<&str> {
    line.split_once(':')
        .map(|(_, rest)| split_units(rest))
        .unwrap_or_default()
}

fn split_units(line: &str) -> Vec<&str> {
    line.split([',', ' ', '\t'])
        .map(str::trim)
        .filter(|value| {
            value.ends_with(".service") || value.ends_with(".target") || value.ends_with(".socket")
        })
        .collect()
}

fn extend_units(target: &mut Vec<String>, units: Vec<&str>) {
    target.extend(units.into_iter().map(ToOwned::to_owned));
}

fn dedup(values: &mut Vec<String>) {
    let mut seen = HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}
