//! Internal developer-tooling dispatcher for the remargin workspace.
//!
//! Follows the rust-analyzer / Cargo-team `xtask` idiom: invoke via
//! `cargo xtask <subcommand>`. This binary is marked `publish = false`
//! and is never installed on end-user machines, so it is a safe home
//! for build-time tools that used to leak into `~/.cargo/bin/` via
//! stray `[[bin]]` stanzas.
//!
//! Subcommands:
//!
//! - `generate-types` — regenerate the TypeScript types + Zod schemas
//!   under `packages/remargin-obsidian/src/generated/` from the Rust
//!   models in `remargin-core`.

#![expect(
    clippy::print_stdout,
    reason = "xtask is a dev-time CLI; stdout is its interface"
)]

mod generate_types;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Internal developer tooling dispatcher.
#[derive(Parser)]
#[command(name = "xtask", about = "Remargin workspace dev tooling", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Available `cargo xtask` subcommands.
#[derive(Subcommand)]
enum Commands {
    /// Regenerate TypeScript types + Zod schemas from Rust models.
    GenerateTypes,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::GenerateTypes => generate_types::run(),
    }
}
