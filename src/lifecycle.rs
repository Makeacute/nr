use std::collections::BTreeSet;
use std::env;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use crate::backend;
use crate::cli::{Cli, DiffArgs, GcArgs, RollbackArgs, UpdateArgs};
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
use crate::git::{ensure_git_flake_visible, git_summary};
use crate::impact::{
    ActivationImpact, ClosureDiff, current_generation, current_generation_info,
    diff_current_to_new, parse_activation_impact, parse_closure_diff, reboot_recommendation,
    resolve_result_link,
};
use crate::process::{
    CommandSpec, LogFile, RunOutput, StreamEvent, StreamLine, run_capture, run_capture_interactive,
    run_inherit, stream_command, stream_command_events, stream_command_to_command,
};
use crate::prompts::confirm;
use crate::ui::{OutputMode, RebuildHeader, RebuildReport, Renderer};

pub fn run_lifecycle(action: &str, cli: &Cli, backend_args: &[String]) -> Result<i32> {
    let config = load_config(cli.config_input())?;
    ensure_git_flake_visible(&config.target.path)?;

    let preview = action == "preview" || cli.dry;
    let command_name = if preview { "preview" } else { action };
    let options = lifecycle_backend_options(action, preview, cli, backend_args);
    let mut log = LogFile::create(cli.log_file.clone())?;
    let mut renderer = Renderer::new_for_lifecycle(cli.ui, command_name, config.ui.accent.clone());
    let header = RebuildHeader {
        command: command_name.to_string(),
        target: config.target.clone(),
        git: git_summary(&config.target.path),
        current: current_generation_info(),
        log_path: log.path().to_path_buf(),
    };
    renderer.start(&header);

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
        log.flush()?;
        return Err(NrError::CommandFailed {
            command: build_command.render(),
            code: build.code,
        });
    }
    let build_state = build.state;

    let store_path = resolve_result_link(&build_cwd)?;
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
        log.flush()?;
        return Ok(0);
    }

    let activation_command = backend::nixos_rebuild_activate_command(action, &store_path, &options);
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
        log.flush()?;
        return Err(NrError::CommandFailed {
            command: activation_command.render(),
            code,
        });
    }

    if action == "switch" && !config.hooks.post_switch.is_empty() {
        renderer.phase("post-switch hooks");
        if let Err(error) = run_post_switch_hooks(
            &config.hooks.post_switch,
            &config.target.path,
            &mut log,
            &mut renderer,
        ) {
            let report = failure_report(
                ReportContext {
                    command_name,
                    config: &config,
                    header: &header,
                },
                "post-switch hook failed",
                Some(store_path),
                Some(diff),
                activation,
                build_state,
            );
            finish_lifecycle(cli, &mut renderer, &report, false);
            log.flush()?;
            return Err(error);
        }
    }

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
    log.flush()?;
    Ok(0)
}

pub fn run_update(cli: &Cli, config: &NrConfig, args: &UpdateArgs) -> Result<i32> {
    ensure_git_flake_visible(&config.target.path)?;
    let options = cli.backend_options(&[]);
    let command = backend::nix_flake_update_command(&config.target, &args.inputs, &options);
    let code = run_inherit(&command, true)?;
    if code != 0 {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code,
        });
    }
    if args.switch {
        run_lifecycle("switch", cli, &[])
    } else {
        Ok(0)
    }
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
    let options = cli.backend_options(&args.backend_args);
    let mut log = LogFile::create(cli.log_file.clone())?;
    let mut renderer = Renderer::new_for_lifecycle(cli.ui, "diff", config.ui.accent.clone());
    let header = RebuildHeader {
        command: "diff".to_string(),
        target: config.target.clone(),
        git: git_summary(&config.target.path),
        current: current_generation_info(),
        log_path: log.path().to_path_buf(),
    };
    renderer.start(&header);

    let from = resolve_diff_from(args.from.as_deref())?;
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
        target: config.target,
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
    log.flush()?;
    Ok(0)
}

struct DiffEndpoint {
    path: PathBuf,
    label: String,
    _temp_dir: Option<tempfile::TempDir>,
}

fn resolve_diff_from(value: Option<&str>) -> Result<DiffEndpoint> {
    if let Some(value) = value {
        let path = generation_or_path(value)?;
        return Ok(DiffEndpoint {
            label: value.to_string(),
            path,
            _temp_dir: None,
        });
    }
    Ok(DiffEndpoint {
        path: PathBuf::from("/run/current-system"),
        label: "/run/current-system".to_string(),
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

fn run_post_switch_hooks(
    hooks: &[HookCommand],
    cwd: &Path,
    log: &mut LogFile,
    renderer: &mut Renderer,
) -> Result<()> {
    for hook in hooks {
        let Some((program, args)) = hook.split_first() else {
            return Err(NrError::message(
                "[hooks].post_switch entries cannot be empty.",
            ));
        };
        let command = CommandSpec::new(program.clone())
            .args(args.iter().cloned())
            .cwd(cwd.to_path_buf());
        log.write_command(&command)?;
        let code = stream_plain_command(&command, log, renderer)?;
        if code != 0 {
            return Err(NrError::CommandFailed {
                command: command.render(),
                code,
            });
        }
    }
    Ok(())
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
    backend_args: &[String],
) -> backend::BackendOptions {
    let mut options = cli.backend_options(backend_args);
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
    use super::{apply_default_elevation_with_context, should_apply_default_elevation};
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
}
