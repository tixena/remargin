//! Remargin CLI binary.

#[cfg(feature = "obsidian")]
mod obsidian;

pub(crate) mod cli;
pub(crate) mod dispatch;
pub(crate) mod handlers;
mod io;
mod params;
mod render;

pub(crate) use cli::{
    AssetsArgs, ClaudeAction, Cli, Commands, IdentityAction, IdentityArgs, McpAction,
    ObsidianAction, OutputArgs, PermissionsAction, PlanAction, PlanClaudeAction, PluginAction,
    PretoolAction, PromptAction, RegistryAction, SandboxAction, SessionGuardAction,
    UnrestrictedArgs,
};

use std::io::{stderr as stderr_handle, stdout as stdout_handle};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::Result;
use clap::Parser as _;
use os_shim::System as _;
use os_shim::real::RealSystem;

use crate::io::IoSinks;

/// Default user-scope settings file used by `remargin claude restrict`.
/// Resolved through [`expand_path`] so `$HOME` follows the active
/// [`System`] (the `obsidian` feature already exercises this pattern;
/// we follow the same approach so tests stay hermetic via the
/// `--user-settings` flag).
pub(crate) const DEFAULT_USER_SETTINGS: &str = "~/.claude/settings.json";

pub(crate) const PLUGIN_MARKETPLACE_SOURCE: &str = "tixena/remargin";
pub(crate) const PLUGIN_MARKETPLACE_NAME: &str = "remargin-marketplace";
pub(crate) const PLUGIN_REF: &str = "remargin@remargin-marketplace";

fn main() -> ExitCode {
    // Capture the start time before parsing so `elapsed_ms` includes clap's
    // argument-parsing overhead.
    let _: Result<_, _> = io::START_TIME.set(Instant::now());

    let cli = Cli::parse();

    let output = dispatch::subcommand_output(cli.cmd());
    let verbose = output.is_some_and(|o| o.verbose);

    let system = RealSystem::new();
    if verbose {
        let env_filter_directives = system.env_var("RUST_LOG").unwrap_or_default();
        let base_filter = tracing_subscriber::EnvFilter::try_new(&env_filter_directives)
            .unwrap_or_else(|_err| tracing_subscriber::EnvFilter::new(""));
        tracing_subscriber::fmt()
            .with_env_filter(base_filter.add_directive(tracing::Level::DEBUG.into()))
            .with_writer(stderr_handle)
            .init();
    }

    let cwd = match system.current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("error: could not determine current directory: {err}");
            return ExitCode::from(dispatch::EXIT_ERROR);
        }
    };

    let mut stdout = stdout_handle().lock();
    let mut stderr = stderr_handle().lock();
    let mut sinks = IoSinks::new(&mut stdout, &mut stderr);

    // Non-JSON mode does not emit a timing footer on any stream:
    // stdout stays pure command output and stderr stays clean. The timing
    // value survives as `elapsed_ms` inside the JSON payload.
    dispatch::run(&cli, &system, &cwd, &mut sinks)
}

#[cfg(test)]
mod tests;
