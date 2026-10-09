//! `remargin permissions show / check` CLI + MCP integration tests.
//!
//! Covers:
//!
//! - `permissions show` and `permissions check` surface the parent-walked `.remargin.yaml`
//!   permissions correctly (text + JSON, restricted exit 0).
//! - When no rules cover a path, `check` exits 1 and `show` lists the empty surface.
//! - MCP `permissions_show` and `permissions_check` parity with CLI `--json` output.
//! - The MCP sandbox boundary at dispatch time.
//!
//! The tests stage a `.remargin.yaml` directly.

#[cfg(test)]
#[path = "cli_permissions/tests.rs"]
mod tests;
