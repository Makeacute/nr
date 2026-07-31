use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::ValueEnum;
use serde_json::json;
use terminal_size::{Width, terminal_size};

use crate::config::FlakeTarget;
use crate::events::{Activity, ActivityStatus, BuildState, NixEvent};
use crate::git::GitSummary;
use crate::impact::{ActivationImpact, ClosureDiff, GenerationInfo};
use crate::process::{StreamLine, StreamSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
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

    let lineage = state.active_lineage_ids();
    if lineage.is_empty() {
        lines.push("`-- waiting for Nix activity".to_string());
    } else {
        let children = lineage_children(state, &lineage);
        let roots = children.get(&None).cloned().unwrap_or_default();
        let mut graph = GraphRender {
            state,
            children: &children,
            rendered: 0,
            max_nodes: 10,
            lines: &mut lines,
        };
        for (index, id) in roots.iter().enumerate() {
            if graph.rendered >= graph.max_nodes {
                break;
            }
            graph.push_node(*id, "", index + 1 == roots.len());
        }
        let rendered = graph.rendered;
        if lineage.len() > rendered {
            lines.push(format!(
                "`-- ... {} more graph nodes",
                lineage.len() - rendered
            ));
        }
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

fn lineage_children(
    state: &BuildState,
    lineage: &BTreeSet<u64>,
) -> BTreeMap<Option<u64>, Vec<u64>> {
    let mut children: BTreeMap<Option<u64>, Vec<u64>> = BTreeMap::new();
    for id in lineage {
        let Some(activity) = state.nodes.get(id) else {
            continue;
        };
        let parent = activity.parent.filter(|parent| lineage.contains(parent));
        children.entry(parent).or_default().push(*id);
    }
    children
}

struct GraphRender<'a> {
    state: &'a BuildState,
    children: &'a BTreeMap<Option<u64>, Vec<u64>>,
    rendered: usize,
    max_nodes: usize,
    lines: &'a mut Vec<String>,
}

impl GraphRender<'_> {
    fn push_node(&mut self, id: u64, prefix: &str, is_last: bool) {
        if self.rendered >= self.max_nodes {
            return;
        }
        let Some(activity) = self.state.nodes.get(&id) else {
            return;
        };
        let connector = if is_last { "`--" } else { "|--" };
        self.lines.push(format!(
            "{prefix}{connector} {}",
            activity_summary(activity)
        ));
        self.rendered += 1;

        let Some(child_ids) = self.children.get(&Some(id)) else {
            return;
        };
        let next_prefix = format!("{prefix}{}", if is_last { "    " } else { "|   " });
        for (index, child_id) in child_ids.iter().enumerate() {
            if self.rendered >= self.max_nodes {
                return;
            }
            self.push_node(*child_id, &next_prefix, index + 1 == child_ids.len());
        }
    }
}

fn activity_summary(activity: &Activity) -> String {
    let mut details = vec![
        activity.category.label().to_string(),
        provenance(activity).to_string(),
    ];
    if activity.status == ActivityStatus::Running {
        details.push(format_duration(activity.started_at.elapsed()));
    }
    format!(
        "{} {} [{}]",
        activity_status(activity),
        compact_activity_text(&activity.text),
        details.join(" ")
    )
}

fn activity_status(activity: &Activity) -> &'static str {
    match activity.status {
        ActivityStatus::Running => activity_kind(activity),
        ActivityStatus::Completed => "done",
        ActivityStatus::Failed => "fail",
    }
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

fn provenance(activity: &Activity) -> &'static str {
    if activity.substitute {
        "cache"
    } else if activity.source_build {
        "source"
    } else {
        "event"
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
            "important": diff.important,
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
    .to_string()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::events::{Activity, ActivityStatus, BuildCategory, BuildState};

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
        let now = Instant::now();
        let parent = Activity {
            id: 1,
            parent: None,
            text: "building '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-linux-with-a-very-long-name.drv'"
                .to_string(),
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
            category: BuildCategory::Other,
            source_build: false,
            substitute: false,
            status: ActivityStatus::Running,
            started_at: now - Duration::from_secs(5),
        };
        state.nodes.insert(parent.id, parent);
        state.nodes.insert(child.id, child.clone());
        state.running.insert(child.id, child);

        let lines = build_graph_lines(&state, 64);

        assert!(lines.iter().any(|line| line.contains("build graph")));
        assert!(lines.iter().any(|line| line.contains("system closure")));
        assert!(lines.iter().any(|line| line.contains("done")));
        assert!(lines.iter().any(|line| line.contains("eval")));
        assert!(lines.iter().any(|line| line.starts_with("    `--")));
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
        assert!(!should_print_backend_line(
            "nixos_rebuild.process: captured output with stdout='', stderr=\"error: noisy\""
        ));
        assert!(should_print_backend_line("error: real failure"));
        assert!(should_print_backend_line("warning: real warning"));
    }
}
