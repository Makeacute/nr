#![allow(dead_code)]

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct TestDir {
    path: PathBuf,
}

impl TestDir {
    pub fn new() -> Self {
        let mut attempts = 0u32;
        loop {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("nr-test-{}-{nanos}-{attempts}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Self { path },
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempts < 32 => {
                    attempts += 1;
                }
                Err(error) => panic!("failed to create temp dir: {error}"),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn make_flake(path: &Path) -> PathBuf {
    fs::create_dir_all(path).unwrap();
    fs::write(path.join("flake.nix"), "{}\n").unwrap();
    path.to_path_buf()
}

pub fn fake_bin() -> (TestDir, PathBuf, PathBuf) {
    let temp = TestDir::new();
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let log = temp.path().join("commands.log");
    let shell = shell_path();

    write_executable(
        &bin.join("nixos-rebuild"),
        &script_with_shell(
            &shell,
            r#"set -u
echo "nixos-rebuild $*" >> "$NR_FAKE_LOG"
case "${1:-}" in
  build)
    echo "these derivations will be built:" >&2
    echo "  /nix/store/root-nixos-system-host.drv" >&2
    echo '@nix {"action":"start","id":1,"type":105,"text":"building '\''/nix/store/aaa-linux.drv'\''","fields":[]}' >&2
    echo '@nix {"action":"stop","id":1,"fields":[]}' >&2
    if [ "${NR_FAKE_BUILD_FAIL:-0}" = 1 ]; then
      echo "error: build failed" >&2
      exit 23
    fi
    ln -sfn /nix/store/fake-system "$PWD/result"
    ;;
  dry-activate)
    echo "would restart the following units: sshd.service display-manager.service"
    ;;
  switch|test|boot)
    if [ "${NR_FAKE_ACTIVATE_FAIL:-0}" = 1 ]; then
      echo "activation failed" >&2
      exit 44
    fi
    echo "activated ${1:-}"
    ;;
  list-generations)
    echo "1 current 2026-07-31"
    ;;
  *)
    echo "unexpected nixos-rebuild command: $*" >&2
    exit 99
    ;;
esac
"#,
        ),
    )
    .unwrap();

    write_executable(
        &bin.join("nix-store"),
        &script_with_shell(
            &shell,
            r#"set -u
echo "nix-store $*" >> "$NR_FAKE_LOG"
case "$*" in
  *"--query --graph /nix/store/root-nixos-system-host.drv"*)
    cat <<'DOT'
digraph G {
"root-nixos-system-host.drv" [label = "nixos-system-host"];
"aaa-linux.drv" [label = "linux"];
"aaa-linux.drv" -> "root-nixos-system-host.drv";
}
DOT
    ;;
  *)
    echo "unexpected nix-store command: $*" >&2
    exit 97
    ;;
esac
"#,
        ),
    )
    .unwrap();

    write_executable(
        &bin.join("nix"),
        &script_with_shell(
            &shell,
            r#"set -u
echo "nix $*" >> "$NR_FAKE_LOG"
case "$*" in
  *"store diff-closures"*)
    echo "+ linux-6.10"
    echo "openssl: 3.0 -> 3.1"
    echo "closure size: 1.0 GiB -> 1.1 GiB, +100.0 MiB"
    ;;
  *"flake update"*)
    echo "updated"
    ;;
  *"flake check"*)
    echo "checked"
    ;;
  *)
    echo "unexpected nix command: $*" >&2
    exit 98
    ;;
esac
"#,
        ),
    )
    .unwrap();

    write_executable(
        &bin.join("git"),
        &script_with_shell(
            &shell,
            r#"set -u
case "$*" in
  *"rev-parse --is-inside-work-tree"*)
    exit 128
    ;;
  *)
    /run/current-system/sw/bin/git "$@"
    ;;
esac
"#,
        ),
    )
    .unwrap();

    (temp, bin, log)
}

fn script_with_shell(shell: &Path, body: &str) -> String {
    format!("#!{}\n{body}", shell.display())
}

fn shell_path() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for directory in std::env::split_paths(&path) {
        for name in ["bash", "sh"] {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    for variable in ["CONFIG_SHELL", "SHELL"] {
        if let Some(path) = std::env::var_os(variable).map(PathBuf::from)
            && path.is_file()
        {
            return path;
        }
    }
    PathBuf::from("/bin/sh")
}

pub fn command_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

pub fn git(root: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}

pub fn initialize_repository(root: &Path) {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["init", "--quiet", "--initial-branch", "main"])
        .status()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["config", "user.name", "NR Test"])
        .status()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["config", "user.email", "nr@example.invalid"])
        .status()
        .unwrap();
    fs::write(root.join("flake.nix"), "{}\n").unwrap();
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["add", "flake.nix"])
        .status()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["commit", "--quiet", "-m", "initial"])
        .status()
        .unwrap();
}

fn write_executable(path: &Path, contents: &str) -> io::Result<()> {
    fs::write(path, contents)?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
}
