use crate::errors::Result;

pub fn run_cheat() -> Result<i32> {
    println!(
        "{}",
        r#"
nr - NixOS lifecycle helper

CORE
  nr build                 Build the selected host without activating it.
  nr switch                Build and activate the selected host.
  nr test                  Build and activate until the next reboot.
  nr boot                  Build and make the generation the next boot default.
  nr preview               Build, diff, and dry-activate without mutating.
  nr rollback              Roll back to the previous generation.
  nr rollback LABEL        Roll back to a pinned generation label.
  nr generations           Show NixOS generations.
  nr diff                  Diff current system against a path, generation, or flake.
  nr gc                    Garbage collect generations older than 7d.
  nr gc --dry-run          Preview garbage collection.
  nr pin GEN LABEL         Label a generation for later rollback.

UPDATES AND CHECKS
  nr update                Update flake.lock only.
  nr update nixpkgs        Update one flake input.
  nr update --switch       Update flake.lock, then build and activate.
  nr check                 Run configured checks. Default: nix flake check.
  nr check --all           Also run nixfmt, statix, cargo fmt, and clippy.
  nr doctor                Show target, config, dependency, and Git diagnostics.

PUBLISHING
  nr publish               Review changes, choose commit mode, commit, then ask to push.
  nr publish --mode commit Commit all changes together.
  nr publish --mode per-file
                           Commit each changed file/logical change separately.
  nr publish --push        Push after committing without asking.

TARGET SELECTION
  --flake PATH[#HOST]      Select a flake and optional host.
  --host HOST              Override the NixOS configuration name.
  NR_FLAKE / NR_HOST       Environment-variable equivalents.
  .nr.toml                 Repo defaults at the selected flake root.
  ~/.config/nr/config.toml User defaults.

COMMON FLAGS
  --ui auto|rich|nom|plain|raw|json
                           Select command output mode. auto uses nom for interactive lifecycle builds.
  --log-file PATH          Capture the full backend log at PATH.
  --dry                    Alias lifecycle commands to preview-style behavior.
  --ask                    Ask before activation.
  --offline                Forward offline mode to Nix.
  --show-trace             Forward Nix traces.
  --elevate none|sudo|run0 Forward nixos-rebuild elevation method.
  --ask-elevate-password   Prompt and pipe an elevation password instead of using sudo's prompt.
  --notify                 Send a desktop notification when lifecycle commands finish.
  --specialisation NAME    Build or activate a specialisation.
  --                       Pass remaining arguments directly to the backend.

WORKFLOWS
  Validate:                nr check --all -> nr build
  Preview:                 nr preview -> nr switch
  Update safely:           nr update -> nr preview -> nr switch -> nr publish
  Manual:                  man nr
"#
        .trim()
    );
    Ok(0)
}
