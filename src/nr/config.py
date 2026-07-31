import os
import socket
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from nr.errors import NrError


@dataclass(frozen=True)
class FlakeTarget:
    path: Path
    host: str

    @property
    def reference(self) -> str:
        return f"{self.path}#{self.host}"


Command = tuple[str, ...]


@dataclass(frozen=True)
class CheckSettings:
    flake: bool = True
    nixfmt: bool = False
    statix: bool = False
    ruff: bool = False
    commands: tuple[Command, ...] = field(default_factory=tuple)


@dataclass(frozen=True)
class PublishSettings:
    remote: str = "origin"


@dataclass(frozen=True)
class NrConfig:
    target: FlakeTarget
    check: CheckSettings = field(default_factory=CheckSettings)
    publish: PublishSettings = field(default_factory=PublishSettings)
    user_config_path: Path | None = None
    repo_config_path: Path | None = None


_TOP_LEVEL_KEYS = {"target", "check", "publish"}
_TARGET_KEYS = {"flake", "host"}
_CHECK_KEYS = {"flake", "nixfmt", "statix", "ruff", "commands"}
_PUBLISH_KEYS = {"remote"}


def find_flake(start: Path) -> Path | None:
    current = start.expanduser().resolve()
    if current.is_file():
        current = current.parent

    for candidate in (current, *current.parents):
        if (candidate / "flake.nix").is_file():
            return candidate

    return None


def split_flake_reference(value: str) -> tuple[str, str | None]:
    path, separator, host = value.rpartition("#")
    if not separator:
        return value, None
    if not path:
        raise NrError("Flake path cannot be empty.")
    if not host:
        raise NrError("Flake host cannot be empty after '#'.")
    return path, host


def validate_flake_path(flake_path: Path) -> None:
    if not flake_path.is_dir():
        raise NrError(f"Flake directory does not exist: {flake_path}")
    if not (flake_path / "flake.nix").is_file():
        raise NrError(f"No flake.nix found in: {flake_path}")


def user_config_path(environ: Mapping[str, str] | None = None) -> Path:
    environment = os.environ if environ is None else environ
    xdg_config_home = environment.get("XDG_CONFIG_HOME")
    if xdg_config_home:
        return Path(xdg_config_home).expanduser().resolve() / "nr" / "config.toml"
    return Path.home() / ".config" / "nr" / "config.toml"


def _read_toml(path: Path) -> dict[str, Any]:
    if not path.is_file():
        return {}
    try:
        with path.open("rb") as handle:
            data = tomllib.load(handle)
    except tomllib.TOMLDecodeError as error:
        raise NrError(f"Invalid TOML in {path}: {error}") from error
    if not isinstance(data, dict):
        raise NrError(f"Config must be a TOML table: {path}")
    _validate_config_keys(path, data)
    return data


def _validate_config_keys(path: Path, data: Mapping[str, Any]) -> None:
    unknown = set(data) - _TOP_LEVEL_KEYS
    if unknown:
        raise NrError(f"Unknown config section in {path}: {', '.join(sorted(unknown))}")

    sections = (
        ("target", _TARGET_KEYS),
        ("check", _CHECK_KEYS),
        ("publish", _PUBLISH_KEYS),
    )
    for section, allowed in sections:
        value = data.get(section)
        if value is None:
            continue
        if not isinstance(value, dict):
            raise NrError(f"[{section}] in {path} must be a TOML table.")
        unknown_section_keys = set(value) - allowed
        if unknown_section_keys:
            raise NrError(
                f"Unknown [{section}] key in {path}: "
                f"{', '.join(sorted(unknown_section_keys))}"
            )


def _string_value(
    path: Path,
    data: Mapping[str, Any],
    section: str,
    key: str,
) -> str | None:
    section_data = data.get(section, {})
    if not isinstance(section_data, dict) or key not in section_data:
        return None
    value = section_data[key]
    if not isinstance(value, str):
        raise NrError(f"[{section}].{key} in {path} must be a string.")
    value = value.strip()
    if not value:
        raise NrError(f"[{section}].{key} in {path} cannot be empty.")
    return value


def _bool_value(
    path: Path,
    data: Mapping[str, Any],
    section: str,
    key: str,
) -> bool | None:
    section_data = data.get(section, {})
    if not isinstance(section_data, dict) or key not in section_data:
        return None
    value = section_data[key]
    if not isinstance(value, bool):
        raise NrError(f"[{section}].{key} in {path} must be true or false.")
    return value


def _commands_value(path: Path, data: Mapping[str, Any]) -> tuple[Command, ...] | None:
    section_data = data.get("check", {})
    if not isinstance(section_data, dict) or "commands" not in section_data:
        return None
    commands = section_data["commands"]
    if not isinstance(commands, list):
        raise NrError("[check].commands must be a list of command arrays.")

    parsed: list[Command] = []
    for index, command in enumerate(commands, start=1):
        if not isinstance(command, list) or not command:
            raise NrError(f"[check].commands item {index} in {path} must be a non-empty array.")
        if not all(isinstance(part, str) and part for part in command):
            raise NrError(
                f"[check].commands item {index} in {path} must contain non-empty strings."
            )
        parsed.append(tuple(command))
    return tuple(parsed)


def _resolve_path(value: str, *, base: Path | None) -> Path:
    path = Path(value).expanduser()
    if not path.is_absolute() and base is not None:
        path = base / path
    return path.resolve()


def _merge_check_settings(
    base: CheckSettings,
    *,
    path: Path,
    data: Mapping[str, Any],
) -> CheckSettings:
    values: dict[str, object] = {
        "flake": base.flake,
        "nixfmt": base.nixfmt,
        "statix": base.statix,
        "ruff": base.ruff,
        "commands": base.commands,
    }

    for key in ("flake", "nixfmt", "statix", "ruff"):
        value = _bool_value(path, data, "check", key)
        if value is not None:
            values[key] = value

    commands = _commands_value(path, data)
    if commands is not None:
        values["commands"] = commands

    return CheckSettings(**values)


def _merge_publish_settings(
    base: PublishSettings,
    *,
    path: Path,
    data: Mapping[str, Any],
) -> PublishSettings:
    remote = _string_value(path, data, "publish", "remote") or base.remote
    return PublishSettings(remote=remote)


def load_config(
    *,
    flake: str | None = None,
    host: str | None = None,
    cwd: Path | None = None,
    environ: Mapping[str, str] | None = None,
    hostname: str | None = None,
) -> NrConfig:
    environment = os.environ if environ is None else environ
    working_directory = (Path.cwd() if cwd is None else cwd).expanduser().resolve()

    user_path = user_config_path(environment)
    user_data = _read_toml(user_path)

    raw_flake: str | None = None
    raw_flake_base: Path | None = working_directory
    fragment_host: str | None = None

    if flake:
        raw_flake = flake
    elif environment.get("NR_FLAKE"):
        raw_flake = environment["NR_FLAKE"]
    elif nearest_flake := find_flake(working_directory):
        flake_path = nearest_flake
    elif user_flake := _string_value(user_path, user_data, "target", "flake"):
        raw_flake = user_flake
        raw_flake_base = user_path.parent
    else:
        flake_path = Path("/etc/nixos").resolve()

    if raw_flake is not None:
        path_text, fragment_host = split_flake_reference(raw_flake)
        flake_path = _resolve_path(path_text, base=raw_flake_base)

    validate_flake_path(flake_path)

    repo_path = flake_path / ".nr.toml"
    repo_data = _read_toml(repo_path)
    if _string_value(repo_path, repo_data, "target", "flake") is not None:
        raise NrError(".nr.toml cannot set [target].flake; it already lives in the flake.")

    selected_host = (
        host
        or fragment_host
        or environment.get("NR_HOST")
        or _string_value(repo_path, repo_data, "target", "host")
        or _string_value(user_path, user_data, "target", "host")
        or hostname
        or socket.gethostname()
    ).strip()
    if not selected_host:
        raise NrError("NixOS configuration name cannot be empty.")

    check = CheckSettings()
    publish = PublishSettings()
    if user_data:
        check = _merge_check_settings(check, path=user_path, data=user_data)
        publish = _merge_publish_settings(publish, path=user_path, data=user_data)
    if repo_data:
        check = _merge_check_settings(check, path=repo_path, data=repo_data)
        publish = _merge_publish_settings(publish, path=repo_path, data=repo_data)

    return NrConfig(
        target=FlakeTarget(path=flake_path, host=selected_host),
        check=check,
        publish=publish,
        user_config_path=user_path if user_data else None,
        repo_config_path=repo_path if repo_data else None,
    )


def discover_target(
    *,
    flake: str | None = None,
    host: str | None = None,
    cwd: Path | None = None,
    environ: Mapping[str, str] | None = None,
    hostname: str | None = None,
) -> FlakeTarget:
    return load_config(
        flake=flake,
        host=host,
        cwd=cwd,
        environ=environ,
        hostname=hostname,
    ).target
