//! `remargin plan claude restrict` integration tests.
//!
//! Real-filesystem temp dirs, `assert_cmd` invocations and JSON output assertions cover the
//! no-write invariant, plan + apply parity with the noop on replan, the absence of projected
//! settings rules and of allow-vs-deny overlap conflicts, anchor-surprise detection, and the
//! wildcard form.

#[cfg(test)]
#[path = "cli_plan_claude_restrict/tests.rs"]
mod tests;
