use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use clap::{ArgAction, Args, CommandFactory, Parser, Subcommand, ValueEnum};

use crate::VERSION;
use crate::backend::BackendOptions;
use crate::config::{ConfigInput, load_config};
use crate::errors::{IoContext, NrError, Result};
use crate::ui::OutputMode;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "nr",
    version = VERSION,
    about = "Build, switch, update, check, and publish a NixOS flake.",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        value_name = "PATH[#HOST]",
        help = "Select a flake and optional host"
    )]
    pub flake: Option<String>,
    #[arg(
        long,
        global = true,
        value_name = "HOST",
        help = "Override the NixOS configuration name"
    )]
    pub host: Option<String>,
    #[arg(
        long,
        global = true,
        value_enum,
        default_value = "auto",
        help = "Select command output mode; auto uses nom for interactive lifecycle builds"
    )]
    pub ui: OutputMode,
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Capture the full backend log at PATH"
    )]
    pub log_file: Option<PathBuf>,
    #[arg(
        short = 'v',
        long,
        global = true,
        action = ArgAction::Count,
        help = "Increase backend verbosity"
    )]
    pub verbose: u8,
    #[arg(
        long,
        global = true,
        help = "Alias lifecycle commands to preview-style behavior"
    )]
    pub dry: bool,
    #[arg(long, global = true, help = "Ask before activation")]
    pub ask: bool,
    #[arg(long, global = true, help = "Forward offline mode to Nix")]
    pub offline: bool,
    #[arg(long, global = true, help = "Forward Nix traces")]
    pub show_trace: bool,
    #[arg(
        long,
        global = true,
        value_name = "METHOD",
        value_parser = ["none", "sudo", "run0"],
        help = "Forward nixos-rebuild privilege elevation method"
    )]
    pub elevate: Option<String>,
    #[arg(
        long,
        visible_alias = "ask-sudo-password",
        global = true,
        help = "Prompt and pipe an elevation password instead of letting sudo prompt normally"
    )]
    pub ask_elevate_password: bool,
    #[arg(
        long,
        global = true,
        help = "Send a desktop notification when lifecycle commands finish"
    )]
    pub notify: bool,
    #[arg(
        long,
        global = true,
        value_name = "NAME",
        help = "Build or activate a NixOS specialisation"
    )]
    pub specialisation: Option<String>,
    #[command(subcommand)]
    pub command: Option<NrCommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum NrCommand {
    #[command(about = "Build the selected host without activating it")]
    Build(Passthrough),
    #[command(about = "Build and activate the selected host")]
    Switch(Passthrough),
    #[command(about = "Build and activate until the next reboot")]
    Test(Passthrough),
    #[command(about = "Build and make the generation the next boot default")]
    Boot(Passthrough),
    #[command(about = "Build, diff, and dry-activate without mutating")]
    Preview(Passthrough),
    #[command(about = "Update flake.lock or selected flake inputs")]
    Update(UpdateArgs),
    #[command(about = "Roll back to the previous generation")]
    Rollback(RollbackArgs),
    #[command(about = "Show NixOS generations")]
    Generations(GenerationsArgs),
    #[command(about = "Diff the current system against a path, generation, or flake")]
    Diff(DiffArgs),
    #[command(about = "Run Nix garbage collection with safer defaults")]
    Gc(GcArgs),
    #[command(about = "Pin a NixOS generation with a label")]
    Pin(PinArgs),
    #[command(about = "Review, commit, and optionally push")]
    Publish(PublishArgs),
    #[command(about = "Run configured checks")]
    Check(CheckArgs),
    #[command(about = "Show target, config, dependency, and Git diagnostics")]
    Doctor,
    #[command(about = "Show the complete terminal cheat sheet")]
    Cheat,
}

#[derive(Clone, Debug, Default, Args)]
pub struct Passthrough {
    #[arg(last = true, value_name = "BACKEND_ARG", allow_hyphen_values = true)]
    pub backend_args: Vec<String>,
}

#[derive(Clone, Debug, Default, Args)]
pub struct UpdateArgs {
    #[arg(value_name = "INPUT")]
    pub inputs: Vec<String>,
    #[arg(long)]
    pub switch: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub struct RollbackArgs {
    #[arg(value_name = "LABEL_OR_GENERATION")]
    pub target: Option<String>,
    #[arg(last = true, value_name = "BACKEND_ARG", allow_hyphen_values = true)]
    pub backend_args: Vec<String>,
}

#[derive(Clone, Debug, Default, Args)]
pub struct GenerationsArgs {
    #[arg(long, value_name = "PROFILE")]
    pub profile: Option<String>,
    #[arg(last = true, value_name = "BACKEND_ARG", allow_hyphen_values = true)]
    pub backend_args: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum PublishMode {
    #[value(alias = "single")]
    Commit,
    PerFile,
}

#[derive(Clone, Debug, Default, Args)]
pub struct DiffArgs {
    #[arg(long, value_name = "GEN_OR_PATH")]
    pub from: Option<String>,
    #[arg(long, value_name = "PATH_OR_FLAKE")]
    pub to: Option<String>,
    #[arg(last = true, value_name = "BACKEND_ARG", allow_hyphen_values = true)]
    pub backend_args: Vec<String>,
}

#[derive(Clone, Debug, Args)]
pub struct GcArgs {
    #[arg(long, value_name = "AGE", default_value = "7d")]
    pub older_than: String,
    #[arg(long)]
    pub delete_old: bool,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Clone, Debug, Args)]
pub struct PinArgs {
    #[arg(value_name = "GENERATION")]
    pub generation: u64,
    #[arg(value_name = "LABEL")]
    pub label: String,
    #[arg(long)]
    pub force: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub struct PublishArgs {
    #[arg(short = 'm', long)]
    pub message: Option<String>,
    #[arg(long)]
    pub push: bool,
    #[arg(long, value_enum)]
    pub mode: Option<PublishMode>,
    #[arg(long, value_name = "REMOTE")]
    pub remote: Option<String>,
}

#[derive(Clone, Debug, Default, Args)]
pub struct CheckArgs {
    #[arg(long)]
    pub all: bool,
    #[arg(long)]
    pub nixfmt: bool,
    #[arg(long)]
    pub statix: bool,
    #[arg(long = "cargo-fmt")]
    pub cargo_fmt: bool,
    #[arg(long)]
    pub clippy: bool,
    #[arg(long = "no-flake")]
    pub no_flake: bool,
}

impl Cli {
    pub fn backend_options(&self, backend_args: &[String]) -> BackendOptions {
        BackendOptions {
            verbose: self.verbose,
            offline: self.offline,
            show_trace: self.show_trace,
            specialisation: self.specialisation.clone(),
            elevate: self.elevate.clone(),
            ask_elevate_password: self.ask_elevate_password,
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
        S: Into<OsString> + Clone,
    {
        <Self as Parser>::try_parse_from(args).map_err(|error| NrError::message(error.to_string()))
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
    let cli = match Cli::try_parse_from(env::args_os()) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            error
                .print()
                .with_context("failed to print command-line error")?;
            return Ok(code);
        }
    };
    let Some(command) = &cli.command else {
        print_help()?;
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
        NrCommand::Rollback(args) => crate::lifecycle::run_rollback(&cli, args),
        NrCommand::Generations(args) => crate::generations::run_generations(args),
        NrCommand::Diff(args) => crate::lifecycle::run_diff(&cli, args),
        NrCommand::Gc(args) => crate::lifecycle::run_gc(args),
        NrCommand::Pin(args) => crate::generations::run_pin(args),
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

fn print_help() -> Result<()> {
    let mut command = Cli::command();
    command
        .print_help()
        .with_context("failed to print command help")?;
    println!();
    Ok(())
}
