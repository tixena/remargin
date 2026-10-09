//! Directory / recursive `verify` at the CLI and MCP surfaces.
//!
//! Covers folder-mode `--json` (`FolderVerifyReport`), folder-mode text
//! (only damaged files listed), the single-file shape, and
//! CLI/MCP parity including the `file` alias for a directory.

#[cfg(test)]
#[path = "cli_verify_dir/tests.rs"]
mod tests;
