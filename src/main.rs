//! # `Remargin`
//!
//! `Remargin` is a command-line tool for enhanced inline review of markdown documents.
//! It provides comment parsing, writing, threading, signatures, and cross-document queries.

use std::process::ExitCode;

use clap::Parser;

/// Enhanced inline review protocol for markdown.
#[derive(Parser)]
#[command(
    name = "remargin",
    version,
    about = "Enhanced inline review protocol for markdown"
)]
struct Cli {
    /// Subcommand to execute.
    #[command(subcommand)]
    command: Commands,
}

/// Available subcommands.
#[derive(clap::Subcommand)]
enum Commands {
    /// Print version information.
    Version,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Commands::Version => {
            eprintln!("remargin {}", env!("CARGO_PKG_VERSION"));
        }
    }

    ExitCode::SUCCESS
}
