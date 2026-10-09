//! Per-subcommand `--config` happy-path tests.
//!
//! Two realms are set up: a `walker` realm whose `.remargin.yaml` declares `walker-agent` and a
//! `flag` realm whose `.remargin.yaml` declares `flag-agent`. Each test runs the subcommand from
//! inside `walker` with `--config` pointing at `flag`'s yaml. A subcommand that drops `--config`
//! attributes the operation to `walker-agent` and fails the `flag-agent` assertion.
//!
//! Tests that inspect author attribution prove `--config` end-to-end. Tests that merely exercise
//! a subcommand (write / rm / purge) prove `--config` at least reaches the resolver.

#[cfg(test)]
#[path = "cli_config_honored/tests.rs"]
mod tests;
