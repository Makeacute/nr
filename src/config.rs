use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NrConfig {
    pub target: FlakeTarget,
    pub check: CheckSettings,
    pub publish: PublishSettings,
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

#[derive(Clone, Debug, Default)]
struct ConfigData {
    values: BTreeMap<(String, String), ConfigValue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ConfigValue {
    String(String),
    Bool(bool),
    Commands(Vec<CheckCommand>),
}

const TOP_LEVEL_KEYS: &[&str] = &["target", "check", "publish"];
const TARGET_KEYS: &[&str] = &["flake", "host"];
const CHECK_KEYS: &[&str] = &[
    "flake",
    "nixfmt",
    "statix",
    "cargo_fmt",
    "clippy",
    "commands",
];
const PUBLISH_KEYS: &[&str] = &["remote"];

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
    } else if let Some(value) = string_value(&user_data, "target", "flake")? {
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
    if string_value(&repo_data, "target", "flake")?.is_some() {
        return Err(NrError::message(
            ".nr.toml cannot set [target].flake; it already lives in the flake.",
        ));
    }

    let repo_host = string_value(&repo_data, "target", "host")?;
    let user_host = string_value(&user_data, "target", "host")?;
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
    if let Some(data) = &user_data {
        check = merge_check_settings(check, data)?;
        publish = merge_publish_settings(publish, data)?;
    }
    if let Some(data) = &repo_data {
        check = merge_check_settings(check, data)?;
        publish = merge_publish_settings(publish, data)?;
    }

    Ok(NrConfig {
        target: FlakeTarget {
            path: flake_path,
            host: selected_host,
        },
        check,
        publish,
        user_config_path: user_data.map(|_| user_path),
        repo_config_path: repo_data.map(|_| repo_path),
    })
}

fn read_config(path: &Path) -> Result<Option<ConfigData>> {
    if !path.is_file() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(path).with_context(format!("failed to read {}", path.display()))?;
    parse_config(path, &text).map(Some)
}

fn parse_config(path: &Path, text: &str) -> Result<ConfigData> {
    let mut data = ConfigData::default();
    let mut section = String::new();
    let mut lines = text.lines().enumerate().peekable();
    let mut sections = BTreeSet::new();

    while let Some((line_number, raw_line)) = lines.next() {
        let line = strip_comment(raw_line).trim().to_string();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_string();
            validate_section(path, &section)?;
            sections.insert(section.clone());
            continue;
        }
        if section.is_empty() {
            return Err(NrError::message(format!(
                "Key outside a section in {}:{}",
                path.display(),
                line_number + 1
            )));
        }

        let Some((key, value)) = line.split_once('=') else {
            return Err(NrError::message(format!(
                "Invalid config line in {}:{}",
                path.display(),
                line_number + 1
            )));
        };
        let key = key.trim().to_string();
        validate_key(path, &section, &key)?;
        let mut value = value.trim().to_string();
        if section == "check" && key == "commands" {
            while bracket_balance(&value) > 0 {
                let Some((_, next)) = lines.next() else {
                    break;
                };
                value.push('\n');
                value.push_str(next);
            }
            data.values.insert(
                (section.clone(), key),
                ConfigValue::Commands(parse_commands(&value)?),
            );
        } else if value == "true" || value == "false" {
            data.values
                .insert((section.clone(), key), ConfigValue::Bool(value == "true"));
        } else {
            data.values.insert(
                (section.clone(), key),
                ConfigValue::String(parse_string(&value)?),
            );
        }
    }

    let unknown = sections
        .iter()
        .filter(|section| !TOP_LEVEL_KEYS.contains(&section.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(NrError::message(format!(
            "Unknown config section in {}: {}",
            path.display(),
            unknown.join(", ")
        )));
    }
    Ok(data)
}

fn validate_section(path: &Path, section: &str) -> Result<()> {
    if !TOP_LEVEL_KEYS.contains(&section) {
        return Err(NrError::message(format!(
            "Unknown config section in {}: {section}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_key(path: &Path, section: &str, key: &str) -> Result<()> {
    let allowed = match section {
        "target" => TARGET_KEYS,
        "check" => CHECK_KEYS,
        "publish" => PUBLISH_KEYS,
        _ => return validate_section(path, section),
    };
    if !allowed.contains(&key) {
        return Err(NrError::message(format!(
            "Unknown [{section}] key in {}: {key}",
            path.display()
        )));
    }
    Ok(())
}

fn string_value(data: &Option<ConfigData>, section: &str, key: &str) -> Result<Option<String>> {
    let Some(data) = data else {
        return Ok(None);
    };
    let Some(value) = data.values.get(&(section.to_string(), key.to_string())) else {
        return Ok(None);
    };
    match value {
        ConfigValue::String(value) if !value.trim().is_empty() => {
            Ok(Some(value.trim().to_string()))
        }
        ConfigValue::String(_) => Err(NrError::message(format!(
            "[{section}].{key} cannot be empty."
        ))),
        _ => Err(NrError::message(format!(
            "[{section}].{key} must be a string."
        ))),
    }
}

fn bool_value(data: &ConfigData, section: &str, key: &str) -> Result<Option<bool>> {
    let Some(value) = data.values.get(&(section.to_string(), key.to_string())) else {
        return Ok(None);
    };
    match value {
        ConfigValue::Bool(value) => Ok(Some(*value)),
        _ => Err(NrError::message(format!(
            "[{section}].{key} must be true or false."
        ))),
    }
}

fn commands_value(data: &ConfigData) -> Result<Option<Vec<CheckCommand>>> {
    let Some(value) = data
        .values
        .get(&("check".to_string(), "commands".to_string()))
    else {
        return Ok(None);
    };
    match value {
        ConfigValue::Commands(commands) => Ok(Some(commands.clone())),
        _ => Err(NrError::message(
            "[check].commands must be a list of command arrays.",
        )),
    }
}

fn merge_check_settings(mut base: CheckSettings, data: &ConfigData) -> Result<CheckSettings> {
    for key in ["flake", "nixfmt", "statix", "cargo_fmt", "clippy"] {
        if let Some(value) = bool_value(data, "check", key)? {
            match key {
                "flake" => base.flake = value,
                "nixfmt" => base.nixfmt = value,
                "statix" => base.statix = value,
                "cargo_fmt" => base.cargo_fmt = value,
                "clippy" => base.clippy = value,
                _ => unreachable!(),
            }
        }
    }
    if let Some(commands) = commands_value(data)? {
        base.commands = commands;
    }
    Ok(base)
}

fn merge_publish_settings(mut base: PublishSettings, data: &ConfigData) -> Result<PublishSettings> {
    if let Some(remote) = string_value(&Some(data.clone()), "publish", "remote")? {
        base.remote = remote;
    }
    Ok(base)
}

fn parse_string(value: &str) -> Result<String> {
    let value = value.trim();
    if !(value.starts_with('"') && value.ends_with('"')) {
        return Err(NrError::message(format!(
            "Expected a quoted string, got {value}"
        )));
    }
    unescape_string(&value[1..value.len() - 1])
}

fn parse_commands(value: &str) -> Result<Vec<CheckCommand>> {
    let mut commands = Vec::new();
    let mut index = 0;
    let bytes = value.as_bytes();
    while index < bytes.len() {
        if bytes[index] != b'[' {
            index += 1;
            continue;
        }
        index += 1;
        let mut command = Vec::new();
        loop {
            while index < bytes.len() && matches!(bytes[index], b' ' | b'\n' | b'\t' | b'\r' | b',')
            {
                index += 1;
            }
            if index >= bytes.len() || bytes[index] == b']' {
                break;
            }
            if bytes[index] != b'"' {
                index += 1;
                continue;
            }
            let (text, next) = parse_quoted_at(value, index)?;
            command.push(text);
            index = next;
        }
        if !command.is_empty() {
            commands.push(command);
        }
        index += 1;
    }
    Ok(commands)
}

fn parse_quoted_at(value: &str, start: usize) -> Result<(String, usize)> {
    let mut escaped = false;
    let mut output = String::new();
    for (offset, character) in value[start + 1..].char_indices() {
        if escaped {
            output.push(match character {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '"' => '"',
                '\\' => '\\',
                other => other,
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            return Ok((output, start + 1 + offset + 1));
        } else {
            output.push(character);
        }
    }
    Err(NrError::message("Unterminated string in commands array."))
}

fn unescape_string(value: &str) -> Result<String> {
    parse_quoted_at(&format!("\"{value}\""), 0).map(|(value, _)| value)
}

fn bracket_balance(value: &str) -> i32 {
    let mut balance = 0;
    let mut in_string = false;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if in_string {
            if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '[' => balance += 1,
            ']' => balance -= 1,
            _ => {}
        }
    }
    balance
}

fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if in_string {
            if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
        } else if character == '"' {
            in_string = true;
        } else if character == '#' {
            return &line[..index];
        }
    }
    line
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
