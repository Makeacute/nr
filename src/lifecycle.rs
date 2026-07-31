use std::env;
use std::path::{Path, PathBuf};

use crate::backend;
use crate::cli::{Cli, UpdateArgs};
use crate::config::{NrConfig, load_config};
use crate::errors::{NrError, Result};
use crate::events::{BuildState, ParsedLine, parse_line};
use crate::git::{ensure_git_flake_visible, git_summary};
use crate::impact::{
    ActivationImpact, ClosureDiff, current_generation, current_generation_info,
    diff_current_to_new, parse_activation_impact, reboot_recommendation, resolve_result_link,
};
use crate::process::{
    CommandSpec, LogFile, StreamLine, run_capture, run_inherit, stream_command,
    stream_command_to_command,
};
use crate::prompts::confirm;
use crate::ui::{OutputMode, RebuildHeader, RebuildReport, Renderer};

pub fn run_lifecycle(action: &str, cli: &Cli, backend_args: &[String]) -> Result<i32> {
    let config = load_config(cli.config_input())?;
    ensure_git_flake_visible(&config.target.path)?;

    let preview = action == "preview" || cli.dry;
    let command_name = if preview { "preview" } else { action };
    let options = cli.backend_options(backend_args);
    let mut log = LogFile::create(cli.log_file.clone())?;
    let mut renderer = Renderer::new_for_lifecycle(cli.ui, command_name);
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
        renderer.finish(&report);
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
        renderer.finish(&report);
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
        renderer.finish(&report);
        log.flush()?;
        return Ok(0);
    }

    let activation_command = backend::nixos_rebuild_activate_command(action, &store_path, &options);
    log.write_command(&activation_command)?;
    let code = stream_plain_command(&activation_command, &mut log, &mut renderer)?;
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
        renderer.finish(&report);
        log.flush()?;
        return Err(NrError::CommandFailed {
            command: activation_command.render(),
            code,
        });
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
    renderer.finish(&report);
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

pub fn run_rollback(cli: &Cli, backend_args: &[String]) -> Result<i32> {
    let options = cli.backend_options(backend_args);
    let command = backend::rollback_command(&options);
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

pub fn run_generations(args: &crate::cli::GenerationsArgs) -> Result<i32> {
    let command = backend::generations_command(args.profile.as_deref(), &args.backend_args);
    let code = run_inherit(&command, true)?;
    if code != 0 {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code,
        });
    }
    Ok(0)
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
    let mut fallback_announced = false;
    let code = if renderer.mode() == OutputMode::Nom {
        let nom_command = backend::nom_json_command();
        log.write_command(&nom_command)?;
        stream_command_to_command(
            command,
            &nom_command,
            true,
            |line| {
                ingest_build_line(
                    &mut state,
                    &line,
                    log,
                    renderer,
                    &mut fallback_announced,
                    false,
                    false,
                );
            },
            pipe_line_to_nom,
        )?
    } else {
        stream_command(command, true, |line| {
            ingest_build_line(
                &mut state,
                &line,
                log,
                renderer,
                &mut fallback_announced,
                true,
                true,
            );
        })?
    };
    Ok(BuildRun { code, state })
}

fn ingest_build_line(
    state: &mut BuildState,
    line: &StreamLine,
    log: &mut LogFile,
    renderer: &mut Renderer,
    fallback_announced: &mut bool,
    load_dependency_graph: bool,
    render_backend: bool,
) {
    let _ = log.write_line(line.source, &line.line);
    if load_dependency_graph {
        learn_dependency_graphs(state, &line.line, log);
    } else {
        state.note_derivation_paths_from_text(&line.line);
    }
    if state.parser_fallback {
        if render_backend {
            renderer.backend_line(line);
        }
        return;
    }
    match parse_line(&line.line) {
        ParsedLine::Event(event) => {
            state.ingest(&event);
            for field in &event.fields {
                if load_dependency_graph {
                    learn_dependency_graphs(state, field, log);
                } else {
                    state.note_derivation_paths_from_text(field);
                }
            }
            if render_backend {
                renderer.nix_event(&event, state);
            }
        }
        ParsedLine::Plain(_) => {
            if render_backend {
                renderer.backend_line(line);
            }
        }
        ParsedLine::BrokenInternalJson(_) => {
            state.parser_fallback = true;
            if !*fallback_announced {
                renderer.parser_fallback();
                *fallback_announced = true;
            }
            if render_backend {
                renderer.backend_line(line);
            }
        }
    }
}

fn learn_dependency_graphs(state: &mut BuildState, text: &str, log: &mut LogFile) {
    state.note_derivation_paths_from_text(text);
    for root in state.dependency_graph_roots_to_load() {
        let command = backend::nix_store_query_graph_command(&root);
        let _ = log.write_command(&command);
        match run_capture(&command, false) {
            Ok(output) if output.code == 0 => {
                let _ = log.write_output(&output);
                state.note_derivation_graph(&root, &output.stdout);
            }
            Ok(output) => {
                let _ = log.write_output(&output);
                state.mark_derivation_graph_attempted(&root);
            }
            Err(error) => {
                let _ = log.write_line(
                    crate::process::StreamSource::Stderr,
                    &format!("failed to load derivation graph for {root}: {error}"),
                );
                state.mark_derivation_graph_attempted(&root);
            }
        }
    }
}

fn stream_plain_command(
    command: &CommandSpec,
    log: &mut LogFile,
    renderer: &mut Renderer,
) -> Result<i32> {
    stream_command(command, true, |line| {
        let _ = log.write_line(line.source, &line.line);
        renderer.backend_line(&line);
    })
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
    let output = run_capture(&command, true)?;
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

fn pipe_line_to_nom(line: &StreamLine) -> bool {
    line.line.trim_start().starts_with("@nix ")
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
