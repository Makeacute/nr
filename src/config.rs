use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::errors::{IoContext, NrError, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlakeTarget {
    pub path: PathBuf,
    pub host: String,
}

impl FlakeTarget {
    pub fn reference(&self) -> String {
        format!("{}#{}", self.path.display(), self.host)
    }
}

pub type CheckCommand = Vec<String>;
pub type HookCommand = Vec<String>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckSettings {
    pub flake: bool,
    pub nixfmt: bool,
    pub statix: bool,
    pub cargo_fmt: bool,
    pub clippy: bool,
    pub commands: Vec<CheckCommand>,
}

impl Default for CheckSettings {
    fn default() -> Self {
        Self {
            flake: true,
            nixfmt: false,
            statix: false,
            cargo_fmt: false,
            clippy: false,
            commands: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishSettings {
    pub remote: String,
}

impl Default for PublishSettings {
    fn default() -> Self {
        Self {
            remote: "origin".to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookSettings {
    pub post_switch: Vec<HookCommand>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiSettings {
    pub accent: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NrConfig {
    pub target: FlakeTarget,
    pub check: CheckSettings,
    pub publish: PublishSettings,
    pub hooks: HookSettings,
    pub ui: UiSettings,
    pub user_config_path: Option<PathBuf>,
    pub repo_config_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct ConfigInput {
    pub flake: Option<String>,
    pub host: Option<String>,
    pub cwd: Option<PathBuf>,
    pub environ: Option<Vec<(String, String)>>,
    pub hostname: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigData {
    target: TargetConfig,
    check: CheckConfig,
    publish: PublishConfig,
    hooks: HooksConfig,
    ui: UiConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TargetConfig {
    flake: Option<String>,
    host: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CheckConfig {
    flake: Option<bool>,
    nixfmt: Option<bool>,
    statix: Option<bool>,
    cargo_fmt: Option<bool>,
    clippy: Option<bool>,
    commands: Option<Vec<CheckCommand>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PublishConfig {
    remote: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct HooksConfig {
    post_switch: Option<Vec<HookCommand>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct UiConfig {
    accent: Option<String>,
}

pub fn find_flake(start: &Path) -> Option<PathBuf> {
    let mut current = absolute_path(start);
    if current.is_file() {
        current.pop();
    }

    for candidate in current.ancestors() {
        if candidate.join("flake.nix").is_file() {
            return Some(existing_path(candidate));
        }
    }
    None
}

pub fn split_flake_reference(value: &str) -> Result<(String, Option<String>)> {
    match value.rsplit_once('#') {
        Some(("", _)) => Err(NrError::message("Flake path cannot be empty.")),
        Some((_, "")) => Err(NrError::message("Flake host cannot be empty after '#'.")),
        Some((path, host)) => Ok((path.to_string(), Some(host.to_string()))),
        None => Ok((value.to_string(), None)),
    }
}

pub fn validate_flake_path(path: &Path) -> Result<()> {
    if !path.is_dir() {
        return Err(NrError::message(format!(
            "Flake directory does not exist: {}",
            path.display()
        )));
    }
    if !path.join("flake.nix").is_file() {
        return Err(NrError::message(format!(
            "No flake.nix found in: {}",
            path.display()
        )));
    }
    Ok(())
}

pub fn user_config_path(environ: &[(String, String)]) -> PathBuf {
    if let Some(value) = env_value(environ, "XDG_CONFIG_HOME") {
        return expand_home(value, environ).join("nr/config.toml");
    }
    let home = env_value(environ, "HOME")
        .map(|value| expand_home(value, environ))
        .or_else(|| env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".config/nr/config.toml")
}

pub fn load_config(input: ConfigInput) -> Result<NrConfig> {
    let environment = input
        .environ
        .unwrap_or_else(|| env::vars().collect::<Vec<(String, String)>>());
    let cwd = input
        .cwd
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let working_directory = absolute_path(&cwd);

    let user_path = user_config_path(&environment);
    let user_data = read_config(&user_path)?;

    let mut raw_flake = None;
    let mut raw_flake_base = Some(working_directory.clone());
    let mut fragment_host = None;

    if let Some(value) = input.flake.filter(|value| !value.trim().is_empty()) {
        raw_flake = Some(value);
    } else if let Some(value) = env_value(&environment, "NR_FLAKE") {
        raw_flake = Some(value.to_string());
    } else if let Some(nearest) = find_flake(&working_directory) {
        return finish_config(
            nearest,
            input.host,
            fragment_host,
            &environment,
            input.hostname,
            user_path,
            user_data,
        );
    } else if let Some(value) = target_flake(&user_data)? {
        raw_flake = Some(value);
        raw_flake_base = user_path.parent().map(Path::to_path_buf);
    }

    let flake_path = if let Some(raw) = raw_flake {
        let (path_text, host) = split_flake_reference(&raw)?;
        fragment_host = host;
        resolve_config_path(&path_text, raw_flake_base.as_deref(), &environment)
    } else {
        PathBuf::from("/etc/nixos")
    };

    finish_config(
        flake_path,
        input.host,
        fragment_host,
        &environment,
        input.hostname,
        user_path,
        user_data,
    )
}

pub fn discover_target(input: ConfigInput) -> Result<FlakeTarget> {
    Ok(load_config(input)?.target)
}

fn finish_config(
    flake_path: PathBuf,
    cli_host: Option<String>,
    fragment_host: Option<String>,
    environment: &[(String, String)],
    hostname: Option<String>,
    user_path: PathBuf,
    user_data: Option<ConfigData>,
) -> Result<NrConfig> {
    let flake_path = existing_path(&flake_path);
    validate_flake_path(&flake_path)?;

    let repo_path = flake_path.join(".nr.toml");
    let repo_data = read_config(&repo_path)?;
    reject_repo_flake(&repo_data)?;

    let repo_host = target_host(&repo_data)?;
    let user_host = target_host(&user_data)?;
    let selected_host = cli_host
        .filter(|value| !value.trim().is_empty())
        .or(fragment_host)
        .or_else(|| env_value(environment, "NR_HOST").map(ToOwned::to_owned))
        .or(repo_host)
        .or(user_host)
        .or(hostname)
        .or_else(|| env_value(environment, "HOSTNAME").map(ToOwned::to_owned))
        .unwrap_or_else(|| "nixos".to_string())
        .trim()
        .to_string();

    if selected_host.is_empty() {
        return Err(NrError::message(
            "NixOS configuration name cannot be empty.",
        ));
    }

    let mut check = CheckSettings::default();
    let mut publish = PublishSettings::default();
    let mut hooks = HookSettings::default();
    let mut ui = UiSettings::default();
    if let Some(data) = &user_data {
        check = merge_check_settings(check, data);
        publish = merge_publish_settings(publish, data)?;
        hooks = merge_hook_settings(hooks, data);
        ui = merge_ui_settings(ui, data)?;
    }
    if let Some(data) = &repo_data {
        check = merge_check_settings(check, data);
        publish = merge_publish_settings(publish, data)?;
        hooks = merge_hook_settings(hooks, data);
        ui = merge_ui_settings(ui, data)?;
    }

    Ok(NrConfig {
        target: FlakeTarget {
            path: flake_path,
            host: selected_host,
        },
        check,
        publish,
        hooks,
        ui,
        user_config_path: user_data.as_ref().map(|_| user_path),
        repo_config_path: repo_data.as_ref().map(|_| repo_path),
    })
}

fn read_config(path: &Path) -> Result<Option<ConfigData>> {
    if !path.is_file() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(path).with_context(format!("failed to read {}", path.display()))?;
    toml::from_str(&text)
        .map(Some)
        .map_err(|error| NrError::message(format!("Invalid config in {}: {error}", path.display())))
}

fn reject_repo_flake(data: &Option<ConfigData>) -> Result<()> {
    let Some(value) = data.as_ref().and_then(|data| data.target.flake.as_ref()) else {
        return Ok(());
    };
    if value.trim().is_empty() {
        return Err(NrError::message("[target].flake cannot be empty."));
    }
    Err(NrError::message(
        ".nr.toml cannot set [target].flake; it already lives in the flake.",
    ))
}

fn target_flake(data: &Option<ConfigData>) -> Result<Option<String>> {
    non_empty_string(
        data.as_ref().and_then(|data| data.target.flake.as_ref()),
        "[target].flake",
    )
}

fn target_host(data: &Option<ConfigData>) -> Result<Option<String>> {
    non_empty_string(
        data.as_ref().and_then(|data| data.target.host.as_ref()),
        "[target].host",
    )
}

fn publish_remote(data: &ConfigData) -> Result<Option<String>> {
    non_empty_string(data.publish.remote.as_ref(), "[publish].remote")
}

fn non_empty_string(value: Option<&String>, label: &str) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(NrError::message(format!("{label} cannot be empty.")));
    }
    Ok(Some(trimmed.to_string()))
}

fn merge_check_settings(mut base: CheckSettings, data: &ConfigData) -> CheckSettings {
    if let Some(value) = data.check.flake {
        base.flake = value;
    }
    if let Some(value) = data.check.nixfmt {
        base.nixfmt = value;
    }
    if let Some(value) = data.check.statix {
        base.statix = value;
    }
    if let Some(value) = data.check.cargo_fmt {
        base.cargo_fmt = value;
    }
    if let Some(value) = data.check.clippy {
        base.clippy = value;
    }
    if let Some(commands) = &data.check.commands {
        base.commands = commands.clone();
    }
    base
}

fn merge_publish_settings(mut base: PublishSettings, data: &ConfigData) -> Result<PublishSettings> {
    if let Some(remote) = publish_remote(data)? {
        base.remote = remote;
    }
    Ok(base)
}

fn merge_hook_settings(mut base: HookSettings, data: &ConfigData) -> HookSettings {
    if let Some(commands) = &data.hooks.post_switch {
        base.post_switch = commands.clone();
    }
    base
}

fn merge_ui_settings(mut base: UiSettings, data: &ConfigData) -> Result<UiSettings> {
    if let Some(accent) = &data.ui.accent {
        let accent = accent.trim();
        if !is_hex_color(accent) {
            return Err(NrError::message(
                "[ui].accent must be a hex color like \"#cba6f7\".",
            ));
        }
        base.accent = Some(accent.to_string());
    }
    Ok(base)
}

fn is_hex_color(value: &str) -> bool {
    let Some(hex) = value.strip_prefix('#') else {
        return false;
    };
    hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn resolve_config_path(
    value: &str,
    base: Option<&Path>,
    environment: &[(String, String)],
) -> PathBuf {
    let path = expand_home(value, environment);
    let path = if path.is_absolute() {
        path
    } else if let Some(base) = base {
        base.join(path)
    } else {
        absolute_path(&path)
    };
    existing_path(&path)
}

fn expand_home(value: &str, environment: &[(String, String)]) -> PathBuf {
    if value == "~" {
        return home_dir(environment);
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home_dir(environment).join(rest);
    }
    PathBuf::from(value)
}

fn home_dir(environment: &[(String, String)]) -> PathBuf {
    env_value(environment, "HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn env_value<'a>(environment: &'a [(String, String)], key: &str) -> Option<&'a str> {
    environment
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn existing_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
