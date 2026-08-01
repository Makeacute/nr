use nr::backend::{
    BackendOptions, generations_json_command, nix_collect_garbage_command,
    nix_store_diff_closures_command, nixos_rebuild_activate_command, nixos_rebuild_build_command,
    nixos_rebuild_dry_activate_command, nom_json_command, notify_send_command, rollback_command,
    rollback_to_store_path_command,
};
use nr::cli::{Cli, NrCommand, PublishMode};
use nr::config::FlakeTarget;
use std::path::PathBuf;

#[test]
fn backend_build_command_uses_nixos_rebuild_json_logs() {
    let target = FlakeTarget {
        path: PathBuf::from("/flake"),
        host: "host".to_string(),
    };
    let command = nixos_rebuild_build_command(
        &target,
        &BackendOptions {
            offline: true,
            show_trace: true,
            specialisation: Some("perf".to_string()),
            backend_args: vec!["--fast".to_string()],
            ..BackendOptions::default()
        },
    );
    assert_eq!(
        command.to_vec(),
        [
            "nixos-rebuild",
            "build",
            "--flake",
            "/flake#host",
            "--log-format",
            "internal-json",
            "--verbose",
            "--offline",
            "--show-trace",
            "--specialisation",
            "perf",
            "--fast",
        ]
        .map(String::from)
    );
}

#[test]
fn rollback_is_official_previous_generation_only() {
    let command = rollback_command(&BackendOptions::default());
    assert_eq!(
        command.to_vec(),
        ["nixos-rebuild", "switch", "--rollback"].map(String::from)
    );
}

#[test]
fn rollback_to_generation_uses_store_path_activation() {
    let command = rollback_to_store_path_command(
        &PathBuf::from("/nix/var/nix/profiles/system-41-link"),
        &BackendOptions::default(),
    );
    assert_eq!(
        command.to_vec(),
        [
            "nixos-rebuild",
            "switch",
            "--store-path",
            "/nix/var/nix/profiles/system-41-link",
        ]
        .map(String::from)
    );
}

#[test]
fn gc_generations_and_notify_commands_are_constructed() {
    assert_eq!(
        generations_json_command().to_vec(),
        ["nixos-rebuild", "list-generations", "--json"].map(String::from)
    );
    assert_eq!(
        nix_collect_garbage_command("7d", false, true).to_vec(),
        [
            "nix-collect-garbage",
            "--delete-older-than",
            "7d",
            "--dry-run"
        ]
        .map(String::from)
    );
    assert_eq!(
        nix_collect_garbage_command("7d", true, false).to_vec(),
        ["nix-collect-garbage", "-d"].map(String::from)
    );
    assert_eq!(
        notify_send_command("nr switch", "done").to_vec(),
        ["notify-send", "nr switch", "done"].map(String::from)
    );
}

#[test]
fn activation_commands_forward_elevation_options() {
    let options = BackendOptions {
        elevate: Some("sudo".to_string()),
        ask_elevate_password: true,
        ..BackendOptions::default()
    };

    let dry_activate = nixos_rebuild_dry_activate_command(&PathBuf::from("/new-system"), &options);
    assert_eq!(
        dry_activate.to_vec(),
        [
            "nixos-rebuild",
            "dry-activate",
            "--store-path",
            "/new-system",
            "--elevate",
            "sudo",
            "--ask-elevate-password",
        ]
        .map(String::from)
    );

    let switch = nixos_rebuild_activate_command("switch", &PathBuf::from("/new-system"), &options);
    assert_eq!(
        switch.to_vec(),
        [
            "nixos-rebuild",
            "switch",
            "--store-path",
            "/new-system",
            "--elevate",
            "sudo",
            "--ask-elevate-password",
        ]
        .map(String::from)
    );
}

#[test]
fn nom_command_consumes_internal_json_logs() {
    assert_eq!(
        nom_json_command().to_vec(),
        ["nom", "--json"].map(String::from)
    );
}

#[test]
fn nix_global_flags_precede_store_subcommands() {
    let command = nix_store_diff_closures_command(
        &PathBuf::from("/old"),
        &PathBuf::from("/new"),
        &BackendOptions {
            offline: true,
            show_trace: true,
            ..BackendOptions::default()
        },
    );
    assert_eq!(
        command.to_vec(),
        [
            "nix",
            "--offline",
            "--show-trace",
            "store",
            "diff-closures",
            "/old",
            "/new"
        ]
        .map(String::from)
    );
}

#[test]
fn cli_exposes_preview_and_global_target_options() {
    let cli = Cli::parse_from(["nr", "--flake", "/one#host", "preview", "--", "--fast"]).unwrap();
    assert_eq!(cli.flake.as_deref(), Some("/one#host"));
    let Some(NrCommand::Preview(args)) = cli.command else {
        panic!("expected preview");
    };
    assert_eq!(args.backend_args, ["--fast"].map(String::from));
}

#[test]
fn cli_accepts_equals_forms_globals_after_command_and_verbose_count() {
    let cli = Cli::parse_from([
        "nr",
        "build",
        "--flake=/one#host",
        "--ui=plain",
        "-vvv",
        "--",
        "--fast",
    ])
    .unwrap();

    assert_eq!(cli.flake.as_deref(), Some("/one#host"));
    assert_eq!(cli.verbose, 3);
    let Some(NrCommand::Build(args)) = cli.command else {
        panic!("expected build");
    };
    assert_eq!(args.backend_args, ["--fast"].map(String::from));
}

#[test]
fn cli_accepts_elevation_globals_after_command() {
    let cli = Cli::parse_from([
        "nr",
        "switch",
        "--elevate",
        "run0",
        "--ask-elevate-password",
    ])
    .unwrap();

    assert_eq!(cli.elevate.as_deref(), Some("run0"));
    assert!(cli.ask_elevate_password);
}

#[test]
fn cli_accepts_new_lifecycle_subcommands() {
    let cli = Cli::parse_from([
        "nr",
        "--notify",
        "diff",
        "--from",
        "41",
        "--to",
        "/tmp/system",
        "--",
        "--offline",
    ])
    .unwrap();
    assert!(cli.notify);
    let Some(NrCommand::Diff(args)) = cli.command else {
        panic!("expected diff");
    };
    assert_eq!(args.from.as_deref(), Some("41"));
    assert_eq!(args.to.as_deref(), Some("/tmp/system"));
    assert_eq!(args.backend_args, ["--offline"].map(String::from));

    let cli = Cli::parse_from(["nr", "gc", "--dry-run", "--older-than", "30d"]).unwrap();
    let Some(NrCommand::Gc(args)) = cli.command else {
        panic!("expected gc");
    };
    assert_eq!(args.older_than, "30d");
    assert!(args.dry_run);

    let cli = Cli::parse_from(["nr", "pin", "41", "last-good", "--force"]).unwrap();
    let Some(NrCommand::Pin(args)) = cli.command else {
        panic!("expected pin");
    };
    assert_eq!(args.generation, 41);
    assert_eq!(args.label, "last-good");
    assert!(args.force);

    let cli = Cli::parse_from(["nr", "rollback", "last-good"]).unwrap();
    let Some(NrCommand::Rollback(args)) = cli.command else {
        panic!("expected rollback");
    };
    assert_eq!(args.target.as_deref(), Some("last-good"));
}

#[test]
fn cli_reports_missing_values() {
    let error = Cli::parse_from(["nr", "--flake"]).unwrap_err().to_string();

    assert!(error.contains("a value is required"));
}

#[test]
fn cli_publish_mode_defaults_to_commit_name_with_single_alias() {
    let cli = Cli::parse_from(["nr", "publish", "--mode", "commit"]).unwrap();
    let Some(NrCommand::Publish(args)) = cli.command else {
        panic!("expected publish");
    };
    assert_eq!(args.mode, Some(PublishMode::Commit));

    let cli = Cli::parse_from(["nr", "publish", "--mode", "single"]).unwrap();
    let Some(NrCommand::Publish(args)) = cli.command else {
        panic!("expected publish");
    };
    assert_eq!(args.mode, Some(PublishMode::Commit));
}
