//! End-to-end tests for `remargin get` outbound-link extraction.
//!
//! Verifies the additive `links` array on `--json` and the trailing
//! pretty `Links` block on the default human output, including same-folder
//! internal resolution, broken-link omission, and zero-link suppression.

#[cfg(test)]
#[path = "cli_get_links/tests.rs"]
mod tests;
