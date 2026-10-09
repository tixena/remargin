//! Integration tests for path expansion.
//!
//! Covers the adapter-boundary behaviour:
//!
//! - CLI string/PathBuf args (`get`, `metadata`, `ls`, `rm`, `obsidian`
//!   `--vault-path`) expand `~`, `$VAR`, `${VAR}` before the command
//!   dispatches.
//! - MCP tools receive already-expanded paths through the in-process
//!   `mcp::process_request` entry point.
//! - Both surfaces agree on the same inputs (adapter parity).
//! - Undefined env vars and `~user` produce a clear named error.
//!
//! Env-var manipulation is done via a hermetic fixture home: we set
//! `HOME` in the child process (for CLI runs) and on a `MemorySystem`
//! (for MCP runs). No test mutates the parent process environment.

#[cfg(test)]
#[path = "cli_path_expansion/tests.rs"]
mod tests;
