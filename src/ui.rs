use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::ValueEnum;
use serde_json::json;
use terminal_size::{Width, terminal_size};

use crate::config::{FlakeTarget, UiSettings};
use crate::events::{BuildState, NixEvent};
use crate::git::GitSummary;
use crate::impact::{ActivationImpact, ClosureDiff, GenerationInfo};
use crate::process::{StreamLine, StreamSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum OutputMode {
    Auto,
    Rich,
    Nom,
    Plain,
    Raw,
    Json,
    Jsonl,
}

impl OutputMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "rich" => Some(Self::Rich),
            "nom" => Some(Self::Nom),
            "plain" => Some(Self::Plain),
            "raw" => Some(Self::Raw),
            "json" => Some(Self::Json),
            "jsonl" => Some(Self::Jsonl),
            _ => None,
        }
    }

    pub fn effective(self) -> Self {
        self.effective_with_terminal(io::stdout().is_terminal() && io::stderr().is_terminal())
    }

    pub fn effective_for_lifecycle(self, action: &str) -> Self {
        self.effective_for_lifecycle_with_terminal(
            action,
            io::stdout().is_terminal() && io::stderr().is_terminal(),
        )
    }

    pub fn effective_for_lifecycle_with_terminal(self, action: &str, interactive: bool) -> Self {
        match self {
            Self::Auto if interactive && uses_build_ui_by_default(action) => Self::Rich,
            Self::Auto => self.effective_with_terminal(interactive),
            value => value,
        }
    }

    fn effective_with_terminal(self, interactive: bool) -> Self {
        match self {
            Self::Auto if interactive => Self::Rich,
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
    accent: Option<AccentColor>,
    last_rich_render: Instant,
    last_rich_lines: usize,
    last_rich_width: Option<usize>,
    graph_depth: usize,
    refresh: Duration,
    verbose_backend: bool,
}

impl Renderer {
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode: mode.effective(),
            accent: None,
            last_rich_render: Instant::now() - Duration::from_secs(1),
            last_rich_lines: 0,
            last_rich_width: None,
            graph_depth: 12,
            refresh: Duration::from_millis(500),
            verbose_backend: false,
        }
    }

    pub fn new_for_lifecycle(mode: OutputMode, action: &str, settings: UiSettings) -> Self {
        Self {
            mode: mode.effective_for_lifecycle(action),
            accent: settings.accent.and_then(|value| AccentColor::parse(&value)),
            last_rich_render: Instant::now() - Duration::from_secs(1),
            last_rich_lines: 0,
            last_rich_width: None,
            graph_depth: settings.graph_depth,
            refresh: Duration::from_millis(settings.refresh_ms),
            verbose_backend: settings.verbose_backend,
        }
    }

    pub fn start(&mut self, header: &RebuildHeader) {
        match self.mode {
            OutputMode::Json => {}
            OutputMode::Jsonl => println!(
                "{}",
                json!({
                    "event": "start",
                    "command": header.command,
                    "target": header.target.reference(),
                    "log_path": header.log_path.display().to_string(),
                })
            ),
            OutputMode::Raw => {
                eprintln!("nr {} {}", header.command, header.target.reference());
            }
            OutputMode::Plain => {
                println!("◆ nr {} {}", header.command, header.target.reference());
                print_header_details(header);
            }
            OutputMode::Rich | OutputMode::Nom | OutputMode::Auto => {
                println!(
                    "{}",
                    self.accent_line(&format!(
                        "◆ nr {} {}",
                        header.command,
                        header.target.reference()
                    ))
                );
                print_header_details(header);
            }
        }
    }

    pub fn phase(&mut self, phase: &str) {
        match self.mode {
            OutputMode::Json | OutputMode::Raw => {}
            OutputMode::Jsonl => println!("{}", json!({"event": "phase", "phase": phase})),
            OutputMode::Plain => println!("▶ {phase}"),
            OutputMode::Nom => println!("{}", self.accent_line(&format!("▶ {phase}"))),
            OutputMode::Rich | OutputMode::Auto => {
                self.clear_rich_block();
                println!("{}", self.accent_line(&format!("▶ {phase}")));
            }
        }
    }

    pub fn nix_event(&mut self, _event: &NixEvent, state: &BuildState) {
        match self.mode {
            OutputMode::Rich | OutputMode::Auto => self.render_rich_state(state),
            OutputMode::Nom | OutputMode::Plain => {}
            OutputMode::Raw | OutputMode::Json => {}
            OutputMode::Jsonl => println!(
                "{}",
                json!({
                    "event": "build",
                    "action": _event.action,
                    "id": _event.id,
                    "phase": state.phase,
                    "running": state.running.len(),
                    "completed": state.completed,
                    "failed": state.failed,
                })
            ),
        }
    }

    pub fn backend_line(&mut self, line: &StreamLine) {
        match self.mode {
            OutputMode::Raw => match line.source {
                StreamSource::Stdout => println!("{}", line.line),
                StreamSource::Stderr => eprintln!("{}", line.line),
            },
            OutputMode::Rich | OutputMode::Nom | OutputMode::Plain | OutputMode::Auto => {
                if self.verbose_backend || should_print_backend_line(&line.line) {
                    if matches!(self.mode, OutputMode::Rich | OutputMode::Auto) {
                        self.clear_rich_block();
                    }
                    eprintln!("{}", line.line);
                }
            }
            OutputMode::Json => {}
            OutputMode::Jsonl => {
                if self.verbose_backend || should_print_backend_line(&line.line) {
                    println!(
                        "{}",
                        json!({
                            "event": "backend",
                            "source": match line.source {
                                StreamSource::Stdout => "stdout",
                                StreamSource::Stderr => "stderr",
                            },
                            "line": line.line,
                        })
                    );
                }
            }
        }
    }

    pub fn parser_fallback(&mut self) {
        if self.mode == OutputMode::Jsonl {
            println!("{}", json!({"event": "parser_fallback"}));
        } else if self.mode != OutputMode::Json {
            if matches!(self.mode, OutputMode::Rich | OutputMode::Auto) {
                self.clear_rich_block();
            }
            if self.mode == OutputMode::Nom {
                eprintln!("Nix JSON output changed; nom output may be incomplete.");
            } else {
                eprintln!("Nix JSON output changed; falling back to plain log streaming.");
            }
        }
    }

    pub fn resize(&mut self, state: &BuildState) {
        if matches!(self.mode, OutputMode::Rich | OutputMode::Auto) {
            self.render_rich_state_now(state);
        }
    }

    pub fn diff(&mut self, diff: &ClosureDiff) {
        match self.mode {
            OutputMode::Json => {}
            OutputMode::Jsonl => println!(
                "{}",
                json!({
                    "event": "diff",
                    "additions": diff.additions.len(),
                    "removals": diff.removals.len(),
                    "upgrades": diff.upgrades.len(),
                    "downgrades": diff.downgrades.len(),
                    "changes": diff.changes.len(),
                    "important": &diff.important,
                    "size_delta": &diff.size_delta,
                    "unavailable": &diff.unavailable,
                })
            ),
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
            OutputMode::Jsonl => println!(
                "{}",
                json!({
                    "event": "activation",
                    "stopped": activation.stopped,
                    "started": activation.started,
                    "restarted": activation.restarted,
                    "reloaded": activation.reloaded,
                    "failed": activation.failed,
                    "caveats": &activation.caveats,
                    "unavailable": &activation.unavailable,
                })
            ),
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
            OutputMode::Jsonl => println!(
                "{}",
                json!({
                    "event": "finish",
                    "report": report_value(report),
                })
            ),
            OutputMode::Raw => {}
            OutputMode::Plain | OutputMode::Rich | OutputMode::Nom | OutputMode::Auto => {
                if matches!(self.mode, OutputMode::Rich | OutputMode::Auto) {
                    self.clear_rich_block();
                }
                println!("✓ result: {}", report.result);
                if let Some(path) = &report.store_path {
                    println!("▣ store path: {}", path.display());
                }
                if let Some(generation) = report.new_generation {
                    println!("№ generation: {generation}");
                }
                println!("↻ reboot: {}", report.reboot);
                println!("↶ rollback: {}", report.rollback);
                println!("▣ log: {}", report.log_path.display());
            }
        }
    }

    pub fn mode(&self) -> OutputMode {
        self.mode
    }

    fn accent_line(&self, text: &str) -> String {
        if let Some(accent) = self.accent {
            format!(
                "\x1b[1;38;2;{};{};{}m{text}\x1b[0m",
                accent.red, accent.green, accent.blue
            )
        } else {
            text.to_string()
        }
    }

    fn render_rich_state(&mut self, state: &BuildState) {
        let width = terminal_width();
        if self.last_rich_render.elapsed() < self.refresh && self.last_rich_width == Some(width) {
            return;
        }
        self.render_rich_state_at_width(state, width);
    }

    fn render_rich_state_now(&mut self, state: &BuildState) {
        self.render_rich_state_at_width(state, terminal_width());
    }

    fn render_rich_state_at_width(&mut self, state: &BuildState, width: usize) {
        self.last_rich_render = Instant::now();
        self.last_rich_width = Some(width);
        let lines = build_graph_lines_with_depth(state, width, self.graph_depth);
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

#[derive(Clone, Copy, Debug)]
struct AccentColor {
    red: u8,
    green: u8,
    blue: u8,
}

impl AccentColor {
    fn parse(value: &str) -> Option<Self> {
        let hex = value.strip_prefix('#')?;
        if hex.len() != 6 {
            return None;
        }
        Some(Self {
            red: u8::from_str_radix(&hex[0..2], 16).ok()?,
            green: u8::from_str_radix(&hex[2..4], 16).ok()?,
            blue: u8::from_str_radix(&hex[4..6], 16).ok()?,
        })
    }
}

fn uses_build_ui_by_default(action: &str) -> bool {
    matches!(action, "build" | "switch" | "test" | "boot" | "preview")
}

#[cfg(test)]
fn build_graph_lines(state: &BuildState, width: usize) -> Vec<String> {
    build_graph_lines_with_depth(state, width, 12)
}

fn build_graph_lines_with_depth(
    state: &BuildState,
    width: usize,
    graph_depth: usize,
) -> Vec<String> {
    let width = width.min(180);
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
    lines.push("Dependency Graph:".to_string());
    let active_derivations = state.active_derivation_paths();
    if let Some(path) = state.dependency_graph.active_path(&active_derivations) {
        push_dependency_path(state, &path, &mut lines, graph_depth.max(1));
    } else if state.dependency_graph.roots_loaded() {
        lines.push("`-- waiting for an active derivation in the loaded graph".to_string());
        push_active_derivations(&active_derivations, &mut lines);
    } else {
        lines.push("`-- waiting for nix-store derivation graph".to_string());
        push_active_derivations(&active_derivations, &mut lines);
    }

    if let Some(activity) = state.slowest_active() {
        lines.push(format!(
            "slowest: {} {} [{}]",
            format_duration(activity.started_at.elapsed()),
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

fn push_dependency_path(
    state: &BuildState,
    path: &[String],
    lines: &mut Vec<String>,
    max_nodes: usize,
) {
    for (index, key) in path.iter().take(max_nodes).enumerate() {
        let indent = "    ".repeat(index);
        let connector = if index + 1 == path.len() {
            "`--"
        } else {
            "|--"
        };
        lines.push(format!(
            "{indent}{connector} {}",
            state.dependency_graph.label(key)
        ));
    }
    if path.len() > max_nodes {
        lines.push(format!(
            "{} `-- ... {} more dependencies",
            "    ".repeat(max_nodes),
            path.len() - max_nodes
        ));
    }
}

fn push_active_derivations(active_derivations: &[String], lines: &mut Vec<String>) {
    if active_derivations.is_empty() {
        return;
    }
    lines.push("active derivations:".to_string());
    for path in active_derivations.iter().take(4) {
        lines.push(format!("  - {}", compact_activity_text(path)));
    }
}

fn terminal_width() -> usize {
    if let Some((Width(width), _)) = terminal_size() {
        let width = usize::from(width);
        if width >= 40 {
            return width;
        }
    }
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| *width >= 40)
        .unwrap_or(100)
}

fn format_duration(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis < 1_000 {
        return format!("{millis}ms");
    }
    let seconds = duration.as_secs();
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m{seconds:02}s");
    }
    let hours = minutes / 60;
    let minutes = minutes % 60;
    format!("{hours}h{minutes:02}m")
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
    if lower.starts_with("debug:")
        || lower.starts_with("trace:")
        || (lower.starts_with("warning: git tree ") && lower.ends_with(" is dirty"))
        || lower.contains("nixos_rebuild.process: captured output")
        || lower.contains("nixos-rebuild.process: captured output")
        || lower.contains("captured output with stdout=")
    {
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
        println!("⎇ git: {branch} ({})", git_status_label(&header.git));
    } else {
        println!("⎇ git: not a repository");
    }
    if let Some(generation) = header.current.generation {
        println!("№ current generation: {generation}");
    }
    if let Some(version) = &header.current.nixos_version {
        println!("◇ current NixOS: {version}");
    }
    if let Some(kernel) = &header.current.kernel_version {
        println!("● current kernel: {kernel}");
    }
    println!("▣ log: {}", header.log_path.display());
}

fn git_status_label(summary: &GitSummary) -> String {
    if !summary.dirty {
        return "clean".to_string();
    }
    if summary.untracked > 0 {
        format!("dirty, {} untracked", summary.untracked)
    } else {
        "dirty".to_string()
    }
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
    if let Some(reason) = &impact.unavailable {
        println!("  unavailable: {reason}");
    }
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

pub fn report_json(report: &RebuildReport) -> String {
    report_value(report).to_string()
}

pub fn report_value(report: &RebuildReport) -> serde_json::Value {
    let store_path = report
        .store_path
        .as_ref()
        .map(|path| path.display().to_string());
    let diff = report.diff.as_ref().map(|diff| {
        json!({
            "additions": diff.additions.len(),
            "removals": diff.removals.len(),
            "upgrades": diff.upgrades.len(),
            "downgrades": diff.downgrades.len(),
            "changes": diff.changes.len(),
            "important": &diff.important,
            "size_delta": &diff.size_delta,
            "unavailable": &diff.unavailable,
        })
    });
    let activation = report.activation.as_ref().map(|activation| {
        json!({
            "stopped": activation.stopped,
            "started": activation.started,
            "restarted": activation.restarted,
            "reloaded": activation.reloaded,
            "skipped": activation.skipped,
            "failed": activation.failed,
            "caveats": &activation.caveats,
            "unavailable": &activation.unavailable,
        })
    });

    json!({
        "command": report.command,
        "target": report.target.reference(),
        "result": report.result,
        "store_path": store_path,
        "current_generation": report.current.generation,
        "new_generation": report.new_generation,
        "reboot": report.reboot,
        "rollback": report.rollback,
        "log_path": report.log_path.display().to_string(),
        "build": {
            "completed": report.build.completed,
            "failed": report.build.failed,
            "running": report.build.running.len(),
            "downloads": report.build.downloads,
            "source_builds": report.build.source_builds,
            "binary_substitutes": report.build.binary_substitutes,
            "parser_fallback": report.build.parser_fallback,
        },
        "diff": diff,
        "activation": activation,
    })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::events::{Activity, ActivityStatus, BuildCategory, BuildState};
    use crate::git::GitSummary;

    use super::{
        OutputMode, Renderer, build_graph_lines, git_status_label, should_print_backend_line,
        truncate_line,
    };

    #[test]
    fn auto_uses_rich_for_interactive_lifecycle_builds() {
        for action in ["build", "switch", "test", "boot", "preview"] {
            assert_eq!(
                OutputMode::Auto.effective_for_lifecycle_with_terminal(action, true),
                OutputMode::Rich
            );
        }
    }

    #[test]
    fn auto_preserves_plain_for_noninteractive_lifecycle_output() {
        assert_eq!(
            OutputMode::Auto.effective_for_lifecycle_with_terminal("switch", false),
            OutputMode::Plain
        );
    }

    #[test]
    fn explicit_lifecycle_output_mode_wins_over_auto_policy() {
        assert_eq!(
            OutputMode::Rich.effective_for_lifecycle_with_terminal("switch", true),
            OutputMode::Rich
        );
        assert_eq!(
            OutputMode::Json.effective_for_lifecycle_with_terminal("switch", true),
            OutputMode::Json
        );
    }

    #[test]
    fn unset_accent_uses_terminal_foreground() {
        let renderer = Renderer::new(OutputMode::Rich);

        assert_eq!(
            renderer.accent_line("▶ evaluating/building"),
            "▶ evaluating/building"
        );
    }

    #[test]
    fn git_status_label_omits_zero_untracked_count() {
        let mut summary = GitSummary {
            repository: true,
            branch: Some("main".to_string()),
            dirty: true,
            untracked: 0,
        };
        assert_eq!(git_status_label(&summary), "dirty");

        summary.untracked = 2;
        assert_eq!(git_status_label(&summary), "dirty, 2 untracked");
    }

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
        let now = Instant::now();
        let parent = Activity {
            id: 1,
            parent: None,
            text: "building '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-linux-with-a-very-long-name.drv'"
                .to_string(),
            drv_path: Some(
                "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-linux-with-a-very-long-name.drv"
                    .to_string(),
            ),
            category: BuildCategory::KernelBoot,
            source_build: true,
            substitute: false,
            status: ActivityStatus::Completed,
            started_at: now - Duration::from_secs(20),
        };
        let child = Activity {
            id: 2,
            parent: Some(1),
            text: "evaluating derivation 'git+file:///etc/nixos#nixosConfigurations.\"nixos\".config.system.build.toplevel'"
                .to_string(),
            drv_path: Some("/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-man-cache.drv".to_string()),
            category: BuildCategory::Other,
            source_build: false,
            substitute: false,
            status: ActivityStatus::Running,
            started_at: now - Duration::from_secs(5),
        };
        state.nodes.insert(parent.id, parent);
        state.nodes.insert(child.id, child.clone());
        state.running.insert(child.id, child);
        state
            .dependency_graph
            .note_path("/nix/store/cccccccccccccccccccccccccccccccc-nixos-system-nixos.drv");
        state.dependency_graph.note_dot_graph(
            "/nix/store/cccccccccccccccccccccccccccccccc-nixos-system-nixos.drv",
            r#"
digraph G {
"cccccccccccccccccccccccccccccccc-nixos-system-nixos.drv" [label = "nixos-system-nixos"];
"dddddddddddddddddddddddddddddddd-etc.drv" [label = "etc"];
"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-man-cache.drv" [label = "man-cache"];
"dddddddddddddddddddddddddddddddd-etc.drv" -> "cccccccccccccccccccccccccccccccc-nixos-system-nixos.drv";
"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-man-cache.drv" -> "dddddddddddddddddddddddddddddddd-etc.drv";
}
"#,
        );

        let lines = build_graph_lines(&state, 64);

        assert!(lines.iter().any(|line| line.contains("build graph")));
        assert!(lines.iter().any(|line| line.contains("Dependency Graph")));
        assert!(lines.iter().any(|line| line.contains("nixos-system-nixos")));
        assert!(lines.iter().any(|line| line.contains("man-cache")));
        assert!(lines.iter().any(|line| line.contains("`-- man-cache")));
        assert!(lines.iter().all(|line| line.chars().count() <= 64));
    }

    #[test]
    fn truncation_keeps_requested_width() {
        let truncated = truncate_line("abcdefghijklmnopqrstuvwxyz", 12);

        assert_eq!(truncated.chars().count(), 12);
        assert!(truncated.contains("..."));
    }

    #[test]
    fn graph_lines_reflow_for_different_widths() {
        let mut state = BuildState {
            phase: "building".to_string(),
            ..BuildState::default()
        };
        let activity = Activity {
            id: 1,
            parent: None,
            text: "building '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-package-with-a-long-name.drv'"
                .to_string(),
            drv_path: Some(
                "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-package-with-a-long-name.drv"
                    .to_string(),
            ),
            category: BuildCategory::Other,
            source_build: true,
            substitute: false,
            status: ActivityStatus::Running,
            started_at: Instant::now(),
        };
        state.running.insert(activity.id, activity);

        let narrow = build_graph_lines(&state, 48);
        let wide = build_graph_lines(&state, 120);

        assert!(narrow.iter().all(|line| line.chars().count() <= 48));
        assert!(wide.iter().all(|line| line.chars().count() <= 120));
        assert_ne!(narrow, wide);
    }

    #[test]
    fn graph_lines_fit_forty_column_terminal() {
        let state = BuildState {
            phase: "building".to_string(),
            completed: 12,
            downloads: 2,
            source_builds: 1,
            binary_substitutes: 8,
            ..BuildState::default()
        };

        let lines = build_graph_lines(&state, 40);

        assert!(lines.iter().all(|line| line.chars().count() <= 40));
    }

    #[test]
    fn backend_filter_skips_wrapped_debug_errors() {
        assert!(!should_print_backend_line(
            "debug: nixos_rebuild.process: captured output stderr=\"error: noisy\""
        ));
        assert!(!should_print_backend_line(
            "nixos_rebuild.process: captured output with stdout='', stderr=\"error: noisy\""
        ));
        assert!(!should_print_backend_line(
            "warning: Git tree '/etc/nixos' is dirty"
        ));
        assert!(should_print_backend_line("error: real failure"));
        assert!(should_print_backend_line("warning: real warning"));
    }
}
