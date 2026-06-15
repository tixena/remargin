//! `remargin rm` directory-removal integration tests.
//!
//! Exercises the CLI subcommand against real-filesystem temp dirs.
//! The unit-test layer (`crates/remargin-core/src/document/tests.rs`)
//! covers the algorithm over a mock filesystem; this surface check
//! confirms the CLI plumbs through to it, the JSON report shape is
//! documented, the all-or-nothing abort path leaves the tree intact on a
//! real unreadable file, and a real symlink is unlinked rather than
//! followed.

#[cfg(test)]
#[path = "cli_rm/tests.rs"]
mod tests;
