//! `remargin claude session-guard` integration tests.
//!
//! Exercises the CLI subcommand against a real-filesystem temp realm. The
//! `SessionStart` hook contract is the source of truth: the guard cannot
//! block a session (`SessionStart` has no blocking or decision control),
//! so it always exits 0 and its only signal is the diagnostic JSON on
//! stdout (`hookSpecificOutput.additionalContext` for Claude,
//! `systemMessage` for the user). Install / uninstall / test manage the
//! hook entry.

#[cfg(test)]
#[path = "cli_session_guard/tests.rs"]
mod tests;
