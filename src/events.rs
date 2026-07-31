use std::collections::{BTreeMap, VecDeque};

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    pub id: u64,
    pub parent: Option<u64>,
    pub text: String,
    pub category: BuildCategory,
    pub source_build: bool,
    pub substitute: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum BuildCategory {
    KernelBoot,
    DesktopStack,
    Services,
    Libraries,
    DevTools,
    #[default]
    Unknown,
}

impl BuildCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::KernelBoot => "kernel/boot",
            Self::DesktopStack => "desktop stack",
            Self::Services => "services",
            Self::Libraries => "libraries",
            Self::DevTools => "dev tools",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildState {
    pub phase: String,
    pub running: BTreeMap<u64, Activity>,
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
        self.running.values().next()
    }

    pub fn why_building(&self, activity: &Activity) -> Option<String> {
        let mut parent = activity.parent;
        let mut chain = VecDeque::new();
        while let Some(parent_id) = parent {
            let Some(parent_activity) = self.running.get(&parent_id) else {
                break;
            };
            chain.push_front(parent_activity.text.clone());
            parent = parent_activity.parent;
        }
        if chain.is_empty() {
            None
        } else {
            Some(format!(
                "dependency of {}",
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
            self.running.insert(
                id,
                Activity {
                    id,
                    parent: event.parent,
                    category: categorize(&text),
                    text,
                    source_build,
                    substitute,
                },
            );
        }
    }

    fn stop(&mut self, event: &NixEvent) {
        if let Some(id) = event.id
            && self.running.remove(&id).is_some()
        {
            self.completed += 1;
        }
    }

    fn result(&mut self, event: &NixEvent) {
        let text = event_text(event);
        let lower = text.to_lowercase();
        if lower.contains("failed") || lower.contains("error") {
            self.failed += 1;
            self.errors.push(text);
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

pub fn parse_line(line: &str) -> ParsedLine {
    let trimmed = line.trim_start();
    let (json_text, internal) = if let Some(rest) = trimmed.strip_prefix("@nix ") {
        (rest.trim_start(), true)
    } else if trimmed.starts_with('{') {
        (trimmed, false)
    } else {
        return ParsedLine::Plain(line.to_string());
    };

    if !(json_text.starts_with('{') && json_text.ends_with('}')) {
        return if internal {
            ParsedLine::BrokenInternalJson(line.to_string())
        } else {
            ParsedLine::Plain(line.to_string())
        };
    }

    let action = json_string_field(json_text, "action").unwrap_or_else(|| "unknown".to_string());
    let fields = json_string_array_field(json_text, "fields");
    let text = json_string_field(json_text, "text")
        .or_else(|| json_string_field(json_text, "msg"))
        .or_else(|| fields.first().cloned())
        .unwrap_or_default();

    ParsedLine::Event(NixEvent {
        action,
        id: json_u64_field(json_text, "id"),
        parent: json_u64_field(json_text, "parent"),
        activity_type: json_u64_field(json_text, "type"),
        level: json_u64_field(json_text, "level"),
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
            "gnome", "kde", "plasma", "xorg", "wayland", "sddm", "gdm", "mesa", "nvidia",
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
        BuildCategory::Unknown
    }
}

fn json_string_field(json: &str, key: &str) -> Option<String> {
    let value_start = find_json_value(json, key)?;
    let bytes = json.as_bytes();
    if bytes.get(value_start) != Some(&b'"') {
        return None;
    }
    parse_json_string(json, value_start).map(|(value, _)| value)
}

fn json_u64_field(json: &str, key: &str) -> Option<u64> {
    let mut index = find_json_value(json, key)?;
    let bytes = json.as_bytes();
    while bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
    {
        index += 1;
    }
    let start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    if start == index {
        None
    } else {
        json[start..index].parse().ok()
    }
}

fn json_string_array_field(json: &str, key: &str) -> Vec<String> {
    let Some(mut index) = find_json_value(json, key) else {
        return Vec::new();
    };
    let bytes = json.as_bytes();
    if bytes.get(index) != Some(&b'[') {
        return Vec::new();
    }
    index += 1;
    let mut values = Vec::new();
    while index < bytes.len() {
        while bytes
            .get(index)
            .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t' | b','))
        {
            index += 1;
        }
        match bytes.get(index) {
            Some(b'"') => match parse_json_string(json, index) {
                Some((value, next)) => {
                    values.push(value);
                    index = next;
                }
                None => break,
            },
            Some(b']') | None => break,
            _ => {
                while bytes
                    .get(index)
                    .is_some_and(|byte| !matches!(byte, b',' | b']'))
                {
                    index += 1;
                }
            }
        }
    }
    values
}

fn find_json_value(json: &str, key: &str) -> Option<usize> {
    let needle = format!("\"{key}\"");
    let key_index = json.find(&needle)?;
    let after_key = key_index + needle.len();
    let colon_offset = json[after_key..].find(':')?;
    let mut index = after_key + colon_offset + 1;
    let bytes = json.as_bytes();
    while bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
    {
        index += 1;
    }
    Some(index)
}

fn parse_json_string(json: &str, start: usize) -> Option<(String, usize)> {
    let mut escaped = false;
    let mut output = String::new();
    for (offset, character) in json[start + 1..].char_indices() {
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
            return Some((output, start + 1 + offset + 1));
        } else {
            output.push(character);
        }
    }
    None
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
    lower.contains("building") || lower.contains(".drv")
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
