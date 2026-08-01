use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::cli::{FindArgs, FindFormat, FindType};
use crate::color::ColorChoice;
use crate::config::NrConfig;
use crate::errors::{IoContext, NrError, Result};

pub fn run_find(config: &NrConfig, args: &FindArgs) -> Result<i32> {
    let hint = type_hint(&args.query, args.search_type);
    let files = nix_files(&config.target.path)?;
    let mut results = Vec::new();
    for path in files {
        let relative = relative_path(&config.target.path, &path)?;
        let text =
            fs::read_to_string(&path).with_context(format!("failed to read {}", path.display()))?;
        let matches = file_matches(&text, &args.query, args.case_sensitive, args.context);
        if !matches.is_empty() {
            results.push(FileResult {
                path: relative.display().to_string(),
                match_count: matches.len(),
                matches,
            });
        }
    }
    let total_matches = results.iter().map(|file| file.match_count).sum::<usize>();
    match args.format {
        FindFormat::Text => print_text_results(
            &args.query,
            hint,
            &results,
            total_matches,
            ColorChoice::new(args.no_color),
            args.case_sensitive,
        ),
        FindFormat::Json => print_json_results(&args.query, hint, &results, total_matches)?,
    }
    Ok(0)
}

#[derive(Clone, Debug, Serialize)]
struct JsonOutput<'a> {
    query: &'a str,
    type_hint: &'a str,
    total_matches: usize,
    files: &'a [FileResult],
}

#[derive(Clone, Debug, Serialize)]
struct FileResult {
    path: String,
    match_count: usize,
    matches: Vec<MatchResult>,
}

#[derive(Clone, Debug, Serialize)]
struct MatchResult {
    line: usize,
    text: String,
    context_before: Vec<String>,
    context_after: Vec<String>,
}

fn print_text_results(
    query: &str,
    hint: &str,
    results: &[FileResult],
    total_matches: usize,
    color: ColorChoice,
    case_sensitive: bool,
) {
    println!("Detected type hint: {hint}");
    for file in results {
        println!("{} matches in {}", file.match_count, file.path);
        for item in &file.matches {
            for context in &item.context_before {
                println!("  | {context}");
            }
            println!(
                "{:>4}: {}",
                item.line,
                bold_matches(&item.text, query, case_sensitive, color)
            );
            for context in &item.context_after {
                println!("  | {context}");
            }
        }
    }
    println!(
        "Found {} matches across {} files",
        total_matches,
        results.len()
    );
}

fn print_json_results(
    query: &str,
    hint: &str,
    results: &[FileResult],
    total_matches: usize,
) -> Result<()> {
    let output = JsonOutput {
        query,
        type_hint: hint,
        total_matches,
        files: results,
    };
    let text = serde_json::to_string_pretty(&output)
        .map_err(|error| NrError::message(format!("failed to serialize find JSON: {error}")))?;
    println!("{text}");
    Ok(())
}

fn file_matches(text: &str, query: &str, case_sensitive: bool, context: usize) -> Vec<MatchResult> {
    let lines = text.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let mut results = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !contains_query(line, query, case_sensitive) {
            continue;
        }
        let start = index.saturating_sub(context);
        let after_end = (index + context + 1).min(lines.len());
        results.push(MatchResult {
            line: index + 1,
            text: line.clone(),
            context_before: lines[start..index].to_vec(),
            context_after: lines[index + 1..after_end].to_vec(),
        });
    }
    results
}

fn contains_query(line: &str, query: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        line.contains(query)
    } else {
        line.to_lowercase().contains(&query.to_lowercase())
    }
}

fn bold_matches(line: &str, query: &str, case_sensitive: bool, color: ColorChoice) -> String {
    if query.is_empty() || !color.enabled() {
        return line.to_string();
    }
    let ranges = match_ranges(line, query, case_sensitive);
    if ranges.is_empty() {
        return line.to_string();
    }
    let mut output = String::new();
    let mut cursor = 0usize;
    for (start, end) in ranges {
        let Some(before) = line.get(cursor..start) else {
            return line.to_string();
        };
        let Some(matched) = line.get(start..end) else {
            return line.to_string();
        };
        output.push_str(before);
        output.push_str(&color.bold(matched));
        cursor = end;
    }
    if let Some(rest) = line.get(cursor..) {
        output.push_str(rest);
    }
    output
}

fn match_ranges(line: &str, query: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    let haystack = if case_sensitive {
        line.to_string()
    } else {
        line.to_lowercase()
    };
    let needle = if case_sensitive {
        query.to_string()
    } else {
        query.to_lowercase()
    };
    let mut ranges = Vec::new();
    let mut offset = 0usize;
    while let Some(position) = haystack[offset..].find(&needle) {
        let start = offset + position;
        let end = start + needle.len();
        ranges.push((start, end));
        offset = end;
        if offset >= haystack.len() {
            break;
        }
    }
    ranges
}

fn type_hint(query: &str, requested: FindType) -> &'static str {
    match requested {
        FindType::Package => "package",
        FindType::Option => "option",
        FindType::String => "string",
        FindType::AutoDetect if looks_like_option(query) => "option",
        FindType::AutoDetect if looks_like_package(query) => "package",
        FindType::AutoDetect => "string",
    }
}

fn looks_like_option(query: &str) -> bool {
    query.contains('.')
        || ["programs.", "services.", "home.", "boot.", "environment."]
            .iter()
            .any(|prefix| query.starts_with(prefix))
}

fn looks_like_package(query: &str) -> bool {
    !query.contains('.')
        && query.contains('-')
        && query.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn nix_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_nix_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_nix_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(directory).with_context(format!("failed to read {}", directory.display()))?
    {
        let entry =
            entry.with_context(format!("failed to read entry in {}", directory.display()))?;
        let path = entry.path();
        if skip_path(&path) {
            continue;
        }
        let metadata = entry
            .metadata()
            .with_context(format!("failed to stat {}", path.display()))?;
        if metadata.is_dir() {
            collect_nix_files(&path, files)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("nix") {
            files.push(path);
        }
    }
    Ok(())
}

fn skip_path(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::Normal(value)
                if matches!(
                    value.to_str(),
                    Some(".git" | "target" | ".direnv" | "__pycache__" | "node_modules")
                )
        )
    })
}

fn relative_path(root: &Path, path: &Path) -> Result<PathBuf> {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(|error| {
            NrError::message(format!(
                "failed to make {} relative to {}: {error}",
                path.display(),
                root.display()
            ))
        })
}
