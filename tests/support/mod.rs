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
    echo "debug: fake backend noise" >&2
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
    if [ "${NR_FAKE_DRY_ACTIVATE_FAIL:-0}" = 1 ]; then
      echo "Failed to start transient service unit: Access denied" >&2
      exit 55
    fi
    echo "would restart the following units: sshd.service display-manager.service"
    echo "warning: user services are not handled by this dry activation"
    ;;
  switch|test|boot)
    if [ "${NR_FAKE_ACTIVATE_FAIL:-0}" = 1 ]; then
      echo "activation failed" >&2
      exit 44
    fi
    echo "activated ${1:-}"
    ;;
  list-generations)
    case "$*" in
      *"--json"*)
        cat <<'JSON'
[
  {"generation":2,"date":"2026-08-01 10:00:00","nixosVersion":"26.11","kernelVersion":"6.18.40","configurationRevision":"rev2","specialisations":[],"current":true},
  {"generation":1,"date":"2026-07-31 10:00:00","nixosVersion":"26.11","kernelVersion":"6.18.39","configurationRevision":"rev1","specialisations":[],"current":false}
]
JSON
        ;;
      *)
        echo "Generation  Build-date           NixOS version  Kernel  Configuration Revision  Specialisation  Current"
        echo "2           2026-08-01 10:00:00  26.11         6.18.40 rev2                    []              True"
        echo "1           2026-07-31 10:00:00  26.11         6.18.39 rev1                    []              False"
        ;;
    esac
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
  *"--add-root"*)
    root=""
    target=""
    previous=""
    for arg in "$@"; do
      if [ "$previous" = "--add-root" ]; then
        root="$arg"
      fi
      target="$arg"
      previous="$arg"
    done
    if [ -n "$root" ]; then
      ln -sfn "$target" "$root"
    fi
    ;;
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
	    flake="."
	    previous=""
	    for arg in "$@"; do
	      if [ "$previous" = "--flake" ]; then
	        flake="$arg"
	      fi
	      previous="$arg"
	    done
	    if [ "${NR_FAKE_UPDATE_WRITE_LOCK:-0}" = 1 ]; then
	      printf 'updated\n' > "$flake/flake.lock"
	    fi
	    echo "updated"
	    ;;
  *"flake metadata"*)
    cat <<'JSON'
{"locks":{"nodes":{"root":{"inputs":{"nixpkgs":"nixpkgs"}},"nixpkgs":{"locked":{"rev":"abc"}}}}}
JSON
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
        &bin.join("nix-collect-garbage"),
        &script_with_shell(
            &shell,
            r#"set -u
echo "nix-collect-garbage $*" >> "$NR_FAKE_LOG"
echo "gc $*"
"#,
        ),
    )
    .unwrap();

    write_executable(
        &bin.join("notify-send"),
        &script_with_shell(
            &shell,
            r#"set -u
echo "notify-send $*" >> "$NR_FAKE_LOG"
"#,
        ),
    )
    .unwrap();

    write_executable(
        &bin.join("hook-success"),
        &script_with_shell(
            &shell,
            r#"set -u
	echo "hook-success $*" >> "$NR_FAKE_LOG"
	echo "hook-env ${NR_HOOK:-} ${NR_TARGET:-} ${NR_STORE_PATH:-}" >> "$NR_FAKE_LOG"
	echo "hook ran"
	"#,
        ),
    )
    .unwrap();

    write_executable(
        &bin.join("hook-fail"),
        &script_with_shell(
            &shell,
            r#"set -u
echo "hook-fail $*" >> "$NR_FAKE_LOG"
echo "hook failed" >&2
exit 66
"#,
        ),
    )
    .unwrap();

    write_executable(
        &bin.join("hook-slow"),
        &script_with_shell(
            &shell,
            r#"set -u
	echo "hook-slow $*" >> "$NR_FAKE_LOG"
	sleep 3
	echo "hook should have timed out"
	"#,
        ),
    )
    .unwrap();

    write_executable(
        &bin.join("nom"),
        &script_with_shell(
            &shell,
            r#"set -u
echo "nom $*" >> "$NR_FAKE_LOG"
case "$*" in
  "--json")
    while IFS= read -r line; do
      echo "nom-input $line" >> "$NR_FAKE_LOG"
      case "$line" in
        @nix*) echo "nom: $line" ;;
      esac
    done
    ;;
  *)
    echo "unexpected nom command: $*" >&2
    exit 96
    ;;
esac
"#,
        ),
    )
    .unwrap();

    write_executable(
        &bin.join("ssh"),
        &script_with_shell(
            &shell,
            r#"set -u
	echo "ssh $*" >> "$NR_FAKE_LOG"
	case "$*" in
	  *"readlink -f '/run/current-system'"*)
	    echo "/nix/store/remote-current-system"
	    ;;
	  *"readlink -f '/nix/var/nix/profiles/system'"*)
	    echo "/nix/var/nix/profiles/system-7-link"
	    ;;
	  *"uname -r"*)
	    echo "6.18.40-remote"
	    ;;
	  *"cat '/run/current-system/nixos-version'"*)
	    echo "26.11-remote"
	    ;;
	  *)
	    echo "unexpected ssh command: $*" >&2
	    exit 95
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
