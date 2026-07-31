use std::path::Path;

use crate::config::FlakeTarget;
use crate::process::CommandSpec;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackendOptions {
    pub verbose: u8,
    pub offline: bool,
    pub show_trace: bool,
    pub specialisation: Option<String>,
    pub backend_args: Vec<String>,
}

pub fn passthrough_args(values: &[String]) -> Vec<String> {
    let mut values = values.to_vec();
    if values.first().is_some_and(|value| value == "--") {
        values.remove(0);
    }
    values
}

pub fn nix_common_args(options: &BackendOptions) -> Vec<String> {
    let mut args = Vec::new();
    if options.offline {
        args.push("--offline".to_string());
    }
    if options.show_trace {
        args.push("--show-trace".to_string());
    }
    args
}

pub fn nixos_rebuild_build_command(target: &FlakeTarget, options: &BackendOptions) -> CommandSpec {
    let mut args = vec![
        "build".to_string(),
        "--flake".to_string(),
        target.reference(),
        "--log-format".to_string(),
        "internal-json".to_string(),
        "--verbose".to_string(),
    ];
    for _ in 0..options.verbose {
        args.push("--verbose".to_string());
    }
    args.extend(nix_common_args(options));
    if let Some(specialisation) = &options.specialisation {
        args.push("--specialisation".to_string());
        args.push(specialisation.clone());
    }
    args.extend(passthrough_args(&options.backend_args));
    CommandSpec::new("nixos-rebuild").args(args)
}

pub fn nixos_rebuild_dry_activate_command(
    store_path: &Path,
    options: &BackendOptions,
) -> CommandSpec {
    let mut args = vec![
        "dry-activate".to_string(),
        "--store-path".to_string(),
        store_path.display().to_string(),
    ];
    args.extend(nix_common_args(options));
    args.extend(passthrough_args(&options.backend_args));
    CommandSpec::new("nixos-rebuild").args(args)
}

pub fn nixos_rebuild_activate_command(
    action: &str,
    store_path: &Path,
    options: &BackendOptions,
) -> CommandSpec {
    let mut args = vec![
        action.to_string(),
        "--store-path".to_string(),
        store_path.display().to_string(),
    ];
    args.extend(nix_common_args(options));
    if let Some(specialisation) = &options.specialisation {
        args.push("--specialisation".to_string());
        args.push(specialisation.clone());
    }
    args.extend(passthrough_args(&options.backend_args));
    CommandSpec::new("nixos-rebuild").args(args)
}

pub fn nix_store_diff_closures_command(
    old: &Path,
    new: &Path,
    options: &BackendOptions,
) -> CommandSpec {
    let mut args = nix_common_args(options);
    args.extend([
        "store".to_string(),
        "diff-closures".to_string(),
        old.display().to_string(),
        new.display().to_string(),
    ]);
    CommandSpec::new("nix").args(args)
}

pub fn nix_store_query_graph_command(path: &str) -> CommandSpec {
    CommandSpec::new("nix-store").args([
        "--query".to_string(),
        "--graph".to_string(),
        path.to_string(),
    ])
}

pub fn nix_flake_update_command(
    target: &FlakeTarget,
    inputs: &[String],
    options: &BackendOptions,
) -> CommandSpec {
    let mut args = Vec::new();
    args.extend(nix_common_args(options));
    args.extend([
        "flake".to_string(),
        "update".to_string(),
        "--flake".to_string(),
        target.path.display().to_string(),
    ]);
    args.extend(inputs.iter().cloned());
    CommandSpec::new("nix").args(args)
}

pub fn rollback_command(options: &BackendOptions) -> CommandSpec {
    let mut args = vec!["switch".to_string(), "--rollback".to_string()];
    args.extend(nix_common_args(options));
    args.extend(passthrough_args(&options.backend_args));
    CommandSpec::new("nixos-rebuild").args(args)
}

pub fn generations_command(profile: Option<&str>, backend_args: &[String]) -> CommandSpec {
    if let Some(profile) = profile {
        let mut args = vec![
            "--profile".to_string(),
            profile.to_string(),
            "--list-generations".to_string(),
        ];
        args.extend(passthrough_args(backend_args));
        CommandSpec::new("nix-env").args(args)
    } else {
        let mut args = vec!["list-generations".to_string()];
        args.extend(passthrough_args(backend_args));
        CommandSpec::new("nixos-rebuild").args(args)
    }
}
