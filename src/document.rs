//! Document access layer: ls, get, write, rm, metadata.
//!
//! Agents never touch files directly. They use these functions to interact
//! with the filesystem through path sandboxing, file type allowlisting,
//! and dotfile hiding.

pub mod allowlist;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;

use crate::config::ResolvedConfig;
use crate::frontmatter;
use crate::parser;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A single entry from a directory listing.
#[derive(Debug)]
#[non_exhaustive]
pub struct ListEntry {
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// Relative path from the base directory.
    pub path: PathBuf,
    /// Only populated for markdown files with remargin comments.
    pub remargin_last_activity: Option<String>,
    /// Only populated for markdown files with remargin comments.
    pub remargin_pending: Option<u32>,
    /// File size in bytes (None for directories).
    pub size: Option<u64>,
}

/// Result of a file removal operation.
#[derive(Debug)]
#[non_exhaustive]
pub struct RmResult {
    /// Whether the file existed before removal.
    pub existed: bool,
    /// The path that was (or would have been) deleted.
    pub path: PathBuf,
}

/// Metadata for a single document.
#[derive(Debug)]
#[non_exhaustive]
pub struct DocumentMetadata {
    /// Number of remargin comments in the document.
    pub comment_count: usize,
    /// Parsed frontmatter (if present).
    pub frontmatter: Option<serde_yaml::Value>,
    /// Most recent activity timestamp.
    pub last_activity: Option<String>,
    /// Total line count.
    pub line_count: usize,
    /// Number of comments with no ack entries.
    pub pending_count: usize,
    /// Unique recipients on unacked comments.
    pub pending_for: Vec<String>,
}

// ---------------------------------------------------------------------------
// ls
// ---------------------------------------------------------------------------

/// List files and directories at the given path.
///
/// Filters by allowlist, hides dotfiles and dot-directories.
/// Respects ignore patterns from config.
/// For markdown files, includes remargin metadata (pending count, last activity).
///
/// # Errors
///
/// Returns an error if:
/// - The path escapes the sandbox
/// - The directory cannot be read
pub fn ls(
    system: &dyn System,
    base_dir: &Path,
    path: &Path,
    config: &ResolvedConfig,
) -> Result<Vec<ListEntry>> {
    let resolved = allowlist::resolve_sandboxed(system, base_dir, path, config.unrestricted)?;

    let entries = system
        .read_dir(&resolved)
        .with_context(|| format!("reading directory {}", resolved.display()))?;

    let ignore_set: HashSet<&str> = config.ignore.iter().map(String::as_str).collect();

    let mut result = Vec::new();
    for entry_path in &entries {
        let Some(filename) = entry_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        // Check ignore patterns.
        if ignore_set.contains(filename) {
            continue;
        }

        let is_dir = system.is_dir(entry_path).unwrap_or(false);

        if !allowlist::is_visible(entry_path, is_dir) {
            continue;
        }

        let size = if is_dir {
            None
        } else {
            system.metadata(entry_path).ok().map(|m| m.len)
        };

        // For markdown files, try to get remargin metadata.
        let has_md_extension = Path::new(filename)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        let (remargin_pending, remargin_last_activity) = if !is_dir && has_md_extension {
            get_remargin_metadata(system, entry_path)
        } else {
            (None, None)
        };

        // Make path relative to resolved dir.
        let relative = entry_path
            .strip_prefix(&resolved)
            .unwrap_or(entry_path)
            .to_path_buf();

        result.push(ListEntry {
            is_dir,
            path: relative,
            remargin_last_activity,
            remargin_pending,
            size,
        });
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// get
// ---------------------------------------------------------------------------

/// Read a file's contents.
///
/// Returns an error for dotfiles, disallowed extensions, and paths outside
/// the sandbox.
///
/// # Errors
///
/// Returns an error if:
/// - The path escapes the sandbox
/// - The file is a dotfile or has a disallowed extension
/// - `lines` is specified for a binary file
/// - The file cannot be read
pub fn get(
    system: &dyn System,
    base_dir: &Path,
    path: &Path,
    lines: Option<(usize, usize)>,
    line_numbers: bool,
    unrestricted: bool,
) -> Result<String> {
    let resolved = allowlist::resolve_sandboxed(system, base_dir, path, unrestricted)?;

    if !allowlist::is_visible(&resolved, false) {
        bail!("file not visible: {}", path.display());
    }

    if lines.is_some() && !allowlist::is_text(&resolved) {
        bail!("--lines is not supported for binary files");
    }

    if line_numbers && !allowlist::is_text(&resolved) {
        bail!("--line-numbers is not supported for binary files");
    }

    let content = system
        .read_to_string(&resolved)
        .with_context(|| format!("reading {}", resolved.display()))?;

    match lines {
        Some((start, end)) => {
            let selected: Vec<&str> = content
                .split('\n')
                .enumerate()
                .filter(|(i, _)| *i + 1 >= start && *i < end)
                .map(|(_, line)| line)
                .collect();
            if line_numbers {
                Ok(format_with_line_numbers(&selected, start))
            } else {
                Ok(selected.join("\n"))
            }
        }
        None => {
            if line_numbers {
                let all_lines: Vec<&str> = content.split('\n').collect();
                Ok(format_with_line_numbers(&all_lines, 1))
            } else {
                Ok(content)
            }
        }
    }
}

/// Format lines with right-aligned line numbers and a pipe separator.
///
/// `start_num` is the 1-indexed line number of the first line in the slice.
fn format_with_line_numbers(lines: &[&str], start_num: usize) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let end_num = start_num + lines.len() - 1;
    let width = end_num.to_string().len();
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| format!("{:>width$}\u{2502} {line}", start_num + i))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// rm
// ---------------------------------------------------------------------------

/// Remove a file from the managed document tree.
///
/// The operation is **idempotent**: deleting a file that does not exist
/// returns success with `existed: false`.
///
/// # Errors
///
/// Returns an error if:
/// - The path escapes the sandbox
/// - The file is a dotfile or otherwise not visible
/// - The path is a directory (only files are supported)
pub fn rm(
    system: &dyn System,
    base_dir: &Path,
    path: &Path,
    config: &ResolvedConfig,
) -> Result<RmResult> {
    let resolved = allowlist::resolve_sandboxed(system, base_dir, path, config.unrestricted)?;

    if system.is_dir(&resolved).unwrap_or(false) {
        bail!("cannot remove directory: {}", path.display());
    }

    if !allowlist::is_visible(&resolved, false) {
        bail!("file not visible: {}", path.display());
    }

    let existed = system.read_to_string(&resolved).is_ok();

    if existed {
        system
            .remove_file(&resolved)
            .with_context(|| format!("removing {}", resolved.display()))?;
    }

    Ok(RmResult {
        existed,
        path: path.to_path_buf(),
    })
}

// ---------------------------------------------------------------------------
// write
// ---------------------------------------------------------------------------

/// Write document contents with comment preservation enforcement.
///
/// The payload must include all existing comment blocks with their original
/// IDs, checksums, and signatures intact.
///
/// When `create` is true, the file is expected to be new: the parent directory
/// must exist, but the file itself must not. Comment preservation checks are
/// skipped since there is no existing file to compare against.
///
/// # Errors
///
/// Returns an error if:
/// - The path escapes the sandbox
/// - Comments were added, removed, or modified (when `create` is false)
/// - `create` is true but the file already exists
/// - `create` is true but the parent directory does not exist
/// - The file cannot be written
pub fn write(
    system: &dyn System,
    base_dir: &Path,
    path: &Path,
    content: &str,
    config: &ResolvedConfig,
    create: bool,
    raw: bool,
) -> Result<()> {
    // Raw mode is not supported for markdown files.
    if raw {
        let is_md = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if is_md {
            bail!("raw mode is not supported for markdown files");
        }
    }

    let resolved = if create {
        let target =
            allowlist::resolve_sandboxed_create(system, base_dir, path, config.unrestricted)?;
        if system.read_to_string(&target).is_ok() {
            bail!(
                "file already exists (use write without --create): {}",
                path.display()
            );
        }
        target
    } else {
        allowlist::resolve_sandboxed(system, base_dir, path, config.unrestricted)?
    };

    if !allowlist::is_visible(&resolved, false) {
        bail!("file not visible: {}", path.display());
    }

    // Raw mode: write content exactly as provided, no frontmatter or comments.
    if raw {
        system
            .write(&resolved, content.as_bytes())
            .with_context(|| format!("writing {}", resolved.display()))?;
        return Ok(());
    }

    // Parse the incoming content.
    let new_doc = parser::parse(content).context("parsing incoming content")?;

    // Comment preservation: only check when overwriting an existing file.
    if !create {
        let existing_content = system.read_to_string(&resolved);
        if let Ok(old_content) = existing_content {
            let old_doc = parser::parse(&old_content).context("parsing existing document")?;

            // Check comment preservation: all original comment IDs must be present.
            let old_ids: HashSet<&str> = old_doc.comment_ids();
            let new_ids: HashSet<&str> = new_doc.comment_ids();

            for old_id in &old_ids {
                if !new_ids.contains(old_id) {
                    bail!("comment {old_id:?} was removed — preservation check failed");
                }
            }

            for new_id in &new_ids {
                if !old_ids.contains(new_id) {
                    bail!("unexpected comment {new_id:?} appeared — preservation check failed");
                }
            }

            // Verify checksums match for existing comments.
            for old_comment in old_doc.comments() {
                if let Some(new_comment) = new_doc.find_comment(&old_comment.id)
                    && new_comment.checksum != old_comment.checksum
                {
                    bail!(
                        "comment {:?} checksum was modified — preservation check failed",
                        old_comment.id
                    );
                }
            }
        }
    }

    // Update frontmatter if needed.
    let mut final_doc = new_doc;
    frontmatter::ensure_frontmatter(&mut final_doc, config)?;

    let final_content = final_doc.to_markdown();
    system
        .write(&resolved, final_content.as_bytes())
        .with_context(|| format!("writing {}", resolved.display()))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// metadata
// ---------------------------------------------------------------------------

/// Get metadata for a document.
///
/// # Errors
///
/// Returns an error if:
/// - The path escapes the sandbox
/// - The file cannot be read or parsed
pub fn metadata(
    system: &dyn System,
    base_dir: &Path,
    path: &Path,
    unrestricted: bool,
) -> Result<DocumentMetadata> {
    let resolved = allowlist::resolve_sandboxed(system, base_dir, path, unrestricted)?;

    if !allowlist::is_visible(&resolved, false) {
        bail!("file not visible: {}", path.display());
    }

    let content = system
        .read_to_string(&resolved)
        .with_context(|| format!("reading {}", resolved.display()))?;

    let line_count = content.split('\n').count();
    let doc = parser::parse(&content).context("parsing document")?;
    let comments = doc.comments();

    let comment_count = comments.len();
    let pending_count = comments.iter().filter(|cm| cm.ack.is_empty()).count();

    let mut pending_for: Vec<String> = Vec::new();
    for cm in &comments {
        if cm.ack.is_empty() {
            for recipient in &cm.to {
                if !pending_for.contains(recipient) {
                    pending_for.push(recipient.clone());
                }
            }
        }
    }
    pending_for.sort();

    let last_activity = comments
        .iter()
        .map(|cm| cm.ts)
        .max()
        .map(|ts| ts.to_rfc3339());

    // Parse frontmatter.
    let fm = extract_frontmatter(&content);

    Ok(DocumentMetadata {
        comment_count,
        frontmatter: fm,
        last_activity,
        line_count,
        pending_count,
        pending_for,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract YAML frontmatter from content (returns None if not present).
fn extract_frontmatter(content: &str) -> Option<serde_yaml::Value> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }

    let lines: Vec<&str> = content.split('\n').collect();
    let opener = lines.iter().position(|line| line.trim() == "---")?;
    let closer = lines
        .iter()
        .enumerate()
        .skip(opener + 1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(i, _)| i)?;

    let yaml_str: String = lines[opener + 1..closer].join("\n");
    serde_yaml::from_str(&yaml_str).ok()
}

/// Try to get remargin metadata for a markdown file.
fn get_remargin_metadata(system: &dyn System, path: &Path) -> (Option<u32>, Option<String>) {
    let Ok(content) = system.read_to_string(path) else {
        return (None, None);
    };

    let Ok(doc) = parser::parse(&content) else {
        return (None, None);
    };

    let comments = doc.comments();
    if comments.is_empty() {
        return (None, None);
    }

    let pending = comments.iter().filter(|cm| cm.ack.is_empty()).count();
    let last_activity = comments
        .iter()
        .map(|cm| cm.ts)
        .max()
        .map(|ts| ts.to_rfc3339());

    (u32::try_from(pending).ok(), last_activity)
}
