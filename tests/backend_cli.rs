use nr::backend::{
    BackendOptions, nix_store_diff_closures_command, nixos_rebuild_build_command, rollback_command,
};
use nr::cli::{Cli, NrCommand};
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
fn cli_reports_missing_values() {
    let error = Cli::parse_from(["nr", "--flake"]).unwrap_err().to_string();

    assert!(error.contains("a value is required"));
}
