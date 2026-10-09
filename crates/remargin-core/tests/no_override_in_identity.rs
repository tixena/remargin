//! Structural guard: the identity overlay model must not come back.
//!
//! Greps the whole `crates/` tree and fails if any symbol of that model is reintroduced, or if
//! the word `override` appears inside the identity-resolution files. Identity is resolved by
//! `config::identity::resolve_identity` alone. The allowlist is empty; every entry must carry
//! an explicit reason.

#[cfg(test)]
#[path = "no_override_in_identity/tests.rs"]
mod tests;
