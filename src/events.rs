use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Instant;

use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct NixEvent {
    pub action: String,
    pub id: Option<u64>,
    pub parent: Option<u64>,
    pub activity_type: Option<u64>,
    pub level: Option<u64>,
    pub text: String,
    pub fields: Vec<String>,
    pub raw: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParsedLine {
    Event(NixEvent),
    Plain(String),
    BrokenInternalJson(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActivityStatus {
    #[default]
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activity {
    pub id: u64,
    pub parent: Option<u64>,
    pub text: String,
    pub category: BuildCategory,
    pub source_build: bool,
    pub substitute: bool,
    pub status: ActivityStatus,
    pub started_at: Instant,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum BuildCategory {
    KernelBoot,
    DesktopStack,
    Services,
    Libraries,
    DevTools,
    #[default]
    Other,
}

impl BuildCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::KernelBoot => "kernel/boot",
            Self::DesktopStack => "desktop stack",
            Self::Services => "services",
            Self::Libraries => "libraries",
            Self::DevTools => "dev tools",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildState {
    pub phase: String,
    pub running: BTreeMap<u64, Activity>,
    pub nodes: BTreeMap<u64, Activity>,
    pub completed: usize,
    pub failed: usize,
    pub downloads: usize,
    pub source_builds: usize,
    pub binary_substitutes: usize,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub unknown_events: usize,
    pub parser_fallback: bool,
}

impl BuildState {
    pub fn ingest(&mut self, event: &NixEvent) {
        match event.action.as_str() {
            "start" => self.start(event),
            "stop" => self.stop(event),
            "result" => self.result(event),
            "msg" | "message" => self.message(event),
            _ => self.unknown_events += 1,
        }
    }

    pub fn running_by_category(&self) -> BTreeMap<BuildCategory, usize> {
        let mut counts = BTreeMap::new();
        for activity in self.running.values() {
            *counts.entry(activity.category).or_insert(0) += 1;
        }
        counts
    }

    pub fn slowest_active(&self) -> Option<&Activity> {
        self.running
            .values()
            .max_by_key(|activity| activity.started_at.elapsed())
    }

    pub fn active_lineage_ids(&self) -> BTreeSet<u64> {
        let mut ids = BTreeSet::new();
        for activity in self.running.values() {
            ids.insert(activity.id);
            let mut parent = activity.parent;
            while let Some(parent_id) = parent {
                if !ids.insert(parent_id) {
                    break;
                }
                parent = self.nodes.get(&parent_id).and_then(|node| node.parent);
            }
        }
        ids
    }

    pub fn why_building(&self, activity: &Activity) -> Option<String> {
        let mut parent = activity.parent;
        let mut chain = VecDeque::new();
        while let Some(parent_id) = parent {
            let Some(parent_activity) = self.nodes.get(&parent_id) else {
                break;
            };
            chain.push_front(parent_activity.text.clone());
            parent = parent_activity.parent;
        }
        if chain.is_empty() {
            None
        } else {
            Some(format!(
                "event parent path: {}",
                chain.into_iter().collect::<Vec<_>>().join(" -> ")
            ))
        }
    }

    fn start(&mut self, event: &NixEvent) {
        let text = event_text(event);
        self.phase = classify_phase(&text).to_string();
        if let Some(id) = event.id {
            let substitute = is_substitute(&text);
            let source_build = is_source_build(&text);
            if substitute {
                self.binary_substitutes += 1;
            }
            if source_build {
                self.source_builds += 1;
            }
            if is_download(&text) {
                self.downloads += 1;
            }
            let activity = Activity {
                id,
                parent: event.parent,
                category: categorize(&text),
                text,
                source_build,
                substitute,
                status: ActivityStatus::Running,
                started_at: Instant::now(),
            };
            self.nodes.insert(id, activity.clone());
            self.running.insert(id, activity);
        }
    }

    fn stop(&mut self, event: &NixEvent) {
        if let Some(id) = event.id {
            let completed = self
                .running
                .get(&id)
                .is_some_and(|activity| activity.status != ActivityStatus::Failed);
            if let Some(mut activity) = self.running.remove(&id) {
                if activity.status != ActivityStatus::Failed {
                    activity.status = ActivityStatus::Completed;
                }
                self.nodes.insert(id, activity);
                if completed {
                    self.completed += 1;
                }
            } else if let Some(activity) = self.nodes.get_mut(&id)
                && activity.status == ActivityStatus::Running
            {
                activity.status = ActivityStatus::Completed;
                self.completed += 1;
            }
        }
    }

    fn result(&mut self, event: &NixEvent) {
        let text = event_text(event);
        let lower = text.to_lowercase();
        if lower.contains("failed") || lower.contains("error") {
            self.failed += 1;
            self.errors.push(text);
            if let Some(id) = event.id {
                if let Some(activity) = self.running.get_mut(&id) {
                    activity.status = ActivityStatus::Failed;
                }
                if let Some(activity) = self.nodes.get_mut(&id) {
                    activity.status = ActivityStatus::Failed;
                }
            }
        }
    }

    fn message(&mut self, event: &NixEvent) {
        let text = event_text(event);
        let lower = text.to_lowercase();
        if lower.contains("warning") {
            self.warnings.push(text);
        } else if lower.contains("error") || lower.contains("failed") {
            self.errors.push(text);
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawNixEvent {
    action: Option<String>,
    id: Option<u64>,
    parent: Option<u64>,
    #[serde(rename = "type")]
    activity_type: Option<u64>,
    level: Option<u64>,
    text: Option<String>,
    msg: Option<String>,
    fields: Option<Vec<Value>>,
}

pub fn parse_line(line: &str) -> ParsedLine {
    let trimmed = line.trim_start();
    let (json_text, internal) = if let Some(rest) = trimmed.strip_prefix("@nix ") {
        (rest.trim_start(), true)
    } else if trimmed.starts_with('{') {
        (trimmed, false)
    } else {
        return ParsedLine::Plain(line.to_string());
    };

    let raw = match serde_json::from_str::<RawNixEvent>(json_text) {
        Ok(raw) => raw,
        Err(_) if internal => return ParsedLine::BrokenInternalJson(line.to_string()),
        Err(_) => return ParsedLine::Plain(line.to_string()),
    };
    let fields = raw
        .fields
        .unwrap_or_default()
        .into_iter()
        .map(|value| match value {
            Value::String(value) => value,
            other => other.to_string(),
        })
        .collect::<Vec<_>>();
    let text = raw
        .text
        .or(raw.msg)
        .or_else(|| fields.first().cloned())
        .unwrap_or_default();

    ParsedLine::Event(NixEvent {
        action: raw.action.unwrap_or_else(|| "unknown".to_string()),
        id: raw.id,
        parent: raw.parent,
        activity_type: raw.activity_type,
        level: raw.level,
        text,
        fields,
        raw: json_text.to_string(),
    })
}

pub fn categorize(text: &str) -> BuildCategory {
    let lower = text.to_lowercase();
    if contains_any(
        &lower,
        &["linux", "kernel", "initrd", "boot", "grub", "systemd-boot"],
    ) {
        BuildCategory::KernelBoot
    } else if contains_any(
        &lower,
        &[
            "gnome",
            "kde",
            "plasma",
            "xorg",
            "xwayland",
            "wayland",
            "wlroots",
            "sddm",
            "gdm",
            "display-manager",
            "mesa",
            "nvidia",
            "niri",
            "quickshell",
            "hyprland",
            "sway",
            "waybar",
            "gtk",
            "qtbase",
            "qtwayland",
            "qtdeclarative",
            "qt-",
        ],
    ) {
        BuildCategory::DesktopStack
    } else if contains_any(
        &lower,
        &["service", "unit", "daemon", "networkmanager", "dbus"],
    ) {
        BuildCategory::Services
    } else if contains_any(&lower, &["gcc", "rustc", "cargo", "clang", "go-", "python"]) {
        BuildCategory::DevTools
    } else if contains_any(&lower, &["lib", "openssl", "glibc", "zlib"]) {
        BuildCategory::Libraries
    } else {
        BuildCategory::Other
    }
}

fn event_text(event: &NixEvent) -> String {
    if !event.text.is_empty() {
        return event.text.clone();
    }
    event.fields.first().cloned().unwrap_or_default()
}

fn classify_phase(text: &str) -> &'static str {
    let lower = text.to_lowercase();
    if is_download(&lower) {
        "downloading"
    } else if lower.contains("fetch") {
        "fetching"
    } else if lower.contains("evaluat") {
        "evaluating"
    } else {
        "building"
    }
}

fn is_source_build(text: &str) -> bool {
    let lower = text.to_lowercase();
    !is_substitute(&lower) && (lower.contains("building") || lower.contains(".drv"))
}

fn is_substitute(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("copying path") || lower.contains("substitut") || lower.contains("download")
}

fn is_download(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("download") || lower.contains("copying path")
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}
