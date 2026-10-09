//! `remargin claude unrestrict` integration tests.
//!
//! Exercises the CLI subcommand against real-filesystem temp dirs: the restrict + unrestrict
//! round-trip, the per-op guard's enforcement before and after, the wildcard cycle, `--json`
//! output, `--strict`, the scrub of rules an older install projected, and the subcommand's
//! absence from the MCP surface.

#[cfg(test)]
#[path = "cli_claude_unrestrict/tests.rs"]
mod tests;
