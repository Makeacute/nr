use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use clap::{ArgAction, Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};

use crate::VERSION;
use crate::backend::BackendOptions;
use crate::config::{ConfigInput, NrConfig, load_config};
use crate::errors::{IoContext, NrError, Result};
use crate::prompts::confirm;
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
        help = "Select command output mode; auto uses rich for interactive lifecycle builds"
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
    #[arg(
        long,
        global = true,
        value_name = "HOST",
        help = "Forward nixos-rebuild --target-host"
    )]
    pub target_host: Option<String>,
    #[arg(
        long,
        global = true,
        value_name = "HOST",
        help = "Forward nixos-rebuild --build-host"
    )]
    pub build_host: Option<String>,
    #[arg(long, global = true, help = "Forward nixos-rebuild --use-remote-sudo")]
    pub use_remote_sudo: bool,
    #[command(subcommand)]
    pub command: Option<NrCommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum NrCommand {
    #[command(about = "Build the selected host without activating it")]
    Build(Passthrough),
    #[command(about = "Build and activate the selected host")]
    Switch(LifecycleArgs),
    #[command(about = "Build and activate until the next reboot")]
    Test(LifecycleArgs),
    #[command(about = "Build and make the generation the next boot default")]
    Boot(LifecycleArgs),
    #[command(about = "Build, diff, and dry-activate without mutating")]
    Preview(Passthrough),
    #[command(about = "Binary-search Git history for a generation regression")]
    Bisect(BisectArgs),
    #[command(subcommand, about = "Run Home Manager lifecycle commands")]
    Hm(HmCommand),
    #[command(about = "Activate a saved preview plan without rebuilding")]
    Apply(ApplyArgs),
    #[command(about = "Update flake.lock or selected flake inputs")]
    Update(UpdateArgs),
    #[command(about = "Copy or print a privacy-aware system summary")]
    Share(ShareArgs),
    #[command(about = "Create a portable archive of the flake")]
    Export(ExportArgs),
    #[command(about = "Render the flake directory as a tree")]
    Tree(TreeArgs),
    #[command(about = "Search Nix files in the flake")]
    Find(FindArgs),
    #[command(about = "Open a Nix file from the flake in $EDITOR")]
    Open(OpenArgs),
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
    #[command(about = "Remove a generation pin and its GC root")]
    Unpin(UnpinArgs),
    #[command(about = "List pinned generations and stale pin state")]
    Pins(PinsArgs),
    #[command(about = "Show recorded switch, test, and boot history")]
    History(HistoryArgs),
    #[command(about = "List retained logs and reports")]
    Logs(LogsArgs),
    #[command(about = "Print a retained lifecycle report")]
    ShowReport(ShowReportArgs),
    #[command(about = "Inspect or update flake inputs")]
    Inputs(InputsArgs),
    #[command(about = "Create a starter repo or user nr config")]
    InitConfig(InitConfigArgs),
    #[command(about = "Generate shell completions")]
    Completions(CompletionArgs),
    #[command(about = "Review, commit, and optionally push")]
    Publish(PublishArgs),
    #[command(about = "Run configured checks")]
    Check(CheckArgs),
    #[command(about = "Run nixfmt and deadnix on changed Nix files")]
    Lint(LintArgs),
    #[command(about = "Create, list, or restore flake snapshots")]
    Snapshot(SnapshotArgs),
    #[command(about = "Show merged config settings and their source")]
    ConfigCheck(ConfigCheckArgs),
    #[command(about = "Show target, config, dependency, and Git diagnostics")]
    Doctor,
    #[command(about = "Show the complete terminal cheat sheet")]
    Cheat,
}

#[derive(Clone, Debug, Default, Args)]
pub struct Passthrough {
    #[arg(
        last = true,
        value_name = "BACKEND_ARG",
        allow_hyphen_values = true,
        help = "Argument passed through after --"
    )]
    pub backend_args: Vec<String>,
}

#[derive(Clone, Debug, Default, Args)]
pub struct LifecycleArgs {
    #[arg(
        long,
        value_name = "PLAN",
        help = "Activate a saved preview plan instead of rebuilding"
    )]
    pub from_plan: Option<String>,
    #[arg(
        last = true,
        value_name = "BACKEND_ARG",
        allow_hyphen_values = true,
        help = "Argument passed through after --"
    )]
    pub backend_args: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum BisectProbe {
    Build,
    #[default]
    Preview,
    Check,
}

#[derive(Clone, Debug, Default, Args)]
pub struct BisectArgs {
    #[arg(
        value_name = "BAD_GENERATION_OR_REV",
        default_value = "current",
        help = "Known-bad generation, pin, or Git revision"
    )]
    pub broken: String,
    #[arg(
        long,
        value_name = "GENERATION_OR_REV",
        help = "Known-good generation, pin, or Git revision; defaults to the previous generation"
    )]
    pub good: Option<String>,
    #[arg(
        long,
        value_name = "REV",
        help = "Known-bad Git revision; overrides BAD_GENERATION_OR_REV revision lookup"
    )]
    pub bad: Option<String>,
    #[arg(
        long,
        value_enum,
        default_value = "preview",
        help = "Built-in probe to run at each commit when no custom command is provided"
    )]
    pub probe: BisectProbe,
    #[arg(long, help = "Allow bisecting with local Git changes present")]
    pub allow_dirty: bool,
    #[arg(long, help = "Leave the repository in Git bisect state when finished")]
    pub no_reset: bool,
    #[arg(
        last = true,
        value_name = "COMMAND",
        allow_hyphen_values = true,
        help = "Custom git bisect run command passed after --"
    )]
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum HmCommand {
    #[command(about = "Build the selected Home Manager configuration")]
    Build(HmLifecycleArgs),
    #[command(about = "Build and activate the selected Home Manager configuration")]
    Switch(HmLifecycleArgs),
    #[command(about = "Build and diff Home Manager without activating")]
    Preview(HmLifecycleArgs),
    #[command(about = "Show Home Manager generations")]
    Generations(HmGenerationsArgs),
}

#[derive(Clone, Debug, Default, Args)]
pub struct HmLifecycleArgs {
    #[arg(
        long,
        value_name = "NAME",
        help = "Home Manager flake output name; defaults to --flake #fragment, --host, or $USER"
    )]
    pub home: Option<String>,
    #[arg(
        long,
        value_name = "PROFILE",
        help = "Current Home Manager profile to diff for preview"
    )]
    pub profile: Option<PathBuf>,
    #[arg(
        last = true,
        value_name = "BACKEND_ARG",
        allow_hyphen_values = true,
        help = "Argument passed through after --"
    )]
    pub backend_args: Vec<String>,
}

#[derive(Clone, Debug, Default, Args)]
pub struct HmGenerationsArgs {
    #[arg(
        last = true,
        value_name = "BACKEND_ARG",
        allow_hyphen_values = true,
        help = "Argument passed through after --"
    )]
    pub backend_args: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ApplyAction {
    #[default]
    Switch,
    Test,
    Boot,
}

impl ApplyAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Switch => "switch",
            Self::Test => "test",
            Self::Boot => "boot",
        }
    }
}

#[derive(Clone, Debug, Default, Args)]
pub struct ApplyArgs {
    #[arg(
        value_name = "PLAN",
        default_value = "latest",
        help = "Preview plan to activate, or latest"
    )]
    pub plan: String,
    #[arg(
        long,
        value_enum,
        default_value = "switch",
        help = "Activation action to run from the saved plan"
    )]
    pub action: ApplyAction,
    #[arg(
        last = true,
        value_name = "BACKEND_ARG",
        allow_hyphen_values = true,
        help = "Argument passed through after --"
    )]
    pub backend_args: Vec<String>,
}

#[derive(Clone, Debug, Default, Args)]
pub struct UpdateArgs {
    #[arg(value_name = "INPUT", help = "Flake input to update")]
    pub inputs: Vec<String>,
    #[arg(long, help = "Build and activate after updating flake.lock")]
    pub switch: bool,
    #[arg(long, help = "Restore flake.lock if the post-update switch fails")]
    pub revert_on_failure: bool,
    #[arg(long, help = "Update, diff, then ask before switching")]
    pub preview: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ShareFormat {
    Text,
    #[default]
    Markdown,
}

#[derive(Clone, Debug, Default, Args)]
pub struct ShareArgs {
    #[arg(long, help = "Print instead of copying to the clipboard")]
    pub no_clipboard: bool,
    #[arg(long, value_enum, default_value = "markdown", help = "Summary format")]
    pub format: ShareFormat,
}

#[derive(Clone, Debug, Args)]
pub struct ExportArgs {
    #[arg(
        long,
        value_name = "PATH",
        help = "Archive path to write; defaults to ./nr-export-<timestamp>.tar.gz"
    )]
    pub output: Option<PathBuf>,
    #[arg(long, help = "Include files that look like secrets")]
    pub include_secrets: bool,
    #[arg(long, help = "List files without creating the archive")]
    pub dry_run: bool,
}

#[derive(Clone, Debug, Args)]
pub struct TreeArgs {
    #[arg(long, help = "Maximum directory depth to render")]
    pub depth: Option<usize>,
    #[arg(long, hide = true, help = "Show files as well as directories")]
    pub files: bool,
    #[arg(long, help = "Show only directories")]
    pub dirs_only: bool,
    #[arg(long, help = "Show file sizes")]
    pub sizes: bool,
    #[arg(long, help = "Disable colored output")]
    pub no_color: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum FindType {
    #[default]
    AutoDetect,
    Package,
    Option,
    String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum FindFormat {
    #[default]
    Text,
    Json,
}

#[derive(Clone, Debug, Args)]
pub struct FindArgs {
    #[arg(value_name = "QUERY", help = "Search query")]
    pub query: String,
    #[arg(
        long = "type",
        value_enum,
        default_value = "auto-detect",
        help = "Search type hint"
    )]
    pub search_type: FindType,
    #[arg(long, help = "Match case exactly")]
    pub case_sensitive: bool,
    #[arg(long, default_value_t = 2, help = "Context lines around each match")]
    pub context: usize,
    #[arg(long, value_enum, default_value = "text", help = "Output format")]
    pub format: FindFormat,
    #[arg(long, help = "Disable colored output")]
    pub no_color: bool,
}

#[derive(Clone, Debug, Args)]
pub struct OpenArgs {
    #[arg(
        value_name = "FILE",
        help = "Relative .nix file path or filename fragment"
    )]
    pub file: String,
}

#[derive(Clone, Debug, Default, Args)]
pub struct RollbackArgs {
    #[arg(
        value_name = "LABEL_OR_GENERATION",
        help = "Pin label or generation number to activate"
    )]
    pub target: Option<String>,
    #[arg(
        last = true,
        value_name = "BACKEND_ARG",
        allow_hyphen_values = true,
        help = "Argument passed through after --"
    )]
    pub backend_args: Vec<String>,
}

#[derive(Clone, Debug, Default, Args)]
pub struct GenerationsArgs {
    #[arg(
        long,
        value_name = "PROFILE",
        help = "Nix profile path to inspect instead of the system profile"
    )]
    pub profile: Option<String>,
    #[arg(long, help = "Disable colored output")]
    pub no_color: bool,
    #[arg(
        last = true,
        value_name = "BACKEND_ARG",
        allow_hyphen_values = true,
        help = "Argument passed through after --"
    )]
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
    #[arg(
        long,
        value_name = "GEN_OR_PATH",
        help = "Generation number or store path to compare from"
    )]
    pub from: Option<String>,
    #[arg(
        long,
        value_name = "PATH_OR_FLAKE",
        help = "Store path or flake reference to compare to"
    )]
    pub to: Option<String>,
    #[arg(
        last = true,
        value_name = "BACKEND_ARG",
        allow_hyphen_values = true,
        help = "Argument passed through after --"
    )]
    pub backend_args: Vec<String>,
}

#[derive(Clone, Debug, Args)]
pub struct GcArgs {
    #[arg(
        long,
        value_name = "AGE",
        default_value = "7d",
        help = "Minimum age for generations considered by garbage collection"
    )]
    pub older_than: String,
    #[arg(long, help = "Delete all old generations with nix-collect-garbage -d")]
    pub delete_old: bool,
    #[arg(long, help = "Preview collection with nix-collect-garbage --dry-run")]
    pub dry_run: bool,
}

#[derive(Clone, Debug, Args)]
pub struct PinArgs {
    #[arg(value_name = "GENERATION", help = "Generation number to label")]
    pub generation: u64,
    #[arg(value_name = "LABEL", help = "Label to use for later rollback")]
    pub label: String,
    #[arg(long, help = "Replace an existing pin with the same label")]
    pub force: bool,
    #[arg(long, help = "Record the pin without creating a Nix GC root")]
    pub no_gc_root: bool,
}

#[derive(Clone, Debug, Args)]
pub struct UnpinArgs {
    #[arg(value_name = "LABEL", help = "Pin label to remove")]
    pub label: String,
}

#[derive(Clone, Debug, Default, Args)]
pub struct PinsArgs {
    #[arg(long, help = "Check whether pinned generations or GC roots are stale")]
    pub check_stale: bool,
}

#[derive(Clone, Debug, Args)]
pub struct HistoryArgs {
    #[arg(long, default_value_t = 20, help = "Maximum history entries to print")]
    pub limit: usize,
    #[arg(long, value_enum, default_value = "text", help = "Output format")]
    pub format: HistoryFormat,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum HistoryFormat {
    #[default]
    Text,
    Json,
}

#[derive(Clone, Debug, Args)]
pub struct LogsArgs {
    #[arg(long, default_value_t = 20, help = "Maximum retained items to print")]
    pub limit: usize,
    #[arg(long, help = "Print the log path for the latest failed report")]
    pub last_failed: bool,
}

#[derive(Clone, Debug, Args)]
pub struct ShowReportArgs {
    #[arg(
        value_name = "REPORT",
        default_value = "latest",
        help = "Report path, latest, or a filename fragment"
    )]
    pub report: String,
}

#[derive(Clone, Debug, Default, Args)]
pub struct InputsArgs {
    #[arg(long, help = "Print flake input data as JSON")]
    pub json: bool,
    #[arg(
        long,
        value_name = "INPUT",
        help = "Update one flake input before listing"
    )]
    pub update: Vec<String>,
}

#[derive(Clone, Debug, Args)]
pub struct InitConfigArgs {
    #[arg(long, help = "Write the starter config to the user config directory")]
    pub user: bool,
    #[arg(long, help = "Overwrite an existing config file")]
    pub force: bool,
}

#[derive(Clone, Debug, Args)]
pub struct CompletionArgs {
    #[arg(value_enum, help = "Shell to generate completions for")]
    pub shell: Shell,
}

#[derive(Clone, Debug, Default, Args)]
pub struct PublishArgs {
    #[arg(short = 'm', long, help = "Commit message to use")]
    pub message: Option<String>,
    #[arg(long, help = "Push after committing")]
    pub push: bool,
    #[arg(long, value_enum, help = "Commit strategy")]
    pub mode: Option<PublishMode>,
    #[arg(long, value_name = "REMOTE", help = "Git remote to push to")]
    pub remote: Option<String>,
}

#[derive(Clone, Debug, Default, Args)]
pub struct CheckArgs {
    #[arg(long, help = "Run every configured check")]
    pub all: bool,
    #[arg(long, help = "Run nixfmt")]
    pub nixfmt: bool,
    #[arg(long, help = "Run statix")]
    pub statix: bool,
    #[arg(long = "cargo-fmt", help = "Run cargo fmt --check")]
    pub cargo_fmt: bool,
    #[arg(long, help = "Run cargo clippy with warnings denied")]
    pub clippy: bool,
    #[arg(long = "no-flake", help = "Skip flake checks")]
    pub no_flake: bool,
    #[arg(long, help = "Print check results as JSON")]
    pub json: bool,
    #[arg(
        long,
        value_name = "NAME",
        help = "Run checks whose name or command contains NAME"
    )]
    pub only: Vec<String>,
    #[arg(long, value_name = "SECONDS", help = "Per-check timeout in seconds")]
    pub timeout: Option<u64>,
}

#[derive(Clone, Debug, Default, Args)]
pub struct LintArgs {}

#[derive(Clone, Debug, Default, Args)]
pub struct SnapshotArgs {
    #[arg(long, value_name = "LABEL", help = "Create a snapshot with this name")]
    pub name: Option<String>,
    #[arg(long, help = "List snapshots")]
    pub list: bool,
    #[arg(long, value_name = "LABEL", help = "Restore a named snapshot")]
    pub restore: Option<String>,
}

#[derive(Clone, Debug, Default, Args)]
pub struct ConfigCheckArgs {}

impl Cli {
    pub fn backend_options(&self, backend_args: &[String]) -> BackendOptions {
        BackendOptions {
            verbose: self.verbose,
            offline: self.offline,
            show_trace: self.show_trace,
            specialisation: self.specialisation.clone(),
            elevate: self.elevate.clone(),
            ask_elevate_password: self.ask_elevate_password,
            target_host: self.target_host.clone(),
            build_host: self.build_host.clone(),
            use_remote_sudo: self.use_remote_sudo,
            backend_args: backend_args.to_vec(),
        }
    }

    pub fn backend_options_with_config(
        &self,
        config: &NrConfig,
        backend_args: &[String],
    ) -> BackendOptions {
        let mut options = self.backend_options(backend_args);
        if options.target_host.is_none() {
            options.target_host = config.remote.target_host.clone();
        }
        if options.build_host.is_none() {
            options.build_host = config.remote.build_host.clone();
        }
        options.use_remote_sudo |= config.remote.use_remote_sudo;
        options
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
        NrCommand::Switch(args) => crate::lifecycle::run_lifecycle_command("switch", &cli, args),
        NrCommand::Test(args) => crate::lifecycle::run_lifecycle_command("test", &cli, args),
        NrCommand::Boot(args) => crate::lifecycle::run_lifecycle_command("boot", &cli, args),
        NrCommand::Preview(args) => {
            crate::lifecycle::run_lifecycle("preview", &cli, &args.backend_args)
        }
        NrCommand::Bisect(args) => {
            let config = load_config(cli.config_input())?;
            crate::bisect::run_bisect(&cli, &config, args)
        }
        NrCommand::Hm(command) => {
            let config = load_config(cli.config_input())?;
            crate::hm::run_hm(&cli, &config, command)
        }
        NrCommand::Apply(args) => crate::lifecycle::run_apply(&cli, args),
        NrCommand::Update(args) => {
            let config = load_config(cli.config_input())?;
            crate::lifecycle::run_update(&cli, &config, args)
        }
        NrCommand::Share(args) => {
            let config = load_config(cli.config_input())?;
            crate::share::run_share(&cli, &config, args)
        }
        NrCommand::Export(args) => {
            let config = load_config(cli.config_input())?;
            crate::export::run_export(&config, args)
        }
        NrCommand::Tree(args) => {
            let config = load_config(cli.config_input())?;
            crate::tree::run_tree(&config, args)
        }
        NrCommand::Find(args) => {
            let config = load_config(cli.config_input())?;
            crate::find::run_find(&config, args)
        }
        NrCommand::Open(args) => {
            let config = load_config(cli.config_input())?;
            crate::open::run_open(&config, args)
        }
        NrCommand::Rollback(args) => crate::lifecycle::run_rollback(&cli, args),
        NrCommand::Generations(args) => crate::generations::run_generations(args),
        NrCommand::Diff(args) => crate::lifecycle::run_diff(&cli, args),
        NrCommand::Gc(args) => crate::lifecycle::run_gc(args),
        NrCommand::Pin(args) => crate::generations::run_pin(args),
        NrCommand::Unpin(args) => crate::generations::run_unpin(args),
        NrCommand::Pins(args) => crate::generations::run_pins(args),
        NrCommand::History(args) => crate::lifecycle::run_history(args),
        NrCommand::Logs(args) => crate::lifecycle::run_logs(args),
        NrCommand::ShowReport(args) => crate::lifecycle::run_show_report(args),
        NrCommand::Inputs(args) => {
            let config = load_config(cli.config_input())?;
            crate::inputs::run_inputs(&cli, &config, args)
        }
        NrCommand::InitConfig(args) => crate::config::run_init_config(&cli.config_input(), args),
        NrCommand::Completions(args) => crate::cli::run_completions(args),
        NrCommand::Publish(args) => {
            let config = load_config(cli.config_input())?;
            crate::publish::run_publish(&config, args)
        }
        NrCommand::Check(args) => {
            let config = load_config(cli.config_input())?;
            crate::checks::run_check(&cli, &config, args)
        }
        NrCommand::Lint(args) => {
            let config = load_config(cli.config_input())?;
            crate::lint::run_lint(&config, args)
        }
        NrCommand::Snapshot(args) => {
            let config = load_config(cli.config_input())?;
            crate::snapshot::run_snapshot(&cli, &config, args)
        }
        NrCommand::ConfigCheck(args) => {
            let config = load_config(cli.config_input())?;
            crate::config::run_config_check(&config, args)
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

pub fn run_completions(args: &CompletionArgs) -> Result<i32> {
    let mut command = Cli::command();
    let mut output = Vec::new();
    generate(args.shell, &mut command, "nr", &mut output);

    if let Some(detected) = detected_shell()
        && detected != args.shell
    {
        eprintln!("detected shell differs from requested completions; printing to stdout instead");
        print_completion_bytes(&output)?;
        return Ok(0);
    }

    let Some(path) = completion_install_path(args.shell) else {
        print_completion_bytes(&output)?;
        return Ok(0);
    };

    if !io::stdin().is_terminal() {
        print_completion_bytes(&output)?;
        return Ok(0);
    }

    println!("completion path: {}", path.display());
    if confirm("Write completions here?", true) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(format!("failed to create {}", parent.display()))?;
        }
        fs::write(&path, output).with_context(format!("failed to write {}", path.display()))?;
        println!("wrote {}", path.display());
        print_reload_instructions(args.shell);
    } else {
        print_completion_bytes(&output)?;
    }
    Ok(0)
}

fn print_completion_bytes(output: &[u8]) -> Result<()> {
    let text = String::from_utf8(output.to_vec()).map_err(|error| {
        NrError::message(format!("generated completions were not UTF-8: {error}"))
    })?;
    print!("{text}");
    Ok(())
}

fn completion_install_path(shell: Shell) -> Option<PathBuf> {
    match shell {
        Shell::Fish => Some(home_dir().join(".config/fish/completions/nr.fish")),
        Shell::Bash => Some(home_dir().join(".local/share/bash-completion/completions/nr")),
        Shell::Zsh => zsh_completion_path(),
        _ => None,
    }
}

fn detected_shell() -> Option<Shell> {
    let shell = env::var_os("SHELL")?;
    let name = Path::new(&shell).file_name()?.to_str()?;
    match name {
        "fish" => Some(Shell::Fish),
        "bash" => Some(Shell::Bash),
        "zsh" => Some(Shell::Zsh),
        _ => None,
    }
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn zsh_completion_path() -> Option<PathBuf> {
    if let Some(fpath) = env::var_os("fpath").or_else(|| env::var_os("FPATH")) {
        for path in env::split_paths(&fpath) {
            if writable_directory(&path) {
                return Some(path.join("nr"));
            }
        }
    }
    Some(home_dir().join(".zsh/completions/nr"))
}

fn writable_directory(path: &Path) -> bool {
    path.is_dir()
        && path
            .metadata()
            .map(|metadata| !metadata.permissions().readonly())
            .unwrap_or(false)
}

fn print_reload_instructions(shell: Shell) {
    match shell {
        Shell::Fish => println!("Reload with: exec fish"),
        Shell::Bash => println!("Reload with: source ~/.bashrc or start a new shell"),
        Shell::Zsh => println!("Reload with: exec zsh"),
        _ => println!("Start a new shell to load completions."),
    }
}
