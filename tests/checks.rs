mod support;

use std::process::Command;

#[test]
fn failed_check_prints_grouped_output() {
    let flake = support::TestDir::new();
    support::make_flake(flake.path());
    std::fs::write(
        flake.path().join(".nr.toml"),
        r#"
[check]
flake = false
commands = [["hook-fail"]]
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
        .env("XDG_CONFIG_HOME", flake.path().join("xdg"))
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "check",
        ])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(1),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Failed checks:"));
    assert!(stderr.contains("Custom check 1: hook-fail"));
    assert!(stderr.contains("exited with 66"));
    assert!(stderr.contains("command: hook-fail"));
    assert!(stderr.contains("stderr:"));
    assert!(stderr.contains("hook failed"));
}

fn nr_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nr"))
}
