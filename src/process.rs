use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::errors::{IoContext, NrError, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
        }
    }

    pub fn arg(mut self, value: impl Into<String>) -> Self {
        self.args.push(value.into());
        self
    }

    pub fn args<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(values.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, path: impl Into<PathBuf>) -> Self {
        self.cwd = Some(path.into());
        self
    }

    pub fn to_vec(&self) -> Vec<String> {
        let mut values = vec![self.program.clone()];
        values.extend(self.args.clone());
        values
    }

    pub fn render(&self) -> String {
        render_command(&self.to_vec())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamSource {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamLine {
    pub source: StreamSource,
    pub line: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn render_command(parts: &[String]) -> String {
    let rendered = parts
        .iter()
        .map(|part| shell_quote(part))
        .collect::<Vec<_>>()
        .join(" ");
    if rendered.len() > 180 {
        let prefix = parts
            .iter()
            .take(3)
            .map(|part| shell_quote(part))
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "{prefix} ... ({} more arguments)",
            parts.len().saturating_sub(3)
        )
    } else {
        rendered
    }
}

pub fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'_' | b'-' | b'.' | b'/' | b':' | b'@' | b'+' | b'=' | b','
            )
    }) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub fn run_capture(command: &CommandSpec, announce: bool) -> Result<RunOutput> {
    if announce {
        println!("-> {}", command.render());
    }

    let output = command_builder(command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|source| spawn_error(command, source))?;

    Ok(RunOutput {
        code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub fn run_checked(command: &CommandSpec, announce: bool) -> Result<RunOutput> {
    let output = run_capture(command, announce)?;
    if output.code != 0 {
        return Err(NrError::CommandFailed {
            command: command.render(),
            code: output.code,
        });
    }
    Ok(output)
}

pub fn run_inherit(command: &CommandSpec, announce: bool) -> Result<i32> {
    if announce {
        println!("-> {}", command.render());
    }

    let status = command_builder(command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|source| spawn_error(command, source))?;
    Ok(status.code().unwrap_or(1))
}

pub fn stream_command<F>(command: &CommandSpec, announce: bool, mut on_line: F) -> Result<i32>
where
    F: FnMut(StreamLine),
{
    if announce {
        println!("-> {}", command.render());
    }

    let mut child = command_builder(command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| spawn_error(command, source))?;

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let (sender, receiver) = mpsc::channel();

    spawn_reader(stdout, StreamSource::Stdout, sender.clone());
    spawn_reader(stderr, StreamSource::Stderr, sender.clone());
    drop(sender);

    for line in receiver {
        on_line(line);
    }

    let status = child
        .wait()
        .with_context(format!("failed to wait for {}", command.render()))?;
    Ok(status.code().unwrap_or(1))
}

pub fn stream_command_to_command<F, P>(
    producer: &CommandSpec,
    consumer: &CommandSpec,
    announce: bool,
    mut on_line: F,
    mut pipe_line: P,
) -> Result<i32>
where
    F: FnMut(StreamLine),
    P: FnMut(&StreamLine) -> bool,
{
    if announce {
        println!("-> {} | {}", producer.render(), consumer.render());
    }

    let mut consumer_child = command_builder(consumer)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|source| spawn_error(consumer, source))?;
    let mut consumer_stdin = consumer_child.stdin.take().expect("stdin was piped");

    let mut producer_child = match command_builder(producer)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(source) => {
            drop(consumer_stdin);
            let _ = consumer_child.kill();
            let _ = consumer_child.wait();
            return Err(spawn_error(producer, source));
        }
    };

    let stdout = producer_child.stdout.take().expect("stdout was piped");
    let stderr = producer_child.stderr.take().expect("stderr was piped");
    let (sender, receiver) = mpsc::channel();

    spawn_reader(stdout, StreamSource::Stdout, sender.clone());
    spawn_reader(stderr, StreamSource::Stderr, sender.clone());
    drop(sender);

    let mut pipe_error = None;
    for line in receiver {
        if pipe_line(&line)
            && pipe_error.is_none()
            && let Err(error) = writeln!(consumer_stdin, "{}", line.line)
        {
            pipe_error = Some(error);
        }
        on_line(line);
    }
    drop(consumer_stdin);

    let producer_status = producer_child
        .wait()
        .with_context(format!("failed to wait for {}", producer.render()))?;
    let consumer_status = consumer_child
        .wait()
        .with_context(format!("failed to wait for {}", consumer.render()))?;

    let producer_code = producer_status.code().unwrap_or(1);
    let consumer_code = consumer_status.code().unwrap_or(1);
    if producer_code == 0 {
        if let Some(error) = pipe_error
            && consumer_code == 0
        {
            return Err(NrError::Io {
                context: format!(
                    "failed to pipe {} output to {}",
                    producer.render(),
                    consumer.render()
                ),
                source: error,
            });
        }
        if consumer_code != 0 {
            return Err(NrError::CommandFailed {
                command: consumer.render(),
                code: consumer_code,
            });
        }
    }
    Ok(producer_code)
}

fn spawn_reader<R>(reader: R, source: StreamSource, sender: mpsc::Sender<StreamLine>)
where
    R: io::Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    while line.ends_with('\n') || line.ends_with('\r') {
                        line.pop();
                    }
                    if sender
                        .send(StreamLine {
                            source,
                            line: line.clone(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

fn command_builder(command: &CommandSpec) -> Command {
    let mut builder = Command::new(&command.program);
    builder.args(&command.args);
    if let Some(cwd) = &command.cwd {
        builder.current_dir(cwd);
    }
    builder
}

fn spawn_error(command: &CommandSpec, source: io::Error) -> NrError {
    if source.kind() == io::ErrorKind::NotFound {
        NrError::MissingCommand(command.program.clone())
    } else {
        NrError::Io {
            context: format!("failed to run {}", command.render()),
            source,
        }
    }
}

#[derive(Debug)]
pub struct LogFile {
    path: PathBuf,
    writer: BufWriter<File>,
}

impl LogFile {
    pub fn create(path: Option<PathBuf>) -> Result<Self> {
        let path = match path {
            Some(path) => path,
            None => default_log_path(),
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(format!("failed to create {}", parent.display()))?;
        }
        let file =
            File::create(&path).with_context(format!("failed to create {}", path.display()))?;
        Ok(Self {
            path,
            writer: BufWriter::new(file),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_command(&mut self, command: &CommandSpec) -> Result<()> {
        writeln!(self.writer, "$ {}", command.render()).with_context("failed to write log")
    }

    pub fn write_line(&mut self, source: StreamSource, line: &str) -> Result<()> {
        let label = match source {
            StreamSource::Stdout => "stdout",
            StreamSource::Stderr => "stderr",
        };
        writeln!(self.writer, "[{label}] {line}").with_context("failed to write log")
    }

    pub fn write_output(&mut self, output: &RunOutput) -> Result<()> {
        for line in output.stdout.lines() {
            self.write_line(StreamSource::Stdout, line)?;
        }
        for line in output.stderr.lines() {
            self.write_line(StreamSource::Stderr, line)?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush().with_context("failed to flush log")
    }
}

fn default_log_path() -> PathBuf {
    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    root.join("nr")
        .join("logs")
        .join(format!("nr-{seconds}-{}.log", std::process::id()))
}

pub fn os_str(value: &Path) -> &OsStr {
    value.as_os_str()
}
