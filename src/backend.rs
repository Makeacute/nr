use std::path::Path;

use crate::config::FlakeTarget;
use crate::process::CommandSpec;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackendOptions {
    pub verbose: u8,
    pub offline: bool,
    pub show_trace: bool,
    pub specialisation: Option<String>,
    pub elevate: Option<String>,
    pub ask_elevate_password: bool,
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
        "--no-reexec".to_string(),
    ];
    args.extend(nix_common_args(options));
    args.extend(nixos_rebuild_elevation_args(options));
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
        "--no-reexec".to_string(),
    ];
    args.extend(nix_common_args(options));
    args.extend(nixos_rebuild_elevation_args(options));
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

pub fn nom_json_command() -> CommandSpec {
    CommandSpec::new("nom").arg("--json")
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
    let mut args = vec![
        "switch".to_string(),
        "--rollback".to_string(),
        "--no-reexec".to_string(),
    ];
    args.extend(nix_common_args(options));
    args.extend(nixos_rebuild_elevation_args(options));
    args.extend(passthrough_args(&options.backend_args));
    CommandSpec::new("nixos-rebuild").args(args)
}

pub fn rollback_to_store_path_command(store_path: &Path, options: &BackendOptions) -> CommandSpec {
    let mut args = vec![
        "switch".to_string(),
        "--store-path".to_string(),
        store_path.display().to_string(),
        "--no-reexec".to_string(),
    ];
    args.extend(nix_common_args(options));
    args.extend(nixos_rebuild_elevation_args(options));
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

pub fn generations_json_command() -> CommandSpec {
    CommandSpec::new("nixos-rebuild").args(["list-generations".to_string(), "--json".to_string()])
}

pub fn nix_collect_garbage_command(
    older_than: &str,
    delete_old: bool,
    dry_run: bool,
) -> CommandSpec {
    let mut args = Vec::new();
    if delete_old {
        args.push("-d".to_string());
    } else {
        args.push("--delete-older-than".to_string());
        args.push(older_than.to_string());
    }
    if dry_run {
        args.push("--dry-run".to_string());
    }
    CommandSpec::new("nix-collect-garbage").args(args)
}

pub fn notify_send_command(title: &str, body: &str) -> CommandSpec {
    CommandSpec::new("notify-send")
        .arg(title.to_string())
        .arg(body.to_string())
}

pub fn nixos_rebuild_elevation_args(options: &BackendOptions) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(elevate) = &options.elevate {
        args.push("--elevate".to_string());
        args.push(elevate.clone());
    }
    if options.ask_elevate_password {
        args.push("--ask-elevate-password".to_string());
    }
    args
}

impl BackendOptions {
    pub fn has_elevation_request(&self) -> bool {
        self.elevate.is_some()
            || self.ask_elevate_password
            || passthrough_args_have_elevation_request(&self.backend_args)
    }

    pub fn uses_interactive_elevation(&self) -> bool {
        self.ask_elevate_password
            || self.elevate.as_deref().is_some_and(|value| value != "none")
            || passthrough_args_have_interactive_elevation(&self.backend_args)
    }
}

fn passthrough_args_have_elevation_request(values: &[String]) -> bool {
    passthrough_args(values).iter().any(|value| {
        matches!(
            value.as_str(),
            "--elevate"
                | "--ask-elevate-password"
                | "--ask-sudo-password"
                | "--sudo"
                | "--use-remote-sudo"
        ) || value.starts_with("--elevate=")
    })
}

fn passthrough_args_have_interactive_elevation(values: &[String]) -> bool {
    passthrough_args(values).iter().any(|value| {
        matches!(
            value.as_str(),
            "--ask-elevate-password" | "--ask-sudo-password" | "--sudo" | "--use-remote-sudo"
        ) || value
            .strip_prefix("--elevate=")
            .is_some_and(|method| method != "none")
    })
}
