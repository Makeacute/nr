mod support;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn publish_numbers_commit_mode_and_defaults_empty_message_to_commit() {
    let flake = support::TestDir::new();
    support::initialize_repository(flake.path());
    std::fs::write(flake.path().join("flake.nix"), "{ changed = true; }\n").unwrap();

    let mut child = nr_command()
        .args([
            "--flake",
            &format!("{}#host", flake.path().display()),
            "publish",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"2\ny\n\ny\n\nn\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("1. commit: one commit for all changes (default)"));
    assert!(stdout.contains("2. per-file: one commit per file/logical change"));
    assert!(stdout.contains("Commit mode:"));
    assert!(stdout.contains("Commit message: [commit]"));

    let subject = support::git(flake.path(), &["log", "-1", "--pretty=%s"]);
    assert_eq!(
        String::from_utf8_lossy(&subject.stdout).trim(),
        "commit",
        "stderr:\n{}",
        String::from_utf8_lossy(&subject.stderr)
    );
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
