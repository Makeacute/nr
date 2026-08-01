use std::env;
use std::path::{Path, PathBuf};

use crate::backend;
use crate::cli::{Cli, HmCommand, HmGenerationsArgs, HmLifecycleArgs};
use crate::config::{FlakeTarget, NrConfig, split_flake_reference};
use crate::errors::{NrError, Result};
use crate::git::ensure_git_flake_visible;
use crate::impact::resolve_result_link;
use crate::process::{LogFile, run_capture, run_inherit};
use crate::prompts::confirm;

pub fn run_hm(cli: &Cli, config: &NrConfig, command: &HmCommand) -> Result<i32> {
    match command {
        HmCommand::Build(args) => run_hm_lifecycle("build", cli, config, args),
        HmCommand::Switch(args) if cli.dry => run_hm_preview(cli, config, args),
        HmCommand::Switch(args) => run_hm_lifecycle("switch", cli, config, args),
        HmCommand::Preview(args) => run_hm_preview(cli, config, args),
        HmCommand::Generations(args) => run_hm_generations(args),
    }
}

fn run_hm_lifecycle(
    action: &str,
    cli: &Cli,
    config: &NrConfig,
    args: &HmLifecycleArgs,
) -> Result<i32> {
    ensure_git_flake_visible(&config.target.path)?;
    let target = home_target(cli, config, args.home.as_deref())?;
    let options = cli.backend_options(&args.backend_args);
    let command = match action {
        "build" => backend::home_manager_build_command(&target, &options),
        "switch" => backend::home_manager_switch_command(&target, &options),
        _ => {
            return Err(NrError::message(format!(
                "unsupported Home Manager action: {action}"
            )));
        }
    };
    if action == "switch"
        && cli.ask
        && !confirm(
            &format!("Run Home Manager switch for {}?", target.reference()),
            false,
        )
    {
        println!("Home Manager switch skipped.");
        return Ok(0);
    }
    let code = run_inherit(&command, true)?;
    if code != 0 {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code,
        });
    }
    Ok(0)
}

fn run_hm_preview(cli: &Cli, config: &NrConfig, args: &HmLifecycleArgs) -> Result<i32> {
    ensure_git_flake_visible(&config.target.path)?;
    let target = home_target(cli, config, args.home.as_deref())?;
    let options = cli.backend_options(&args.backend_args);
    let directory = tempfile::Builder::new()
        .prefix("nr-hm-preview-")
        .tempdir()
        .map_err(|source| NrError::Io {
            context: "failed to create Home Manager preview build directory".to_string(),
            source,
        })?;
    let build_command =
        backend::home_manager_build_command(&target, &options).cwd(directory.path().to_path_buf());

    println!("Home Manager target: {}", target.reference());
    let code = run_inherit(&build_command, true)?;
    if code != 0 {
        return Err(NrError::CommandFailed {
            command: build_command.render(),
            code,
        });
    }

    let result = resolve_result_link(directory.path())?;
    println!("built {}", result.display());
    if let Some(profile) = resolve_hm_profile(args.profile.as_deref())? {
        print_hm_diff(&profile, &result, &options, cli, config)?;
    } else {
        println!("No Home Manager profile found; skipping closure diff.");
    }
    println!("preview complete; no activation performed");
    Ok(0)
}

fn print_hm_diff(
    profile: &Path,
    result: &Path,
    options: &backend::BackendOptions,
    cli: &Cli,
    config: &NrConfig,
) -> Result<()> {
    let mut log = LogFile::create_with_limit(cli.log_file.clone(), config.state.keep_logs)?;
    let mut diff_options = options.clone();
    diff_options.backend_args.clear();
    let command = backend::nix_store_diff_closures_command(profile, result, &diff_options);
    log.write_command(&command)?;
    println!(
        "closure diff: {} -> {}",
        profile.display(),
        result.display()
    );
    let output = run_capture(&command, false)?;
    log.write_output(&output)?;
    log.flush()?;
    print!("{}", output.stdout);
    eprint!("{}", output.stderr);
    if output.code != 0 {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code: output.code,
        });
    }
    Ok(())
}

fn run_hm_generations(args: &HmGenerationsArgs) -> Result<i32> {
    let command = backend::home_manager_generations_command(&args.backend_args);
    let code = run_inherit(&command, true)?;
    if code != 0 {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code,
        });
    }
    Ok(0)
}

fn home_target(cli: &Cli, config: &NrConfig, explicit: Option<&str>) -> Result<FlakeTarget> {
    let home = explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| cli.host.as_ref().map(|value| value.trim().to_string()))
        .filter(|value| !value.is_empty())
        .or_else(|| flake_fragment(cli.flake.as_deref()))
        .or_else(|| {
            env::var("NR_HOME")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            env::var("USER")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| config.target.host.clone());
    if home.trim().is_empty() {
        return Err(NrError::message(
            "Home Manager configuration name cannot be empty.",
        ));
    }
    Ok(FlakeTarget {
        path: config.target.path.clone(),
        host: home.trim().to_string(),
    })
}

fn flake_fragment(value: Option<&str>) -> Option<String> {
    let value = value?;
    let Ok((_, fragment)) = split_flake_reference(value) else {
        return None;
    };
    fragment
}

fn resolve_hm_profile(explicit: Option<&Path>) -> Result<Option<PathBuf>> {
    if let Some(path) = explicit {
        if path.symlink_metadata().is_ok() || path.exists() {
            return Ok(Some(path.to_path_buf()));
        }
        return Err(NrError::message(format!(
            "Home Manager profile does not exist: {}",
            path.display()
        )));
    }
    Ok(default_hm_profile_candidates()
        .into_iter()
        .find(|path| path.symlink_metadata().is_ok() || path.exists()))
}

fn default_hm_profile_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(value) = env::var_os("XDG_STATE_HOME") {
        candidates.push(PathBuf::from(value).join("nix/profiles/home-manager"));
    }
    if let Some(value) = env::var_os("HOME") {
        let home = PathBuf::from(value);
        candidates.push(home.join(".local/state/nix/profiles/home-manager"));
        candidates.push(home.join(".nix-profile"));
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        CheckSettings, HookSettings, PublishSettings, RemoteSettings, StateSettings, UiSettings,
    };

    #[test]
    fn flake_fragment_extracts_home_name() {
        assert_eq!(
            flake_fragment(Some("/etc/nixos#lucian")).as_deref(),
            Some("lucian")
        );
        assert_eq!(flake_fragment(Some("/etc/nixos")), None);
    }

    #[test]
    fn explicit_home_wins_over_config_target() {
        let config = test_config();
        let cli = Cli::parse_from(["nr", "hm", "switch"]).expect("parse cli");

        let target = home_target(&cli, &config, Some("me")).expect("home target");

        assert_eq!(target.host, "me");
        assert_eq!(target.path, PathBuf::from("/flake"));
    }

    fn test_config() -> NrConfig {
        NrConfig {
            target: FlakeTarget {
                path: PathBuf::from("/flake"),
                host: "nixos".to_string(),
            },
            remote: RemoteSettings::default(),
            check: CheckSettings::default(),
            publish: PublishSettings::default(),
            hooks: HookSettings::default(),
            ui: UiSettings::default(),
            state: StateSettings::default(),
            user_config_path: None,
            repo_config_path: None,
        }
    }
}
