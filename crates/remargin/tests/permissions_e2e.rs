//! End-to-end permissions integration tests.
//!
//! Covers the cross-cutting scenarios the per-feature integration files do not exercise on their
//! own:
//!
//! - The MCP surface has no `restrict` tool, and calling it leaves the realm untouched.
//! - Multi-path: restrict A + B, unrestrict A leaves B.
//! - Per-op no-cache: a manual `.remargin.yaml` edit is picked up on the very next op.
//! - Realms that have no `permissions:` block keep working.
//! - Dot-folder default-deny under `trusted_roots`, and the `allow_dot_folders` exception.
//! - `--also-deny-bash` and `--cli-allowed` land on the `.remargin.yaml` entry.

#[cfg(test)]
#[path = "permissions_e2e/tests.rs"]
mod tests;
