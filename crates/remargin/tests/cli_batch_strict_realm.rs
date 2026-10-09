//! Cross-mode realm hazard for `remargin batch`.
//!
//! A caller standing in an open-mode directory who batch-writes into a strict-mode realm must
//! not leave unsigned comments in that realm, where `remargin verify` would then fail. Either
//! the batch escalates to strict (signs) or it refuses with a cross-mode error.

#[cfg(test)]
#[path = "cli_batch_strict_realm/tests.rs"]
mod tests;
