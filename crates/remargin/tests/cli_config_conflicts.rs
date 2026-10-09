//! `--config` must clap-conflict with `--identity`, `--type`, and `--key` on every
//! identity-aware subcommand.
//!
//! Mixing a whole-identity declaration with partial-identity flags produces the
//! "inherited-part-from-walk, replaced-part-from-flag" class of silent misattribution, so clap
//! rejects the combination at parse time, before the resolver sees it.
//!
//! The identity group is per-subcommand (not global), so the flags go AFTER the subcommand
//! name. This file iterates over every subcommand that flattens `IdentityArgs` and locks the
//! conflict in; `subcommands_table_matches_identity_flattening_commands` parses the `Commands`
//! enum in `cli.rs` directly, so a new `IdentityArgs`-flattening subcommand missing from the
//! table fails the test run.

#[cfg(test)]
#[path = "cli_config_conflicts/tests.rs"]
mod tests;
