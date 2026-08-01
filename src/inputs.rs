use serde_json::Value;

use crate::backend;
use crate::cli::{Cli, InputsArgs, UpdateArgs};
use crate::config::NrConfig;
use crate::errors::{NrError, Result};
use crate::lifecycle::run_update;
use crate::process::run_capture;

pub fn run_inputs(cli: &Cli, config: &NrConfig, args: &InputsArgs) -> Result<i32> {
    if !args.update.is_empty() {
        return run_update(
            cli,
            config,
            &UpdateArgs {
                inputs: args.update.clone(),
                switch: false,
                revert_on_failure: false,
            },
        );
    }

    let options = cli.backend_options_with_config(config, &[]);
    let command = backend::nix_flake_metadata_command(&config.target, &options);
    let output = run_capture(&command, false)?;
    if output.code != 0 {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code: output.code,
        });
    }

    if args.json {
        print!("{}", output.stdout);
        return Ok(0);
    }

    let value = serde_json::from_str::<Value>(&output.stdout)
        .map_err(|error| NrError::message(format!("failed to parse flake metadata: {error}")))?;
    println!("inputs for {}", config.target.reference());
    if let Some(nodes) = value
        .get("locks")
        .and_then(|locks| locks.get("nodes"))
        .and_then(Value::as_object)
    {
        for name in nodes.keys().filter(|name| name.as_str() != "root") {
            println!("  {name}");
        }
    } else {
        println!("  no lock nodes found");
    }
    Ok(0)
}
