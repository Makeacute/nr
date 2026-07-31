# Contributing

`nr` is alpha software. Keep changes small, tested, and explicit about safety
tradeoffs.

## Local checks

```console
nix develop
PYTHONPATH=src python -m unittest discover -s tests -p 'test_*.py' -v
ruff check src tests
nix flake check
nix run . -- cheat
nix run . -- doctor --flake .
```

## Design rules

- Lifecycle commands may call `nh` and `nix`, but they must not stage, commit,
  push, or otherwise mutate Git.
- `publish` is the only command that may create commits or push.
- Custom checks must be argument arrays from TOML, not shell strings.
- Use full command names. Do not add short aliases such as `nrb`.
- Tests should use fake commands or temporary repositories. Do not run real
  activation commands in automated tests.
