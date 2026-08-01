use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::write::GzEncoder;
use tar::{Builder, Header};

use crate::VERSION;
use crate::cli::ExportArgs;
use crate::config::NrConfig;
use crate::errors::{IoContext, NrError, Result};
use crate::generations::{current_generation, load_system_generations};

pub fn run_export(config: &NrConfig, args: &ExportArgs) -> Result<i32> {
    if args.include_secrets {
        eprintln!("WARNING: --include-secrets is set. Secret-looking files may be exported.");
    }
    let export = collect_export(config, args.include_secrets)?;
    if args.dry_run {
        println!("dry run: files that would be included");
        for file in &export.files {
            println!("{}", file.display());
        }
        println!("MANIFEST.md");
        println!("RESTORE.md");
        println!("{} files", export.files.len() + 2);
        return Ok(0);
    }

    let output = args.output.clone().unwrap_or_else(default_output_path);
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(format!("failed to create {}", parent.display()))?;
    }

    let file =
        File::create(&output).with_context(format!("failed to create {}", output.display()))?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut archive = Builder::new(encoder);

    for relative in &export.files {
        println!("{}", relative.display());
        archive
            .append_path_with_name(config.target.path.join(relative), relative)
            .with_context(format!("failed to add {}", relative.display()))?;
    }
    append_generated_file(&mut archive, Path::new("MANIFEST.md"), &export.manifest)?;
    append_generated_file(&mut archive, Path::new("RESTORE.md"), &export.restore)?;
    archive
        .finish()
        .with_context(format!("failed to finish {}", output.display()))?;
    let encoder = archive.into_inner().with_context(format!(
        "failed to finish gzip stream for {}",
        output.display()
    ))?;
    let mut file = encoder
        .finish()
        .with_context(format!("failed to write {}", output.display()))?;
    file.flush()
        .with_context(format!("failed to flush {}", output.display()))?;

    let size = fs::metadata(&output)
        .with_context(format!("failed to stat {}", output.display()))?
        .len();
    println!(
        "exported {} files, {} bytes: {}",
        export.files.len() + 2,
        size,
        output.display()
    );
    Ok(0)
}

#[derive(Clone, Debug)]
struct ExportPlan {
    files: Vec<PathBuf>,
    manifest: String,
    restore: String,
}

fn collect_export(config: &NrConfig, include_secrets: bool) -> Result<ExportPlan> {
    let mut files = BTreeSet::new();
    collect_nix_files(
        &config.target.path,
        &config.target.path,
        include_secrets,
        &mut files,
    )?;
    let lock = PathBuf::from("flake.lock");
    if include_file(&config.target.path.join(&lock), &lock, include_secrets)? {
        files.insert(lock);
    }

    let nix_files = files
        .iter()
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("nix"))
        .cloned()
        .collect::<Vec<_>>();
    for nix_file in nix_files {
        let text = fs::read_to_string(config.target.path.join(&nix_file))
            .with_context(format!("failed to read {}", nix_file.display()))?;
        for literal in string_literals(&text) {
            if let Some(relative) = resolve_asset_reference(&config.target.path, &literal)
                && include_file(
                    &config.target.path.join(&relative),
                    &relative,
                    include_secrets,
                )?
            {
                files.insert(relative);
            }
        }
    }

    let files = files.into_iter().collect::<Vec<_>>();
    Ok(ExportPlan {
        manifest: manifest(config, &files),
        restore: restore_doc(config),
        files,
    })
}

fn collect_nix_files(
    root: &Path,
    directory: &Path,
    include_secrets: bool,
    files: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in
        fs::read_dir(directory).with_context(format!("failed to read {}", directory.display()))?
    {
        let entry =
            entry.with_context(format!("failed to read entry in {}", directory.display()))?;
        let path = entry.path();
        let relative = relative_path(root, &path)?;
        if should_skip_entry(&path, &relative)? {
            continue;
        }
        let metadata = entry
            .metadata()
            .with_context(format!("failed to stat {}", path.display()))?;
        if metadata.is_dir() {
            collect_nix_files(root, &path, include_secrets, files)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("nix")
            && include_file(&path, &relative, include_secrets)?
        {
            files.insert(relative);
        }
    }
    Ok(())
}

fn include_file(path: &Path, relative: &Path, include_secrets: bool) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    if is_under_nix_store(path)
        || symlink_points_to_nix_store(path)?
        || relative.components().any(forbidden_component)
    {
        return Ok(false);
    }
    if is_result_symlink(path, relative)? {
        return Ok(false);
    }
    if !include_secrets && looks_like_secret(relative) {
        return Ok(false);
    }
    Ok(path.is_file())
}

fn should_skip_entry(path: &Path, relative: &Path) -> Result<bool> {
    Ok(relative.components().any(forbidden_component)
        || is_under_nix_store(path)
        || is_result_symlink(path, relative)?)
}

fn forbidden_component(component: Component<'_>) -> bool {
    let Component::Normal(name) = component else {
        return false;
    };
    matches!(
        name.to_str(),
        Some(".git" | "node_modules" | "target" | ".direnv" | "__pycache__")
    )
}

fn is_under_nix_store(path: &Path) -> bool {
    path.starts_with("/nix/store")
}

fn symlink_points_to_nix_store(path: &Path) -> Result<bool> {
    let metadata =
        fs::symlink_metadata(path).with_context(format!("failed to inspect {}", path.display()))?;
    if !metadata.file_type().is_symlink() {
        return Ok(false);
    }
    let target =
        fs::read_link(path).with_context(format!("failed to read symlink {}", path.display()))?;
    Ok(target.starts_with("/nix/store"))
}

fn is_result_symlink(path: &Path, relative: &Path) -> Result<bool> {
    let Some(name) = relative.file_name().and_then(|value| value.to_str()) else {
        return Ok(false);
    };
    if !name.starts_with("result") {
        return Ok(false);
    }
    let metadata =
        fs::symlink_metadata(path).with_context(format!("failed to inspect {}", path.display()))?;
    Ok(metadata.file_type().is_symlink())
}

fn looks_like_secret(relative: &Path) -> bool {
    let Some(name) = relative.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    lower == ".env"
        || lower.ends_with(".key")
        || lower.ends_with(".pem")
        || lower.ends_with(".cert")
        || lower.ends_with(".secret")
        || lower.starts_with("id_rsa")
        || lower.starts_with("id_ed25519")
        || lower.starts_with("secrets.")
}

fn resolve_asset_reference(root: &Path, literal: &str) -> Option<PathBuf> {
    let trimmed = literal.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("git+")
        || trimmed.contains("${")
    {
        return None;
    }
    let candidate = trimmed.strip_prefix("./").unwrap_or(trimmed);
    let path = root.join(candidate);
    if !path.is_file() {
        return None;
    }
    relative_path(root, &path).ok()
}

fn string_literals(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'"' {
            let (value, next) = double_quoted_literal(bytes, index + 1);
            values.push(value);
            index = next;
        } else if bytes.get(index..index + 2) == Some(b"''") {
            let (value, next) = indented_literal(bytes, index + 2);
            values.push(value);
            index = next;
        } else {
            index += 1;
        }
    }
    values
}

fn double_quoted_literal(bytes: &[u8], mut index: usize) -> (String, usize) {
    let mut value = Vec::new();
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        index += 1;
        if escaped {
            value.push(byte);
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            continue;
        }
        if byte == b'"' {
            break;
        }
        value.push(byte);
    }
    (String::from_utf8_lossy(&value).into_owned(), index)
}

fn indented_literal(bytes: &[u8], mut index: usize) -> (String, usize) {
    let mut value = Vec::new();
    while index < bytes.len() {
        if bytes.get(index..index + 2) == Some(b"''") {
            index += 2;
            break;
        }
        value.push(bytes[index]);
        index += 1;
    }
    (String::from_utf8_lossy(&value).into_owned(), index)
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

fn manifest(config: &NrConfig, files: &[PathBuf]) -> String {
    let generations = load_system_generations().unwrap_or_default();
    let current = current_generation(&generations);
    let nixos = current
        .map(|generation| generation.nixos_version.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let mut text = vec![
        "# nr export manifest".to_string(),
        String::new(),
        format!("- flake host: {}", config.target.host),
        format!("- NixOS version: {nixos}"),
        format!("- exported at: {}", timestamp()),
        format!("- nr version: {}", VERSION),
        String::new(),
        "## Files".to_string(),
    ];
    for file in files {
        text.push(format!("- {}", file.display()));
    }
    text.push("- MANIFEST.md".to_string());
    text.push("- RESTORE.md".to_string());
    text.push(String::new());
    text.join("\n")
}

fn restore_doc(config: &NrConfig) -> String {
    format!(
        "# Restore nr export\n\n1. Copy this archive to the fresh NixOS machine.\n2. Unpack it into the directory that will hold your flake.\n3. Review `MANIFEST.md` and inspect the included files.\n4. If needed, copy the unpacked files into `/etc/nixos` or your chosen flake directory.\n5. Run `nix flake check` from the restored directory.\n6. Run `sudo nixos-rebuild switch --flake .#{}`.\n7. Install `nr`, then use `nr --flake .#{} doctor` and `nr --flake .#{} switch` for future changes.\n",
        config.target.host, config.target.host, config.target.host
    )
}

fn append_generated_file<W: Write>(
    archive: &mut Builder<W>,
    path: &Path,
    contents: &str,
) -> Result<()> {
    println!("{}", path.display());
    let mut header = Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(timestamp());
    header.set_cksum();
    archive
        .append_data(&mut header, path, contents.as_bytes())
        .with_context(format!("failed to add {}", path.display()))
}

fn default_output_path() -> PathBuf {
    PathBuf::from(format!("./nr-export-{}.tar.gz", timestamp()))
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
