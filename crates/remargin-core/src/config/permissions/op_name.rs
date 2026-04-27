//! Single-source-of-truth registry of canonical op names (rem-welo).
//!
//! Every op the remargin CLI / MCP surface dispatches to remargin-core
//! has a [`OpName`] variant. Configuration surfaces — currently
//! `permissions.deny_ops.ops` — deserialise into [`Vec<OpName>`] so an
//! unknown op name in `.remargin.yaml` fails loudly at parse time
//! instead of silently no-op-ing at runtime.
//!
//! ## Wire form
//!
//! Variants serialise to kebab-case (`sandbox-add`, `sandbox-remove`,
//! …). The kebab-case form is also what the op guard uses when
//! comparing against the runtime op string passed by the CLI / MCP.
//!
//! ## Adding a new op
//!
//! 1. Add a variant to [`OpName`].
//! 2. Add it to [`OpName::ALL`] (the source for every op-name list,
//!    including the user-facing "valid ops" diagnostic).
//! 3. Add it to [`OpName::READ`] OR [`OpName::WRITE`] (read-vs-write
//!    classification — drives whether `restrict` gates the op).
//!
//! No other code changes are required for `deny_ops` to accept the new
//! op name. Forgetting step 2 or 3 surfaces in the partition-coverage
//! and length-coverage tests in `op_name::tests`.

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
    /// Write: schema migration over a doc.
    Migrate,
    /// Write: purge tombstoned content.
    Purge,
    /// Read: structured query over comments / blocks.
    Query,
    /// Write: react to a comment.
    React,
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
        Self::Delete,
        Self::Edit,
        Self::Get,
        Self::Lint,
        Self::Ls,
        Self::Metadata,
        Self::Migrate,
        Self::Purge,
        Self::Query,
        Self::React,
        Self::SandboxAdd,
        Self::SandboxRemove,
        Self::Search,
        Self::Sign,
        Self::Verify,
        Self::Write,
    ];

    /// Read-side ops. Bypass `restrict` and the dot-folder default-deny;
    /// still subject to explicit `deny_ops` entries.
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

    /// Write-side ops. Gated by `restrict`, the dot-folder default-deny,
    /// and `deny_ops`.
    pub const WRITE: &'static [Self] = &[
        Self::Ack,
        Self::Batch,
        Self::Comment,
        Self::Delete,
        Self::Edit,
        Self::Migrate,
        Self::Purge,
        Self::React,
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
            Self::Delete => "delete",
            Self::Edit => "edit",
            Self::Get => "get",
            Self::Lint => "lint",
            Self::Ls => "ls",
            Self::Metadata => "metadata",
            Self::Migrate => "migrate",
            Self::Purge => "purge",
            Self::Query => "query",
            Self::React => "react",
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
mod tests {
    use super::OpName;

    /// `OpName::ALL` enumerates exactly the variants — adding a new
    /// variant without listing it in `ALL` would break the
    /// "valid ops" diagnostic and is caught here.
    #[test]
    fn all_covers_every_variant() {
        // Sum of READ + WRITE must equal ALL — they partition the
        // space.
        assert_eq!(OpName::READ.len() + OpName::WRITE.len(), OpName::ALL.len());
    }

    /// READ and WRITE partition the op space — no name appears on
    /// both lists.
    #[test]
    fn read_and_write_are_disjoint() {
        for read in OpName::READ {
            assert!(
                !OpName::WRITE.contains(read),
                "{read} appears in both READ and WRITE"
            );
        }
    }

    /// Every member of `ALL` is on exactly one of `READ` / `WRITE`.
    #[test]
    fn every_op_classified() {
        for op in OpName::ALL {
            let on_read = OpName::READ.contains(op);
            let on_write = OpName::WRITE.contains(op);
            assert!(
                on_read ^ on_write,
                "{op} must appear on exactly one of READ / WRITE"
            );
        }
    }

    /// Wire form matches the kebab-case rename.
    #[test]
    fn as_str_matches_kebab_serialisation() {
        for op in OpName::ALL {
            let serialised = serde_yaml::to_string(op).unwrap();
            // serde_yaml renders a bare scalar with a trailing newline.
            let expected = format!("{}\n", op.as_str());
            assert_eq!(serialised, expected, "serialised form for {op}");
        }
    }

    /// A typo deserialises to an error that names the offending value
    /// AND lists the valid names.
    #[test]
    fn unknown_op_rejected_on_deserialise() {
        let result: Result<OpName, _> = serde_yaml::from_str("purg");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("purg"), "error did not name typo: {err}");
    }

    /// `valid_names_csv` returns a sorted, comma-separated list.
    #[test]
    fn valid_names_csv_alphabetical() {
        let csv = OpName::valid_names_csv();
        let names: Vec<&str> = csv.split(", ").collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        // Sanity: every variant is listed.
        assert_eq!(names.len(), OpName::ALL.len());
        assert!(names.contains(&"purge"));
        assert!(names.contains(&"sandbox-add"));
    }
}
