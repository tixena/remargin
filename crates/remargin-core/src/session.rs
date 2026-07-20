//! Session orchestration for `remargin session launch`.
//!
//! Where [`crate::config::identity`] walks *up* to answer "who am I?" for
//! a single caller, this module walks *down* from a directory to
//! enumerate every realm below it that declares its own identity — the
//! fan-out that `remargin session launch` needs. Gated behind the
//! `session` Cargo feature; absent from the default build.

pub mod backend;
pub mod discovery;
pub mod spec;

#[cfg(test)]
mod tests;
