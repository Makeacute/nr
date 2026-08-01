use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::backend;
use crate::cli::{
    ApplyArgs, Cli, DiffArgs, GcArgs, HistoryArgs, LifecycleArgs, LogsArgs, RollbackArgs,
    ShowReportArgs, UpdateArgs,
};
use crate::config::{
    FlakeTarget, HookCommand, NrConfig, load_config, split_flake_reference, validate_flake_path,
};
use crate::errors::{NrError, Result};
use crate::events::{BuildState, ParsedLine, parse_line};
use crate::generations::{
    current_generation as listed_current_generation, generation_by_number, generation_path,
    load_pins, load_system_generations, previous_generation, resolve_generation_reference,
    rollback_target_command,
};
use crate::git::{current_revision, ensure_git_flake_visible, git_command, git_summary};
use crate::impact::{
    ActivationImpact, ClosureDiff, current_generation, current_generation_info_for_options,
    current_system_path_for_options, diff_current_to_new, parse_activation_impact,
    parse_closure_diff, reboot_recommendation, resolve_result_link,
};
use crate::process::{
    CommandSpec, LogFile, RunOutput, StreamEvent, StreamLine, run_capture, run_capture_interactive,
    run_capture_timeout, run_inherit, stream_command, stream_command_events,
    stream_command_to_command,
};
use crate::prompts::confirm;
use crate::state;
use crate::ui::{OutputMode, RebuildHeader, RebuildReport, Renderer, report_value};

pub fn run_lifecycle_command(action: &str, cli: &Cli, args: &LifecycleArgs) -> Result<i32> {
    if let Some(plan) = &args.from_plan {
        return run_lifecycle_from_plan(action, cli, plan, &args.backend_args);
    }
    run_lifecycle(action, cli, &args.backend_args)
}

pub fn run_lifecycle(action: &str, cli: &Cli, backend_args: &[String]) -> Result<i32> {
    let config = load_config(cli.config_input())?;
    ensure_git_flake_visible(&config.target.path)?;

    let preview = action == "preview" || cli.dry;
    let command_name = if preview { "preview" } else { action };
    let options = lifecycle_backend_options(action, preview, cli, &config, backend_args);
    let mut log = LogFile::create_with_limit(cli.log_file.clone(), config.state.keep_logs)?;
    let mut renderer = Renderer::new_for_lifecycle(cli.ui, command_name, config.ui.clone());
    let header = RebuildHeader {
        command: command_name.to_string(),
        target: config.target.clone(),
        git: git_summary(&config.target.path),
        current: current_generation_info_for_options(&options),
        log_path: log.path().to_path_buf(),
    };
    renderer.start(&header);

    if !config.hooks.pre_build.is_empty() {
        renderer.phase("pre-build hooks");
        run_hook_phase("pre_build", &config, None, &mut log, &mut renderer)?;
    }

    let _temp_dir;
    let build_cwd = if action == "build" && !preview {
        _temp_dir = None;
        env::current_dir().map_err(|error| NrError::Io {
            context: "failed to determine current directory".to_string(),
            source: error,
        })?
    } else {
        let directory = tempfile::Builder::new()
            .prefix("nr-build-")
            .tempdir()
            .map_err(|error| NrError::Io {
                context: "failed to create build directory".to_string(),
                source: error,
            })?;
        let path = directory.path().to_path_buf();
        _temp_dir = Some(directory);
        path
    };

    renderer.phase("evaluating/building");
    let build_command =
        backend::nixos_rebuild_build_command(&config.target, &options).cwd(build_cwd.clone());
    let build = stream_nix_build(&build_command, &mut log, &mut renderer)?;
    if build.code != 0 {
        let report = failure_report(
            ReportContext {
                command_name,
                config: &config,
                header: &header,
            },
            "build failed",
            None,
            None,
            None,
            build.state,
        );
        finish_lifecycle(cli, &mut renderer, &report, false);
        persist_report_state(&config, &report, false)?;
        run_failure_hooks(&config, &mut log, &mut renderer, "build failed")?;
        log.flush()?;
        return Err(NrError::CommandFailed {
            command: build_command.render(),
            code: build.code,
        });
    }
    let build_state = build.state;

    let store_path = resolve_result_link(&build_cwd)?;
    if !config.hooks.post_build.is_empty() {
        renderer.phase("post-build hooks");
        run_hook_phase(
            "post_build",
            &config,
            Some(&store_path),
            &mut log,
            &mut renderer,
        )?;
    }
    renderer.phase("diffing");
    let diff =
        diff_current_to_new(&store_path, &options, &mut log).unwrap_or_else(|error| ClosureDiff {
            unavailable: Some(error.to_string()),
            ..ClosureDiff::default()
        });
    renderer.diff(&diff);

    let mut activation = None;
    if preview || matches!(action, "switch" | "test") {
        renderer.phase("dry activation");
        activation = Some(run_dry_activation(
            &store_path,
            &options,
            &mut log,
            &mut renderer,
            !preview,
        )?);
    }

    if preview || action == "build" {
        let report = success_report(
            ReportContext {
                command_name,
                config: &config,
                header: &header,
            },
            &store_path,
            diff,
            activation,
            None,
            build_state,
        );
        finish_lifecycle(cli, &mut renderer, &report, true);
        persist_success_state(&config, &header, &report, &options, preview)?;
        log.flush()?;
        return Ok(0);
    }

    if action == "boot" {
        renderer.phase("boot registration");
    } else {
        renderer.phase("activation");
    }

    if cli.ask
        && !confirm(
            &format!("Run nixos-rebuild {action} for this store path?"),
            false,
        )
    {
        let report = success_report(
            ReportContext {
                command_name,
                config: &config,
                header: &header,
            },
            &store_path,
            diff,
            activation,
            current_generation(),
            build_state,
        );
        finish_lifecycle(cli, &mut renderer, &report, true);
        persist_report_state(&config, &report, true)?;
        log.flush()?;
        return Ok(0);
    }

    let activation_command = backend::nixos_rebuild_activate_command(action, &store_path, &options);
    if !config.hooks.pre_activate.is_empty() {
        renderer.phase("pre-activate hooks");
        run_hook_phase(
            "pre_activate",
            &config,
            Some(&store_path),
            &mut log,
            &mut renderer,
        )?;
    }
    log.write_command(&activation_command)?;
    let code = stream_activation_command(&activation_command, &options, &mut log, &mut renderer)?;
    if code != 0 {
        let report = failure_report(
            ReportContext {
                command_name,
                config: &config,
                header: &header,
            },
            "activation failed",
            Some(store_path),
            Some(diff),
            activation,
            build_state,
        );
        finish_lifecycle(cli, &mut renderer, &report, false);
        persist_report_state(&config, &report, false)?;
        run_failure_hooks(&config, &mut log, &mut renderer, "activation failed")?;
        log.flush()?;
        return Err(NrError::CommandFailed {
            command: activation_command.render(),
            code,
        });
    }

    let mut hook_warnings = Vec::new();
    if !config.hooks.post_activate.is_empty() {
        renderer.phase("post-activate hooks");
        run_nonfatal_hook_phase(
            "post_activate",
            &config,
            Some(&store_path),
            &mut log,
            &mut renderer,
            &mut hook_warnings,
        )?;
    }

    if action == "switch" && !config.hooks.post_switch.is_empty() {
        renderer.phase("post-switch hooks");
        run_nonfatal_hook_phase(
            "post_switch",
            &config,
            Some(&store_path),
            &mut log,
            &mut renderer,
            &mut hook_warnings,
        )?;
    }

    let mut report = success_report(
        ReportContext {
            command_name,
            config: &config,
            header: &header,
        },
        &store_path,
        diff,
        activation,
        current_generation(),
        build_state,
    );
    apply_hook_warnings(&mut report, &hook_warnings);
    finish_lifecycle(cli, &mut renderer, &report, true);
    persist_success_state(&config, &header, &report, &options, preview)?;
    log.flush()?;
    Ok(0)
}

pub fn run_update(cli: &Cli, config: &NrConfig, args: &UpdateArgs) -> Result<i32> {
    ensure_git_flake_visible(&config.target.path)?;
    let lockfile_snapshot = if args.revert_on_failure || !args.inputs.is_empty() {
        Some(LockfileSnapshot::capture(&config.target.path)?)
    } else {
        None
    };
    let options = cli.backend_options_with_config(config, &[]);
    let command = backend::nix_flake_update_command(&config.target, &args.inputs, &options);
    let code = run_inherit(&command, true)?;
    if code != 0 {
        restore_lockfile_after_failure(args.revert_on_failure, lockfile_snapshot.as_ref());
        return Err(NrError::CommandFailed {
            command: command.render(),
            code,
        });
    }
    show_lockfile_diff(
        &config.target.path,
        &args.inputs,
        lockfile_snapshot.as_ref(),
    )?;
    if args.switch {
        match run_lifecycle("switch", cli, &[]) {
            Ok(code) => Ok(code),
            Err(error) => {
                restore_lockfile_after_failure(args.revert_on_failure, lockfile_snapshot.as_ref());
                Err(error)
            }
        }
    } else {
        Ok(0)
    }
}

fn show_lockfile_diff(
    flake_path: &Path,
    requested_inputs: &[String],
    lockfile_before: Option<&LockfileSnapshot>,
) -> Result<()> {
    if !flake_path.join(".git").exists() {
        return Ok(());
    }
    let status = run_capture(
        &git_command(flake_path, &["status", "--short", "--", "flake.lock"]),
        false,
    )?;
    if status.stdout.trim().is_empty() {
        return Ok(());
    }
    if !requested_inputs.is_empty()
        && let Some(before) = lockfile_before
        && show_focused_lockfile_diff(flake_path, requested_inputs, before)?
    {
        return Ok(());
    }
    println!("flake.lock changed:");
    let code = run_inherit(
        &git_command(flake_path, &["--no-pager", "diff", "--", "flake.lock"]),
        true,
    )?;
    if code != 0 {
        eprintln!("warning: git diff exited with {code}");
    }
    Ok(())
}

fn show_focused_lockfile_diff(
    flake_path: &Path,
    requested_inputs: &[String],
    before: &LockfileSnapshot,
) -> Result<bool> {
    let Some(before_contents) = &before.contents else {
        return Ok(false);
    };
    let lock_path = flake_path.join("flake.lock");
    let after_contents = fs::read(&lock_path).map_err(|source| NrError::Io {
        context: format!("failed to read {}", lock_path.display()),
        source,
    })?;
    let Some(diff) = focused_lockfile_diff(before_contents, &after_contents, requested_inputs)
    else {
        return Ok(false);
    };

    println!("flake.lock changed:");
    if diff.requested.is_empty() {
        println!(
            "  requested inputs unchanged: {}",
            requested_inputs.join(", ")
        );
    } else {
        println!("  requested input changes:");
        for change in &diff.requested {
            println!("    {}:", change.input);
            if change.fields.is_empty() {
                println!("      locked: changed");
            } else {
                for field in &change.fields {
                    println!("      {}: {} -> {}", field.name, field.before, field.after);
                }
            }
        }
    }
    if diff.other_changed > 0 {
        eprintln!(
            "warning: {} other lock node(s) changed; run `git -C {} diff -- flake.lock` to inspect them",
            diff.other_changed,
            flake_path.display()
        );
    }
    Ok(true)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FocusedLockDiff {
    requested: Vec<FocusedInputChange>,
    other_changed: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FocusedInputChange {
    input: String,
    fields: Vec<FocusedFieldChange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FocusedFieldChange {
    name: String,
    before: String,
    after: String,
}

fn focused_lockfile_diff(
    before_contents: &[u8],
    after_contents: &[u8],
    requested_inputs: &[String],
) -> Option<FocusedLockDiff> {
    let before: Value = serde_json::from_slice(before_contents).ok()?;
    let after: Value = serde_json::from_slice(after_contents).ok()?;
    let before_nodes = before.get("nodes")?.as_object()?;
    let after_nodes = after.get("nodes")?.as_object()?;
    let requested = requested_inputs
        .iter()
        .map(|input| {
            (
                input.as_str(),
                lock_node_for_input(&after, input).unwrap_or(input.as_str()),
            )
        })
        .collect::<Vec<_>>();
    let requested_nodes = requested
        .iter()
        .map(|(_, node)| *node)
        .collect::<std::collections::BTreeSet<_>>();

    let mut requested_changes = Vec::new();
    for (input, node) in requested {
        let before_locked = before_nodes.get(node).and_then(|node| node.get("locked"));
        let after_locked = after_nodes.get(node).and_then(|node| node.get("locked"));
        if before_locked == after_locked {
            continue;
        }
        requested_changes.push(FocusedInputChange {
            input: input.to_string(),
            fields: focused_field_changes(before_locked, after_locked),
        });
    }

    let mut changed_nodes = std::collections::BTreeSet::new();
    for node in before_nodes.keys().chain(after_nodes.keys()) {
        let before_locked = before_nodes.get(node).and_then(|node| node.get("locked"));
        let after_locked = after_nodes.get(node).and_then(|node| node.get("locked"));
        if before_locked != after_locked {
            changed_nodes.insert(node.as_str());
        }
    }
    let other_changed = changed_nodes
        .into_iter()
        .filter(|node| !requested_nodes.contains(node))
        .count();

    Some(FocusedLockDiff {
        requested: requested_changes,
        other_changed,
    })
}

fn lock_node_for_input<'a>(lock: &'a Value, input: &str) -> Option<&'a str> {
    let first = input.split('/').next().filter(|value| !value.is_empty())?;
    let root = lock.get("root").and_then(Value::as_str).unwrap_or("root");
    let reference = lock.get("nodes")?.get(root)?.get("inputs")?.get(first)?;
    if let Some(node) = reference.as_str() {
        Some(node)
    } else {
        reference
            .as_array()
            .and_then(|values| values.last())
            .and_then(Value::as_str)
    }
}

fn focused_field_changes(
    before_locked: Option<&Value>,
    after_locked: Option<&Value>,
) -> Vec<FocusedFieldChange> {
    let mut fields = Vec::new();
    for name in [
        "rev",
        "narHash",
        "lastModified",
        "ref",
        "owner",
        "repo",
        "type",
    ] {
        let before = before_locked.and_then(|value| value.get(name));
        let after = after_locked.and_then(|value| value.get(name));
        if before != after {
            fields.push(FocusedFieldChange {
                name: name.to_string(),
                before: lock_value_text(before),
                after: lock_value_text(after),
            });
        }
    }
    fields
}

fn lock_value_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
        None => "<missing>".to_string(),
    }
}

#[derive(Clone)]
struct LockfileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

impl LockfileSnapshot {
    fn capture(flake_path: &Path) -> Result<Self> {
        let path = flake_path.join("flake.lock");
        let contents = match fs::read(&path) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(NrError::Io {
                    context: format!("failed to read {}", path.display()),
                    source,
                });
            }
        };
        Ok(Self { path, contents })
    }

    fn restore(&self) -> Result<()> {
        match &self.contents {
            Some(contents) => fs::write(&self.path, contents).map_err(|source| NrError::Io {
                context: format!("failed to restore {}", self.path.display()),
                source,
            }),
            None => match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(source) => Err(NrError::Io {
                    context: format!("failed to remove {}", self.path.display()),
                    source,
                }),
            },
        }
    }
}

fn restore_lockfile_after_failure(enabled: bool, snapshot: Option<&LockfileSnapshot>) {
    if !enabled {
        return;
    }
    let Some(snapshot) = snapshot else {
        return;
    };
    match snapshot.restore() {
        Ok(()) => eprintln!("reverted flake.lock to its pre-update state"),
        Err(error) => eprintln!("warning: failed to revert flake.lock: {error}"),
    }
}

pub fn run_apply(cli: &Cli, args: &ApplyArgs) -> Result<i32> {
    run_lifecycle_from_plan(args.action.as_str(), cli, &args.plan, &args.backend_args)
}

fn run_lifecycle_from_plan(
    action: &str,
    cli: &Cli,
    plan_reference: &str,
    backend_args: &[String],
) -> Result<i32> {
    let plan_path = state::resolve_json_reference(&state::plans_dir(), "plan", plan_reference)?;
    let plan_text = fs::read_to_string(&plan_path).map_err(|source| NrError::Io {
        context: format!("failed to read {}", plan_path.display()),
        source,
    })?;
    let plan: PreviewPlan = serde_json::from_str(&plan_text)
        .map_err(|error| NrError::message(format!("failed to parse preview plan: {error}")))?;
    let mut config = load_config(cli.config_input())?;
    config.target = FlakeTarget {
        path: plan.target.path.clone(),
        host: plan.target.host.clone(),
    };
    let mut options = backend_options_for_plan(cli, &config, backend_args, &plan.backend_options);
    apply_default_elevation(action, false, &mut options);

    let mut log = LogFile::create_with_limit(cli.log_file.clone(), config.state.keep_logs)?;
    let mut renderer = Renderer::new_for_lifecycle(cli.ui, action, config.ui.clone());
    let header = RebuildHeader {
        command: action.to_string(),
        target: config.target.clone(),
        git: git_summary(&config.target.path),
        current: current_generation_info_for_options(&options),
        log_path: log.path().to_path_buf(),
    };
    renderer.start(&header);
    renderer.phase("using saved preview plan");
    log.write_line(
        crate::process::StreamSource::Stdout,
        &format!("using preview plan {}", plan_path.display()),
    )?;

    let diff = diff_current_to_new(&plan.store_path, &options, &mut log).unwrap_or_else(|error| {
        ClosureDiff {
            unavailable: Some(error.to_string()),
            ..ClosureDiff::default()
        }
    });
    renderer.diff(&diff);

    let mut activation = None;
    if matches!(action, "switch" | "test") {
        renderer.phase("dry activation");
        activation = Some(run_dry_activation(
            &plan.store_path,
            &options,
            &mut log,
            &mut renderer,
            true,
        )?);
    }

    let mut hook_warnings = Vec::new();
    if action == "boot" {
        renderer.phase("boot registration");
    } else {
        renderer.phase("activation");
    }
    if !config.hooks.pre_activate.is_empty() {
        renderer.phase("pre-activate hooks");
        run_hook_phase(
            "pre_activate",
            &config,
            Some(&plan.store_path),
            &mut log,
            &mut renderer,
        )?;
    }
    let activation_command =
        backend::nixos_rebuild_activate_command(action, &plan.store_path, &options);
    log.write_command(&activation_command)?;
    let code = stream_activation_command(&activation_command, &options, &mut log, &mut renderer)?;
    if code != 0 {
        let report = failure_report(
            ReportContext {
                command_name: action,
                config: &config,
                header: &header,
            },
            "activation failed",
            Some(plan.store_path),
            Some(diff),
            activation,
            BuildState::default(),
        );
        finish_lifecycle(cli, &mut renderer, &report, false);
        persist_report_state(&config, &report, false)?;
        run_failure_hooks(&config, &mut log, &mut renderer, "activation failed")?;
        log.flush()?;
        return Err(NrError::CommandFailed {
            command: activation_command.render(),
            code,
        });
    }

    if !config.hooks.post_activate.is_empty() {
        renderer.phase("post-activate hooks");
        run_nonfatal_hook_phase(
            "post_activate",
            &config,
            Some(&plan.store_path),
            &mut log,
            &mut renderer,
            &mut hook_warnings,
        )?;
    }
    if action == "switch" && !config.hooks.post_switch.is_empty() {
        renderer.phase("post-switch hooks");
        run_nonfatal_hook_phase(
            "post_switch",
            &config,
            Some(&plan.store_path),
            &mut log,
            &mut renderer,
            &mut hook_warnings,
        )?;
    }

    let mut report = success_report(
        ReportContext {
            command_name: action,
            config: &config,
            header: &header,
        },
        &plan.store_path,
        diff,
        activation,
        current_generation(),
        BuildState::default(),
    );
    apply_hook_warnings(&mut report, &hook_warnings);
    finish_lifecycle(cli, &mut renderer, &report, true);
    persist_success_state(&config, &header, &report, &options, false)?;
    log.flush()?;
    Ok(0)
}

pub fn run_rollback(cli: &Cli, args: &RollbackArgs) -> Result<i32> {
    let options = rollback_backend_options(cli, &args.backend_args);
    let generations = match load_system_generations() {
        Ok(generations) => generations,
        Err(error) => {
            eprintln!("warning: failed to inspect system generations: {error}");
            Vec::new()
        }
    };
    let pins = match load_pins() {
        Ok(pins) => pins,
        Err(error) if args.target.is_none() => {
            eprintln!("warning: failed to read generation pins: {error}");
            Default::default()
        }
        Err(error) => return Err(error),
    };
    let target_generation = if let Some(target) = &args.target {
        Some(resolve_generation_reference(target, &pins)?)
    } else {
        previous_generation(&generations).map(|generation| generation.generation)
    };
    print_rollback_target(&generations, target_generation);
    wait_for_rollback_confirmation()?;

    let command = rollback_target_command(
        target_generation.filter(|_| args.target.is_some()),
        &options,
    );
    let code = run_inherit(&command, true)?;
    if code != 0 {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code,
        });
    }
    println!("rollback complete. To inspect generations, run: nr generations");
    Ok(0)
}

pub fn run_gc(args: &GcArgs) -> Result<i32> {
    let command =
        backend::nix_collect_garbage_command(&args.older_than, args.delete_old, args.dry_run);
    let code = run_inherit(&command, true)?;
    if code != 0 {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code,
        });
    }
    Ok(0)
}

fn backend_options_for_plan(
    cli: &Cli,
    config: &NrConfig,
    backend_args: &[String],
    plan_options: &backend::BackendOptions,
) -> backend::BackendOptions {
    let cli_options = cli.backend_options_with_config(config, backend_args);
    let mut options = plan_options.clone();

    if cli.verbose > 0 {
        options.verbose = cli_options.verbose;
    }
    options.offline |= cli_options.offline;
    options.show_trace |= cli_options.show_trace;
    if cli.specialisation.is_some() {
        options.specialisation = cli_options.specialisation;
    }
    if cli.elevate.is_some() {
        options.elevate = cli_options.elevate;
    }
    options.ask_elevate_password |= cli_options.ask_elevate_password;
    if cli.target_host.is_some() || options.target_host.is_none() {
        options.target_host = cli_options.target_host;
    }
    if cli.build_host.is_some() || options.build_host.is_none() {
        options.build_host = cli_options.build_host;
    }
    options.use_remote_sudo |= cli_options.use_remote_sudo;
    if !backend_args.is_empty() {
        options.backend_args = cli_options.backend_args;
    }

    options
}

pub fn run_history(args: &HistoryArgs) -> Result<i32> {
    let path = state::history_path();
    if !path.is_file() {
        println!("No switch history recorded.");
        return Ok(0);
    }
    let text = fs::read_to_string(&path).map_err(|source| NrError::Io {
        context: format!("failed to read {}", path.display()),
        source,
    })?;
    let history = serde_json::from_str::<HistoryFile>(&text)
        .map_err(|error| NrError::message(format!("failed to parse history: {error}")))?;
    for entry in history.entries.iter().rev().take(args.limit) {
        println!(
            "{} {} {} old:{:?} new:{:?} {} log:{}",
            entry.timestamp,
            entry.action,
            entry.target,
            entry.old_generation,
            entry.new_generation,
            entry.report_result,
            entry.log_path.display()
        );
    }
    Ok(0)
}

pub fn run_logs(args: &LogsArgs) -> Result<i32> {
    if args.last_failed {
        let mut reports = state::sorted_json_files(&state::reports_dir(), "report")?;
        reports.reverse();
        for report in reports {
            let text = fs::read_to_string(&report).map_err(|source| NrError::Io {
                context: format!("failed to read {}", report.display()),
                source,
            })?;
            let stored = serde_json::from_str::<StoredReport>(&text).map_err(|error| {
                NrError::message(format!(
                    "failed to parse report {}: {error}",
                    report.display()
                ))
            })?;
            if !stored.success {
                if let Some(log_path) = stored
                    .report
                    .get("log_path")
                    .and_then(|value| value.as_str())
                {
                    println!("{log_path}");
                } else {
                    println!("{}", report.display());
                }
                return Ok(0);
            }
        }
        println!("No failed reports retained.");
        return Ok(0);
    }

    for path in state::sorted_log_files()?
        .into_iter()
        .rev()
        .take(args.limit)
    {
        println!("{}", path.display());
    }
    for path in state::sorted_json_files(&state::reports_dir(), "report")?
        .into_iter()
        .rev()
        .take(args.limit)
    {
        println!("{}", path.display());
    }
    Ok(0)
}

pub fn run_show_report(args: &ShowReportArgs) -> Result<i32> {
    let path = state::resolve_json_reference(&state::reports_dir(), "report", &args.report)?;
    let text = fs::read_to_string(&path).map_err(|source| NrError::Io {
        context: format!("failed to read {}", path.display()),
        source,
    })?;
    println!("{text}");
    Ok(0)
}

fn print_rollback_target(
    generations: &[crate::generations::SystemGeneration],
    target_generation: Option<u64>,
) {
    let current = listed_current_generation(generations);
    if let Some(target_number) = target_generation {
        let target = generation_by_number(generations, target_number);
        match (current, target) {
            (Some(current), Some(target)) => println!(
                "Rolling back from generation {} ({}) to generation {} ({})",
                current.generation, current.date, target.generation, target.date
            ),
            (Some(current), None) => println!(
                "Rolling back from generation {} ({}) to generation {}",
                current.generation, current.date, target_number
            ),
            (None, Some(target)) => println!(
                "Rolling back to generation {} ({})",
                target.generation, target.date
            ),
            (None, None) => println!("Rolling back to generation {target_number}"),
        }
    } else if let Some(current) = current {
        println!(
            "Rolling back from generation {} ({}) to the previous generation",
            current.generation, current.date
        );
    } else {
        println!("Rolling back to the previous generation");
    }
}

fn wait_for_rollback_confirmation() -> Result<()> {
    if !io::stdin().is_terminal() {
        return Ok(());
    }
    println!("Press Enter to confirm, or Ctrl-C to abort.");
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|source| NrError::Io {
            context: "failed to read rollback confirmation".to_string(),
            source,
        })?;
    Ok(())
}

pub fn run_diff(cli: &Cli, args: &DiffArgs) -> Result<i32> {
    let config = load_config(cli.config_input())?;
    let options = cli.backend_options_with_config(&config, &args.backend_args);
    let mut log = LogFile::create_with_limit(cli.log_file.clone(), config.state.keep_logs)?;
    let mut renderer = Renderer::new_for_lifecycle(cli.ui, "diff", config.ui.clone());
    let header = RebuildHeader {
        command: "diff".to_string(),
        target: config.target.clone(),
        git: git_summary(&config.target.path),
        current: current_generation_info_for_options(&options),
        log_path: log.path().to_path_buf(),
    };
    renderer.start(&header);

    let from = resolve_diff_from(args.from.as_deref(), &options)?;
    let to = resolve_diff_to(
        args.to.as_deref(),
        &config.target,
        &options,
        &mut log,
        &mut renderer,
    )?;
    renderer.phase("diffing");
    let diff = diff_paths(&from.path, &to.path, &options, &mut log)?;
    renderer.diff(&diff);

    let report = RebuildReport {
        command: "diff".to_string(),
        target: config.target.clone(),
        result: format!("diff complete: {} -> {}", from.label, to.label),
        store_path: Some(to.path),
        current: header.current,
        new_generation: None,
        build: BuildState::default(),
        reboot: reboot_recommendation("diff", &diff),
        rollback: "nr rollback".to_string(),
        diff: Some(diff),
        activation: None,
        log_path: header.log_path,
    };
    finish_lifecycle(cli, &mut renderer, &report, true);
    persist_report_state(&config, &report, true)?;
    log.flush()?;
    Ok(0)
}

struct DiffEndpoint {
    path: PathBuf,
    label: String,
    _temp_dir: Option<tempfile::TempDir>,
}

fn resolve_diff_from(
    value: Option<&str>,
    options: &backend::BackendOptions,
) -> Result<DiffEndpoint> {
    if let Some(value) = value {
        let path = generation_or_path(value)?;
        return Ok(DiffEndpoint {
            label: value.to_string(),
            path,
            _temp_dir: None,
        });
    }
    let path = current_system_path_for_options(options).ok_or_else(|| {
        NrError::message("failed to inspect /run/current-system on remote target")
    })?;
    let label = if let Some(host) = options.target_host.as_deref() {
        format!("{host}:/run/current-system")
    } else {
        "/run/current-system".to_string()
    };
    Ok(DiffEndpoint {
        path,
        label,
        _temp_dir: None,
    })
}

fn resolve_diff_to(
    value: Option<&str>,
    default_target: &FlakeTarget,
    options: &backend::BackendOptions,
    log: &mut LogFile,
    renderer: &mut Renderer,
) -> Result<DiffEndpoint> {
    if let Some(value) = value {
        if let Some(target) = flake_target_from_reference(value, &default_target.host)? {
            return build_diff_target(target, options, log, renderer);
        }
        let path = generation_or_path(value)?;
        return Ok(DiffEndpoint {
            label: value.to_string(),
            path,
            _temp_dir: None,
        });
    }
    build_diff_target(default_target.clone(), options, log, renderer)
}

fn build_diff_target(
    target: FlakeTarget,
    options: &backend::BackendOptions,
    log: &mut LogFile,
    renderer: &mut Renderer,
) -> Result<DiffEndpoint> {
    if target.path.join("flake.nix").is_file() {
        ensure_git_flake_visible(&target.path)?;
    }
    renderer.phase("evaluating/building");
    let directory = tempfile::Builder::new()
        .prefix("nr-diff-")
        .tempdir()
        .map_err(|error| NrError::Io {
            context: "failed to create diff build directory".to_string(),
            source: error,
        })?;
    let build_command =
        backend::nixos_rebuild_build_command(&target, options).cwd(directory.path().to_path_buf());
    let build = stream_nix_build(&build_command, log, renderer)?;
    if build.code != 0 {
        return Err(NrError::CommandFailed {
            command: build_command.render(),
            code: build.code,
        });
    }
    let path = resolve_result_link(directory.path())?;
    Ok(DiffEndpoint {
        label: target.reference(),
        path,
        _temp_dir: Some(directory),
    })
}

fn generation_or_path(value: &str) -> Result<PathBuf> {
    if let Ok(generation) = value.parse::<u64>() {
        return Ok(generation_path(generation));
    }
    Ok(PathBuf::from(value))
}

fn flake_target_from_reference(value: &str, default_host: &str) -> Result<Option<FlakeTarget>> {
    let (path_text, host) = split_flake_reference(value)?;
    let path = PathBuf::from(&path_text);
    let path = if path.is_absolute() {
        path
    } else {
        env::current_dir()
            .map_err(|error| NrError::Io {
                context: "failed to determine current directory".to_string(),
                source: error,
            })?
            .join(path)
    };
    if path.join("flake.nix").is_file() {
        validate_flake_path(&path)?;
        Ok(Some(FlakeTarget {
            path,
            host: host.unwrap_or_else(|| default_host.to_string()),
        }))
    } else if host.is_some() && is_explicit_local_flake_reference(&path_text) {
        Err(NrError::message(format!(
            "No flake.nix found for --to flake reference: {value}"
        )))
    } else if host.is_some() || looks_like_flake_uri(&path_text) {
        Ok(Some(FlakeTarget {
            path: PathBuf::from(path_text),
            host: host.unwrap_or_else(|| default_host.to_string()),
        }))
    } else {
        Ok(None)
    }
}

fn is_explicit_local_flake_reference(value: &str) -> bool {
    let path = Path::new(value);
    path.is_absolute()
        || matches!(value, "." | "..")
        || value.starts_with("./")
        || value.starts_with("../")
        || (value.contains('/') && !looks_like_flake_uri(value))
}

fn looks_like_flake_uri(value: &str) -> bool {
    value
        .split_once(':')
        .is_some_and(|(prefix, _)| !prefix.is_empty() && !prefix.contains('/'))
}

fn diff_paths(
    from: &Path,
    to: &Path,
    options: &backend::BackendOptions,
    log: &mut LogFile,
) -> Result<ClosureDiff> {
    let command = backend::nix_store_diff_closures_command(from, to, options);
    log.write_command(&command)?;
    let output = run_capture(&command, false)?;
    log.write_output(&output)?;
    if output.code != 0 {
        return Ok(ClosureDiff {
            raw: format!("{}{}", output.stdout, output.stderr),
            unavailable: Some(format!(
                "nix store diff-closures exited with {}",
                output.code
            )),
            ..ClosureDiff::default()
        });
    }
    Ok(parse_closure_diff(&output.stdout))
}

struct BuildRun {
    code: i32,
    state: BuildState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PreviewPlan {
    id: String,
    created_at: u64,
    target: FlakeTargetSnapshot,
    store_path: PathBuf,
    log_path: PathBuf,
    backend_options: backend::BackendOptions,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FlakeTargetSnapshot {
    path: PathBuf,
    host: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredReport {
    success: bool,
    saved_at: u64,
    report: serde_json::Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct HistoryFile {
    entries: Vec<HistoryEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HistoryEntry {
    timestamp: u64,
    action: String,
    target: String,
    old_generation: Option<u64>,
    new_generation: Option<u64>,
    store_path: Option<PathBuf>,
    git_revision: Option<String>,
    log_path: PathBuf,
    report_result: String,
}

fn persist_success_state(
    config: &NrConfig,
    header: &RebuildHeader,
    report: &RebuildReport,
    options: &backend::BackendOptions,
    preview: bool,
) -> Result<()> {
    let report_path = persist_report_state(config, report, true)?;
    if preview {
        let plan = PreviewPlan {
            id: format!("plan-{}-{}", state::timestamp(), std::process::id()),
            created_at: state::timestamp(),
            target: FlakeTargetSnapshot {
                path: config.target.path.clone(),
                host: config.target.host.clone(),
            },
            store_path: report
                .store_path
                .clone()
                .ok_or_else(|| NrError::message("cannot save preview plan without a store path"))?,
            log_path: header.log_path.clone(),
            backend_options: options.clone(),
        };
        let path = state::write_json(&state::plans_dir(), "plan", &plan, config.state.keep_plans)?;
        eprintln!("preview plan saved: {}", path.display());
    } else if matches!(report.command.as_str(), "switch" | "test" | "boot") {
        append_history(config, header, report)?;
    }
    eprintln!("report saved: {}", report_path.display());
    Ok(())
}

fn persist_report_state(
    config: &NrConfig,
    report: &RebuildReport,
    success: bool,
) -> Result<PathBuf> {
    let stored = StoredReport {
        success,
        saved_at: state::timestamp(),
        report: report_value(report),
    };
    state::write_json(
        &state::reports_dir(),
        "report",
        &stored,
        config.state.keep_reports,
    )
}

fn append_history(config: &NrConfig, header: &RebuildHeader, report: &RebuildReport) -> Result<()> {
    let path = state::history_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| NrError::Io {
            context: format!("failed to create {}", parent.display()),
            source,
        })?;
    }
    let mut history = if path.is_file() {
        let text = fs::read_to_string(&path).map_err(|source| NrError::Io {
            context: format!("failed to read {}", path.display()),
            source,
        })?;
        serde_json::from_str::<HistoryFile>(&text)
            .map_err(|error| NrError::message(format!("failed to parse history: {error}")))?
    } else {
        HistoryFile::default()
    };
    history.entries.push(HistoryEntry {
        timestamp: state::timestamp(),
        action: report.command.clone(),
        target: report.target.reference(),
        old_generation: header.current.generation,
        new_generation: report.new_generation,
        store_path: report.store_path.clone(),
        git_revision: current_revision(&config.target.path),
        log_path: header.log_path.clone(),
        report_result: report.result.clone(),
    });
    if history.entries.len() > config.state.keep_history {
        let remove = history.entries.len() - config.state.keep_history;
        history.entries.drain(0..remove);
    }
    let text = serde_json::to_string_pretty(&history)
        .map_err(|error| NrError::message(format!("failed to serialize history: {error}")))?;
    fs::write(&path, text).map_err(|source| NrError::Io {
        context: format!("failed to write {}", path.display()),
        source,
    })
}

fn stream_nix_build(
    command: &CommandSpec,
    log: &mut LogFile,
    renderer: &mut Renderer,
) -> Result<BuildRun> {
    log.write_command(command)?;
    let mut state = BuildState::default();
    let mut graph_loader = DependencyGraphLoader::new();
    let mut fallback_announced = false;
    let announce = should_announce_backend(renderer);
    let code = if renderer.mode() == OutputMode::Nom {
        let nom_command = backend::nom_json_command();
        log.write_command(&nom_command)?;
        stream_command_to_command(
            command,
            &nom_command,
            announce,
            |line| {
                ingest_build_line(
                    &mut state,
                    &line,
                    log,
                    renderer,
                    &mut graph_loader,
                    &mut fallback_announced,
                    IngestOptions {
                        load_dependency_graph: false,
                        render_backend: false,
                    },
                )
            },
            pipe_line_to_nom,
        )?
    } else {
        stream_command_events(command, announce, |event| match event {
            StreamEvent::Line(line) => ingest_build_line(
                &mut state,
                &line,
                log,
                renderer,
                &mut graph_loader,
                &mut fallback_announced,
                IngestOptions {
                    load_dependency_graph: true,
                    render_backend: true,
                },
            ),
            StreamEvent::Resize => {
                graph_loader.drain(&mut state, log)?;
                renderer.resize(&state);
                Ok(())
            }
        })?
    };
    graph_loader.finish(&mut state, log)?;
    Ok(BuildRun { code, state })
}

fn finish_lifecycle(cli: &Cli, renderer: &mut Renderer, report: &RebuildReport, success: bool) {
    renderer.finish(report);
    if cli.notify {
        notify_lifecycle(report, success);
    }
}

fn notify_lifecycle(report: &RebuildReport, success: bool) {
    let title = format!("nr {}", report.command);
    let body = if success {
        report.result.clone()
    } else {
        format!("failed: {}", report.result)
    };
    let command = backend::notify_send_command(&title, &body);
    match run_capture(&command, false) {
        Ok(output) if output.code == 0 => {}
        Ok(output) => eprintln!("warning: notify-send exited with {}", output.code),
        Err(NrError::MissingCommand(_)) => eprintln!("warning: notify-send is not available"),
        Err(error) => eprintln!("warning: failed to send notification: {error}"),
    }
}

fn run_hook_phase(
    phase: &str,
    config: &NrConfig,
    store_path: Option<&Path>,
    log: &mut LogFile,
    renderer: &mut Renderer,
) -> Result<()> {
    let hooks = hooks_for_phase(&config.hooks, phase);
    for hook in hooks {
        let Some((program, args)) = hook.split_first() else {
            return Err(NrError::message(format!(
                "[hooks].{phase} entries cannot be empty."
            )));
        };
        let command = CommandSpec::new(program.clone())
            .args(args.iter().cloned())
            .cwd(config.target.path.to_path_buf())
            .env("NR_HOOK", phase)
            .env("NR_TARGET", config.target.reference())
            .env("NR_FLAKE", config.target.path.display().to_string())
            .env("NR_HOST", config.target.host.clone())
            .env("NR_STORE_PATH", path_env(store_path))
            .env("NR_LOG_FILE", log.path().display().to_string());
        log.write_command(&command)?;
        let output = run_capture_timeout(
            &command,
            should_announce_backend(renderer),
            Some(Duration::from_secs(config.hooks.timeout_seconds)),
        )?;
        log.write_output(&output)?;
        render_output_lines(&output, renderer);
        let code = output.code;
        if code != 0 {
            return Err(NrError::CommandFailed {
                command: command.render(),
                code,
            });
        }
    }
    Ok(())
}

fn run_nonfatal_hook_phase(
    phase: &str,
    config: &NrConfig,
    store_path: Option<&Path>,
    log: &mut LogFile,
    renderer: &mut Renderer,
    warnings: &mut Vec<String>,
) -> Result<()> {
    match run_hook_phase(phase, config, store_path, log, renderer) {
        Ok(()) => Ok(()),
        Err(error) => {
            let warning = format!(
                "warning: {} failed after successful activation: {error}",
                hook_phase_label(phase)
            );
            log.write_line(crate::process::StreamSource::Stderr, &warning)?;
            renderer.backend_line(&StreamLine {
                source: crate::process::StreamSource::Stderr,
                line: warning.clone(),
            });
            warnings.push(warning);
            Ok(())
        }
    }
}

fn apply_hook_warnings(report: &mut RebuildReport, warnings: &[String]) {
    if !warnings.is_empty() {
        report.result = format!(
            "{}; {} post-activation hook warning(s), see log",
            report.result,
            warnings.len()
        );
    }
}

fn hook_phase_label(phase: &str) -> &str {
    match phase {
        "post_activate" => "post-activate hook",
        "post_switch" => "post-switch hook",
        _ => "hook",
    }
}

fn run_failure_hooks(
    config: &NrConfig,
    log: &mut LogFile,
    renderer: &mut Renderer,
    reason: &str,
) -> Result<()> {
    if config.hooks.on_failure.is_empty() {
        return Ok(());
    }
    renderer.phase("failure hooks");
    if let Err(error) = run_hook_phase("on_failure", config, None, log, renderer) {
        log.write_line(
            crate::process::StreamSource::Stderr,
            &format!("failure hook failed after {reason}: {error}"),
        )?;
    }
    Ok(())
}

fn hooks_for_phase<'a>(hooks: &'a crate::config::HookSettings, phase: &str) -> &'a [HookCommand] {
    match phase {
        "pre_build" => &hooks.pre_build,
        "post_build" => &hooks.post_build,
        "pre_activate" => &hooks.pre_activate,
        "post_activate" => &hooks.post_activate,
        "post_switch" => &hooks.post_switch,
        "on_failure" => &hooks.on_failure,
        _ => &[],
    }
}

fn path_env(path: Option<&Path>) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_default()
}

fn render_output_lines(output: &RunOutput, renderer: &mut Renderer) {
    for line in output.stdout.lines() {
        renderer.backend_line(&StreamLine {
            source: crate::process::StreamSource::Stdout,
            line: line.to_string(),
        });
    }
    for line in output.stderr.lines() {
        renderer.backend_line(&StreamLine {
            source: crate::process::StreamSource::Stderr,
            line: line.to_string(),
        });
    }
}

fn should_announce_backend(renderer: &Renderer) -> bool {
    renderer.mode() == OutputMode::Raw
}

fn ingest_build_line(
    state: &mut BuildState,
    line: &StreamLine,
    log: &mut LogFile,
    renderer: &mut Renderer,
    graph_loader: &mut DependencyGraphLoader,
    fallback_announced: &mut bool,
    options: IngestOptions,
) -> Result<()> {
    log.write_line(line.source, &line.line)?;
    if options.load_dependency_graph {
        graph_loader.note_text(state, &line.line, log)?;
    } else {
        state.note_derivation_paths_from_text(&line.line);
    }
    if state.parser_fallback {
        if options.render_backend {
            renderer.backend_line(line);
        }
        return Ok(());
    }
    match parse_line(&line.line) {
        ParsedLine::Event(event) => {
            state.ingest(&event);
            for field in &event.fields {
                if options.load_dependency_graph {
                    graph_loader.note_text(state, field, log)?;
                } else {
                    state.note_derivation_paths_from_text(field);
                }
            }
            if options.render_backend {
                renderer.nix_event(&event, state);
            }
        }
        ParsedLine::Plain(_) => {
            if options.render_backend {
                renderer.backend_line(line);
            }
        }
        ParsedLine::BrokenInternalJson(_) => {
            state.parser_fallback = true;
            if !*fallback_announced {
                renderer.parser_fallback();
                *fallback_announced = true;
            }
            if options.render_backend {
                renderer.backend_line(line);
            }
        }
    }
    graph_loader.drain(state, log)?;
    Ok(())
}

struct DependencyGraphLoader {
    sender: mpsc::Sender<GraphLoadResult>,
    receiver: mpsc::Receiver<GraphLoadResult>,
    queued: BTreeSet<String>,
    handles: Vec<JoinHandle<()>>,
}

struct GraphLoadResult {
    root: String,
    output: Result<RunOutput>,
}

impl DependencyGraphLoader {
    fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            queued: BTreeSet::new(),
            handles: Vec::new(),
        }
    }

    fn note_text(&mut self, state: &mut BuildState, text: &str, log: &mut LogFile) -> Result<()> {
        state.note_derivation_paths_from_text(text);
        self.queue_roots(state, log)
    }

    fn queue_roots(&mut self, state: &mut BuildState, log: &mut LogFile) -> Result<()> {
        for root in state.dependency_graph_roots_to_load() {
            if !self.queued.insert(root.clone()) {
                continue;
            }
            state.mark_derivation_graph_attempted(&root);
            let command = backend::nix_store_query_graph_command(&root);
            log.write_command(&command)?;
            let sender = self.sender.clone();
            self.handles.push(thread::spawn(move || {
                let output = run_capture(&command, false);
                let _ = sender.send(GraphLoadResult { root, output });
            }));
        }
        Ok(())
    }

    fn drain(&mut self, state: &mut BuildState, log: &mut LogFile) -> Result<()> {
        while let Ok(result) = self.receiver.try_recv() {
            self.apply_result(state, log, result)?;
        }
        Ok(())
    }

    fn finish(mut self, state: &mut BuildState, log: &mut LogFile) -> Result<()> {
        for handle in self.handles.drain(..) {
            if handle.join().is_err() {
                log.write_line(
                    crate::process::StreamSource::Stderr,
                    "failed to join derivation graph loader thread",
                )?;
            }
        }
        while let Ok(result) = self.receiver.try_recv() {
            self.apply_result(state, log, result)?;
        }
        Ok(())
    }

    fn apply_result(
        &mut self,
        state: &mut BuildState,
        log: &mut LogFile,
        result: GraphLoadResult,
    ) -> Result<()> {
        self.queued.remove(&result.root);
        match result.output {
            Ok(output) if output.code == 0 => {
                log.write_output(&output)?;
                state.note_derivation_graph(&result.root, &output.stdout);
            }
            Ok(output) => {
                log.write_output(&output)?;
            }
            Err(error) => {
                log.write_line(
                    crate::process::StreamSource::Stderr,
                    &format!(
                        "failed to load derivation graph for {}: {error}",
                        result.root
                    ),
                )?;
            }
        }
        Ok(())
    }
}

fn stream_plain_command(
    command: &CommandSpec,
    log: &mut LogFile,
    renderer: &mut Renderer,
) -> Result<i32> {
    stream_command(command, should_announce_backend(renderer), |line| {
        log.write_line(line.source, &line.line)?;
        renderer.backend_line(&line);
        Ok(())
    })
}

fn stream_activation_command(
    command: &CommandSpec,
    options: &backend::BackendOptions,
    log: &mut LogFile,
    renderer: &mut Renderer,
) -> Result<i32> {
    if !options.uses_interactive_elevation() {
        return stream_plain_command(command, log, renderer);
    }

    let output = run_capture_interactive(command, should_announce_backend(renderer), true, true)?;
    log.write_output(&output)?;
    Ok(output.code)
}

fn run_dry_activation(
    store_path: &Path,
    options: &backend::BackendOptions,
    log: &mut LogFile,
    renderer: &mut Renderer,
    required: bool,
) -> Result<ActivationImpact> {
    let command = backend::nixos_rebuild_dry_activate_command(store_path, options);
    log.write_command(&command)?;
    let output = run_dry_activation_command(&command, options, should_announce_backend(renderer))?;
    log.write_output(&output)?;
    let combined = format!("{}{}", output.stdout, output.stderr);
    let mut impact = parse_activation_impact(&combined);
    if output.code != 0 {
        impact.unavailable = Some(format!(
            "nixos-rebuild dry-activate exited with {}",
            output.code
        ));
    }
    renderer.activation(&impact);
    if output.code != 0 && required {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code: output.code,
        });
    }
    Ok(impact)
}

fn run_dry_activation_command(
    command: &CommandSpec,
    options: &backend::BackendOptions,
    announce: bool,
) -> Result<RunOutput> {
    if options.uses_interactive_elevation() {
        run_capture_interactive(command, announce, false, true)
    } else {
        run_capture(command, announce)
    }
}

fn lifecycle_backend_options(
    action: &str,
    preview: bool,
    cli: &Cli,
    config: &NrConfig,
    backend_args: &[String],
) -> backend::BackendOptions {
    let mut options = cli.backend_options_with_config(config, backend_args);
    apply_default_elevation(action, preview, &mut options);
    options
}

fn rollback_backend_options(cli: &Cli, backend_args: &[String]) -> backend::BackendOptions {
    let mut options = cli.backend_options(backend_args);
    apply_default_elevation("rollback", false, &mut options);
    options
}

fn apply_default_elevation(action: &str, preview: bool, options: &mut backend::BackendOptions) {
    apply_default_elevation_with_context(
        action,
        preview,
        options,
        std::io::stdin().is_terminal(),
        running_as_root(),
    );
}

fn apply_default_elevation_with_context(
    action: &str,
    preview: bool,
    options: &mut backend::BackendOptions,
    stdin_interactive: bool,
    running_as_root: bool,
) {
    if should_apply_default_elevation(action, preview, options, stdin_interactive, running_as_root)
    {
        options.elevate = Some("sudo".to_string());
    }
}

fn should_apply_default_elevation(
    action: &str,
    preview: bool,
    options: &backend::BackendOptions,
    stdin_interactive: bool,
    running_as_root: bool,
) -> bool {
    !preview
        && matches!(action, "switch" | "test" | "boot" | "rollback")
        && !options.has_elevation_request()
        && !running_as_root
        && stdin_interactive
}

#[cfg(unix)]
fn running_as_root() -> bool {
    unsafe { geteuid() == 0 }
}

#[cfg(unix)]
unsafe extern "C" {
    fn geteuid() -> u32;
}

#[cfg(not(unix))]
fn running_as_root() -> bool {
    false
}

fn pipe_line_to_nom(line: &StreamLine) -> bool {
    line.line.trim_start().starts_with("@nix ")
}

#[derive(Clone, Copy)]
struct IngestOptions {
    load_dependency_graph: bool,
    render_backend: bool,
}

struct ReportContext<'a> {
    command_name: &'a str,
    config: &'a NrConfig,
    header: &'a RebuildHeader,
}

fn success_report(
    context: ReportContext<'_>,
    store_path: &Path,
    diff: ClosureDiff,
    activation: Option<ActivationImpact>,
    new_generation: Option<u64>,
    build: BuildState,
) -> RebuildReport {
    RebuildReport {
        command: context.command_name.to_string(),
        target: context.config.target.clone(),
        result: match context.command_name {
            "preview" => "preview complete; no activation performed".to_string(),
            "build" => "build complete".to_string(),
            "boot" => "boot generation registered".to_string(),
            "test" => {
                "test activation complete; reboot returns to the previous boot default".to_string()
            }
            "switch" => "switch complete".to_string(),
            _ => "complete".to_string(),
        },
        store_path: Some(store_path.to_path_buf()),
        current: context.header.current.clone(),
        new_generation,
        build,
        reboot: reboot_recommendation(context.command_name, &diff),
        rollback: "nr rollback".to_string(),
        diff: Some(diff),
        activation,
        log_path: context.header.log_path.clone(),
    }
}

fn failure_report(
    context: ReportContext<'_>,
    result: &str,
    store_path: Option<PathBuf>,
    diff: Option<ClosureDiff>,
    activation: Option<ActivationImpact>,
    build: BuildState,
) -> RebuildReport {
    RebuildReport {
        command: context.command_name.to_string(),
        target: context.config.target.clone(),
        result: result.to_string(),
        store_path,
        current: context.header.current.clone(),
        new_generation: current_generation(),
        build,
        diff,
        activation,
        reboot: "not evaluated".to_string(),
        rollback: "nr rollback".to_string(),
        log_path: context.header.log_path.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_default_elevation_with_context, focused_lockfile_diff, should_apply_default_elevation,
    };
    use crate::backend::BackendOptions;

    #[test]
    fn mutating_interactive_user_gets_default_sudo_elevation() {
        assert!(should_apply_default_elevation(
            "switch",
            false,
            &BackendOptions::default(),
            true,
            false,
        ));
        let mut options = BackendOptions::default();
        apply_default_elevation_with_context("switch", false, &mut options, true, false);
        assert_eq!(options.elevate.as_deref(), Some("sudo"));
        assert!(!options.ask_elevate_password);
    }

    #[test]
    fn preview_root_and_explicit_elevation_skip_default_sudo_elevation() {
        assert!(!should_apply_default_elevation(
            "switch",
            true,
            &BackendOptions::default(),
            true,
            false,
        ));
        assert!(!should_apply_default_elevation(
            "switch",
            false,
            &BackendOptions::default(),
            true,
            true,
        ));
        assert!(!should_apply_default_elevation(
            "switch",
            false,
            &BackendOptions {
                elevate: Some("none".to_string()),
                ..BackendOptions::default()
            },
            true,
            false,
        ));
    }

    #[test]
    fn focused_lockfile_diff_reports_requested_input_only() {
        let before = test_lockfile("old-nr", "old-home-manager");
        let after = test_lockfile("new-nr", "new-home-manager");
        let diff = focused_lockfile_diff(before.as_bytes(), after.as_bytes(), &["nr".to_string()])
            .expect("focused lock diff");

        assert_eq!(diff.requested.len(), 1);
        assert_eq!(diff.requested[0].input, "nr");
        assert!(
            diff.requested[0]
                .fields
                .iter()
                .any(|field| field.name == "rev"
                    && field.before == "old-nr"
                    && field.after == "new-nr")
        );
        assert_eq!(diff.other_changed, 1);
    }

    fn test_lockfile(nr_rev: &str, home_manager_rev: &str) -> String {
        format!(
            r#"{{
  "root": "root",
  "nodes": {{
    "root": {{"inputs": {{"nr": "nr", "home-manager": "home-manager"}}}},
    "nr": {{"locked": {{"type": "github", "rev": "{nr_rev}", "narHash": "sha256-{nr_rev}"}}}},
    "home-manager": {{"locked": {{"type": "github", "rev": "{home_manager_rev}", "narHash": "sha256-{home_manager_rev}"}}}}
  }}
}}"#
        )
    }
}
