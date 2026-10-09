//! End-to-end tests for `remargin get --binary`.
//!
//! Verifies the three binary-mode output shapes:
//! - Default (no `--out`, no `--json`): raw bytes to stdout.
//! - `--json`: base64 payload alongside `mime`, `size_bytes`, `path`.
//! - `--out <path>`: bytes written to the target file; stdout gets a summary.
//!
//! Also covers the `.md` rejection symmetry with `write --binary`.

#[cfg(test)]
#[path = "cli_binary_get/tests.rs"]
mod tests;
