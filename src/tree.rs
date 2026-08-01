use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::cli::TreeArgs;
use crate::color::ColorChoice;
use crate::config::NrConfig;
use crate::errors::{IoContext, Result};

pub fn run_tree(config: &NrConfig, args: &TreeArgs) -> Result<i32> {
    let color = ColorChoice::new(args.no_color);
    let mut summary = TreeSummary::default();
    count_summary(&config.target.path, 1, args.depth, &mut summary)?;
    println!("{}", config.target.host);
    let entries = read_entries(&config.target.path, args.files)?;
    render_entries(
        &config.target.path,
        &entries,
        "",
        1,
        args.depth,
        args.files,
        color,
    )?;
    println!(
        "{} directories, {} .nix files, {} other files",
        summary.directories, summary.nix_files, summary.other_files
    );
    Ok(0)
}

#[derive(Clone, Debug)]
struct TreeEntry {
    path: PathBuf,
    name: String,
    kind: EntryKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind {
    Directory,
    File,
    Symlink,
}

#[derive(Default)]
struct TreeSummary {
    directories: usize,
    nix_files: usize,
    other_files: usize,
}

fn render_entries(
    root: &Path,
    entries: &[TreeEntry],
    prefix: &str,
    depth: usize,
    max_depth: usize,
    show_files: bool,
    color: ColorChoice,
) -> Result<()> {
    if depth > max_depth {
        return Ok(());
    }
    for (index, entry) in entries.iter().enumerate() {
        let last = index + 1 == entries.len();
        let connector = if last { "└── " } else { "├── " };
        println!(
            "{prefix}{connector}{}",
            entry_label(root, entry, show_files, color)?
        );
        if entry.kind == EntryKind::Directory {
            let children = read_entries(&entry.path, show_files)?;
            let child_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });
            render_entries(
                root,
                &children,
                &child_prefix,
                depth + 1,
                max_depth,
                show_files,
                color,
            )?;
        }
    }
    Ok(())
}

fn count_summary(
    directory: &Path,
    depth: usize,
    max_depth: usize,
    summary: &mut TreeSummary,
) -> Result<()> {
    if depth > max_depth {
        return Ok(());
    }
    for entry in read_entries(directory, true)? {
        count_entry(&entry, summary);
        if entry.kind == EntryKind::Directory {
            count_summary(&entry.path, depth + 1, max_depth, summary)?;
        }
    }
    Ok(())
}

fn read_entries(directory: &Path, show_files: bool) -> Result<Vec<TreeEntry>> {
    let mut entries = Vec::new();
    for entry in
        fs::read_dir(directory).with_context(format!("failed to read {}", directory.display()))?
    {
        let entry =
            entry.with_context(format!("failed to read entry in {}", directory.display()))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if should_skip(&path, &name)? {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .with_context(format!("failed to inspect {}", path.display()))?;
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            EntryKind::Symlink
        } else if file_type.is_dir() {
            EntryKind::Directory
        } else if file_type.is_file() {
            EntryKind::File
        } else {
            continue;
        };
        if kind != EntryKind::Directory && !show_files {
            continue;
        }
        entries.push(TreeEntry { path, name, kind });
    }
    entries.sort_by(|left, right| {
        sort_rank(left)
            .cmp(&sort_rank(right))
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(entries)
}

fn sort_rank(entry: &TreeEntry) -> u8 {
    match entry.kind {
        EntryKind::Directory => 0,
        EntryKind::Symlink => 1,
        EntryKind::File => 2,
    }
}

fn should_skip(path: &Path, name: &str) -> Result<bool> {
    if matches!(
        name,
        ".git" | "target" | ".direnv" | "__pycache__" | "node_modules"
    ) {
        return Ok(true);
    }
    if name.starts_with("result")
        && fs::symlink_metadata(path)
            .with_context(format!("failed to inspect {}", path.display()))?
            .file_type()
            .is_symlink()
    {
        return Ok(true);
    }
    Ok(path
        .components()
        .any(|component| matches!(component, Component::Normal(value) if value == "node_modules")))
}

fn entry_label(
    root: &Path,
    entry: &TreeEntry,
    show_files: bool,
    color: ColorChoice,
) -> Result<String> {
    let mut label = entry.name.clone();
    if entry.kind == EntryKind::Symlink {
        let destination = fs::read_link(&entry.path)
            .with_context(format!("failed to read symlink {}", entry.path.display()))?;
        label = format!("{label} -> {}", destination.display());
    }
    if entry.kind == EntryKind::File && show_files {
        let size = fs::metadata(&entry.path)
            .with_context(format!("failed to stat {}", entry.path.display()))?
            .len();
        label = format!("{label} ({})", format_size(size));
    }
    let relative = entry.path.strip_prefix(root).unwrap_or(&entry.path);
    Ok(match entry.kind {
        EntryKind::Directory => color.blue(label),
        EntryKind::Symlink => color.yellow(label),
        EntryKind::File
            if relative.file_name().and_then(|value| value.to_str()) == Some("default.nix") =>
        {
            color.bold_green(label)
        }
        EntryKind::File
            if entry.path.extension().and_then(|value| value.to_str()) == Some("nix") =>
        {
            color.green(label)
        }
        EntryKind::File if is_asset(&entry.path) => color.magenta(label),
        EntryKind::File => label,
    })
}

fn count_entry(entry: &TreeEntry, summary: &mut TreeSummary) {
    match entry.kind {
        EntryKind::Directory => summary.directories += 1,
        EntryKind::File | EntryKind::Symlink
            if entry.path.extension().and_then(|value| value.to_str()) == Some("nix") =>
        {
            summary.nix_files += 1;
        }
        EntryKind::File | EntryKind::Symlink => summary.other_files += 1,
    }
}

fn is_asset(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .as_deref(),
        Some(
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "svg"
                | "webp"
                | "txt"
                | "md"
                | "json"
                | "toml"
                | "yaml"
                | "yml"
        )
    )
}

fn format_size(size: u64) -> String {
    if size < 1024 {
        return format!("{size} B");
    }
    if size < 1024 * 1024 {
        return format!("{} KiB", size / 1024);
    }
    format!("{} MiB", size / (1024 * 1024))
}
