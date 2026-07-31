import argparse


def command_cheat(_args: argparse.Namespace) -> int:
    print(
        """
nr - NixOS lifecycle helper

CORE
  nr build                 Build the selected host without activating it.
  nr switch                Build and activate the selected host.
  nr test                  Build and activate until the next reboot.
  nr boot                  Build and make the generation the next boot default.
  nr rollback              Roll back with nh.
  nr generations           Show NixOS generations with nh.

UPDATES AND CHECKS
  nr update                Update flake.lock only.
  nr update nixpkgs        Update one flake input.
  nr update --switch       Update flake.lock, then build and activate.
  nr check                 Run configured checks. Default: nix flake check.
  nr check --all           Also run nixfmt, statix, and ruff when files exist.
  nr doctor                Show target, config, dependency, and Git diagnostics.

PUBLISHING
  nr publish               Review changes, choose commit mode, commit, then ask to push.
  nr publish --mode single Commit all changes together.
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
  --dry                    Forward dry-run behavior to nh/nix where supported.
  --ask                    Ask before nh activation steps.
  --offline                Forward offline mode to nix-backed commands.
  --show-trace             Forward Nix traces.
  --specialisation NAME    Activate/build a specialisation through nh.
  --                       Pass remaining arguments directly to the backend.

WORKFLOWS
  Validate:                nr check --all -> nr build
  Apply:                   nr switch -> nr publish
  Update safely:           nr update -> nr build -> nr switch -> nr publish
""".strip()
    )
    return 0
