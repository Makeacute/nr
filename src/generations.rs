use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::backend;
use crate::cli::{GenerationsArgs, PinArgs, PinsArgs, UnpinArgs};
use crate::errors::{IoContext, NrError, Result};
use crate::process::{CommandSpec, run_capture, run_inherit, state_dir};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SystemGeneration {
    pub generation: u64,
    pub date: String,
    #[serde(rename = "nixosVersion", default)]
    pub nixos_version: String,
    #[serde(rename = "kernelVersion", default)]
    pub kernel_version: String,
    #[serde(rename = "configurationRevision", default)]
    pub configuration_revision: String,
    #[serde(default)]
    pub current: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinsFile {
    #[serde(default)]
    pub pins: BTreeMap<String, u64>,
}

pub fn run_generations(args: &GenerationsArgs) -> Result<i32> {
    if args.profile.is_some() || !args.backend_args.is_empty() {
        let command = backend::generations_command(args.profile.as_deref(), &args.backend_args);
        let code = run_inherit(&command, true)?;
        if code != 0 {
            return Err(NrError::CommandFailed {
                command: command.render(),
                code,
            });
        }
        return Ok(0);
    }

    let generations = load_system_generations()?;
    let pins = load_pins()?;
    print_generations(&generations, &pins);
    Ok(0)
}

pub fn run_pin(args: &PinArgs) -> Result<i32> {
    let pins_path = pins_path();
    let previous = read_pins_from_path(&pins_path)?
        .pins
        .get(args.label.trim())
        .copied();
    pin_generation(args.generation, &args.label, args.force, &pins_path)?;
    if !args.no_gc_root
        && let Err(error) = create_pin_gc_root(args.generation, &args.label)
    {
        if let Some(previous_generation) = previous {
            let _ = pin_generation(previous_generation, &args.label, true, &pins_path);
        } else {
            let _ = unpin_generation(&args.label, &pins_path);
        }
        return Err(error);
    }
    println!(
        "pinned generation {} as {}",
        args.generation,
        args.label.trim()
    );
    Ok(0)
}

pub fn run_unpin(args: &UnpinArgs) -> Result<i32> {
    unpin_generation(&args.label, &pins_path())?;
    remove_pin_gc_root(&args.label)?;
    println!("removed pin {}", args.label.trim());
    Ok(0)
}

pub fn run_pins(args: &PinsArgs) -> Result<i32> {
    let pins = load_pins()?;
    if pins.pins.is_empty() {
        println!("No generation pins.");
        return Ok(0);
    }
    let mut stale_count = 0usize;
    for (label, generation) in &pins.pins {
        let stale = stale_pin(label, *generation);
        if stale {
            stale_count += 1;
        }
        println!(
            "{} -> {}{}",
            label,
            generation,
            if stale { " (stale)" } else { "" }
        );
        if args.check_stale && stale {
            eprintln!("stale pin: {label}");
        }
    }
    Ok(if args.check_stale && stale_count > 0 {
        1
    } else {
        0
    })
}

pub fn load_system_generations() -> Result<Vec<SystemGeneration>> {
    let command = backend::generations_json_command();
    let output = run_capture(&command, false)?;
    if output.code != 0 {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code: output.code,
        });
    }
    parse_generations_json(&output.stdout)
}

pub fn parse_generations_json(output: &str) -> Result<Vec<SystemGeneration>> {
    serde_json::from_str(output)
        .map_err(|error| NrError::message(format!("failed to parse generation JSON: {error}")))
}

pub fn load_pins() -> Result<PinsFile> {
    read_pins_from_path(&pins_path())
}

pub fn read_pins_from_path(path: &Path) -> Result<PinsFile> {
    if !path.is_file() {
        return Ok(PinsFile::default());
    }
    let text =
        fs::read_to_string(path).with_context(format!("failed to read {}", path.display()))?;
    toml::from_str(&text)
        .map_err(|error| NrError::message(format!("Invalid pins in {}: {error}", path.display())))
}

pub fn write_pins_to_path(path: &Path, pins: &PinsFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(format!("failed to create {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(pins)
        .map_err(|error| NrError::message(format!("failed to serialize pins: {error}")))?;
    fs::write(path, text).with_context(format!("failed to write {}", path.display()))
}

pub fn pin_generation(generation: u64, label: &str, force: bool, path: &Path) -> Result<()> {
    let label = label.trim();
    if label.is_empty() {
        return Err(NrError::message("pin label cannot be empty."));
    }
    let mut pins = read_pins_from_path(path)?;
    if pins.pins.contains_key(label) && !force {
        return Err(NrError::message(format!(
            "pin label already exists: {label}. Use --force to overwrite."
        )));
    }
    pins.pins.insert(label.to_string(), generation);
    write_pins_to_path(path, &pins)
}

pub fn unpin_generation(label: &str, path: &Path) -> Result<()> {
    let label = label.trim();
    if label.is_empty() {
        return Err(NrError::message("pin label cannot be empty."));
    }
    let mut pins = read_pins_from_path(path)?;
    if pins.pins.remove(label).is_none() {
        return Err(NrError::message(format!("unknown pin: {label}")));
    }
    write_pins_to_path(path, &pins)
}

pub fn resolve_generation_reference(reference: &str, pins: &PinsFile) -> Result<u64> {
    if let Ok(generation) = reference.parse::<u64>() {
        return Ok(generation);
    }
    pins.pins
        .get(reference)
        .copied()
        .ok_or_else(|| NrError::message(format!("unknown generation or pin: {reference}")))
}

pub fn generation_path(generation: u64) -> PathBuf {
    PathBuf::from(format!("/nix/var/nix/profiles/system-{generation}-link"))
}

pub fn previous_generation(generations: &[SystemGeneration]) -> Option<&SystemGeneration> {
    let current = generations.iter().find(|generation| generation.current)?;
    generations
        .iter()
        .filter(|generation| generation.generation < current.generation)
        .max_by_key(|generation| generation.generation)
}

pub fn current_generation(generations: &[SystemGeneration]) -> Option<&SystemGeneration> {
    generations.iter().find(|generation| generation.current)
}

pub fn generation_by_number(
    generations: &[SystemGeneration],
    number: u64,
) -> Option<&SystemGeneration> {
    generations
        .iter()
        .find(|generation| generation.generation == number)
}

pub fn rollback_target_command(
    target: Option<u64>,
    options: &backend::BackendOptions,
) -> CommandSpec {
    if let Some(generation) = target {
        backend::rollback_to_store_path_command(&generation_path(generation), options)
    } else {
        backend::rollback_command(options)
    }
}

pub fn pins_path() -> PathBuf {
    state_dir().join("pins.toml")
}

pub fn pin_root_path(label: &str) -> PathBuf {
    state_dir().join("pin-roots").join(safe_pin_label(label))
}

fn create_pin_gc_root(generation: u64, label: &str) -> Result<()> {
    let root = pin_root_path(label);
    if let Some(parent) = root.parent() {
        fs::create_dir_all(parent)
            .with_context(format!("failed to create {}", parent.display()))?;
    }
    let target = generation_path(generation);
    let command = backend::nix_store_add_root_command(&root, &target);
    let output = run_capture(&command, false)?;
    if output.code != 0 {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code: output.code,
        });
    }
    Ok(())
}

fn remove_pin_gc_root(label: &str) -> Result<()> {
    let path = pin_root_path(label);
    if path.exists() || path.symlink_metadata().is_ok() {
        fs::remove_file(&path).with_context(format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

fn stale_pin(label: &str, generation: u64) -> bool {
    pin_root_path(label).symlink_metadata().is_err() || !generation_path(generation).exists()
}

fn safe_pin_label(label: &str) -> String {
    let mut escaped = String::new();
    for byte in label.trim().bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            escaped.push(byte as char);
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    if escaped.is_empty() {
        "_empty".to_string()
    } else {
        escaped
    }
}

fn print_generations(generations: &[SystemGeneration], pins: &PinsFile) {
    println!("Generation  Build-date           NixOS version            Kernel   Current  Pin");
    for generation in generations {
        let labels = pins
            .pins
            .iter()
            .filter_map(|(label, pinned)| {
                if *pinned == generation.generation {
                    if stale_pin(label, *pinned) {
                        Some(format!("{label} (stale)"))
                    } else {
                        Some(label.clone())
                    }
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "{:<10}  {:<19}  {:<23}  {:<7}  {:<7}  {}",
            generation.generation,
            generation.date,
            generation.nixos_version,
            generation.kernel_version,
            if generation.current { "yes" } else { "no" },
            labels
        );
    }
}
