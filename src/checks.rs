use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::backend::{self, BackendOptions};
use crate::cli::{CheckArgs, Cli};
use crate::config::{CheckSettings, NrConfig};
use crate::errors::{NrError, Result};
use crate::process::{CommandSpec, RunOutput, render_command, run_capture_timeout};

const EXCLUDED_DIRECTORIES: &[&str] = &[
    ".cache",
    ".direnv",
    ".git",
    ".venv",
    "__pycache__",
    "result",
    "target",
];

pub fn run_check(cli: &Cli, config: &NrConfig, args: &CheckArgs) -> Result<i32> {
    let settings = apply_check_overrides(config.check.clone(), args);
    let mut checks = configured_checks(
        &config.target.path,
        &settings,
        &cli.backend_options_with_config(config, &[]),
    );
    if !args.only.is_empty() {
        let filters = args
            .only
            .iter()
            .map(|value| value.to_lowercase())
            .collect::<Vec<_>>();
        checks.retain(|(name, command)| {
            let haystack = format!(
                "{} {}",
                name.to_lowercase(),
                command.render().to_lowercase()
            );
            filters.iter().any(|filter| haystack.contains(filter))
        });
    }
    if checks.is_empty() {
        if args.json {
            println!("{}", serde_json::json!({"checks": [], "failed": 0}));
        } else {
            println!("No checks enabled.");
        }
        return Ok(0);
    }

    let timeout = args
        .timeout
        .map(|seconds| Duration::from_secs(seconds.max(1)));
    let mut handles = Vec::new();
    for (name, command) in checks.into_iter() {
        if !args.json {
            println!("\n[{name}] {}", command.render());
        }
        handles.push(thread::spawn(move || {
            let output = run_capture_timeout(&command, false, timeout);
            (name, command, output)
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        let (name, command, output) = handle
            .join()
            .map_err(|_| NrError::message("check worker thread panicked"))?;
        let output = output?;
        if !args.json && output.code == 0 {
            println!("[{name}] passed");
        }
        results.push(CheckFailure {
            name,
            command,
            output,
        });
    }

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "checks": results.iter().map(check_json).collect::<Vec<_>>(),
                "failed": results.iter().filter(|result| result.output.code != 0).count(),
            })
        );
    }

    let failed = results
        .into_iter()
        .filter(|result| result.output.code != 0)
        .collect::<Vec<_>>();
    if failed.is_empty() {
        if !args.json {
            println!("\nAll checks passed.");
        }
        Ok(0)
    } else {
        if !args.json {
            eprintln!("\nFailed checks:");
            for failure in &failed {
                print_check_failure(failure);
            }
        }
        Err(NrError::CommandFailed {
            command: "nr check".to_string(),
            code: 1,
        })
    }
}

fn check_json(failure: &CheckFailure) -> serde_json::Value {
    serde_json::json!({
        "name": failure.name,
        "command": failure.command.render(),
        "code": failure.output.code,
        "stdout": failure.output.stdout,
        "stderr": failure.output.stderr,
    })
}

struct CheckFailure {
    name: String,
    command: CommandSpec,
    output: RunOutput,
}

fn print_check_failure(failure: &CheckFailure) {
    eprintln!("\n[{}] exited with {}", failure.name, failure.output.code);
    eprintln!("command: {}", failure.command.render());
    print_stream_block("stdout", &failure.output.stdout);
    print_stream_block("stderr", &failure.output.stderr);
}

fn print_stream_block(label: &str, text: &str) {
    if text.trim().is_empty() {
        eprintln!("{label}: <empty>");
    } else {
        eprintln!("{label}:");
        for line in text.lines() {
            eprintln!("  {line}");
        }
    }
}

pub fn apply_check_overrides(mut settings: CheckSettings, args: &CheckArgs) -> CheckSettings {
    if args.all {
        settings.flake = true;
        settings.nixfmt = true;
        settings.statix = true;
        settings.cargo_fmt = true;
        settings.clippy = true;
    }
    if args.no_flake {
        settings.flake = false;
    }
    if args.nixfmt {
        settings.nixfmt = true;
    }
    if args.statix {
        settings.statix = true;
    }
    if args.cargo_fmt {
        settings.cargo_fmt = true;
    }
    if args.clippy {
        settings.clippy = true;
    }
    settings
}

pub fn configured_checks(
    flake_path: &Path,
    settings: &CheckSettings,
    options: &BackendOptions,
) -> Vec<(String, CommandSpec)> {
    let mut checks = Vec::new();
    let nix_files = source_files(flake_path, ".nix");

    if settings.nixfmt {
        if nix_files.is_empty() {
            println!("No .nix files found; skipping nixfmt.");
        } else {
            checks.push((
                "Nix formatting".to_string(),
                CommandSpec::new("nixfmt")
                    .arg("--check")
                    .args(nix_files.iter().map(|path| path.display().to_string())),
            ));
        }
    }
    if settings.statix {
        checks.push((
            "Nix static analysis".to_string(),
            CommandSpec::new("statix")
                .arg("check")
                .arg(flake_path.display().to_string()),
        ));
    }
    if settings.cargo_fmt {
        checks.push((
            "Rust formatting".to_string(),
            CommandSpec::new("cargo")
                .arg("fmt")
                .arg("--")
                .arg("--check")
                .cwd(flake_path.to_path_buf()),
        ));
    }
    if settings.clippy {
        checks.push((
            "Rust static analysis".to_string(),
            CommandSpec::new("cargo")
                .arg("clippy")
                .arg("--all-targets")
                .arg("--")
                .arg("-D")
                .arg("warnings")
                .cwd(flake_path.to_path_buf()),
        ));
    }
    if settings.flake {
        checks.push((
            "Flake checks".to_string(),
            CommandSpec::new("nix")
                .args(backend::nix_common_args(options))
                .arg("flake")
                .arg("check")
                .arg(format!("path:{}", flake_path.display())),
        ));
    }
    for (index, command) in settings.commands.iter().enumerate() {
        if let Some((program, args)) = command.split_first() {
            let spec = CommandSpec::new(program.clone())
                .args(args.iter().cloned())
                .cwd(flake_path.to_path_buf());
            checks.push((
                format!("Custom check {}: {}", index + 1, render_command(command)),
                spec,
            ));
        }
    }

    checks
}

pub fn source_files(root: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_source_files(root, suffix, &mut files);
    files.sort();
    files
}

fn collect_source_files(path: &Path, suffix: &str, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if path.is_dir() {
            if !EXCLUDED_DIRECTORIES.contains(&name) {
                collect_source_files(&path, suffix, files);
            }
        } else if name.ends_with(suffix) {
            files.push(path);
        }
    }
}
