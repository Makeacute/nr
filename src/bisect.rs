use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::backend;
use crate::cli::{BisectArgs, BisectProbe, Cli};
use crate::config::NrConfig;
use crate::errors::{NrError, Result};
use crate::generations::{
    PinsFile, SystemGeneration, current_generation, load_pins, load_system_generations,
    resolve_generation_reference,
};
use crate::git::{ensure_git_repository, git_command, status_entries};
use crate::process::{CommandSpec, run_capture, run_inherit};
use crate::state;

pub fn run_bisect(cli: &Cli, config: &NrConfig, args: &BisectArgs) -> Result<i32> {
    ensure_git_repository(&config.target.path)?;
    ensure_clean_for_bisect(&config.target.path, args.allow_dirty)?;

    let generations = load_system_generations().unwrap_or_else(|error| {
        eprintln!("warning: failed to inspect system generations: {error}");
        Vec::new()
    });
    let pins = load_pins().unwrap_or_else(|error| {
        eprintln!("warning: failed to read generation pins: {error}");
        PinsFile::default()
    });

    let broken_generation = generation_number_for_reference(&args.broken, &generations, &pins)?;
    let bad = if let Some(revision) = &args.bad {
        rev_parse_commit(&config.target.path, revision)?
    } else {
        resolve_revision_reference(&config.target.path, &args.broken, &generations, &pins)?
    };
    let good = if let Some(value) = &args.good {
        resolve_revision_reference(&config.target.path, value, &generations, &pins)?
    } else {
        let Some(generation) = broken_generation else {
            return Err(NrError::message(
                "Pass --good when BAD_GENERATION_OR_REV is not a generation.",
            ));
        };
        let previous = previous_generation_before(&generations, generation).ok_or_else(|| {
            NrError::message(format!(
                "No previous generation found before {generation}; pass --good."
            ))
        })?;
        revision_for_generation(&config.target.path, previous, &generations)?
    };
    if good == bad {
        return Err(NrError::message(format!(
            "Good and bad revisions resolve to the same commit: {good}"
        )));
    }

    let probe = bisect_probe_command(cli, config, args)?;
    println!("bisect good: {}", short_revision(&good));
    println!("bisect bad:  {}", short_revision(&bad));
    println!("bisect run:  {}", probe.render());

    run_git_checked(&config.target.path, &["bisect", "start", &bad, &good])?;
    let run_result = run_inherit(&git_bisect_run_command(&config.target.path, &probe), true);
    let reset_result = if args.no_reset {
        Ok(())
    } else {
        reset_bisect(&config.target.path)
    };
    reset_result?;

    let code = run_result?;
    if code != 0 {
        return Err(NrError::CommandFailed {
            command: format!("git bisect run {}", probe.render()),
            code,
        });
    }
    Ok(0)
}

fn ensure_clean_for_bisect(flake_path: &Path, allow_dirty: bool) -> Result<()> {
    if allow_dirty {
        return Ok(());
    }
    let dirty = status_entries(flake_path)?
        .into_iter()
        .map(|entry| format!("  {} {}", entry.status(), entry.label()))
        .collect::<Vec<_>>();
    if dirty.is_empty() {
        return Ok(());
    }
    Err(NrError::message(format!(
        "Git worktree must be clean before bisecting.\n{}\n\nCommit, stash, or rerun with --allow-dirty.",
        dirty.join("\n")
    )))
}

fn resolve_revision_reference(
    flake_path: &Path,
    value: &str,
    generations: &[SystemGeneration],
    pins: &PinsFile,
) -> Result<String> {
    if let Some(generation) = generation_number_for_reference(value, generations, pins)? {
        return revision_for_generation(flake_path, generation, generations);
    }
    rev_parse_commit(flake_path, value)
}

fn generation_number_for_reference(
    value: &str,
    generations: &[SystemGeneration],
    pins: &PinsFile,
) -> Result<Option<u64>> {
    if value == "current" {
        return Ok(current_generation(generations).map(|generation| generation.generation));
    }
    if value.parse::<u64>().is_ok() || pins.pins.contains_key(value) {
        return resolve_generation_reference(value, pins).map(Some);
    }
    Ok(None)
}

fn revision_for_generation(
    flake_path: &Path,
    generation: u64,
    generations: &[SystemGeneration],
) -> Result<String> {
    if let Some(revision) = generations
        .iter()
        .find(|entry| entry.generation == generation)
        .and_then(|entry| clean_generation_revision(&entry.configuration_revision))
    {
        return rev_parse_commit(flake_path, &revision);
    }
    if let Some(revision) = history_revision_for_generation(generation)? {
        return rev_parse_commit(flake_path, &revision);
    }
    Err(NrError::message(format!(
        "No Git revision recorded for generation {generation}. Pass a commit with --good or --bad."
    )))
}

fn clean_generation_revision(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || matches!(value, "unknown" | "dirty" | "null" | "none" | "N/A" | "-") {
        return None;
    }
    Some(value.to_string())
}

fn history_revision_for_generation(generation: u64) -> Result<Option<String>> {
    let path = state::history_path();
    history_revision_for_generation_at(generation, &path)
}

fn history_revision_for_generation_at(generation: u64, path: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).map_err(|source| NrError::Io {
        context: format!("failed to read {}", path.display()),
        source,
    })?;
    let history = serde_json::from_str::<HistoryFile>(&text)
        .map_err(|error| NrError::message(format!("failed to parse history: {error}")))?;
    Ok(history
        .entries
        .iter()
        .rev()
        .find(|entry| entry.new_generation == Some(generation))
        .and_then(|entry| entry.git_revision.clone()))
}

fn previous_generation_before(generations: &[SystemGeneration], generation: u64) -> Option<u64> {
    generations
        .iter()
        .filter(|entry| entry.generation < generation)
        .max_by_key(|entry| entry.generation)
        .map(|entry| entry.generation)
}

fn rev_parse_commit(flake_path: &Path, value: &str) -> Result<String> {
    let spec = format!("{value}^{{commit}}");
    let output = run_capture(
        &git_command(flake_path, &["rev-parse", "--verify", &spec]),
        false,
    )?;
    if output.code != 0 {
        return Err(NrError::message(format!("Unknown Git revision: {value}")));
    }
    Ok(output.stdout.trim().to_string())
}

fn bisect_probe_command(cli: &Cli, config: &NrConfig, args: &BisectArgs) -> Result<CommandSpec> {
    let command = backend::passthrough_args(&args.command);
    if !command.is_empty() {
        let mut values = command.into_iter();
        let program = values.next().unwrap_or_default();
        return Ok(CommandSpec::new(program).args(values));
    }

    let executable = std::env::current_exe().map_err(|source| NrError::Io {
        context: "failed to locate current nr executable".to_string(),
        source,
    })?;
    let mut command = CommandSpec::new(executable.display().to_string());
    command = command
        .arg("--flake")
        .arg(config.target.reference())
        .arg("--ui")
        .arg("plain");
    if cli.offline {
        command = command.arg("--offline");
    }
    if cli.show_trace {
        command = command.arg("--show-trace");
    }
    if let Some(target_host) = &cli.target_host {
        command = command.arg("--target-host").arg(target_host.clone());
    }
    if let Some(build_host) = &cli.build_host {
        command = command.arg("--build-host").arg(build_host.clone());
    }
    if cli.use_remote_sudo {
        command = command.arg("--use-remote-sudo");
    }
    Ok(command.args(match args.probe {
        BisectProbe::Build => vec!["build".to_string()],
        BisectProbe::Preview => vec!["preview".to_string()],
        BisectProbe::Check => vec!["check".to_string()],
    }))
}

fn git_bisect_run_command(flake_path: &Path, probe: &CommandSpec) -> CommandSpec {
    CommandSpec::new("git")
        .arg("-C")
        .arg(flake_path.display().to_string())
        .args([
            "bisect".to_string(),
            "run".to_string(),
            probe.program.clone(),
        ])
        .args(probe.args.clone())
}

fn run_git_checked(flake_path: &Path, arguments: &[&str]) -> Result<()> {
    let command = git_command(flake_path, arguments);
    let code = run_inherit(&command, true)?;
    if code != 0 {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code,
        });
    }
    Ok(())
}

fn reset_bisect(flake_path: &Path) -> Result<()> {
    let command = git_command(flake_path, &["bisect", "reset"]);
    let code = run_inherit(&command, true)?;
    if code != 0 {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code,
        });
    }
    Ok(())
}

fn short_revision(value: &str) -> String {
    value.chars().take(12).collect()
}

#[derive(Clone, Debug, Default, Deserialize)]
struct HistoryFile {
    #[serde(default)]
    entries: Vec<HistoryEntry>,
}

#[derive(Clone, Debug, Deserialize)]
struct HistoryEntry {
    new_generation: Option<u64>,
    git_revision: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn generation_revision_filters_unknown_values() {
        assert_eq!(
            clean_generation_revision("abc123").as_deref(),
            Some("abc123")
        );
        assert_eq!(clean_generation_revision("unknown"), None);
        assert_eq!(clean_generation_revision("dirty"), None);
        assert_eq!(clean_generation_revision(""), None);
    }

    #[test]
    fn previous_generation_before_selects_nearest_lower_generation() {
        let generations = vec![
            generation(1, false),
            generation(2, false),
            generation(5, true),
        ];

        assert_eq!(previous_generation_before(&generations, 5), Some(2));
        assert_eq!(previous_generation_before(&generations, 2), Some(1));
        assert_eq!(previous_generation_before(&generations, 1), None);
    }

    #[test]
    fn history_revision_uses_latest_matching_generation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("history.json");
        fs::write(
            &path,
            r#"{
  "entries": [
    {"new_generation": 4, "git_revision": "old"},
    {"new_generation": 5, "git_revision": "wrong"},
    {"new_generation": 4, "git_revision": "new"}
  ]
}"#,
        )
        .expect("write history");
        let revision = history_revision_for_generation_at(4, &path).expect("history revision");

        assert_eq!(revision.as_deref(), Some("new"));
    }

    fn generation(number: u64, current: bool) -> SystemGeneration {
        SystemGeneration {
            generation: number,
            date: String::new(),
            nixos_version: String::new(),
            kernel_version: String::new(),
            configuration_revision: number.to_string(),
            current,
        }
    }
}
