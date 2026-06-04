//! Single source of truth for canonical op names.
//!
//! Config surfaces deserialise into [`Vec<OpName>`] so unknown op
//! names in `.remargin.yaml` fail loudly at parse time. Variants
//! serialise to kebab-case, the same form the op guard compares
//! against the runtime op string. Adding an op: add a variant, add
//! it to [`OpName::ALL`], and add it to [`OpName::READ`] or
//! [`OpName::WRITE`] — partition-coverage tests catch omissions.

use core::fmt;

use serde::{Deserialize, Serialize};

/// Canonical op name, used wherever remargin validates or classifies an op.
///
/// Variants are listed alphabetically; classification (read vs write)
/// lives on the [`OpName::READ`] and [`OpName::WRITE`] partitions,
/// not in the variant ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum OpName {
    /// Write: acknowledge a comment.
    Ack,
    /// Write: batch op container.
    Batch,
    /// Write: append a new comment.
    Comment,
    /// Read: list comments on a doc.
    Comments,
    /// Write: copy a file (body-only for markdown; preserves source).
    Cp,
    /// Write: delete a comment / block.
    Delete,
    /// Write: edit a comment / block.
    Edit,
    /// Read: get the rendered body of a doc.
    Get,
    /// Read: structural lint over a doc.
    Lint,
    /// Read: list files under a path.
    Ls,
    /// Read: doc / block metadata.
    Metadata,
    /// Write: rename / move a file or directory.
    Mv,
    /// Write: purge tombstoned content.
    Purge,
    /// Read: structured query over comments / blocks.
    Query,
    /// Write: react to a comment.
    React,
    /// Write: find/replace across document body text.
    Replace,
    /// Write: stage a doc into a sandbox.
    SandboxAdd,
    /// Write: remove a doc from a sandbox.
    SandboxRemove,
    /// Read: text search over a doc.
    Search,
    /// Write: sign a comment / block.
    Sign,
    /// Read: integrity verification.
    Verify,
    /// Write: full-doc rewrite.
    Write,
}

impl OpName {
    /// Every variant, sorted by kebab-case wire form. Drives the
    /// user-visible "valid ops: …" diagnostic and the lint surface.
    pub const ALL: &'static [Self] = &[
        Self::Ack,
        Self::Batch,
        Self::Comment,
        Self::Comments,
        Self::Cp,
        Self::Delete,
        Self::Edit,
        Self::Get,
        Self::Lint,
        Self::Ls,
        Self::Metadata,
        Self::Mv,
        Self::Purge,
        Self::Query,
        Self::React,
        Self::Replace,
        Self::SandboxAdd,
        Self::SandboxRemove,
        Self::Search,
        Self::Sign,
        Self::Verify,
        Self::Write,
    ];

    /// Read-side ops. Bypass `trusted_roots` and the dot-folder
    /// default-deny; still subject to explicit `deny_ops` entries.
    pub const READ: &'static [Self] = &[
        Self::Comments,
        Self::Get,
        Self::Lint,
        Self::Ls,
        Self::Metadata,
        Self::Query,
        Self::Search,
        Self::Verify,
    ];

    /// Write-side ops. Gated by `trusted_roots`, the dot-folder
    /// default-deny, and `deny_ops`.
    pub const WRITE: &'static [Self] = &[
        Self::Ack,
        Self::Batch,
        Self::Comment,
        Self::Cp,
        Self::Delete,
        Self::Edit,
        Self::Mv,
        Self::Purge,
        Self::React,
        Self::Replace,
        Self::SandboxAdd,
        Self::SandboxRemove,
        Self::Sign,
        Self::Write,
    ];

    /// Kebab-case wire form (matches the deserialised representation).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ack => "ack",
            Self::Batch => "batch",
            Self::Comment => "comment",
            Self::Comments => "comments",
            Self::Cp => "cp",
            Self::Delete => "delete",
            Self::Edit => "edit",
            Self::Get => "get",
            Self::Lint => "lint",
            Self::Ls => "ls",
            Self::Metadata => "metadata",
            Self::Mv => "mv",
            Self::Purge => "purge",
            Self::Query => "query",
            Self::React => "react",
            Self::Replace => "replace",
            Self::SandboxAdd => "sandbox-add",
            Self::SandboxRemove => "sandbox-remove",
            Self::Search => "search",
            Self::Sign => "sign",
            Self::Verify => "verify",
            Self::Write => "write",
        }
    }

    /// Render the comma-separated list of valid op names, sorted by
    /// kebab-case form. Used in user-facing diagnostics surfaced when
    /// `.remargin.yaml` carries an unknown op name.
    #[must_use]
    pub fn valid_names_csv() -> String {
        let mut buf = String::new();
        for (idx, op) in Self::ALL.iter().enumerate() {
            if idx > 0 {
                buf.push_str(", ");
            }
            buf.push_str(op.as_str());
        }
        buf
    }
}

impl fmt::Display for OpName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
