use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::config::FlakeTarget;
use crate::events::{BuildState, NixEvent};
use crate::git::GitSummary;
use crate::impact::{ActivationImpact, ClosureDiff, GenerationInfo};
use crate::process::{StreamLine, StreamSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMode {
    Auto,
    Rich,
    Plain,
    Raw,
    Json,
}

impl OutputMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "rich" => Some(Self::Rich),
            "plain" => Some(Self::Plain),
            "raw" => Some(Self::Raw),
            "json" => Some(Self::Json),
            _ => None,
        }
    }

    pub fn effective(self) -> Self {
        match self {
            Self::Auto if io::stdout().is_terminal() && io::stderr().is_terminal() => Self::Rich,
            Self::Auto => Self::Plain,
            value => value,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RebuildHeader {
    pub command: String,
    pub target: FlakeTarget,
    pub git: GitSummary,
    pub current: GenerationInfo,
    pub log_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct RebuildReport {
    pub command: String,
    pub target: FlakeTarget,
    pub result: String,
    pub store_path: Option<PathBuf>,
    pub current: GenerationInfo,
    pub new_generation: Option<u64>,
    pub build: BuildState,
    pub diff: Option<ClosureDiff>,
    pub activation: Option<ActivationImpact>,
    pub reboot: String,
    pub rollback: String,
    pub log_path: PathBuf,
}

pub struct Renderer {
    mode: OutputMode,
    last_rich_render: Instant,
}

impl Renderer {
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode: mode.effective(),
            last_rich_render: Instant::now() - Duration::from_secs(1),
        }
    }

    pub fn start(&mut self, header: &RebuildHeader) {
        match self.mode {
            OutputMode::Json => {}
            OutputMode::Raw => {
                eprintln!("nr {} {}", header.command, header.target.reference());
            }
            OutputMode::Plain => {
                println!("nr {} {}", header.command, header.target.reference());
                print_header_details(header);
            }
            OutputMode::Rich | OutputMode::Auto => {
                println!(
                    "\x1b[1;36mnr {}\x1b[0m {}",
                    header.command,
                    header.target.reference()
                );
                print_header_details(header);
            }
        }
    }

    pub fn phase(&mut self, phase: &str) {
        match self.mode {
            OutputMode::Json | OutputMode::Raw => {}
            OutputMode::Plain => println!("phase: {phase}"),
            OutputMode::Rich | OutputMode::Auto => println!("\x1b[1;34m{phase}\x1b[0m"),
        }
    }

    pub fn nix_event(&mut self, _event: &NixEvent, state: &BuildState) {
        match self.mode {
            OutputMode::Rich | OutputMode::Auto => self.render_rich_state(state),
            OutputMode::Plain => {
                if let Some(activity) = state.slowest_active() {
                    println!(
                        "building: {} [{}]",
                        activity.text,
                        activity.category.label()
                    );
                }
            }
            OutputMode::Raw | OutputMode::Json => {}
        }
    }

    pub fn backend_line(&mut self, line: &StreamLine) {
        match self.mode {
            OutputMode::Raw => match line.source {
                StreamSource::Stdout => println!("{}", line.line),
                StreamSource::Stderr => eprintln!("{}", line.line),
            },
            OutputMode::Rich | OutputMode::Plain | OutputMode::Auto => {
                let lower = line.line.to_lowercase();
                if lower.contains("warning") || lower.contains("error") || lower.contains("failed")
                {
                    eprintln!("{}", line.line);
                }
            }
            OutputMode::Json => {}
        }
    }

    pub fn parser_fallback(&mut self) {
        if self.mode != OutputMode::Json {
            eprintln!("Nix JSON output changed; falling back to plain log streaming.");
        }
    }

    pub fn diff(&mut self, diff: &ClosureDiff) {
        match self.mode {
            OutputMode::Json => {}
            OutputMode::Raw => {
                if !diff.raw.is_empty() {
                    print!("{}", diff.raw);
                }
            }
            _ => print_diff_summary(diff),
        }
    }

    pub fn activation(&mut self, activation: &ActivationImpact) {
        match self.mode {
            OutputMode::Json => {}
            OutputMode::Raw => {
                if !activation.raw.is_empty() {
                    print!("{}", activation.raw);
                }
            }
            _ => print_activation_summary(activation),
        }
    }

    pub fn finish(&mut self, report: &RebuildReport) {
        match self.mode {
            OutputMode::Json => println!("{}", report_json(report)),
            OutputMode::Raw => {}
            OutputMode::Plain | OutputMode::Rich | OutputMode::Auto => {
                if matches!(self.mode, OutputMode::Rich | OutputMode::Auto) {
                    print!("\r\x1b[2K");
                }
                println!("result: {}", report.result);
                if let Some(path) = &report.store_path {
                    println!("store path: {}", path.display());
                }
                if let Some(generation) = report.new_generation {
                    println!("generation: {generation}");
                }
                println!("reboot: {}", report.reboot);
                println!("rollback: {}", report.rollback);
                println!("log: {}", report.log_path.display());
            }
        }
    }

    fn render_rich_state(&mut self, state: &BuildState) {
        if self.last_rich_render.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.last_rich_render = Instant::now();
        let active = state.running.len();
        let phase = if state.phase.is_empty() {
            "building"
        } else {
            &state.phase
        };
        let categories = state
            .running_by_category()
            .into_iter()
            .map(|(category, count)| format!("{}:{count}", category.label()))
            .collect::<Vec<_>>()
            .join(" ");
        let slowest = state
            .slowest_active()
            .map(|activity| activity.text.as_str())
            .unwrap_or("waiting for Nix output");
        print!(
            "\r\x1b[2K\x1b[1;32m{phase}\x1b[0m active:{active} done:{} failed:{} downloads:{} {} | {}",
            state.completed, state.failed, state.downloads, categories, slowest
        );
        let _ = io::stdout().flush();
    }
}

fn print_header_details(header: &RebuildHeader) {
    if header.git.repository {
        let branch = header.git.branch.as_deref().unwrap_or("detached");
        let dirty = if header.git.dirty { "dirty" } else { "clean" };
        println!(
            "git: {branch} ({dirty}, {} untracked)",
            header.git.untracked
        );
    } else {
        println!("git: not a repository");
    }
    if let Some(generation) = header.current.generation {
        println!("current generation: {generation}");
    }
    if let Some(version) = &header.current.nixos_version {
        println!("current NixOS: {version}");
    }
    if let Some(kernel) = &header.current.kernel_version {
        println!("current kernel: {kernel}");
    }
    println!("log: {}", header.log_path.display());
}

fn print_diff_summary(diff: &ClosureDiff) {
    println!("changes:");
    if let Some(reason) = &diff.unavailable {
        println!("  unavailable: {reason}");
        return;
    }
    if !diff.additions.is_empty() {
        println!("  additions: {}", diff.additions.len());
    }
    if !diff.removals.is_empty() {
        println!("  removals: {}", diff.removals.len());
    }
    if !diff.upgrades.is_empty() {
        println!("  upgrades: {}", diff.upgrades.len());
    }
    if !diff.downgrades.is_empty() {
        println!("  downgrades: {}", diff.downgrades.len());
    }
    if !diff.changes.is_empty() {
        println!("  other changes: {}", diff.changes.len());
    }
    if diff.important.is_empty() {
        println!("  important: none detected");
    } else {
        println!("  important:");
        for item in diff.important.iter().take(12) {
            println!("    {item}");
        }
    }
    if let Some(size) = &diff.size_delta {
        println!("  {size}");
    }
    if !diff.changed() && diff.size_delta.is_none() {
        println!("  no package changes detected");
    }
}

fn print_activation_summary(impact: &ActivationImpact) {
    println!("activation impact:");
    print_units("stopped", &impact.stopped);
    print_units("started", &impact.started);
    print_units("restarted", &impact.restarted);
    print_units("reloaded", &impact.reloaded);
    print_units("skipped", &impact.skipped);
    print_units("failed", &impact.failed);
    if impact.caveats.is_empty() {
        println!("  caveats: none");
    } else {
        println!("  caveats:");
        for caveat in impact.caveats.iter().take(8) {
            println!("    {caveat}");
        }
    }
}

fn print_units(label: &str, values: &[String]) {
    if values.is_empty() {
        println!("  {label}: none");
    } else {
        println!("  {label}: {}", values.join(", "));
    }
}

fn report_json(report: &RebuildReport) -> String {
    let mut fields = Vec::new();
    fields.push(json_field("command", &report.command));
    fields.push(json_field("target", &report.target.reference()));
    fields.push(json_field("result", &report.result));
    if let Some(path) = &report.store_path {
        fields.push(json_field("store_path", &path.display().to_string()));
    }
    fields.push(json_number_or_null(
        "current_generation",
        report.current.generation,
    ));
    fields.push(json_number_or_null("new_generation", report.new_generation));
    fields.push(json_field("reboot", &report.reboot));
    fields.push(json_field("rollback", &report.rollback));
    fields.push(json_field(
        "log_path",
        &report.log_path.display().to_string(),
    ));
    fields.push(format!(
        "\"build\":{{\"completed\":{},\"failed\":{},\"running\":{},\"downloads\":{},\"source_builds\":{},\"binary_substitutes\":{},\"parser_fallback\":{}}}",
        report.build.completed,
        report.build.failed,
        report.build.running.len(),
        report.build.downloads,
        report.build.source_builds,
        report.build.binary_substitutes,
        report.build.parser_fallback
    ));
    if let Some(diff) = &report.diff {
        fields.push(format!(
            "\"diff\":{{\"additions\":{},\"removals\":{},\"upgrades\":{},\"downgrades\":{},\"important\":{}}}",
            diff.additions.len(),
            diff.removals.len(),
            diff.upgrades.len(),
            diff.downgrades.len(),
            json_string_array(&diff.important)
        ));
    }
    if let Some(activation) = &report.activation {
        fields.push(format!(
            "\"activation\":{{\"stopped\":{},\"started\":{},\"restarted\":{},\"reloaded\":{},\"skipped\":{},\"failed\":{}}}",
            json_string_array(&activation.stopped),
            json_string_array(&activation.started),
            json_string_array(&activation.restarted),
            json_string_array(&activation.reloaded),
            json_string_array(&activation.skipped),
            json_string_array(&activation.failed)
        ));
    }
    format!("{{{}}}", fields.join(","))
}

fn json_field(key: &str, value: &str) -> String {
    format!("\"{key}\":\"{}\"", json_escape(value))
}

fn json_number_or_null(key: &str, value: Option<u64>) -> String {
    match value {
        Some(value) => format!("\"{key}\":{value}"),
        None => format!("\"{key}\":null"),
    }
}

fn json_string_array(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| format!("\"{}\"", json_escape(value)))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn json_escape(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            other => output.push(other),
        }
    }
    output
}
