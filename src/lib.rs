pub mod backend;
pub mod checks;
pub mod cli;
pub mod config;
pub mod diagnostics;
pub mod errors;
pub mod events;
pub mod generations;
pub mod git;
pub mod help;
pub mod impact;
pub mod lifecycle;
pub mod process;
pub mod prompts;
pub mod publish;
pub mod ui;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
