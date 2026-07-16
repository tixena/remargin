//! Layer 1 permissions enforcement.
//!
//! Every mutating op calls [`op_guard::pre_mutate_check`] after
//! resolving its target path. Enforcement re-runs the resolver on
//! every check — no caching, so config changes take effect at the
//! next op without a restart.

pub mod claude_sync;
pub mod doctor;
mod hook_settings;
pub mod inspect;
pub mod op_guard;
pub mod pretool;
pub mod pretool_install;
pub mod restrict;
pub mod session_guard;
pub mod session_guard_install;
pub mod sidecar;
pub mod unprotect;
