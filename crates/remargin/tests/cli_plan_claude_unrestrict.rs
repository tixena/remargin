//! `remargin plan claude unrestrict` integration tests.
//!
//! Real-filesystem temp dirs, `assert_cmd` invocations and JSON output assertions cover the
//! no-write invariant, plan-then-act parity, the wildcard form end to end, drift detection,
//! multi-path independence, and a path that was never restricted.

#[cfg(test)]
#[path = "cli_plan_claude_unrestrict/tests.rs"]
mod tests;
