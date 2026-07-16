//! `remargin doctor` integration tests for the `SessionStart` guard check.
//!
//! Installs the hooks through the CLI, then runs `remargin doctor` and
//! asserts on its exit code and structured findings: a fully-wired stack
//! (`PreToolUse` hook + `SessionStart` guard) is clean, and a stack
//! missing the guard reports `SessionGuardMissing`.

#[cfg(test)]
#[path = "cli_doctor/tests.rs"]
mod tests;
