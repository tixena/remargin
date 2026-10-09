//! Regression guard for CLI/MCP adapter bloat.
//!
//! Every mutating surface in this workspace is implemented twice: once as a clap `cmd_*` helper
//! in the CLI binary, and once as a `handle_*` tool handler in the in-process MCP server. Both
//! adapter layers are meant to stay thin: argument extraction plus response formatting, with any
//! non-trivial logic living once in core.
//!
//! This test parses the two adapter files, counts the physical lines of every `cmd_*` /
//! `handle_*` free-standing function, and asserts each is under a cap. A new handler over the cap
//! must either shrink by pushing logic to core or be allowlisted with a rationale.
//!
//! To refresh after a legitimate migration: if a function drops below the cap, remove its
//! allowlist entry. If a new function intentionally exceeds the cap, add it with a one-line
//! reason.
#[cfg(test)]
#[path = "adapter_loc_cap/tests.rs"]
mod tests;
