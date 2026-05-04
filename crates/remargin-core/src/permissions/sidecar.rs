//! Sidecar manager for `restrict` reversal (rem-yj1j.4 / T25, slice 2 —
//! `rem-70za`).
//!
//! `remargin restrict` writes Claude permission rules into both
//! `.claude/settings.local.json` (project-scope) and
//! `~/.claude/settings.json` (user-scope). To `unprotect` cleanly the
//! reverse path needs to know **exactly** which rule strings each
//! `restrict` invocation added — otherwise it would be guessing at
//! manually-maintained rules.
//!
//! The sidecar lives at `<anchor>/.claude/.remargin-restrictions.json`
//! and records, per restricted path, the deny + allow strings that
//! were appended and which settings files they were appended to,
//! plus an ISO 8601 `added_at` timestamp.
//!
//! ## Format
//!
//! ```json
//! {
//!   "version": 1,
//!   "entries": {
//!     "/abs/path/to/restricted": {
//!       "added_at": "2026-04-26T10:00:00Z",
//!       "added_to_files": [
//!         ".claude/settings.local.json",
//!         "/home/u/.claude/settings.json"
//!       ],
//!       "allow": [],
//!       "deny": ["Edit(/abs/path/to/restricted/**)", "..."]
//!     }
//!   }
//! }
//! ```
//!
//! `entries` keys are the canonical absolute restricted-path strings
//! used as the sidecar's primary index — these are the same paths the
//! rule generator (slice 1) builds the rule strings around.
//!
//! ## `.gitignore` automation
//!
//! Per Decision 3, the sidecar must not be committed (it embeds
//! absolute paths and per-machine timestamps). [`save`] adds
//! `.claude/.remargin-restrictions.json` to `<anchor>/.gitignore` on
//! the first write. Idempotent: re-running [`save`] does not duplicate
//! the line.

#[cfg(test)]
mod tests;

extern crate alloc;

use alloc::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use os_shim::System;
use serde::{Deserialize, Serialize};

/// Current sidecar format version. Bumped only on incompatible shape
/// changes. Loaders refuse files whose version they do not recognise.
pub const SIDECAR_VERSION: u32 = 1;

/// Sidecar relative path under the anchor directory.
pub const SIDECAR_RELATIVE_PATH: &str = ".claude/.remargin-restrictions.json";

/// `.gitignore` entry written on first sidecar save.
pub const SIDECAR_GITIGNORE_ENTRY: &str = ".claude/.remargin-restrictions.json";

/// Top-level sidecar shape persisted as JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Sidecar {
    /// One entry per restricted path (canonical absolute string used
    /// as the key). [`BTreeMap`] keeps the on-disk JSON deterministic.
    pub entries: BTreeMap<String, SidecarEntry>,
    /// Format version. Loaders check this against [`SIDECAR_VERSION`].
    pub version: u32,
}

impl Sidecar {
    /// Build an empty sidecar pinned to the current
    /// [`SIDECAR_VERSION`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            version: SIDECAR_VERSION,
        }
    }
}

/// One restricted path's tracking record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SidecarEntry {
    /// ISO 8601 timestamp. Caller-supplied so tests can pin a value
    /// and the production caller can use [`chrono::Utc::now`].
    pub added_at: String,
    /// Settings files the rules were merged into. Mix of relative
    /// (project-scope) and absolute (user-scope) paths. Reverse uses
    /// this list to know which files to scan.
    pub added_to_files: Vec<PathBuf>,
    /// Allow rules added by the matching `apply_rules` call.
    pub allow: Vec<String>,
    /// Deny rules added by the matching `apply_rules` call.
    pub deny: Vec<String>,
}

/// Add an entry under `target_path`.
///
/// Replaces any prior entry for the same path so a re-apply records
/// the latest deltas; the apply caller (slice 3) is responsible for
/// de-duping rules in the settings files themselves. Calls [`save`]
/// internally so the sidecar is always persisted before returning.
///
/// # Errors
///
/// Forwards I/O / serde failures from [`load`] and [`save`].
pub fn add_entry(
    system: &dyn System,
    anchor: &Path,
    target_path: &str,
    entry: SidecarEntry,
) -> Result<()> {
    let mut sidecar = load(system, anchor)?;
    sidecar.entries.insert(String::from(target_path), entry);
    save(system, anchor, &sidecar)
}

/// Load the sidecar at `<anchor>/.claude/.remargin-restrictions.json`.
///
/// Missing file ⇒ empty [`Sidecar`] at the current version. A version
/// mismatch errors with a clear message naming the on-disk version so
/// the user knows which release introduced the change.
///
/// # Errors
///
/// - JSON parse failures (with the file path in the context).
/// - Version mismatch.
pub fn load(system: &dyn System, anchor: &Path) -> Result<Sidecar> {
    let path = sidecar_path(anchor);
    let body = match system.read_to_string(&path) {
        Ok(body) => body,
        Err(_err) => return Ok(Sidecar::new()),
    };
    let sidecar: Sidecar = serde_json::from_str(&body)
        .with_context(|| format!("parsing sidecar JSON at {}", path.display()))?;
    if sidecar.version != SIDECAR_VERSION {
        anyhow::bail!(
            "sidecar at {} has version {}, expected {}; the running remargin does not understand this format",
            path.display(),
            sidecar.version,
            SIDECAR_VERSION
        );
    }
    Ok(sidecar)
}

/// Remove the entry for `target_path` and return it. Returns
/// `Ok(None)` when nothing was tracked. Persists the updated sidecar.
///
/// # Errors
///
/// Forwards I/O / serde failures from [`load`] and [`save`].
pub fn remove_entry(
    system: &dyn System,
    anchor: &Path,
    target_path: &str,
) -> Result<Option<SidecarEntry>> {
    let mut sidecar = load(system, anchor)?;
    let removed = sidecar.entries.remove(target_path);
    save(system, anchor, &sidecar)?;
    Ok(removed)
}

/// Persist `sidecar` to disk and ensure
/// `.claude/.remargin-restrictions.json` is in `<anchor>/.gitignore`.
///
/// JSON is pretty-printed for diff-friendly inspection. Writes go
/// through `system.write` directly — atomic write-then-rename lives
/// with slice 3 (`rem-7m4u`) since it needs careful coordination with
/// the settings-file merge.
///
/// # Errors
///
/// I/O failures from creating the `.claude/` directory, writing the
/// sidecar JSON, or updating `.gitignore`.
pub fn save(system: &dyn System, anchor: &Path, sidecar: &Sidecar) -> Result<()> {
    let path = sidecar_path(anchor);
    if let Some(parent) = path.parent() {
        system
            .create_dir_all(parent)
            .with_context(|| format!("creating sidecar directory {}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(sidecar).context("serializing sidecar JSON")?;
    system
        .write(&path, body.as_bytes())
        .with_context(|| format!("writing sidecar to {}", path.display()))?;
    ensure_gitignored(system, anchor)?;
    Ok(())
}

/// Resolved on-disk path for the sidecar.
#[must_use]
pub fn sidecar_path(anchor: &Path) -> PathBuf {
    anchor.join(SIDECAR_RELATIVE_PATH)
}

/// Append the sidecar's entry to `<anchor>/.gitignore` if absent.
/// Creates `.gitignore` if missing.
fn ensure_gitignored(system: &dyn System, anchor: &Path) -> Result<()> {
    let gitignore = anchor.join(".gitignore");
    let existing = system.read_to_string(&gitignore).unwrap_or_default();
    if existing
        .lines()
        .any(|line| line.trim() == SIDECAR_GITIGNORE_ENTRY)
    {
        return Ok(());
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(SIDECAR_GITIGNORE_ENTRY);
    updated.push('\n');
    system
        .write(&gitignore, updated.as_bytes())
        .with_context(|| format!("updating {}", gitignore.display()))
}
