//! A realm declaring `mode: strict` admits document reads only from
//! active participants of its registry. An anonymous caller, a caller
//! resolved in a neighbouring open realm, and a revoked participant are
//! all refused — with no document text on stdout. An active participant
//! reads normally, and a walk rooted in an admitted directory
//! omits the nested strict realm instead of failing outright.

#[cfg(test)]
#[path = "cli_strict_realm_read_gate/tests.rs"]
mod tests;
