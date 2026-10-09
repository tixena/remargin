//! End-to-end CLI tests for `remargin query --json`.
//!
//! Proves the verbose `--json` payload and its `base_path`, and that the command line has no
//! columnar `--compact` flag (the columnar shape is served by the MCP `query` tool only).

#[cfg(test)]
#[path = "cli_query_json/tests.rs"]
mod tests;
