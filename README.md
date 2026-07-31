# nr
`nr` is a safe, Git-aware NixOS lifecycle CLI built on top of `nh`.

It is currently alpha software. The goal is to make the daily NixOS workflow
clearer than raw `nixos-rebuild` and more opinionated than `nh`, without hiding
what will mutate the machine or Git repository.

## Commands

```text
nr build                Build without activating
nr switch               Build and activate
nr test                 Activate until the next reboot
nr boot                 Build and make the generation the next boot default
nr rollback             Roll back with nh
nr generations          Show NixOS generations with nh
nr update               Update flake.lock only
nr update nixpkgs       Update one flake input
nr update --switch      Update, build, and activate
nr publish              Review, commit, and optionally push
nr check                Run configured checks
nr doctor               Show target/config/dependency/Git diagnostics
nr cheat                Show the complete terminal cheat sheet
```

Short aliases such as `nrb` are intentionally not part of the public interface.
Use full commands so scripts, docs, and terminal history stay readable.

## Flake discovery

For commands that need a NixOS flake, `nr` checks these sources in order:

1. `--flake PATH[#HOST]`
2. `NR_FLAKE`
3. A `flake.nix` in the current directory or one of its parents
4. `[target].flake` in `~/.config/nr/config.toml`
5. `/etc/nixos`

The configuration name comes from `--host`, the `#HOST` fragment,
`NR_HOST`, `[target].host` in `.nr.toml`, `[target].host` in the user config,
or the machine hostname.

## Install

Run without installing:

```console
nix run github:Makeacute/nr -- cheat
```

Install into your profile:

```console
nix profile install github:Makeacute/nr
```

Use as a pinned flake input in `/etc/nixos`:

```nix
{
  inputs.nr.url = "github:Makeacute/nr/<commit-or-tag>";

  outputs = { self, nixpkgs, nr, ... }: {
    nixosConfigurations.your-host = nixpkgs.lib.nixosSystem {
      modules = [
        ({ pkgs, ... }: {
          environment.systemPackages = [ nr.packages.${pkgs.system}.default ];
        })
      ];
    };
  };
}
```

For the first public alpha, prefer pinning a commit until `v0.1.0a1` is tagged.

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
ruff = false
commands = [
  ["deadnix", "--fail", "."],
]

[publish]
remote = "origin"
```

CLI options win over environment variables, repo config, user config, and
built-in defaults. Custom checks are arrays, not shell strings.

## Safety model

`build`, `switch`, `test`, `boot`, and `update --switch` do not stage, commit,
or push Git changes. If a Git flake has untracked files, `nr` stops and prints
the exact `git add` command you can run intentionally.

`publish` is the only command that commits or pushes. It shows Git status,
asks whether to use one commit or one commit per file/logical change, shows the
staged diff summary, optionally shows the full diff, requires a non-empty
message, and asks before pushing unless `--push` is used.

`switch` does not call `sudo -v`; elevation is left to `nh`.

## Status

This repo is the standalone replacement for the temporary `/etc/nixos`
embedded rebuild helper. Keep the old embedded helper until the standalone
flake is pinned in `/etc/nixos` and a real `nr build`/`nr switch` has been
tested on the machine.

## Development

```console
nix develop
PYTHONPATH=src python -m unittest discover -s tests -p 'test_*.py' -v
ruff check src tests
nix flake check
nix run . -- cheat
nix run . -- doctor --flake .
```
