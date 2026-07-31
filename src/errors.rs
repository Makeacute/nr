use std::error::Error;
use std::fmt;
use std::io;

pub type Result<T> = std::result::Result<T, NrError>;

#[derive(Debug)]
pub enum NrError {
    Message(String),
    MissingCommand(String),
    CommandFailed { command: String, code: i32 },
    Io { context: String, source: io::Error },
}

impl NrError {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::MissingCommand(_) => 127,
            Self::CommandFailed { code, .. } => *code,
            Self::Message(_) | Self::Io { .. } => 1,
        }
    }
}

impl fmt::Display for NrError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => write!(formatter, "{message}"),
            Self::MissingCommand(command) => {
                write!(formatter, "required command not found: {command}")
            }
            Self::CommandFailed { command, code } => {
                write!(formatter, "command failed with exit code {code}: {command}")
            }
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
        }
    }
}

impl Error for NrError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub trait IoContext<T> {
    fn with_context(self, context: impl Into<String>) -> Result<T>;
}

impl<T> IoContext<T> for io::Result<T> {
    fn with_context(self, context: impl Into<String>) -> Result<T> {
        self.map_err(|source| NrError::Io {
            context: context.into(),
            source,
        })
    }
}
