//! End-to-end CLI tests for `--pending-for-me`, `--pending-broadcast` and the broadcasts
//! `--pending` includes.
//!
//! These tests exercise the binary (not just core) to prove the CLI adapter wires the flags
//! through and picks up the caller's identity from `.remargin.yaml`.

#[cfg(test)]
#[path = "cli_query_pending/tests.rs"]
mod tests;
