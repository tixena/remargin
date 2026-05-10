//! Comment stripping and purge operations.
//!
//! Remove all Remargin comment blocks from a document, producing a clean
//! document with only body content and user-owned frontmatter.
//!
//! Supports both single-file purge (the default) and recursive directory
//! purge: `purge_dir` walks a directory, applies a per-file
//! `op_guard` check to every visible `.md` file, and returns aggregate
//! per-file outcomes so a partial-block scenario surfaces cleanly to
//! callers.

#[cfg(test)]
mod tests;

use std::path::{Component, Path, PathBuf};

use core::mem;

use anyhow::{Context as _, Result, bail};
use os_shim::{System, WalkEntry};
use serde_json::{Value as JsonValue, json};
use serde_yaml::Value;

use crate::config::ResolvedConfig;
use crate::document::allowlist;
use crate::operations::verify::commit_with_verify;
use crate::parser::{self, Segment};
use crate::permissions::op_guard::pre_mutate_check_for_caller;
use crate::writer::ensure_not_forbidden_target;

/// Result of a purge operation.
#[derive(Debug)]
#[non_exhaustive]
pub struct PurgeResult {
    /// Number of attachment files cleaned up.
    pub attachments_cleaned: usize,
    /// Number of comment blocks removed.
    pub comments_removed: usize,
}

/// Per-file outcome record produced by [`purge_dir`].
///
/// Mirrors the per-path-failure shape used by
/// [`crate::operations::sandbox::SandboxBulkResult`]. A directory purge
/// never short-circuits on a single-file failure: every visible markdown
/// file under the directory is attempted, and refusals (`op_guard`,
/// allow-list, forbidden basename) land in `failed` while the rest of
/// the walk keeps going.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct PurgeBulkResult {
    /// Files that were attempted but refused; carries the reason verbatim.
    pub failed: Vec<PurgeBulkFailure>,
    /// Files where comments were stripped.
    pub purged: Vec<PurgeBulkFile>,
    /// Files attempted that already had no comments (no-op writes).
    pub skipped: Vec<PathBuf>,
}

impl PurgeBulkResult {
    /// Sum of per-file `attachments_cleaned` across every entry in
    /// `purged`. Mirrors [`Self::comments_removed_total`].
    #[must_use]
    pub fn attachments_cleaned_total(&self) -> usize {
        self.purged.iter().map(|f| f.attachments_cleaned).sum()
    }

    /// Sum of per-file `comments_removed` across every entry in `purged`.
    /// Convenient for adapters that want a single `comments_removed`
    /// counter on the response.
    #[must_use]
    pub fn comments_removed_total(&self) -> usize {
        self.purged.iter().map(|f| f.comments_removed).sum()
    }

    /// Render the result as the canonical JSON shape used by both the
    /// CLI and MCP adapters. `base_dir` is stripped from each path so
    /// the response uses caller-friendly relative paths.
    #[must_use]
    pub fn to_json(&self, base_dir: &Path) -> JsonValue {
        let purged: Vec<JsonValue> = self
            .purged
            .iter()
            .map(|f| {
                json!({
                    "path": strip_prefix_display(&f.path, base_dir),
                    "comments_removed": f.comments_removed,
                    "attachments_cleaned": f.attachments_cleaned,
                })
            })
            .collect();
        let skipped: Vec<String> = self
            .skipped
            .iter()
            .map(|p| strip_prefix_display(p, base_dir))
            .collect();
        let failed: Vec<JsonValue> = self
            .failed
            .iter()
            .map(|f| {
                json!({
                    "path": strip_prefix_display(&f.path, base_dir),
                    "reason": f.reason,
                })
            })
            .collect();
        json!({
            "purged": purged,
            "skipped": skipped,
            "failed": failed,
            "comments_removed": self.comments_removed_total(),
            "attachments_cleaned": self.attachments_cleaned_total(),
        })
    }
}

/// One successful per-file purge in a [`PurgeBulkResult`].
#[derive(Debug)]
#[non_exhaustive]
pub struct PurgeBulkFile {
    /// Number of attachment files cleaned up for this file.
    pub attachments_cleaned: usize,
    /// Number of comment blocks removed from this file.
    pub comments_removed: usize,
    /// Absolute path of the purged file.
    pub path: PathBuf,
}

/// One per-file failure recorded during a directory purge.
#[derive(Debug)]
#[non_exhaustive]
pub struct PurgeBulkFailure {
    /// Absolute path of the file that failed to purge.
    pub path: PathBuf,
    /// Human-readable refusal reason, formatted via `{err:#}`.
    pub reason: String,
}

/// Strip `base` from `path` for display; if `path` is not anchored at
/// `base`, render the full path. Mirrors the helper in `sandbox.rs`.
fn strip_prefix_display(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Remove all Remargin comment blocks from a document.
///
/// Callers who want to preview the outcome without writing should use
/// `remargin plan purge`; the per-op `--dry-run` flag has been
/// dropped in favour of the uniform plan projection.
///
/// # Errors
///
/// Returns an error if:
/// - The file cannot be read or written
/// - The document cannot be parsed
pub fn purge(system: &dyn System, path: &Path, config: &ResolvedConfig) -> Result<PurgeResult> {
    ensure_not_forbidden_target(path)?;
    pre_mutate_check_for_caller(system, "purge", path, &config.caller_info())?;
    let mut doc = parser::parse_file(system, path)?;

    // Count comments and collect attachment paths.
    let comments = doc.comments();
    let comments_removed = comments.len();

    let attachment_paths: Vec<String> = comments
        .iter()
        .flat_map(|cm| cm.attachments.clone())
        .collect();

    // Remove all Comment and LegacyComment segments.
    doc.segments.retain(|seg| matches!(seg, Segment::Body(_)));

    // Collapse consecutive empty Body segments and normalize double blank lines.
    collapse_body_segments(&mut doc.segments);

    // Clean up remargin_* frontmatter fields.
    clean_frontmatter(&mut doc);

    // Clean up orphaned attachments.
    let doc_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut attachments_cleaned: usize = 0;
    for attachment in &attachment_paths {
        let attachment_path = doc_dir
            .join(&config.assets_dir)
            .join(Path::new(attachment).file_name().unwrap_or_default());
        if system.remove_file(&attachment_path).is_ok() {
            attachments_cleaned += 1;
        }
    }

    // Write the clean document. Purge removes every comment, so the
    // post-write verify gate has no rows to evaluate — report is
    // vacuously `ok`. Keeping the gate present still guards against
    // future refactors that might mutate comments as part of purge.
    commit_with_verify(&doc, config, path, |verified_doc| {
        let markdown = verified_doc.to_markdown();
        system
            .write(path, markdown.as_bytes())
            .with_context(|| format!("writing {}", path.display()))
    })?;

    Ok(PurgeResult {
        attachments_cleaned,
        comments_removed,
    })
}

/// Clean up remargin_* fields from frontmatter.
fn clean_frontmatter(doc: &mut parser::ParsedDocument) {
    let Some(Segment::Body(text)) = doc.segments.first() else {
        return;
    };

    let trimmed = text.trim_start();
    if !trimmed.starts_with("---") {
        return;
    }

    let lines: Vec<&str> = text.split('\n').collect();
    let opener = lines.iter().position(|line| line.trim() == "---");
    let Some(opener_idx) = opener else {
        return;
    };

    let closer = lines
        .iter()
        .enumerate()
        .skip(opener_idx + 1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(i, _)| i);

    let Some(closer_idx) = closer else {
        return;
    };

    let yaml_str: String = lines[opener_idx + 1..closer_idx].join("\n");
    let parsed: Result<Value, _> = serde_yaml::from_str(&yaml_str);
    let Ok(Value::Mapping(mut mapping)) = parsed else {
        return;
    };

    // Remove remargin_* fields.
    let keys_to_remove: Vec<Value> = mapping
        .keys()
        .filter(|key| key.as_str().is_some_and(|s| s.starts_with("remargin_")))
        .cloned()
        .collect();

    for key in &keys_to_remove {
        mapping.remove(key);
    }

    // Rebuild frontmatter.
    if mapping.is_empty() {
        // No fields left -- remove frontmatter entirely.
        let remaining = lines[closer_idx + 1..].join("\n");
        let cleaned = remaining.trim_start_matches('\n');
        doc.segments[0] = Segment::Body(String::from(cleaned));
    } else {
        let new_yaml = serde_yaml::to_string(&Value::Mapping(mapping)).unwrap_or_default();
        let before_fm = "";
        let after_fm = lines[closer_idx + 1..].join("\n");
        let new_body = format!("{before_fm}---\n{new_yaml}---\n{after_fm}");
        doc.segments[0] = Segment::Body(new_body);
    }
}

/// Collapse consecutive Body segments and remove excessive blank lines.
fn collapse_body_segments(segments: &mut Vec<Segment>) {
    // First pass: merge consecutive Body segments.
    let mut merged = Vec::new();
    let mut current_body = String::new();

    for seg in segments.drain(..) {
        match seg {
            Segment::Body(text) => current_body.push_str(&text),
            other @ (Segment::Comment(_) | Segment::LegacyComment(_)) => {
                if !current_body.is_empty() {
                    merged.push(Segment::Body(mem::take(&mut current_body)));
                }
                merged.push(other);
            }
        }
    }
    if !current_body.is_empty() {
        merged.push(Segment::Body(current_body));
    }

    // Second pass: normalize excessive blank lines in Body segments.
    for seg in &mut merged {
        if let Segment::Body(text) = seg {
            // Replace 3+ consecutive newlines with 2 (max one blank line between paragraphs).
            let mut normalized = String::new();
            let mut consecutive_newlines: usize = 0;
            for ch in text.chars() {
                if ch == '\n' {
                    consecutive_newlines += 1;
                    if consecutive_newlines <= 2 {
                        normalized.push(ch);
                    }
                } else {
                    consecutive_newlines = 0;
                    normalized.push(ch);
                }
            }
            *text = normalized;
        }
    }

    *segments = merged;
}

/// Recursively walk `dir` and purge every visible markdown file under
/// it.
///
/// Behaviour:
///
/// - The walker uses [`os_shim::System::walk_dir`] with `hidden = false`
///   so dot-folders (`.git/`, `.obsidian/`) and dotfiles are skipped
///   exactly like every other remargin op honours the dot-folder
///   default-deny.
/// - Only files passing [`allowlist::is_visible`] AND ending in `.md`
///   are considered. Other allowlist-visible files (`.txt`, source
///   code, etc.) are ignored — purge is a comment-stripping op and
///   non-markdown files have no remargin comments to remove.
/// - Each candidate file goes through the same per-file
///   [`pre_mutate_check_for_caller`] gate the single-file purge uses.
///   Refusals (`op_guard`, allow-list, forbidden basename) land in
///   [`PurgeBulkResult::failed`] without aborting the rest of the
///   walk.
/// - A markdown file that already has zero comments is recorded in
///   [`PurgeBulkResult::skipped`] (no disk write, no spurious churn
///   on a re-run).
///
/// `dir` must be an existing directory; passing a missing or non-
/// directory path returns an error so the caller can distinguish
/// "empty dir" (a successful no-op) from "dir does not exist" (a
/// hard error).
///
/// # Errors
///
/// Returns an error when:
/// - `dir` does not exist or is not a directory.
/// - The directory walk itself fails (I/O error reading the tree).
///
/// Per-file refusals (`op_guard`, parser failure, forbidden basename) do
/// NOT propagate; they are captured in [`PurgeBulkResult::failed`].
pub fn purge_dir(
    system: &dyn System,
    dir: &Path,
    config: &ResolvedConfig,
) -> Result<PurgeBulkResult> {
    if !system.exists(dir).unwrap_or(false) {
        bail!("directory does not exist: {}", dir.display());
    }
    if !system.is_dir(dir).unwrap_or(false) {
        bail!("not a directory: {}", dir.display());
    }

    let entries = system
        .walk_dir(dir, false, false)
        .with_context(|| format!("walking directory {}", dir.display()))?;

    let candidates = collect_purge_candidates(&entries, dir);

    let mut result = PurgeBulkResult::default();
    for path in candidates {
        match purge_one_for_bulk(system, &path, config) {
            Ok((removed, cleaned)) => {
                if removed == 0 {
                    result.skipped.push(path);
                } else {
                    result.purged.push(PurgeBulkFile {
                        attachments_cleaned: cleaned,
                        comments_removed: removed,
                        path,
                    });
                }
            }
            Err(err) => result.failed.push(PurgeBulkFailure {
                path,
                reason: format!("{err:#}"),
            }),
        }
    }

    Ok(result)
}

/// Filter a `walk_dir` result down to the markdown files that
/// [`purge_dir`] should attempt. Pure: no I/O, no allocation beyond
/// the result vector.
///
/// Real `walk_dir(hidden=false)` honours the `ignore` crate's
/// dot-folder + gitignore filtering, but the in-process mock does
/// not — so this function additionally rejects any entry whose path
/// (relative to the walk root) contains a dot-prefixed component.
/// Restricting to `.md` keeps non-markdown allowlist entries
/// (source code, etc.) out — purge is a comment-stripping op and
/// those files have no remargin comments to strip.
fn collect_purge_candidates(entries: &[WalkEntry], root: &Path) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in entries {
        if entry.is_dir {
            continue;
        }
        if !entry.is_file {
            continue;
        }
        if path_has_dot_component_under(&entry.path, root) {
            continue;
        }
        if !allowlist::is_visible(&entry.path, false) {
            continue;
        }
        let is_md = entry
            .path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if !is_md {
            continue;
        }
        candidates.push(entry.path.clone());
    }
    candidates.sort();
    candidates
}

/// `true` when `path` has any path component (relative to `root`)
/// whose name starts with `.`. Used to defensively reject dot-folder
/// descendants when the walker did not already filter them.
fn path_has_dot_component_under(path: &Path, root: &Path) -> bool {
    let suffix = path.strip_prefix(root).unwrap_or(path);
    suffix.components().any(|c| {
        if let Component::Normal(part) = c {
            part.to_str().is_some_and(|s| s.starts_with('.'))
        } else {
            false
        }
    })
}

/// Attempt a single-file purge for the bulk path; on success returns
/// `(comments_removed, attachments_cleaned)`. Mirrors [`purge`] but
/// returns the counters in tuple form so the caller can decide between
/// the `purged` / `skipped` buckets without re-inspecting the
/// `PurgeResult`.
fn purge_one_for_bulk(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
) -> Result<(usize, usize)> {
    let result = purge(system, path, config)?;
    Ok((result.comments_removed, result.attachments_cleaned))
}
