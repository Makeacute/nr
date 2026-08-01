mod support;

use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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
fn preview_saves_plan_and_apply_uses_it_without_rebuilding() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
    let xdg_state = flake.path().join("state");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let preview = nr_command()
        .env("PATH", &path)
        .env("NR_FAKE_LOG", &command_log)
        .env("XDG_CONFIG_HOME", flake.path().join("xdg"))
        .env("XDG_STATE_HOME", &xdg_state)
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "--ui",
            "plain",
            "--target-host",
            "root@remote",
            "--build-host",
            "builder",
            "--use-remote-sudo",
            "preview",
        ])
        .output()
        .unwrap();
    assert!(
        preview.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&preview.stdout),
        String::from_utf8_lossy(&preview.stderr)
    );
    assert_eq!(
        std::fs::read_dir(xdg_state.join("nr/plans"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_dir(xdg_state.join("nr/reports"))
            .unwrap()
            .count(),
        1
    );

    std::fs::write(&command_log, "").unwrap();
    let apply = nr_command()
        .env("PATH", &path)
        .env("NR_FAKE_LOG", &command_log)
        .env("XDG_CONFIG_HOME", flake.path().join("xdg"))
        .env("XDG_STATE_HOME", &xdg_state)
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "--ui",
            "plain",
            "apply",
            "latest",
        ])
        .output()
        .unwrap();
    assert!(
        apply.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let log = support::command_log(&command_log);
    assert!(!log.contains("nixos-rebuild build"));
    assert!(log.contains("nixos-rebuild switch"));
    assert!(log.contains("--target-host root@remote"));
    assert!(log.contains("--build-host builder"));
    assert!(log.contains("--use-remote-sudo"));
    assert!(xdg_state.join("nr/history.json").is_file());
}

#[test]
fn state_retention_limits_plans_and_reports() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    std::fs::write(
        flake.path().join(".nr.toml"),
        r#"
[state]
keep_reports = 1
keep_plans = 1
"#,
    )
    .unwrap();
    let (_fake, bin, command_log) = support::fake_bin();
    let xdg_state = flake.path().join("state");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    for _ in 0..2 {
        let output = nr_command()
            .env("PATH", &path)
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
        assert!(output.status.success());
    }

    assert_eq!(
        std::fs::read_dir(xdg_state.join("nr/plans"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_dir(xdg_state.join("nr/reports"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn pin_creates_gc_root_and_unpin_removes_it() {
    let flake = support::TestDir::new();
    let xdg_state = flake.path().join("state");
    let (_fake, bin, command_log) = support::fake_bin();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let pin = nr_command()
        .env("PATH", &path)
        .env("NR_FAKE_LOG", &command_log)
        .env("XDG_STATE_HOME", &xdg_state)
        .args(["pin", "1", "last-good"])
        .output()
        .unwrap();
    assert!(
        pin.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&pin.stdout),
        String::from_utf8_lossy(&pin.stderr)
    );
    assert!(
        xdg_state
            .join("nr/pin-roots/last-good")
            .symlink_metadata()
            .is_ok()
    );

    let pins = nr_command()
        .env("PATH", &path)
        .env("NR_FAKE_LOG", &command_log)
        .env("XDG_STATE_HOME", &xdg_state)
        .args(["pins"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&pins.stdout).contains("last-good -> 1"));

    let unpin = nr_command()
        .env("PATH", path)
        .env("NR_FAKE_LOG", &command_log)
        .env("XDG_STATE_HOME", &xdg_state)
        .args(["unpin", "last-good"])
        .output()
        .unwrap();
    assert!(unpin.status.success());
    assert!(
        xdg_state
            .join("nr/pin-roots/last-good")
            .symlink_metadata()
            .is_err()
    );
}

#[test]
fn inputs_lists_lock_nodes() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
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
            "inputs",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("nixpkgs"));
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
            "--target-host",
            "root@remote",
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
    assert_eq!(report["diff"]["changes"], 0);
    assert!(report["diff"]["important"].as_array().is_some());
    assert_eq!(
        report["diff"]["size_delta"],
        "closure size: 1.0 GiB -> 1.1 GiB, +100.0 MiB"
    );
    assert!(report["diff"]["unavailable"].is_null());
    assert!(
        !report["activation"]["restarted"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !report["activation"]["caveats"]
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
fn failing_post_switch_hook_is_warning_after_successful_switch() {
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
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("switch complete"));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("warning: post-switch hook failed after successful activation")
    );
    let log = support::command_log(&command_log);
    assert!(log.contains("nixos-rebuild switch"));
    assert!(log.contains("hook-fail"));
}

#[test]
fn hooks_receive_env_and_pre_build_timeout_fails_before_build() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    std::fs::write(
        flake.path().join(".nr.toml"),
        r#"
[hooks]
timeout_seconds = 1
pre_build = [["hook-success", "pre"]]
post_build = [["hook-success", "post"]]
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
        .env("PATH", &path)
        .env("NR_FAKE_LOG", &command_log)
        .env("XDG_CONFIG_HOME", flake.path().join("xdg"))
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "--ui",
            "plain",
            "build",
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
    assert!(log.contains("hook-env pre_build"));
    assert!(log.contains(&format!("{}#host", flake.path().display())));
    assert!(log.contains("hook-env post_build"));
    assert!(log.contains("/nix/store/fake-system"));

    std::fs::write(
        flake.path().join(".nr.toml"),
        r#"
[hooks]
timeout_seconds = 1
pre_build = [["hook-slow"]]
"#,
    )
    .unwrap();
    std::fs::write(&command_log, "").unwrap();
    let output = nr_command()
        .env("PATH", path)
        .env("NR_FAKE_LOG", &command_log)
        .env("XDG_CONFIG_HOME", flake.path().join("xdg"))
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "--ui",
            "plain",
            "build",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(124),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let log = support::command_log(&command_log);
    assert!(log.contains("hook-slow"));
    assert!(!log.contains("nixos-rebuild build"));
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
fn history_is_bounded_and_logs_show_reports_are_discoverable() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    std::fs::write(
        flake.path().join(".nr.toml"),
        r#"
[state]
keep_history = 1
keep_reports = 2
"#,
    )
    .unwrap();
    let (_fake, bin, command_log) = support::fake_bin();
    let xdg_state = flake.path().join("state");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    for _ in 0..2 {
        let output = nr_command()
            .env("PATH", &path)
            .env("NR_FAKE_LOG", &command_log)
            .env("XDG_CONFIG_HOME", flake.path().join("xdg"))
            .env("XDG_STATE_HOME", &xdg_state)
            .args([
                "--flake",
                &format!("{}#host", flake.path().display()),
                "--ui",
                "plain",
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
    }

    let history_text = std::fs::read_to_string(xdg_state.join("nr/history.json")).unwrap();
    let history: serde_json::Value = serde_json::from_str(&history_text).unwrap();
    assert_eq!(history["entries"].as_array().unwrap().len(), 1);

    let history_output = nr_command()
        .env("XDG_STATE_HOME", &xdg_state)
        .args(["history"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&history_output.stdout).contains("switch complete"));

    let logs_output = nr_command()
        .env("XDG_STATE_HOME", &xdg_state)
        .args(["logs", "--limit", "1"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&logs_output.stdout).contains("nr-"));

    let report_output = nr_command()
        .env("XDG_STATE_HOME", &xdg_state)
        .args(["show-report", "latest"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&report_output.stdout).contains("\"success\": true"));
}

#[test]
fn jsonl_ui_outputs_line_delimited_events() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
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
            "jsonl",
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
    let events = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(events.iter().any(|event| event["event"] == "start"));
    assert!(events.iter().any(|event| event["event"] == "finish"));
}

#[test]
fn update_switch_reverts_lockfile_to_pre_update_state_on_failure() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    std::fs::write(flake.path().join("flake.lock"), "original\n").unwrap();
    let (_fake, bin, command_log) = support::fake_bin();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = nr_command()
        .env("PATH", path)
        .env("NR_FAKE_LOG", &command_log)
        .env("NR_FAKE_UPDATE_WRITE_LOCK", "1")
        .env("NR_FAKE_ACTIVATE_FAIL", "1")
        .env("XDG_CONFIG_HOME", flake.path().join("xdg"))
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "--ui",
            "plain",
            "update",
            "--switch",
            "--revert-on-failure",
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
    assert_eq!(
        std::fs::read_to_string(flake.path().join("flake.lock")).unwrap(),
        "original\n"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("reverted flake.lock"));
}

#[test]
fn update_input_prints_focused_lock_change() {
    let flake = support::TestDir::new();
    support::initialize_repository(flake.path());
    std::fs::write(
        flake.path().join("flake.lock"),
        lockfile_json("old-nr", "old-home-manager"),
    )
    .unwrap();
    support::git(flake.path(), &["add", "flake.lock"]);
    support::git(flake.path(), &["commit", "--quiet", "-m", "lock"]);
    let (_fake, bin, command_log) = support::fake_bin();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = nr_command()
        .env("PATH", path)
        .env("NR_FAKE_LOG", &command_log)
        .env(
            "NR_FAKE_UPDATE_LOCK_JSON",
            lockfile_json("new-nr", "new-home-manager"),
        )
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "update",
            "nr",
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
    assert!(stdout.contains("requested input changes"));
    assert!(stdout.contains("nr:"));
    assert!(stdout.contains("old-nr -> new-nr"));
    assert!(!stdout.contains("new-home-manager"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("1 other lock node"));
}

#[test]
fn remote_diff_defaults_from_remote_current_system() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    let (_fake, bin, command_log) = support::fake_bin();
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
            "--target-host",
            "root@remote",
            "--ui",
            "plain",
            "diff",
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
    assert!(stdout.contains("current kernel: 6.18.40-remote"));
    let log = support::command_log(&command_log);
    assert!(log.contains("ssh root@remote uname -r"));
    assert!(log.contains("ssh root@remote readlink -f '/run/current-system'"));
    assert!(log.contains(
        "nix store diff-closures /nix/store/remote-current-system /nix/store/fake-system"
    ));
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_nr"));
    let xdg_config = unique_test_path("config");
    let xdg_state = unique_test_path("state");
    std::fs::create_dir_all(&xdg_config).expect("create XDG config dir");
    std::fs::create_dir_all(&xdg_state).expect("create XDG state dir");
    command
        .env("XDG_CONFIG_HOME", xdg_config)
        .env("XDG_STATE_HOME", xdg_state);
    command
}

fn unique_test_path(kind: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("nr-test-{kind}-{}-{nanos}", std::process::id()))
}

fn lockfile_json(nr_rev: &str, home_manager_rev: &str) -> String {
    format!(
        r#"{{
  "root": "root",
  "nodes": {{
    "root": {{
      "inputs": {{
        "nr": "nr",
        "home-manager": "home-manager"
      }}
    }},
    "nr": {{
      "locked": {{
        "type": "github",
        "owner": "Makeacute",
        "repo": "nr",
        "rev": "{nr_rev}",
        "narHash": "sha256-{nr_rev}",
        "lastModified": 1
      }}
    }},
    "home-manager": {{
      "locked": {{
        "type": "github",
        "owner": "nix-community",
        "repo": "home-manager",
        "rev": "{home_manager_rev}",
        "narHash": "sha256-{home_manager_rev}",
        "lastModified": 1
      }}
    }}
  }}
}}"#
    )
}
