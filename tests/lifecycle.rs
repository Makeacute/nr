mod support;

use std::process::Command;

#[test]
fn preview_builds_diffs_and_dry_activates_without_switching() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
    let nr_log = flake.path().join("nr.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut command = nr_command();
    let output = command
        .env("PATH", path)
        .env("NR_FAKE_LOG", &command_log)
        .env("XDG_CONFIG_HOME", flake.path().join("xdg"))
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "--ui",
            "plain",
            "--log-file",
            nr_log.to_str().unwrap(),
            "preview",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("preview complete; no activation performed"));
    assert!(stdout.contains("activation impact"));

    let log = support::command_log(&command_log);
    assert!(log.contains("nixos-rebuild build"));
    assert!(log.contains("nix-store --query --graph"));
    assert!(log.contains("nixos-rebuild dry-activate"));
    assert!(!log.contains("nixos-rebuild switch"));
}

#[test]
fn nom_ui_pipes_build_logs_to_nom() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
    let nr_log = flake.path().join("nr.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut command = nr_command();
    let output = command
        .env("PATH", path)
        .env("NR_FAKE_LOG", &command_log)
        .env("XDG_CONFIG_HOME", flake.path().join("xdg"))
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "--ui",
            "nom",
            "--log-file",
            nr_log.to_str().unwrap(),
            "preview",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("nom: @nix"));

    let log = support::command_log(&command_log);
    assert!(log.contains("nixos-rebuild build"));
    assert!(log.contains("nom --json"));
    assert!(log.contains("nom-input @nix"));
    assert!(!log.contains("nom-input debug: fake backend noise"));
    assert!(!log.contains("nix-store --query --graph"));
    assert!(log.contains("nixos-rebuild dry-activate"));
}

#[test]
fn preview_reports_dry_activation_failure_without_failing() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
    let nr_log = flake.path().join("nr.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut command = nr_command();
    let output = command
        .env("PATH", path)
        .env("NR_FAKE_LOG", &command_log)
        .env("NR_FAKE_DRY_ACTIVATE_FAIL", "1")
        .env("XDG_CONFIG_HOME", flake.path().join("xdg"))
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "--ui",
            "plain",
            "--log-file",
            nr_log.to_str().unwrap(),
            "preview",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("unavailable: nixos-rebuild dry-activate exited with 55"));
    assert!(stdout.contains("preview complete; no activation performed"));
}

#[test]
fn switch_preserves_activation_exit_code() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
    let nr_log = flake.path().join("nr.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut command = nr_command();
    let output = command
        .env("PATH", path)
        .env("NR_FAKE_LOG", &command_log)
        .env("NR_FAKE_ACTIVATE_FAIL", "1")
        .env("XDG_CONFIG_HOME", flake.path().join("xdg"))
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "--ui",
            "plain",
            "--log-file",
            nr_log.to_str().unwrap(),
            "switch",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(44),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("exit code 44"));
}

#[test]
fn failed_build_preserves_backend_exit_code() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
    let nr_log = flake.path().join("nr.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut command = nr_command();
    let output = command
        .env("PATH", path)
        .env("NR_FAKE_LOG", &command_log)
        .env("NR_FAKE_BUILD_FAIL", "1")
        .env("XDG_CONFIG_HOME", flake.path().join("xdg"))
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "--ui",
            "plain",
            "--log-file",
            nr_log.to_str().unwrap(),
            "build",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(23),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("exit code 23"));
}

fn nr_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nr"))
}
