//! `remargin claude restrict` integration tests.
//!
//! Exercises the CLI subcommand against real-filesystem temp dirs: the `.remargin.yaml` entry and
//! its enforcement by the per-op guard, the wildcard form, `--json` output, `--also-deny-bash`
//! parsing, and what restrict deliberately does not do — project settings files, write a sidecar
//! or a gitignore line, or appear on the MCP surface.

#[cfg(test)]
#[path = "cli_claude_restrict/tests.rs"]
mod tests;
