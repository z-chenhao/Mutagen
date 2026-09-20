//! The `mutagen` command-line interface.
//!
//! Phase 0 scope: version reporting and `mutagen doctor` (local sanity
//! checks). Agent execution, provider configuration, and anything touching
//! the network are deliberately out of scope.

mod doctor;

use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};

/// Evolution infrastructure for high-performance AI agents.
#[derive(Debug, Parser)]
#[command(name = "mutagen", version, about)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Run local sanity checks (deterministic, no network, no API keys).
    Doctor,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match args.command {
        Some(Command::Doctor) => doctor::run(),
        None => {
            // No subcommand: show help and stay non-fatal.
            let mut cmd = Args::command();
            cmd.print_help().expect("help text is static");
            ExitCode::SUCCESS
        }
    }
}
