//! Sandbox frontmatter operations.
//!
//! Per-identity, persisted "staged file" state stored as a top-level
//! `sandbox: [author@timestamp, ...]` frontmatter key. Shape mirrors comment
//! `ack` but lives on the document rather than on a comment.
//!
//! Semantics:
//!
//! - **Idempotent**: adding twice as the same identity preserves the
//!   original timestamp. "Is it staged" is the observable state, not
//!   "when was it last touched".
//! - **Per-identity scope**: removing only ever touches the caller's own
//!   entry. Another identity's entries are invisible.
//! - **Best-effort multi-file**: operations continue across files on
//!   individual failures, returning per-path success and failure details.
//! - **Markdown-only**: non-`.md` inputs fail with `not a markdown file`
//!   and do not mutate state.
//! - **Empty collapse**: removing the last entry deletes the `sandbox`
//!   key entirely from the frontmatter.
//! - **Integrity-safe**: sandbox entries are document-level frontmatter
//!   only; comment checksums and signatures are computed per comment and
//!   do not include any frontmatter, so sandbox mutations never invalidate
//!   existing comments.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, FixedOffset, Utc};
use os_shim::System;
use serde::Serialize;
use serde_json::Value;
use tixschema::model_schema;

use crate::config::ResolvedConfig;
use crate::document::allowlist;
use crate::frontmatter;
use crate::operations::verify::commit_with_verify;
use crate::parser::{self, SandboxEntry};
use crate::permissions::op_guard::pre_mutate_check_for_caller;
use crate::writer::ensure_not_forbidden_target;

/// Outcome of a bulk `sandbox add` or `sandbox remove` invocation.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct SandboxBulkResult {
    /// Files that were mutated (entry added or removed).
    pub changed: Vec<PathBuf>,
    /// Files that failed: path plus a human-readable reason.
    pub failed: Vec<SandboxFailure>,
    /// Files that matched a no-op (already staged, or nothing to remove).
    pub skipped: Vec<PathBuf>,
}

impl SandboxBulkResult {
    /// `changed_key` is `"added"` for `sandbox add`, `"removed"` for remove.
    #[must_use]
    pub fn to_json(&self, base_dir: &Path, changed_key: &str) -> Value {
        let changed = display_paths(&self.changed, base_dir);
        let skipped = display_paths(&self.skipped, base_dir);
        let failed = self
            .failed
            .iter()
            .map(|f| SandboxFailureEntry {
                path: strip_prefix_display(&f.path, base_dir),
                reason: f.reason.clone(),
            })
            .collect();
        let report = match changed_key {
            "added" => serde_json::to_value(SandboxAddReport {
                added: changed,
                failed,
                skipped,
            }),
            _ => serde_json::to_value(SandboxRemoveReport {
                failed,
                removed: changed,
                skipped,
            }),
        };
        report.unwrap_or(Value::Null)
    }
}

/// A per-file failure entry in a sandbox bulk report.
#[derive(Debug, Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct SandboxFailureEntry {
    pub path: String,
    pub reason: String,
}

/// JSON report for `sandbox add`.
#[derive(Debug, Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct SandboxAddReport {
    pub added: Vec<String>,
    pub failed: Vec<SandboxFailureEntry>,
    pub skipped: Vec<String>,
}

/// JSON report for `sandbox remove`.
#[derive(Debug, Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct SandboxRemoveReport {
    pub failed: Vec<SandboxFailureEntry>,
    pub removed: Vec<String>,
    pub skipped: Vec<String>,
}

/// A single entry in a `sandbox list` report.
#[derive(Debug, Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct SandboxListEntry {
    pub path: String,
    pub since: String,
}

impl SandboxListEntry {
    #[must_use]
    pub fn from_listing(listing: &SandboxListing, root: &Path, absolute: bool) -> Self {
        let path = if absolute {
            listing.path.display().to_string()
        } else {
            strip_prefix_display(&listing.path, root)
        };
        Self {
            path,
            since: listing.since.to_rfc3339(),
        }
    }
}

/// A per-file failure record produced by bulk sandbox operations.
#[derive(Debug)]
#[non_exhaustive]
pub struct SandboxFailure {
    pub path: PathBuf,
    pub reason: String,
}

/// A single entry returned from a `sandbox list` walk: the file plus the
/// caller's staging timestamp.
#[derive(Debug)]
#[non_exhaustive]
pub struct SandboxListing {
    pub path: PathBuf,
    pub since: DateTime<FixedOffset>,
}

/// Strip `base` from `path` for display; if `path` is not anchored at
/// `base`, render the full path. Used for adapter-friendly relative
/// paths in response JSON.
fn strip_prefix_display(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn display_paths(paths: &[PathBuf], base_dir: &Path) -> Vec<String> {
    paths
        .iter()
        .map(|p| strip_prefix_display(p, base_dir))
        .collect()
}

/// Add the caller (`identity`) to the sandbox list of each file in
/// `files`. Returns per-file results — the call never short-circuits on a
/// single-file failure.
///
/// - Non-markdown files fail with `not a markdown file` and are recorded
///   in `failed`.
/// - Files where the caller already has an entry are recorded in
///   `skipped` with the existing timestamp preserved.
/// - Files where a new entry was appended are recorded in `changed`.
///
/// # Errors
///
/// Returns an error only on programmer mistakes (e.g. a `None` identity).
/// Per-file I/O failures surface via [`SandboxBulkResult::failed`].
pub fn add_to_files(
    system: &dyn System,
    files: &[PathBuf],
    identity: &str,
    config: &ResolvedConfig,
) -> Result<SandboxBulkResult> {
    if identity.is_empty() {
        bail!("identity is required for sandbox add");
    }

    let now = Utc::now().fixed_offset();
    let mut result = SandboxBulkResult::default();

    for file in files {
        match add_one(system, file, identity, now, config) {
            Ok(true) => result.changed.push(file.clone()),
            Ok(false) => result.skipped.push(file.clone()),
            Err(err) => result.failed.push(SandboxFailure {
                path: file.clone(),
                reason: format!("{err:#}"),
            }),
        }
    }

    Ok(result)
}

/// Remove the caller's (`identity`) sandbox entry from each file in
/// `files`. Same best-effort semantics as [`add_to_files`].
///
/// - Files with no entry for the caller are recorded in `skipped`.
/// - Files where an entry was removed are recorded in `changed`. When the
///   entry was the last one in the list, the entire `sandbox` key is
///   removed from the frontmatter.
///
/// # Errors
///
/// Returns an error only on programmer mistakes (e.g. a `None` identity).
pub fn remove_from_files(
    system: &dyn System,
    files: &[PathBuf],
    identity: &str,
    config: &ResolvedConfig,
) -> Result<SandboxBulkResult> {
    if identity.is_empty() {
        bail!("identity is required for sandbox remove");
    }

    let mut result = SandboxBulkResult::default();

    for file in files {
        match remove_one(system, file, identity, config) {
            Ok(true) => result.changed.push(file.clone()),
            Ok(false) => result.skipped.push(file.clone()),
            Err(err) => result.failed.push(SandboxFailure {
                path: file.clone(),
                reason: format!("{err:#}"),
            }),
        }
    }

    Ok(result)
}

/// Walk `root` and return every markdown file whose sandbox frontmatter
/// contains an entry for `identity`. Directories failing to walk are
/// skipped silently (matches how `query` handles unreadable files).
///
/// The returned paths are absolute (whatever `walk_dir` yields); callers
/// that want relative paths should strip the root prefix themselves — the
/// CLI does exactly that.
///
/// # Errors
///
/// Returns an error if `root` cannot be walked.
pub fn list_for_identity(
    system: &dyn System,
    root: &Path,
    identity: &str,
) -> Result<Vec<SandboxListing>> {
    let entries = system
        .walk_dir(root, false, false)
        .with_context(|| format!("walking directory {}", root.display()))?;

    let mut out = Vec::new();
    for entry in &entries {
        if !entry.is_file {
            continue;
        }
        let has_md = entry
            .path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if !has_md || !allowlist::is_visible(&entry.path, false) {
            continue;
        }

        let Ok(content) = system.read_to_string(&entry.path) else {
            continue;
        };
        let Ok(doc) = parser::parse(&content) else {
            continue;
        };
        let Ok(sandbox) = frontmatter::read_sandbox_entries(&doc) else {
            continue;
        };
        if let Some(entry_for_caller) = sandbox.iter().find(|e| e.author == identity) {
            out.push(SandboxListing {
                path: entry.path.clone(),
                since: entry_for_caller.ts,
            });
        }
    }

    Ok(out)
}

/// Parse a file, append the caller's sandbox entry if absent, and write
/// the result back. Returns `Ok(true)` when a new entry was appended,
/// `Ok(false)` when the file already contained one for the caller.
fn add_one(
    system: &dyn System,
    file: &Path,
    identity: &str,
    now: DateTime<FixedOffset>,
    config: &ResolvedConfig,
) -> Result<bool> {
    ensure_not_forbidden_target(file)?;
    pre_mutate_check_for_caller(system, "sandbox-add", file, &config.caller_info())?;
    ensure_markdown(file)?;
    let mut doc = parser::parse_file(system, file)?;
    let mut entries = frontmatter::read_sandbox_entries(&doc)?;
    let added = frontmatter::add_sandbox_entry_for(&mut entries, identity, now);
    if added {
        frontmatter::write_sandbox_entries(&mut doc, &entries)?;
        commit_with_verify(system, &doc, config, file, |verified_doc| {
            let markdown = verified_doc.to_markdown()?;
            system
                .write(file, markdown.as_bytes())
                .with_context(|| format!("writing {}", file.display()))
        })?;
    }
    Ok(added)
}

/// Parse a file, remove the caller's sandbox entry if present, and write
/// the result back. Returns `Ok(true)` on removal, `Ok(false)` when the
/// caller had no entry.
fn remove_one(
    system: &dyn System,
    file: &Path,
    identity: &str,
    config: &ResolvedConfig,
) -> Result<bool> {
    ensure_not_forbidden_target(file)?;
    pre_mutate_check_for_caller(system, "sandbox-remove", file, &config.caller_info())?;
    ensure_markdown(file)?;
    let mut doc = parser::parse_file(system, file)?;
    let mut entries = frontmatter::read_sandbox_entries(&doc)?;
    let removed = frontmatter::remove_sandbox_entry_for(&mut entries, identity);
    if removed {
        frontmatter::write_sandbox_entries(&mut doc, &entries)?;
        commit_with_verify(system, &doc, config, file, |verified_doc| {
            let markdown = verified_doc.to_markdown()?;
            system
                .write(file, markdown.as_bytes())
                .with_context(|| format!("writing {}", file.display()))
        })?;
    }
    Ok(removed)
}

/// Convenience used by sandbox add/remove and by the `comment --sandbox`
/// atomic path. Appends `identity@now` to the passed-in entries vector if
/// absent. Returns whether a new entry was appended.
#[must_use]
pub fn upsert_entry(
    entries: &mut Vec<SandboxEntry>,
    identity: &str,
    now: DateTime<FixedOffset>,
) -> bool {
    frontmatter::add_sandbox_entry_for(entries, identity, now)
}

fn ensure_markdown(file: &Path) -> Result<()> {
    let is_md = file
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
    if !is_md {
        bail!("not a markdown file");
    }
    Ok(())
}

/// Render a [`SandboxBulkResult`] as a human-readable text block.
///
/// Changed paths are printed one per line (relative to `cwd`).
/// Failed paths are printed with their reason on the same line.
/// The output is suitable for writing directly to stderr.
///
/// `changed_key` is unused in the text format (it is only relevant for
/// the JSON form); the changed entries are always listed without a
/// header label.
#[must_use]
pub fn render_sandbox_bulk_result(
    result: &SandboxBulkResult,
    cwd: &Path,
    _changed_key: &str,
) -> String {
    use core::fmt::Write as _;
    let mut out = String::new();
    for p in &result.changed {
        let _ = writeln!(out, "{}", strip_prefix_display(p, cwd));
    }
    for failure in &result.failed {
        let _ = writeln!(
            out,
            "{}: {}",
            strip_prefix_display(&failure.path, cwd),
            failure.reason,
        );
    }
    out
}
