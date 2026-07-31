use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use crate::VERSION;
use crate::backend::BackendOptions;
use crate::config::{ConfigInput, load_config};
use crate::errors::{NrError, Result};
use crate::ui::OutputMode;

#[derive(Debug, Clone)]
pub struct Cli {
    pub flake: Option<String>,
    pub host: Option<String>,
    pub ui: OutputMode,
    pub log_file: Option<PathBuf>,
    pub verbose: u8,
    pub dry: bool,
    pub ask: bool,
    pub offline: bool,
    pub show_trace: bool,
    pub specialisation: Option<String>,
    pub command: Option<NrCommand>,
}

#[derive(Debug, Clone)]
pub enum NrCommand {
    Build(Passthrough),
    Switch(Passthrough),
    Test(Passthrough),
    Boot(Passthrough),
    Preview(Passthrough),
    Update(UpdateArgs),
    Rollback(Passthrough),
    Generations(GenerationsArgs),
    Publish(PublishArgs),
    Check(CheckArgs),
    Doctor,
    Cheat,
}

#[derive(Clone, Debug, Default)]
pub struct Passthrough {
    pub backend_args: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct UpdateArgs {
    pub inputs: Vec<String>,
    pub switch: bool,
}

#[derive(Clone, Debug, Default)]
pub struct GenerationsArgs {
    pub profile: Option<String>,
    pub backend_args: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishMode {
    Single,
    PerFile,
}

impl PublishMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "single" => Some(Self::Single),
            "per-file" => Some(Self::PerFile),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PublishArgs {
    pub message: Option<String>,
    pub push: bool,
    pub mode: Option<PublishMode>,
    pub remote: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct CheckArgs {
    pub all: bool,
    pub nixfmt: bool,
    pub statix: bool,
    pub cargo_fmt: bool,
    pub clippy: bool,
    pub no_flake: bool,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            flake: None,
            host: None,
            ui: OutputMode::Auto,
            log_file: None,
            verbose: 0,
            dry: false,
            ask: false,
            offline: false,
            show_trace: false,
            specialisation: None,
            command: None,
        }
    }
}

impl Cli {
    pub fn backend_options(&self, backend_args: &[String]) -> BackendOptions {
        BackendOptions {
            verbose: self.verbose,
            offline: self.offline,
            show_trace: self.show_trace,
            specialisation: self.specialisation.clone(),
            backend_args: backend_args.to_vec(),
        }
    }

    pub fn config_input(&self) -> ConfigInput {
        ConfigInput {
            flake: self.flake.clone(),
            host: self.host.clone(),
            ..ConfigInput::default()
        }
    }

    pub fn parse_from<I, S>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let values = args
            .into_iter()
            .map(|value| value.into().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        parse_values(&values)
    }
}

pub fn main() -> i32 {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("Error: {error}");
            error.exit_code()
        }
    }
}

pub fn run() -> Result<i32> {
    let args = env::args().collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--version") {
        println!("nr {VERSION}");
        return Ok(0);
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(0);
    }

    let cli = Cli::parse_from(args)?;
    let Some(command) = &cli.command else {
        print_help();
        return Ok(0);
    };

    match command {
        NrCommand::Build(args) => {
            crate::lifecycle::run_lifecycle("build", &cli, &args.backend_args)
        }
        NrCommand::Switch(args) => {
            crate::lifecycle::run_lifecycle("switch", &cli, &args.backend_args)
        }
        NrCommand::Test(args) => crate::lifecycle::run_lifecycle("test", &cli, &args.backend_args),
        NrCommand::Boot(args) => crate::lifecycle::run_lifecycle("boot", &cli, &args.backend_args),
        NrCommand::Preview(args) => {
            crate::lifecycle::run_lifecycle("preview", &cli, &args.backend_args)
        }
        NrCommand::Update(args) => {
            let config = load_config(cli.config_input())?;
            crate::lifecycle::run_update(&cli, &config, args)
        }
        NrCommand::Rollback(args) => crate::lifecycle::run_rollback(&cli, &args.backend_args),
        NrCommand::Generations(args) => crate::lifecycle::run_generations(args),
        NrCommand::Publish(args) => {
            let config = load_config(cli.config_input())?;
            crate::publish::run_publish(&config, args)
        }
        NrCommand::Check(args) => {
            let config = load_config(cli.config_input())?;
            crate::checks::run_check(&cli, &config, args)
        }
        NrCommand::Doctor => {
            let config = load_config(cli.config_input())?;
            crate::diagnostics::run_doctor(&config)
        }
        NrCommand::Cheat => crate::help::run_cheat(),
    }
}

fn parse_values(values: &[String]) -> Result<Cli> {
    let mut cli = Cli::default();
    let mut index = 1;
    while index < values.len() {
        if parse_global(values, &mut index, &mut cli)? {
            continue;
        }
        let command = values[index].as_str();
        index += 1;
        cli.command = Some(match command {
            "build" => NrCommand::Build(parse_passthrough(values, &mut index, &mut cli)?),
            "switch" => NrCommand::Switch(parse_passthrough(values, &mut index, &mut cli)?),
            "test" => NrCommand::Test(parse_passthrough(values, &mut index, &mut cli)?),
            "boot" => NrCommand::Boot(parse_passthrough(values, &mut index, &mut cli)?),
            "preview" => NrCommand::Preview(parse_passthrough(values, &mut index, &mut cli)?),
            "update" => NrCommand::Update(parse_update(values, &mut index, &mut cli)?),
            "rollback" => NrCommand::Rollback(parse_passthrough(values, &mut index, &mut cli)?),
            "generations" => {
                NrCommand::Generations(parse_generations(values, &mut index, &mut cli)?)
            }
            "publish" => NrCommand::Publish(parse_publish(values, &mut index, &mut cli)?),
            "check" => NrCommand::Check(parse_check(values, &mut index, &mut cli)?),
            "doctor" => NrCommand::Doctor,
            "cheat" => NrCommand::Cheat,
            other => return Err(NrError::message(format!("Unknown command: {other}"))),
        });
        if matches!(cli.command, Some(NrCommand::Doctor | NrCommand::Cheat)) {
            while index < values.len() {
                if !parse_global(values, &mut index, &mut cli)? {
                    return Err(NrError::message(format!(
                        "Unexpected argument: {}",
                        values[index]
                    )));
                }
            }
        }
        break;
    }
    Ok(cli)
}

fn parse_global(values: &[String], index: &mut usize, cli: &mut Cli) -> Result<bool> {
    let Some(arg) = values.get(*index) else {
        return Ok(false);
    };
    if let Some(value) = arg.strip_prefix("--flake=") {
        cli.flake = Some(value.to_string());
        *index += 1;
        return Ok(true);
    }
    if let Some(value) = arg.strip_prefix("--host=") {
        cli.host = Some(value.to_string());
        *index += 1;
        return Ok(true);
    }
    if let Some(value) = arg.strip_prefix("--ui=") {
        cli.ui = parse_ui(value)?;
        *index += 1;
        return Ok(true);
    }
    if let Some(value) = arg.strip_prefix("--log-file=") {
        cli.log_file = Some(PathBuf::from(value));
        *index += 1;
        return Ok(true);
    }
    if let Some(value) = arg.strip_prefix("--specialisation=") {
        cli.specialisation = Some(value.to_string());
        *index += 1;
        return Ok(true);
    }

    match arg.as_str() {
        "--flake" => {
            cli.flake = Some(take_value(values, index, "--flake")?);
            Ok(true)
        }
        "--host" => {
            cli.host = Some(take_value(values, index, "--host")?);
            Ok(true)
        }
        "--ui" => {
            let value = take_value(values, index, "--ui")?;
            cli.ui = parse_ui(&value)?;
            Ok(true)
        }
        "--log-file" => {
            cli.log_file = Some(PathBuf::from(take_value(values, index, "--log-file")?));
            Ok(true)
        }
        "--specialisation" => {
            cli.specialisation = Some(take_value(values, index, "--specialisation")?);
            Ok(true)
        }
        "--dry" => {
            cli.dry = true;
            *index += 1;
            Ok(true)
        }
        "--ask" => {
            cli.ask = true;
            *index += 1;
            Ok(true)
        }
        "--offline" => {
            cli.offline = true;
            *index += 1;
            Ok(true)
        }
        "--show-trace" => {
            cli.show_trace = true;
            *index += 1;
            Ok(true)
        }
        "-v" | "--verbose" => {
            cli.verbose = cli.verbose.saturating_add(1);
            *index += 1;
            Ok(true)
        }
        value
            if value.starts_with("-v")
                && value.chars().skip(1).all(|character| character == 'v') =>
        {
            cli.verbose = cli.verbose.saturating_add((value.len() - 1) as u8);
            *index += 1;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn parse_passthrough(values: &[String], index: &mut usize, cli: &mut Cli) -> Result<Passthrough> {
    let mut backend_args = Vec::new();
    while *index < values.len() {
        if values[*index] == "--" {
            backend_args.extend(values[*index + 1..].iter().cloned());
            *index = values.len();
            break;
        }
        if parse_global(values, index, cli)? {
            continue;
        }
        return Err(NrError::message(format!(
            "Unexpected argument: {}",
            values[*index]
        )));
    }
    Ok(Passthrough { backend_args })
}

fn parse_update(values: &[String], index: &mut usize, cli: &mut Cli) -> Result<UpdateArgs> {
    let mut args = UpdateArgs::default();
    while *index < values.len() {
        if parse_global(values, index, cli)? {
            continue;
        }
        if values[*index] == "--switch" {
            args.switch = true;
            *index += 1;
        } else {
            args.inputs.push(values[*index].clone());
            *index += 1;
        }
    }
    Ok(args)
}

fn parse_generations(
    values: &[String],
    index: &mut usize,
    cli: &mut Cli,
) -> Result<GenerationsArgs> {
    let mut args = GenerationsArgs::default();
    while *index < values.len() {
        if values[*index] == "--" {
            args.backend_args
                .extend(values[*index + 1..].iter().cloned());
            *index = values.len();
            break;
        }
        if parse_global(values, index, cli)? {
            continue;
        }
        if values[*index] == "--profile" {
            args.profile = Some(take_value(values, index, "--profile")?);
        } else if let Some(value) = values[*index].strip_prefix("--profile=") {
            args.profile = Some(value.to_string());
            *index += 1;
        } else {
            return Err(NrError::message(format!(
                "Unexpected argument: {}",
                values[*index]
            )));
        }
    }
    Ok(args)
}

fn parse_publish(values: &[String], index: &mut usize, cli: &mut Cli) -> Result<PublishArgs> {
    let mut args = PublishArgs::default();
    while *index < values.len() {
        if parse_global(values, index, cli)? {
            continue;
        }
        if let Some(value) = values[*index].strip_prefix("--mode=") {
            args.mode = Some(parse_publish_mode(value)?);
            *index += 1;
        } else if let Some(value) = values[*index].strip_prefix("--remote=") {
            args.remote = Some(value.to_string());
            *index += 1;
        } else {
            match values[*index].as_str() {
                "-m" | "--message" => args.message = Some(take_value(values, index, "--message")?),
                "--push" => {
                    args.push = true;
                    *index += 1;
                }
                "--mode" => {
                    args.mode = Some(parse_publish_mode(&take_value(values, index, "--mode")?)?)
                }
                "--remote" => args.remote = Some(take_value(values, index, "--remote")?),
                other => return Err(NrError::message(format!("Unexpected argument: {other}"))),
            }
        }
    }
    Ok(args)
}

fn parse_check(values: &[String], index: &mut usize, cli: &mut Cli) -> Result<CheckArgs> {
    let mut args = CheckArgs::default();
    while *index < values.len() {
        if parse_global(values, index, cli)? {
            continue;
        }
        match values[*index].as_str() {
            "--all" => args.all = true,
            "--nixfmt" => args.nixfmt = true,
            "--statix" => args.statix = true,
            "--cargo-fmt" => args.cargo_fmt = true,
            "--clippy" => args.clippy = true,
            "--no-flake" => args.no_flake = true,
            other => return Err(NrError::message(format!("Unexpected argument: {other}"))),
        }
        *index += 1;
    }
    Ok(args)
}

fn take_value(values: &[String], index: &mut usize, flag: &str) -> Result<String> {
    let value_index = *index + 1;
    let Some(value) = values.get(value_index) else {
        return Err(NrError::message(format!("{flag} requires a value.")));
    };
    *index += 2;
    Ok(value.clone())
}

fn parse_ui(value: &str) -> Result<OutputMode> {
    OutputMode::parse(value).ok_or_else(|| {
        NrError::message("Invalid --ui value. Expected auto, rich, plain, raw, or json.")
    })
}

fn parse_publish_mode(value: &str) -> Result<PublishMode> {
    PublishMode::parse(value)
        .ok_or_else(|| NrError::message("Invalid --mode value. Expected single or per-file."))
}

fn print_help() {
    println!(
        "Build, switch, update, check, and publish a NixOS flake.\n\n\
Usage: nr [OPTIONS] <COMMAND> [ARGS]\n\n\
Commands:\n  build\n  switch\n  test\n  boot\n  preview\n  update\n  rollback\n  generations\n  publish\n  check\n  doctor\n  cheat\n\n\
Options:\n  --flake PATH[#HOST]\n  --host HOST\n  --ui auto|rich|plain|raw|json\n  --log-file PATH\n  -v, --verbose\n  --dry\n  --ask\n  --offline\n  --show-trace\n  --specialisation NAME\n  --version\n  -h, --help"
    );
}
