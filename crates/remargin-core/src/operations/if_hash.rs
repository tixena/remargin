//! Optional `if_hash` guard for write ops: refuse the op when the file changed since the caller
//! read it.
//!
//! The check runs before the write and is not atomic across processes; a server that serialises
//! writers (the hub) makes it atomic.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use os_shim::System;
use serde_json::{Value, json};
use thiserror::Error;

use crate::crypto::file_hash;

/// Error text when `if_hash` cannot name exactly one file; shared so the MCP and CLI cannot drift.
pub const NEEDS_ONE_FILE: &str = "if_hash needs a single file";

/// The file's current hash differs from the caller's `if_hash`. Nothing was written.
///
/// Hosts branch on `error_kind == "stale_version"` (via [`StaleVersion::to_json`]).
#[derive(Debug, Clone, Error)]
#[error("{}", self.headline())]
#[non_exhaustive]
pub struct StaleVersion {
    pub current_hash: String,
    pub expected: String,
    pub path: PathBuf,
}

impl StaleVersion {
    /// One-line plain-English summary carrying the recovery path.
    #[must_use]
    pub fn headline(&self) -> String {
        format!(
            "stale_version: {} changed since it was read (expected {}, current {}); nothing was \
             written. Re-read the file and decide whether to retry, relocate or abandon the change.",
            self.path.display(),
            self.expected,
            self.current_hash,
        )
    }

    /// JSON shape for `--json` / MCP tool errors.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "current_hash": self.current_hash,
            "error_kind": "stale_version",
            "expected": self.expected,
            "headline": self.headline(),
            "path": self.path.display().to_string(),
        })
    }
}

/// Fail with [`StaleVersion`] unless the file at `path` (relative to `base_dir`) hashes to
/// `expected`.
///
/// # Errors
///
/// Returns [`StaleVersion`] on a mismatch, an error when `path` is a directory ("`if_hash` needs a
/// single file"), or when the file cannot be read.
pub fn check_if_hash(
    system: &dyn System,
    base_dir: &Path,
    path: &Path,
    expected: &str,
) -> Result<()> {
    let full = base_dir.join(path);
    if system.is_dir(&full).unwrap_or(false) {
        bail!("{NEEDS_ONE_FILE}, but {} is a folder", full.display());
    }
    let current_hash = file_hash(system, &full)?;
    if current_hash == expected {
        return Ok(());
    }
    Err(StaleVersion {
        current_hash,
        expected: String::from(expected),
        path: full,
    }
    .into())
}
