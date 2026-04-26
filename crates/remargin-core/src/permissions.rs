//! Layer 1 permissions enforcement (rem-yj1j.2 / T23).
//!
//! Every mutating remargin op (CLI or MCP) must call [`op_guard::pre_mutate_check`]
//! immediately after resolving its target path. The check parent-walks
//! `.remargin.yaml` from the target's parent directory, accumulates the
//! `permissions:` blocks via T22's [`crate::config::permissions::resolve::resolve_permissions`],
//! and refuses the op when:
//!
//! - The target sits under a `restrict` entry (any restriction applies
//!   to mutating ops).
//! - A `deny_ops` entry covers the target AND its `ops` list contains
//!   the current op name.
//! - The target is inside a dot-folder under a restricted subtree, and
//!   that dot-folder is not in `allow_dot_folders` (the `.remargin/`
//!   folder is always allowed).
//!
//! Read-side ops (`get`, `metadata`, `comments`, `query`, `search`,
//! `ls`, `verify`, `lint`) are unaffected by `restrict`. To block reads,
//! callers declare an explicit `deny_ops` entry naming the read op.
//!
//! ## No caching
//!
//! Per the unified design (Decision 13), enforcement re-runs the
//! resolver on every check. Configuration changes take effect at the
//! next op without a restart.

pub mod claude_sync;
pub mod inspect;
pub mod op_guard;
pub mod sidecar;
