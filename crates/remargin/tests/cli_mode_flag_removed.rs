//! There is no `--mode` CLI flag. Mode is a property of the directory tree (resolved by walking
//! upward for `.remargin.yaml`) and the caller cannot change it. Passing `--mode` must produce a
//! clap-level "unexpected argument" error, not silent acceptance that would let an agent weaken
//! enforcement on a strict vault.

#[cfg(test)]
#[path = "cli_mode_flag_removed/tests.rs"]
mod tests;
