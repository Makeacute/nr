mod support;

use std::fs;

use nr::git::{ensure_git_flake_visible, status_entries};

#[test]
fn git_preflight_rejects_untracked_flake_files_without_staging() {
    let temp = support::TestDir::new();
    support::initialize_repository(temp.path());
    fs::write(temp.path().join("new.nix"), "{}\n").unwrap();

    let error = ensure_git_flake_visible(temp.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("Untracked files"));
    let staged = support::git(temp.path(), &["diff", "--cached", "--name-only"]);
    assert!(String::from_utf8_lossy(&staged.stdout).trim().is_empty());
}

#[test]
fn git_status_entries_parse_rename() {
    let temp = support::TestDir::new();
    support::initialize_repository(temp.path());
    let output = support::git(temp.path(), &["mv", "flake.nix", "new-name.nix"]);
    assert!(output.status.success());

    let entries = status_entries(temp.path()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].paths,
        vec!["new-name.nix".to_string(), "flake.nix".to_string()]
    );
    assert_eq!(entries[0].label(), "flake.nix -> new-name.nix");
}
