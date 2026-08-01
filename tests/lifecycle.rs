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
    assert!(!stdout.contains("-> nixos-rebuild"));

    let log = support::command_log(&command_log);
    assert!(log.contains("nixos-rebuild build"));
    assert!(log.contains("nix-store --query --graph"));
    assert!(log.contains("nixos-rebuild dry-activate"));
    assert!(!log.contains("nixos-rebuild switch"));
}

#[test]
fn json_ui_outputs_structured_report() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
    let nr_log = flake.path().join("nr.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = nr_command()
        .env("PATH", path)
        .env("NR_FAKE_LOG", &command_log)
        .env("XDG_CONFIG_HOME", flake.path().join("xdg"))
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "--ui",
            "json",
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

    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["command"], "preview");
    assert_eq!(report["target"], format!("{}#host", flake.path().display()));
    assert!(report["diff"]["upgrades"].as_u64().is_some());
    assert!(report["diff"]["important"].as_array().is_some());
    assert!(
        !report["activation"]["restarted"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn rollback_prints_current_and_target_generation() {
    let (_fake, bin, command_log) = support::fake_bin();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = nr_command()
        .env("PATH", path)
        .env("NR_FAKE_LOG", &command_log)
        .args(["rollback"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(
        "Rolling back from generation 2 (2026-08-01 10:00:00) to generation 1 (2026-07-31 10:00:00)"
    ));

    let log = support::command_log(&command_log);
    assert!(log.contains("nixos-rebuild switch --rollback"));
}

#[test]
fn generations_annotates_pinned_labels() {
    let flake = support::TestDir::new();
    let xdg_state = flake.path().join("state");
    std::fs::create_dir_all(xdg_state.join("nr")).unwrap();
    std::fs::write(
        xdg_state.join("nr/pins.toml"),
        r#"
[pins]
last-good = 1
"#,
    )
    .unwrap();
    let (_fake, bin, command_log) = support::fake_bin();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = nr_command()
        .env("PATH", path)
        .env("NR_FAKE_LOG", &command_log)
        .env("XDG_STATE_HOME", &xdg_state)
        .args(["generations"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Pin"));
    assert!(stdout.contains("last-good"));
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
    assert!(!stdout.contains("-> nixos-rebuild"));

    let log = support::command_log(&command_log);
    assert!(log.contains("nixos-rebuild build"));
    assert!(log.contains("nom --json"));
    assert!(log.contains("nom-input @nix"));
    assert!(!log.contains("nom-input debug: fake backend noise"));
    assert!(!log.contains("nix-store --query --graph"));
    assert!(log.contains("nixos-rebuild dry-activate"));
}

#[test]
fn diff_can_compare_generation_to_store_path_without_building() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
    let nr_log = flake.path().join("nr.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = nr_command()
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
            "--notify",
            "diff",
            "--from",
            "1",
            "--to",
            "/nix/store/fake-system",
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
    assert!(stdout.contains("diff complete"));
    assert!(stdout.contains("changes:"));

    let log = support::command_log(&command_log);
    assert!(log.contains(
        "nix store diff-closures /nix/var/nix/profiles/system-1-link /nix/store/fake-system"
    ));
    assert!(!log.contains("nixos-rebuild build"));
}

#[test]
fn diff_can_build_remote_flake_reference_target() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
    let nr_log = flake.path().join("nr.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = nr_command()
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
            "--notify",
            "diff",
            "--from",
            "1",
            "--to",
            "github:Makeacute/nr#nixos",
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
    assert!(stdout.contains("diff complete: 1 -> github:Makeacute/nr#nixos"));

    let log = support::command_log(&command_log);
    assert!(log.contains("nixos-rebuild build --flake github:Makeacute/nr#nixos"));
    assert!(log.contains(
        "nix store diff-closures /nix/var/nix/profiles/system-1-link /nix/store/fake-system"
    ));
    assert!(log.contains("notify-send nr diff"));
}

#[test]
fn gc_uses_safe_age_default_and_dry_run() {
    let (_fake, bin, command_log) = support::fake_bin();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = nr_command()
        .env("PATH", path)
        .env("NR_FAKE_LOG", &command_log)
        .args(["gc", "--dry-run"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let log = support::command_log(&command_log);
    assert!(log.contains("nix-collect-garbage --delete-older-than 7d --dry-run"));
}

#[test]
fn switch_runs_post_switch_hooks_after_activation() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    std::fs::write(
        flake.path().join(".nr.toml"),
        r#"
[hooks]
post_switch = [["hook-success", "waybar"]]
"#,
    )
    .unwrap();
    let (_fake, bin, command_log) = support::fake_bin();
    let nr_log = flake.path().join("nr.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = nr_command()
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
            "switch",
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
    assert!(stdout.contains("post-switch hooks"));

    let log = support::command_log(&command_log);
    assert!(log.contains("nixos-rebuild switch"));
    assert!(log.contains("hook-success waybar"));
}

#[test]
fn failing_post_switch_hook_preserves_hook_exit_code() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    std::fs::write(
        flake.path().join(".nr.toml"),
        r#"
[hooks]
post_switch = [["hook-fail"]]
"#,
    )
    .unwrap();
    let (_fake, bin, command_log) = support::fake_bin();
    let nr_log = flake.path().join("nr.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = nr_command()
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
            "switch",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(66),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("post-switch hook failed"));
}

#[test]
fn lifecycle_notify_invokes_notify_send() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
    let nr_log = flake.path().join("nr.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = nr_command()
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
            "--notify",
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

    let log = support::command_log(&command_log);
    assert!(log.contains("notify-send nr preview"));
}

#[test]
fn lifecycle_notify_reports_failed_build() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
    let nr_log = flake.path().join("nr.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = nr_command()
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
            "--notify",
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

    let log = support::command_log(&command_log);
    assert!(log.contains("notify-send nr build failed: build failed"));
}

#[test]
fn default_logs_rotate_to_latest_twenty() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
    let xdg_state = flake.path().join("state");
    let logs = xdg_state.join("nr/logs");
    std::fs::create_dir_all(&logs).unwrap();
    for index in 0..25 {
        std::fs::write(logs.join(format!("nr-old-{index}.log")), "old\n").unwrap();
    }
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = nr_command()
        .env("PATH", path)
        .env("NR_FAKE_LOG", &command_log)
        .env("XDG_CONFIG_HOME", flake.path().join("xdg"))
        .env("XDG_STATE_HOME", &xdg_state)
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "--ui",
            "plain",
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

    let count = std::fs::read_dir(&logs).unwrap().count();
    assert_eq!(count, 20);
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
