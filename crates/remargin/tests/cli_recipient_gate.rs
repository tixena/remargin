//! CLI + lint end-to-end coverage for recipient registry validation.
//!
//! `remargin comment --to <unknown>` in a registered-mode realm exits non-zero with a message
//! naming the bad recipient, and `remargin lint --json` on a doc with an unknown recipient in a
//! registered-mode realm produces `ok:false` and a recipient finding.

#[cfg(test)]
#[path = "cli_recipient_gate/tests.rs"]
mod tests;
