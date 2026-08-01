# nr

`nr` is a safe, Git-aware NixOS lifecycle CLI. It builds through
`nixos-rebuild`, parses Nix internal JSON build events itself, explains closure
and activation impact, and exits with the real backend status code.

The default interactive lifecycle UI uses `nr`'s quieter rich rebuild graph. Use
`--ui nom` to stream through `nix-output-monitor` when you want its full build
tree.
Rollback avoids manual profile editing.

## Commands

```text
nr build                Build without activating; leaves ./result
nr switch               Build, diff, dry-activate, then activate
nr test                 Activate until the next reboot
nr boot                 Build and make the generation the next boot default
nr preview              Build, diff, and dry-activate without mutating
nr bisect [BAD]         Binary-search Git history for a generation regression
nr hm switch            Build and activate a Home Manager flake output
nr hm preview           Build and diff Home Manager without activating
nr apply [PLAN]         Activate a saved preview plan without rebuilding
nr rollback             Roll back to the previous generation
nr generations          Show NixOS generations
nr diff                 Diff current system against a path, generation, or flake
nr gc                   Garbage collect old generations older than 7d
nr pin GEN LABEL        Label a generation for later reference
nr unpin LABEL          Remove a generation pin and GC root
nr pins                 List generation pins and stale pin state
nr history              Show bounded switch/test/boot history
nr logs                 List retained logs and reports
nr show-report [REPORT] Print a retained lifecycle report
nr inputs               Inspect or update flake inputs
nr init-config          Create a starter repo .nr.toml
nr init-config --user   Create ~/.config/nr/config.toml
nr completions SHELL    Generate shell completions
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

Lifecycle commands use bounded command output rather than a full-screen TUI.
In `--ui rich`, `nr` reads
`nixos-rebuild build --log-format internal-json --verbose`, tracks
active/completed/failed build activities, groups derivations by broad system
category, loads derivation edges with `nix-store --query --graph`, and repaints
a compact dependency graph for the active build path. Long derivation names are
shortened in the terminal; the full backend stream stays in the log file.

After a successful build, `nr` compares `/run/current-system` to the new system
with `nix store diff-closures`. For `switch`, `test`, and `preview`, it also
runs dry activation and summarizes services that would stop, start, restart, or
reload. Build logs are streamed with async I/O, and Nix `internal-json` parsing
is defensive: unknown fields are ignored and malformed internal JSON falls back
to plain backend log filtering.

When `switch`, `test`, `boot`, or `rollback` is run by a non-root user from an
interactive terminal, no extra flag is needed: `nr` passes `--elevate sudo`,
so sudo prompts normally. Use `--elevate run0` or `--elevate none` only when
you want to choose a different method explicitly. `--ask-elevate-password`
makes `nixos-rebuild` prompt and pipe a password to the elevation method; it is
mainly useful for remote or non-TTY elevation flows.

Output modes:

```text
--ui auto               rich for interactive lifecycle builds; plain otherwise
--ui rich               Styled live rebuild graph
--ui nom                Pipe build logs through nom --json for an nh-like tree
--ui plain              Stable script-friendly text; no per-event build spam
--ui raw                Backend output passthrough
--ui json               Final structured report as JSON
--ui jsonl              Line-delimited lifecycle events
--log-file PATH         Capture the full backend log at PATH
--elevate METHOD        Forward nixos-rebuild elevation method: none, sudo, run0
--ask-elevate-password  Prompt and pipe an elevation password instead of using sudo's prompt
--notify                Send notify-send notification when lifecycle commands finish
--target-host HOST      Forward nixos-rebuild --target-host
--build-host HOST       Forward nixos-rebuild --build-host
--use-remote-sudo       Forward nixos-rebuild --use-remote-sudo
```

Without `--log-file`, logs are written under `$XDG_STATE_HOME/nr/logs/` or
`~/.local/state/nr/logs/`. Default logs rotate automatically; `nr` keeps the
latest 20 `nr-*.log` files. Reports, preview plans, and switch history are also
bounded by config defaults: 20 reports, 10 plans, and 50 history entries. A
user-provided `--log-file` is never rotated.

`tempfile` is used for temporary build result directories for commands that
should not leave `./result` behind, such as `preview`, `switch`, `test`, `boot`,
and `diff` builds.

`--ui json` prints a final single-line report. Current shape:

```json
{
  "command": "preview",
  "target": "/etc/nixos#desktop",
  "result": "preview complete; no activation performed",
  "store_path": "/nix/store/...",
  "current_generation": 220,
  "new_generation": null,
  "reboot": "no reboot requirement detected",
  "rollback": "nr rollback",
  "log_path": "/home/user/.local/state/nr/logs/nr-...",
  "build": {
    "completed": 12,
    "failed": 0,
    "running": 0,
    "downloads": 2,
    "source_builds": 1,
    "binary_substitutes": 11,
    "parser_fallback": false
  },
  "diff": {
    "additions": 0,
    "removals": 0,
    "upgrades": 3,
    "downgrades": 0,
    "changes": 0,
    "important": ["linux: 6.18.39 -> 6.18.40"],
    "size_delta": "closure size: 1.0 GiB -> 1.1 GiB, +100.0 MiB",
    "unavailable": null
  },
  "activation": {
    "stopped": [],
    "started": [],
    "restarted": ["sshd.service"],
    "reloaded": [],
    "skipped": [],
    "failed": [],
    "caveats": [],
    "unavailable": null
  }
}
```

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
# target_host = "root@example-host"
# build_host = "builder@example-host"
# use_remote_sudo = true

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

[hooks]
timeout_seconds = 300
pre_build = []
post_build = []
pre_activate = []
post_activate = []
post_switch = [
  ["systemctl", "--user", "restart", "waybar.service"],
]
on_failure = []

[ui]
# accent = "#cba6f7"
graph_depth = 12
refresh_ms = 500
verbose_backend = false

[state]
keep_logs = 20
keep_reports = 20
keep_history = 50
keep_plans = 10
```

CLI options win over environment variables, repo config, user config, and
built-in defaults. Custom checks and hooks are arrays, not shell strings.

## Safety Model

`build`, `switch`, `test`, `boot`, `preview`, and `update` do not stage, commit,
or push Git changes. If a Git flake has untracked files, `nr` stops and prints
the exact `git add` command you can run intentionally, because untracked files
are invisible to Git flakes.

`nr preview` is the recommended no-mutation lifecycle command. `--dry` remains
accepted on lifecycle commands as a preview-style alias.

Build failures stop before activation and preserve the backend exit code.
Dry-activation failures are fatal for `switch` and `test`, but only reported as
unavailable in `preview`. Activation failures preserve the backend exit code.
`pre_*` and `post_build` hook failures are fatal before mutation. `post_activate`
and `post_switch` hook failures happen after successful activation, so they are
reported as warnings and do not misreport the switch as failed. `nr check`
groups failed check stdout and stderr by check name, or emits JSON with
`nr check --json`.

`rollback` without a target uses the official previous-generation path after
printing the current and target generation:

```console
nixos-rebuild switch --rollback
```

`nr rollback LABEL` or `nr rollback GEN` activates the named or numbered
generation via `--store-path`. Labels are created with `nr pin GEN LABEL` and
stored in `$XDG_STATE_HOME/nr/pins.toml` or `~/.local/state/nr/pins.toml`.
By default, pins also create indirect GC roots under
`$XDG_STATE_HOME/nr/pin-roots/`; use `nr pins --check-stale` to detect stale
pin metadata and `nr unpin LABEL` to remove both the label and GC root.

`nr gc` defaults to `nix-collect-garbage --delete-older-than 7d`. Use
`nr gc --delete-old` for the more aggressive `-d` behavior, and
`nr gc --dry-run` to preview.

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
