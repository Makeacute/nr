use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;
use tokio::runtime::Builder;
use tokio::sync::mpsc;

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
pub enum StreamEvent {
    Line(StreamLine),
    Resize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

struct StreamChunk {
    source: StreamSource,
    bytes: Vec<u8>,
}

enum ChunkMessage {
    Chunk(StreamChunk),
    ReadError {
        source: StreamSource,
        error: io::Error,
    },
}

enum StreamMessage {
    Event(StreamEvent),
    ReadError {
        source: StreamSource,
        error: io::Error,
    },
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

pub fn run_capture_interactive(
    command: &CommandSpec,
    announce: bool,
    passthrough_stdout: bool,
    passthrough_stderr: bool,
) -> Result<RunOutput> {
    block_on(async_run_capture_interactive(
        command,
        announce,
        passthrough_stdout,
        passthrough_stderr,
    ))
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
    F: FnMut(StreamLine) -> Result<()>,
{
    stream_command_events(command, announce, |event| match event {
        StreamEvent::Line(line) => on_line(line),
        StreamEvent::Resize => Ok(()),
    })
}

pub fn stream_command_events<F>(
    command: &CommandSpec,
    announce: bool,
    mut on_event: F,
) -> Result<i32>
where
    F: FnMut(StreamEvent) -> Result<()>,
{
    block_on(async_stream_command_events(
        command,
        announce,
        &mut on_event,
    ))
}

pub fn stream_command_to_command<F, P>(
    producer: &CommandSpec,
    consumer: &CommandSpec,
    announce: bool,
    mut on_line: F,
    mut pipe_line: P,
) -> Result<i32>
where
    F: FnMut(StreamLine) -> Result<()>,
    P: FnMut(&StreamLine) -> bool,
{
    block_on(async_stream_command_to_command(
        producer,
        consumer,
        announce,
        &mut on_line,
        &mut pipe_line,
    ))
}

fn block_on<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    Builder::new_current_thread()
        .enable_io()
        .build()
        .map_err(|source| NrError::Io {
            context: "failed to create async runtime".to_string(),
            source,
        })?
        .block_on(future)
}

async fn async_run_capture_interactive(
    command: &CommandSpec,
    announce: bool,
    passthrough_stdout: bool,
    passthrough_stderr: bool,
) -> Result<RunOutput> {
    if announce {
        println!("-> {}", command.render());
    }

    let mut child = tokio_command_builder(command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| spawn_error(command, source))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| missing_pipe(command, "stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| missing_pipe(command, "stderr"))?;
    let (sender, mut receiver) = mpsc::channel(64);

    tokio::spawn(read_chunks(stdout, StreamSource::Stdout, sender.clone()));
    tokio::spawn(read_chunks(stderr, StreamSource::Stderr, sender.clone()));
    drop(sender);

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut read_error = None;
    while let Some(message) = receiver.recv().await {
        match message {
            ChunkMessage::Chunk(chunk) => match chunk.source {
                StreamSource::Stdout => {
                    if passthrough_stdout {
                        write_passthrough(StreamSource::Stdout, &chunk.bytes);
                    }
                    stdout.extend_from_slice(&chunk.bytes);
                }
                StreamSource::Stderr => {
                    if passthrough_stderr {
                        write_passthrough(StreamSource::Stderr, &chunk.bytes);
                    }
                    stderr.extend_from_slice(&chunk.bytes);
                }
            },
            ChunkMessage::ReadError { source, error } => {
                read_error.get_or_insert((source, error));
            }
        }
    }

    let status = child
        .wait()
        .await
        .with_context(format!("failed to wait for {}", command.render()))?;
    if passthrough_stdout && !stdout.is_empty() && !stdout.ends_with(b"\n") {
        println!();
    }
    if passthrough_stderr && !stderr.is_empty() && !stderr.ends_with(b"\n") {
        eprintln!();
    }
    if let Some((source, error)) = read_error {
        return Err(read_error_for(command, source, error));
    }
    Ok(RunOutput {
        code: status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

async fn async_stream_command_events<F>(
    command: &CommandSpec,
    announce: bool,
    on_event: &mut F,
) -> Result<i32>
where
    F: FnMut(StreamEvent) -> Result<()>,
{
    if announce {
        println!("-> {}", command.render());
    }

    let mut child = tokio_command_builder(command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| spawn_error(command, source))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| missing_pipe(command, "stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| missing_pipe(command, "stderr"))?;
    let (sender, mut receiver) = mpsc::channel(64);

    tokio::spawn(read_lines(stdout, StreamSource::Stdout, sender.clone()));
    tokio::spawn(read_lines(stderr, StreamSource::Stderr, sender.clone()));
    drop(sender);

    let mut read_error = None;
    let mut callback_error = None;
    let mut resize_signal = resize_signal();
    loop {
        tokio::select! {
            message = receiver.recv() => {
                let Some(message) = message else {
                    break;
                };
                match message {
                    StreamMessage::Event(event) => {
                        if let Err(error) = on_event(event) {
                            callback_error = Some(error);
                            break;
                        }
                    }
                    StreamMessage::ReadError { source, error } => {
                        read_error.get_or_insert((source, error));
                    }
                }
            }
            resized = recv_resize(&mut resize_signal) => {
                if resized && let Err(error) = on_event(StreamEvent::Resize) {
                    callback_error = Some(error);
                    break;
                }
            }
        }
    }

    if let Some(error) = callback_error {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err(error);
    }

    let status = child
        .wait()
        .await
        .with_context(format!("failed to wait for {}", command.render()))?;
    if let Some((source, error)) = read_error {
        return Err(read_error_for(command, source, error));
    }
    Ok(status.code().unwrap_or(1))
}

async fn async_stream_command_to_command<F, P>(
    producer: &CommandSpec,
    consumer: &CommandSpec,
    announce: bool,
    on_line: &mut F,
    pipe_line: &mut P,
) -> Result<i32>
where
    F: FnMut(StreamLine) -> Result<()>,
    P: FnMut(&StreamLine) -> bool,
{
    if announce {
        println!("-> {} | {}", producer.render(), consumer.render());
    }

    let mut consumer_child = tokio_command_builder(consumer)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|source| spawn_error(consumer, source))?;
    let mut consumer_stdin = consumer_child
        .stdin
        .take()
        .ok_or_else(|| missing_pipe(consumer, "stdin"))?;

    let mut producer_child = match tokio_command_builder(producer)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(source) => {
            drop(consumer_stdin);
            let _ = consumer_child.kill().await;
            let _ = consumer_child.wait().await;
            return Err(spawn_error(producer, source));
        }
    };

    let stdout = producer_child
        .stdout
        .take()
        .ok_or_else(|| missing_pipe(producer, "stdout"))?;
    let stderr = producer_child
        .stderr
        .take()
        .ok_or_else(|| missing_pipe(producer, "stderr"))?;
    let (sender, mut receiver) = mpsc::channel(64);

    tokio::spawn(read_lines(stdout, StreamSource::Stdout, sender.clone()));
    tokio::spawn(read_lines(stderr, StreamSource::Stderr, sender.clone()));
    drop(sender);

    let mut pipe_error = None;
    let mut read_error = None;
    let mut callback_error = None;
    while let Some(message) = receiver.recv().await {
        let line = match message {
            StreamMessage::Event(StreamEvent::Line(line)) => line,
            StreamMessage::Event(StreamEvent::Resize) => continue,
            StreamMessage::ReadError { source, error } => {
                read_error.get_or_insert((source, error));
                continue;
            }
        };
        if pipe_line(&line) && pipe_error.is_none() {
            let mut text = line.line.clone();
            text.push('\n');
            if let Err(error) = consumer_stdin.write_all(text.as_bytes()).await {
                pipe_error = Some(error);
            }
        }
        if let Err(error) = on_line(line) {
            callback_error = Some(error);
            break;
        }
    }
    drop(consumer_stdin);

    if let Some(error) = callback_error {
        let _ = producer_child.kill().await;
        let _ = consumer_child.kill().await;
        let _ = producer_child.wait().await;
        let _ = consumer_child.wait().await;
        return Err(error);
    }

    let producer_status = producer_child
        .wait()
        .await
        .with_context(format!("failed to wait for {}", producer.render()))?;
    let consumer_status = consumer_child
        .wait()
        .await
        .with_context(format!("failed to wait for {}", consumer.render()))?;

    let producer_code = producer_status.code().unwrap_or(1);
    let consumer_code = consumer_status.code().unwrap_or(1);
    if let Some((source, error)) = read_error {
        return Err(read_error_for(producer, source, error));
    }
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

async fn read_lines<R>(reader: R, source: StreamSource, sender: mpsc::Sender<StreamMessage>)
where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader).lines();
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                if sender
                    .send(StreamMessage::Event(StreamEvent::Line(StreamLine {
                        source,
                        line,
                    })))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                let _ = sender
                    .send(StreamMessage::ReadError { source, error })
                    .await;
                break;
            }
        }
    }
}

async fn read_chunks<R>(mut reader: R, source: StreamSource, sender: mpsc::Sender<ChunkMessage>)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0; 4096];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(length) => {
                if sender
                    .send(ChunkMessage::Chunk(StreamChunk {
                        source,
                        bytes: buffer[..length].to_vec(),
                    }))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(error) => {
                let _ = sender.send(ChunkMessage::ReadError { source, error }).await;
                break;
            }
        }
    }
}

fn write_passthrough(source: StreamSource, bytes: &[u8]) {
    match source {
        StreamSource::Stdout => {
            let mut stdout = io::stdout().lock();
            let _ = stdout.write_all(bytes);
            let _ = stdout.flush();
        }
        StreamSource::Stderr => {
            let mut stderr = io::stderr().lock();
            let _ = stderr.write_all(bytes);
            let _ = stderr.flush();
        }
    }
}

#[cfg(unix)]
type ResizeSignal = tokio::signal::unix::Signal;

#[cfg(not(unix))]
struct ResizeSignal;

#[cfg(unix)]
fn resize_signal() -> Option<ResizeSignal> {
    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()).ok()
}

#[cfg(not(unix))]
fn resize_signal() -> Option<ResizeSignal> {
    None
}

#[cfg(unix)]
async fn recv_resize(signal: &mut Option<ResizeSignal>) -> bool {
    let Some(signal) = signal else {
        return std::future::pending::<bool>().await;
    };
    signal.recv().await.is_some()
}

#[cfg(not(unix))]
async fn recv_resize(_signal: &mut Option<ResizeSignal>) -> bool {
    std::future::pending::<bool>().await
}

fn command_builder(command: &CommandSpec) -> Command {
    let mut builder = Command::new(&command.program);
    builder.args(&command.args);
    if let Some(cwd) = &command.cwd {
        builder.current_dir(cwd);
    }
    builder
}

fn tokio_command_builder(command: &CommandSpec) -> TokioCommand {
    let mut builder = TokioCommand::new(&command.program);
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

fn missing_pipe(command: &CommandSpec, stream: &str) -> NrError {
    NrError::Io {
        context: format!("failed to capture {stream} from {}", command.render()),
        source: io::Error::other(format!("{stream} pipe was not available")),
    }
}

fn read_error_for(command: &CommandSpec, source: StreamSource, error: io::Error) -> NrError {
    let stream = match source {
        StreamSource::Stdout => "stdout",
        StreamSource::Stderr => "stderr",
    };
    NrError::Io {
        context: format!("failed to read {stream} from {}", command.render()),
        source: error,
    }
}

#[derive(Debug)]
pub struct LogFile {
    path: PathBuf,
    writer: BufWriter<File>,
}

impl LogFile {
    pub fn create(path: Option<PathBuf>) -> Result<Self> {
        let rotate = path.is_none();
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
        if rotate && let Some(parent) = path.parent() {
            rotate_logs(parent, 20, &path)?;
        }
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
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    state_dir()
        .join("logs")
        .join(format!("nr-{seconds}-{}.log", std::process::id()))
}

pub fn state_dir() -> PathBuf {
    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);
    root.join("nr")
}

fn rotate_logs(directory: &Path, keep: usize, current_log: &Path) -> Result<()> {
    let mut logs = Vec::new();
    for entry in fs::read_dir(directory).with_context(format!(
        "failed to read log directory {}",
        directory.display()
    ))? {
        let entry = entry.with_context(format!(
            "failed to read log directory entry in {}",
            directory.display()
        ))?;
        let path = entry.path();
        if path == current_log {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with("nr-") && name.ends_with(".log") && path.is_file() {
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            logs.push((modified, path));
        }
    }
    logs.sort_by_key(|(modified, _)| *modified);
    let remove_count = logs.len().saturating_sub(keep.saturating_sub(1));
    for (_, path) in logs.into_iter().take(remove_count) {
        fs::remove_file(&path).with_context(format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

pub fn os_str(value: &Path) -> &OsStr {
    value.as_os_str()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use super::rotate_logs;

    #[test]
    fn log_rotation_preserves_current_log_even_when_oldest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let current = temp.path().join("nr-current.log");
        fs::write(&current, "current\n").expect("write current");
        std::thread::sleep(Duration::from_millis(5));
        for index in 0..25 {
            fs::write(temp.path().join(format!("nr-old-{index}.log")), "old\n")
                .expect("write old log");
        }

        rotate_logs(temp.path(), 20, &current).expect("rotate logs");

        assert!(current.is_file());
        assert_eq!(fs::read_dir(temp.path()).expect("read logs").count(), 20);
    }
}
