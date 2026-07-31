# nr

`nr` is a safe, Git-aware NixOS lifecycle CLI. It builds through
`nixos-rebuild`, parses Nix internal JSON build events itself, explains closure
and activation impact, and exits with the real backend status code.

The v2 direction keeps rebuild rendering inside `nr`: no delegated rebuild UI
and no manual profile editing for rollback.

## Commands

```text
nr build                Build without activating; leaves ./result
nr switch               Build, diff, dry-activate, then activate
nr test                 Activate until the next reboot
nr boot                 Build and make the generation the next boot default
nr preview              Build, diff, and dry-activate without mutating
nr rollback             Roll back to the previous generation
nr generations          Show NixOS generations
nr update               Update flake.lock only
nr update nixpkgs       Update one flake input
nr update --switch      Update, build, and activate
nr publish              Review, commit, and optionally push
nr check                Run configured checks
nr doctor               Show target/config/dependency/Git diagnostics
nr cheat                Show the complete terminal cheat sheet
man nr                  Show the installed manual page
```

## Rebuild UI

Lifecycle commands use a bounded command UI rather than a full-screen TUI.
During a build, `nr` reads
`nixos-rebuild build --log-format internal-json --verbose`, tracks
active/completed/failed build activities, groups derivations by broad system
category, and repaints a compact rebuild graph. Long derivation names are
shortened in the terminal; the full backend stream stays in the log file.

After a successful build, `nr` compares `/run/current-system` to the new system
with `nix store diff-closures`. For `switch`, `test`, and `preview`, it also
runs dry activation and summarizes services that would stop, start, restart, or
reload.

Output modes:

```text
--ui auto               Rich output only on interactive terminals
--ui rich               Styled live rebuild graph
--ui plain              Stable script-friendly text; no per-event build spam
--ui raw                Backend output passthrough
--ui json               Final structured report as JSON
--log-file PATH         Capture the full backend log at PATH
```

Without `--log-file`, logs are written under `$XDG_STATE_HOME/nr/logs/` or
`~/.local/state/nr/logs/`.

## Flake Discovery

For commands that need a NixOS flake, `nr` checks these sources in order:

1. `--flake PATH[#HOST]`
2. `NR_FLAKE`
3. A `flake.nix` in the current directory or one of its parents
4. `[target].flake` in `~/.config/nr/config.toml`
5. `/etc/nixos`

The configuration name comes from `--host`, the `#HOST` fragment, `NR_HOST`,
`[target].host` in `.nr.toml`, `[target].host` in the user config, or the
machine hostname.

## Config

User config lives at `$XDG_CONFIG_HOME/nr/config.toml` or
`~/.config/nr/config.toml`. Repo config lives at `.nr.toml` in the selected
flake root.

```toml
[target]
flake = "/etc/nixos"
host = "desktop"

[check]
flake = true
nixfmt = false
statix = false
cargo_fmt = false
clippy = false
commands = [
  ["deadnix", "--fail", "."],
]

[publish]
remote = "origin"
```

CLI options win over environment variables, repo config, user config, and
built-in defaults. Custom checks are arrays, not shell strings.

## Safety Model

`build`, `switch`, `test`, `boot`, `preview`, and `update` do not stage, commit,
or push Git changes. If a Git flake has untracked files, `nr` stops and prints
the exact `git add` command you can run intentionally, because untracked files
are invisible to Git flakes.

`nr preview` is the recommended no-mutation lifecycle command. `--dry` remains
accepted on lifecycle commands as a preview-style alias.

`rollback` uses the official previous-generation path:

```console
nixos-rebuild switch --rollback
```

Targeted rollback by manually editing `/nix/var/nix/profiles/system` is not part
of v2.

## Install

Run without installing:

```console
nix run github:Makeacute/nr -- cheat
```

Install into your profile:

```console
nix profile install github:Makeacute/nr
```

## Development

```console
nix develop
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
nix flake check
nix run . -- cheat
nix run . -- doctor --flake .
```
