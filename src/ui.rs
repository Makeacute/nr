use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::config::FlakeTarget;
use crate::events::{Activity, BuildState, NixEvent};
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
    last_rich_lines: usize,
}

impl Renderer {
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode: mode.effective(),
            last_rich_render: Instant::now() - Duration::from_secs(1),
            last_rich_lines: 0,
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
            OutputMode::Rich | OutputMode::Auto => {
                self.clear_rich_block();
                println!("\x1b[1;34m{phase}\x1b[0m");
            }
        }
    }

    pub fn nix_event(&mut self, _event: &NixEvent, state: &BuildState) {
        match self.mode {
            OutputMode::Rich | OutputMode::Auto => self.render_rich_state(state),
            OutputMode::Plain => {}
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
                if should_print_backend_line(&line.line) {
                    if matches!(self.mode, OutputMode::Rich | OutputMode::Auto) {
                        self.clear_rich_block();
                    }
                    eprintln!("{}", line.line);
                }
            }
            OutputMode::Json => {}
        }
    }

    pub fn parser_fallback(&mut self) {
        if self.mode != OutputMode::Json {
            if matches!(self.mode, OutputMode::Rich | OutputMode::Auto) {
                self.clear_rich_block();
            }
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
            _ => {
                if matches!(self.mode, OutputMode::Rich | OutputMode::Auto) {
                    self.clear_rich_block();
                }
                print_diff_summary(diff);
            }
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
            _ => {
                if matches!(self.mode, OutputMode::Rich | OutputMode::Auto) {
                    self.clear_rich_block();
                }
                print_activation_summary(activation);
            }
        }
    }

    pub fn finish(&mut self, report: &RebuildReport) {
        match self.mode {
            OutputMode::Json => println!("{}", report_json(report)),
            OutputMode::Raw => {}
            OutputMode::Plain | OutputMode::Rich | OutputMode::Auto => {
                if matches!(self.mode, OutputMode::Rich | OutputMode::Auto) {
                    self.clear_rich_block();
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
        if self.last_rich_render.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.last_rich_render = Instant::now();
        let lines = build_graph_lines(state, terminal_width());
        self.clear_rich_block();
        for line in &lines {
            println!("{line}");
        }
        self.last_rich_lines = lines.len();
        let _ = io::stdout().flush();
    }

    fn clear_rich_block(&mut self) {
        if self.last_rich_lines == 0 {
            return;
        }
        for _ in 0..self.last_rich_lines {
            print!("\x1b[1A\r\x1b[2K");
        }
        self.last_rich_lines = 0;
        let _ = io::stdout().flush();
    }
}

fn build_graph_lines(state: &BuildState, width: usize) -> Vec<String> {
    let width = width.clamp(48, 180);
    let phase = if state.phase.is_empty() {
        "building"
    } else {
        &state.phase
    };
    let mut lines = Vec::new();
    lines.push(format!(
        "build graph  phase:{phase} active:{} done:{} failed:{} downloads:{} source:{} cache:{}",
        state.running.len(),
        state.completed,
        state.failed,
        state.downloads,
        state.source_builds,
        state.binary_substitutes
    ));

    let categories = state
        .running_by_category()
        .into_iter()
        .map(|(category, count)| format!("{}:{count}", category.label()))
        .collect::<Vec<_>>()
        .join(" ");
    lines.push(format!(
        "categories: {}",
        if categories.is_empty() {
            "none".to_string()
        } else {
            categories
        }
    ));
    lines.push("system closure".to_string());

    let active = state.running.values().collect::<Vec<_>>();
    let shown = active.len().min(6);
    for (index, activity) in active.iter().take(shown).enumerate() {
        let has_more = active.len() > shown;
        let is_last = index + 1 == shown && !has_more;
        let connector = if is_last { "`--" } else { "|--" };
        lines.push(activity_line(connector, activity));
        if index < 3
            && let Some(why) = state.why_building(activity)
        {
            let prefix = if is_last { "    " } else { "|   " };
            lines.push(format!("{prefix}why: {}", compact_activity_text(&why)));
        }
    }

    if active.is_empty() {
        lines.push("`-- waiting for Nix activity".to_string());
    } else if active.len() > shown {
        lines.push(format!("`-- ... {} more active", active.len() - shown));
    }

    if let Some(activity) = state.slowest_active() {
        lines.push(format!(
            "slowest: {} [{}]",
            compact_activity_text(&activity.text),
            activity.category.label()
        ));
    }
    if !state.errors.is_empty() {
        lines.push(format!("errors: {}", state.errors.len()));
    }
    if !state.warnings.is_empty() {
        lines.push(format!("warnings: {}", state.warnings.len()));
    }

    lines
        .into_iter()
        .map(|line| truncate_line(&line, width))
        .collect()
}

fn activity_line(connector: &str, activity: &Activity) -> String {
    format!(
        "{connector} {} {} [{}]",
        activity_kind(activity),
        compact_activity_text(&activity.text),
        activity.category.label()
    )
}

fn activity_kind(activity: &Activity) -> &'static str {
    let lower = activity.text.to_lowercase();
    if activity.substitute {
        "fetch"
    } else if lower.contains("evaluat") {
        "eval"
    } else if activity.source_build {
        "build"
    } else {
        "wait"
    }
}

fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| *width >= 40)
        .unwrap_or(100)
}

fn compact_activity_text(text: &str) -> String {
    let trimmed = text.trim().trim_matches('\'').trim_matches('"');
    if let Some(store_name) = store_name(trimmed) {
        return store_name;
    }
    if let Some(config_index) = trimmed.find("nixosConfigurations.") {
        let mut value = trimmed[config_index..]
            .trim_matches('\'')
            .trim_matches('"')
            .replace("nixosConfigurations.", "");
        value = value.replace(".config.system.build.", ".");
        value = value.replace("\".", ".");
        value = value.replace(".\"", ".");
        return value;
    }
    trimmed.replace("git+file://", "")
}

fn store_name(text: &str) -> Option<String> {
    let marker = "/nix/store/";
    let index = text.find(marker)?;
    let rest = &text[index + marker.len()..];
    let end = rest
        .find(|character: char| character.is_whitespace() || matches!(character, '\'' | '"'))
        .unwrap_or(rest.len());
    let name = rest[..end].trim_end_matches(".drv");
    Some(
        name.split_once('-')
            .map(|(_, package)| package)
            .unwrap_or(name)
            .to_string(),
    )
}

fn truncate_line(line: &str, max_width: usize) -> String {
    if line.chars().count() <= max_width {
        return line.to_string();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }
    let keep = max_width - 3;
    let head = keep / 2;
    let tail = keep - head;
    let prefix = line.chars().take(head).collect::<String>();
    let suffix = line
        .chars()
        .rev()
        .take(tail)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{prefix}...{suffix}")
}

fn should_print_backend_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let lower = trimmed.to_lowercase();
    if lower.starts_with("debug:") || lower.starts_with("trace:") {
        return false;
    }
    lower.starts_with("warning:")
        || lower.starts_with("error:")
        || lower.starts_with("fatal:")
        || lower.contains("error:")
        || lower.contains("failed")
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

#[cfg(test)]
mod tests {
    use crate::events::{Activity, BuildCategory, BuildState};

    use super::{build_graph_lines, should_print_backend_line, truncate_line};

    #[test]
    fn graph_lines_are_bounded() {
        let mut state = BuildState {
            phase: "building".to_string(),
            completed: 9,
            downloads: 2,
            source_builds: 1,
            binary_substitutes: 1,
            ..BuildState::default()
        };
        state.running.insert(
            1,
            Activity {
                id: 1,
                parent: None,
                text: "building '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-linux-with-a-very-long-name.drv'"
                    .to_string(),
                category: BuildCategory::KernelBoot,
                source_build: true,
                substitute: false,
            },
        );
        state.running.insert(
            2,
            Activity {
                id: 2,
                parent: Some(1),
                text: "evaluating derivation 'git+file:///etc/nixos#nixosConfigurations.\"nixos\".config.system.build.toplevel'"
                    .to_string(),
                category: BuildCategory::Unknown,
                source_build: false,
                substitute: false,
            },
        );

        let lines = build_graph_lines(&state, 64);

        assert!(lines.iter().any(|line| line.contains("build graph")));
        assert!(lines.iter().any(|line| line.contains("system closure")));
        assert!(lines.iter().all(|line| line.chars().count() <= 64));
    }

    #[test]
    fn truncation_keeps_requested_width() {
        let truncated = truncate_line("abcdefghijklmnopqrstuvwxyz", 12);

        assert_eq!(truncated.chars().count(), 12);
        assert!(truncated.contains("..."));
    }

    #[test]
    fn backend_filter_skips_wrapped_debug_errors() {
        assert!(!should_print_backend_line(
            "debug: nixos_rebuild.process: captured output stderr=\"error: noisy\""
        ));
        assert!(should_print_backend_line("error: real failure"));
        assert!(should_print_backend_line("warning: real warning"));
    }
}
