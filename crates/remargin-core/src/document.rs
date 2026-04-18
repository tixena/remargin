//! Document access layer: ls, get, write, rm, metadata.
//!
//! All filesystem access goes through path sandboxing, file type allowlisting,
//! and dotfile hiding.

pub mod allowlist;
pub mod mime;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use os_shim::System;
use serde::Serialize;

use tixschema::model_schema;

use crate::config::ResolvedConfig;
use crate::frontmatter;
use crate::operations::verify::commit_with_verify;
use crate::parser;

/// A single entry from a directory listing.
#[derive(Debug, Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct ListEntry {
    pub is_dir: bool,
    pub path: PathBuf,
    /// Only populated for markdown files with remargin comments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remargin_last_activity: Option<String>,
    /// Only populated for markdown files with remargin comments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remargin_pending: Option<u32>,
    /// `None` for directories.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// Result of a file removal operation.
#[derive(Debug)]
#[non_exhaustive]
pub struct RmResult {
    pub existed: bool,
    pub path: PathBuf,
}

/// Result of a `write` call.
///
/// `noop == true` means the prospective content was byte-identical to the
/// on-disk file and no disk write was performed. Retries and idempotent
/// re-submits of the same content settle here, keeping file mtime stable
/// and avoiding downstream watcher/reload noise (rem-1f2).
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub struct WriteOutcome {
    pub noop: bool,
}

/// Metadata for a single document.
///
/// Shape-switches on file type (rem-lqz):
/// - File-level fields (`binary`, `mime`, `path`, `size_bytes`) are always
///   populated so callers can peek at any allowlisted file.
/// - Markdown-only fields (`comment_count`, `frontmatter`, `last_activity`,
///   `line_count`, `pending_count`, `pending_for`) are `None` / empty for
///   binary files because the parse step is skipped.
#[derive(Debug)]
#[non_exhaustive]
pub struct DocumentMetadata {
    /// True when the file is not `text/*` (derived from `mime`).
    pub binary: bool,
    pub comment_count: Option<usize>,
    pub frontmatter: Option<serde_yaml::Value>,
    pub last_activity: Option<String>,
    pub line_count: Option<usize>,
    /// Extension-based MIME type. Unknown extensions → `application/octet-stream`.
    pub mime: &'static str,
    /// Resolved (canonical) path of the file.
    pub path: PathBuf,
    pub pending_count: Option<usize>,
    /// Unique recipients on unacked comments. Empty for binary files.
    pub pending_for: Vec<String>,
    pub size_bytes: u64,
}

/// Options for the `write` function.
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub struct WriteOptions {
    /// Base64-encoded binary data; implies `raw`.
    pub binary: bool,
    /// Parent dir must exist, file must not.
    pub create: bool,
    /// When set, replace only lines `[start..=end]` (1-indexed, inclusive)
    /// with the provided content and leave the rest of the file
    /// byte-identical. Incompatible with `create`, `raw`, and `binary`.
    /// See rem-24p.
    pub lines: Option<(usize, usize)>,
    /// Skip frontmatter/comment management.
    pub raw: bool,
}

impl WriteOptions {
    #[must_use]
    pub const fn binary(mut self, value: bool) -> Self {
        self.binary = value;
        self
    }

    #[must_use]
    pub const fn create(mut self, value: bool) -> Self {
        self.create = value;
        self
    }

    #[must_use]
    pub const fn lines(mut self, range: Option<(usize, usize)>) -> Self {
        self.lines = range;
        self
    }

    #[must_use]
    pub const fn new() -> Self {
        Self {
            binary: false,
            create: false,
            lines: None,
            raw: false,
        }
    }

    #[must_use]
    pub const fn raw(mut self, value: bool) -> Self {
        self.raw = value;
        self
    }
}

/// Projection result for a planned `write` op (rem-imc).
///
/// Returned by [`project_write`] — the projection-only sibling of
/// [`write`] — so the `plan write` subcommand can report what the op
/// would do without touching disk.
#[derive(Debug)]
#[non_exhaustive]
pub enum WriteProjection {
    /// Normal markdown projection: `before` is the parsed on-disk
    /// document (empty-segmented for `--create`), `after` is the parsed
    /// prospective content with frontmatter already normalized through
    /// [`frontmatter::ensure_frontmatter`]. `noop` mirrors the byte-
    /// identical shortcut [`write`] uses pre-commit.
    Markdown {
        after: parser::ParsedDocument,
        before: parser::ParsedDocument,
        noop: bool,
    },
    /// Projection not representable as a markdown document diff
    /// (`--raw` or `--binary` mode). The caller should emit a degraded
    /// plan report whose comment diff is empty and whose `reject_reason`
    /// explains the limitation.
    Unsupported {
        /// Human-readable reason (`"raw mode"`, `"binary mode"`) suitable
        /// for the `PlanReport::reject_reason` field.
        reason: String,
    },
}

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

        let has_md_extension = Path::new(filename)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        let (remargin_pending, remargin_last_activity) = if !is_dir && has_md_extension {
            get_remargin_metadata(system, entry_path)
        } else {
            (None, None)
        };

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

/// Idempotent: deleting a file that does not exist returns success with
/// `existed: false`.
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
    opts: WriteOptions,
) -> Result<WriteOutcome> {
    validate_write_opts(path, &opts)?;

    let resolved = if opts.create {
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

    if opts.binary {
        let bytes = BASE64_STANDARD
            .decode(content)
            .context("invalid base64 content")?;
        if is_byte_identical(system, &resolved, &bytes) {
            return Ok(WriteOutcome { noop: true });
        }
        system
            .write(&resolved, &bytes)
            .with_context(|| format!("writing {}", resolved.display()))?;
        return Ok(WriteOutcome { noop: false });
    }

    if opts.raw {
        if is_byte_identical(system, &resolved, content.as_bytes()) {
            return Ok(WriteOutcome { noop: true });
        }
        system
            .write(&resolved, content.as_bytes())
            .with_context(|| format!("writing {}", resolved.display()))?;
        return Ok(WriteOutcome { noop: false });
    }

    // Partial write: splice the replacement content into `[start..=end]`,
    // then fall through to the same parse + comment-preservation +
    // verify-gate pipeline whole-file writes use. Everything after this
    // block treats `content_to_parse` as if the caller had supplied it
    // as a full-document payload — so the preservation check still
    // catches any comment block that was clipped or destroyed by the
    // caller's range, and the verify gate still runs pre-commit.
    let content_to_parse: String = if let Some((start, end)) = opts.lines {
        let existing = system
            .read_to_string(&resolved)
            .with_context(|| format!("reading {} for partial write", resolved.display()))?;
        splice_lines(&existing, start, end, content)
    } else {
        String::from(content)
    };

    let new_doc = parser::parse(&content_to_parse).context("parsing incoming content")?;

    // Comment preservation: only check when overwriting an existing file.
    if !opts.create
        && let Ok(old_content) = system.read_to_string(&resolved)
    {
        let old_doc = parser::parse(&old_content).context("parsing existing document")?;
        check_comment_preservation(&old_doc, &new_doc)?;
    }

    let mut final_doc = new_doc;
    frontmatter::ensure_frontmatter(&mut final_doc, config)?;

    // No-op detection (rem-1f2): if the canonical output would be
    // byte-identical to what is already on disk, skip both the disk
    // write AND the post-write verify gate. The verify gate is
    // semantically satisfied: the file is already in the verified state
    // this payload would produce. `create` never no-ops (the path is
    // guaranteed not to exist, ruled out above).
    let final_content = final_doc.to_markdown();
    if !opts.create && is_byte_identical(system, &resolved, final_content.as_bytes()) {
        return Ok(WriteOutcome { noop: true });
    }

    commit_with_verify(&final_doc, config, |verified_doc| {
        let serialized = verified_doc.to_markdown();
        system
            .write(&resolved, serialized.as_bytes())
            .with_context(|| format!("writing {}", resolved.display()))
    })?;

    Ok(WriteOutcome { noop: false })
}

/// Pure projection of a `write` op: runs the same pipeline [`write`]
/// does up through `ensure_frontmatter`, but stops before invoking
/// [`commit_with_verify`] and never calls `system.write`.
///
/// Used by the `remargin plan write` subcommand (rem-imc) to feed a
/// before/after pair into
/// [`crate::operations::plan::project_report`]. The returned
/// [`WriteProjection::Markdown::before`] is the on-disk document (empty
/// for `--create`), and `after` is the prospective document with
/// frontmatter normalized.
///
/// # Errors
///
/// Surfaces the same diagnostics [`write`] would on its pre-commit
/// path: invalid option combinations, sandbox escapes, allowlist
/// rejections, parse failures, and comment-preservation violations.
pub fn project_write(
    system: &dyn System,
    base_dir: &Path,
    path: &Path,
    content: &str,
    config: &ResolvedConfig,
    opts: WriteOptions,
) -> Result<WriteProjection> {
    validate_write_opts(path, &opts)?;

    let resolved = if opts.create {
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

    if opts.binary {
        return Ok(WriteProjection::Unsupported {
            reason: String::from("binary mode is not representable as a markdown plan"),
        });
    }
    if opts.raw {
        return Ok(WriteProjection::Unsupported {
            reason: String::from("raw mode is not representable as a markdown plan"),
        });
    }

    let content_to_parse: String = if let Some((start, end)) = opts.lines {
        let existing = system
            .read_to_string(&resolved)
            .with_context(|| format!("reading {} for partial write", resolved.display()))?;
        splice_lines(&existing, start, end, content)
    } else {
        String::from(content)
    };

    let new_doc = parser::parse(&content_to_parse).context("parsing incoming content")?;

    let before = if opts.create {
        parser::parse("").context("parsing empty before-document for create")?
    } else {
        match system.read_to_string(&resolved) {
            Ok(existing) => {
                let old_doc = parser::parse(&existing).context("parsing existing document")?;
                check_comment_preservation(&old_doc, &new_doc)?;
                old_doc
            }
            Err(_) => parser::parse("").context("parsing empty before-document")?,
        }
    };

    let mut after = new_doc;
    frontmatter::ensure_frontmatter(&mut after, config)?;

    let after_bytes = after.to_markdown();
    let noop = !opts.create && is_byte_identical(system, &resolved, after_bytes.as_bytes());

    Ok(WriteProjection::Markdown {
        after,
        before,
        noop,
    })
}

/// Return true when the on-disk bytes at `path` exactly match `new_bytes`.
///
/// A missing file or a read error returns false, so the caller falls
/// through to a real write (safer default — the write will surface any
/// underlying I/O error with a proper diagnostic). Uses `read_to_string`
/// because that is the only read primitive `System` exposes; binary
/// files that aren't valid UTF-8 won't trip the no-op fast path, but the
/// correctness guarantee (never skip a real change) still holds.
fn is_byte_identical(system: &dyn System, path: &Path, new_bytes: &[u8]) -> bool {
    system
        .read_to_string(path)
        .is_ok_and(|existing| existing.as_bytes() == new_bytes)
}

/// Validate mutually-exclusive `WriteOptions` combinations up front so
/// both CLI and MCP callers surface identical diagnostics (rem-24p).
fn validate_write_opts(path: &Path, opts: &WriteOptions) -> Result<()> {
    let is_md = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));

    if opts.binary && is_md {
        bail!("binary mode is not supported for markdown files");
    }
    if opts.raw && is_md {
        bail!("raw mode is not supported for markdown files");
    }

    if let Some((start, end)) = opts.lines {
        if opts.create {
            bail!("--lines is incompatible with --create");
        }
        if opts.raw {
            bail!("--lines is incompatible with --raw");
        }
        if opts.binary {
            bail!("--lines is incompatible with --binary");
        }
        if start == 0 || start > end {
            bail!("--lines range is invalid: start={start}, end={end} (require 1 <= start <= end)");
        }
    }

    Ok(())
}

/// Enforce the comment-preservation invariant: every comment in
/// `old_doc` must still be present (by id and byte-for-byte checksum)
/// in `new_doc`, and no unexpected ids may have appeared. Factored out
/// of `write` so the partial-write and whole-file paths share the same
/// diagnostics.
fn check_comment_preservation(
    old_doc: &parser::ParsedDocument,
    new_doc: &parser::ParsedDocument,
) -> Result<()> {
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

    Ok(())
}

/// Splice `replacement` into `existing` at 1-indexed inclusive range
/// `[start..=end]`, replacing those lines and leaving every other line
/// byte-identical. If `end` exceeds the line count of `existing`, the
/// range is clamped to the actual line count (matching how `get`
/// treats out-of-bounds ranges — the caller gets what's reasonable
/// rather than an error).
///
/// One trailing `\n` is stripped from `replacement` before splicing so
/// that `--lines 3-3 "new line"` and `--lines 3-3 "new line\n"` behave
/// identically — otherwise the trailing newline would introduce a
/// spurious empty line at the splice boundary.
pub(crate) fn splice_lines(existing: &str, start: usize, end: usize, replacement: &str) -> String {
    let existing_lines: Vec<&str> = existing.split('\n').collect();
    let line_count = existing_lines.len();

    // Clamp to bounds. 1-indexed, so `start..=end` maps to 0-indexed
    // `start-1..=end-1` in the `Vec`. When `end` overshoots the file,
    // splice to the end of the buffer; when `start` also overshoots,
    // the prefix is the whole file and the splice is an append.
    let start_idx = start.saturating_sub(1).min(line_count);
    let end_idx = end.min(line_count);

    let trimmed = replacement.strip_suffix('\n').unwrap_or(replacement);
    let replacement_lines: Vec<&str> = trimmed.split('\n').collect();

    let mut out: Vec<&str> = Vec::with_capacity(line_count + replacement_lines.len());
    out.extend_from_slice(&existing_lines[..start_idx]);
    out.extend_from_slice(&replacement_lines);
    if end_idx < line_count {
        out.extend_from_slice(&existing_lines[end_idx..]);
    }

    out.join("\n")
}

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

    let mime_type = mime::mime_for_extension(&resolved);
    let binary = mime::is_binary_mime(mime_type);

    let size_bytes = system
        .metadata(&resolved)
        .with_context(|| format!("stat {}", resolved.display()))?
        .len;

    // Binary files: return file-level metadata only; skip the parse step.
    if binary {
        return Ok(DocumentMetadata {
            binary,
            comment_count: None,
            frontmatter: None,
            last_activity: None,
            line_count: None,
            mime: mime_type,
            path: resolved,
            pending_count: None,
            pending_for: Vec::new(),
            size_bytes,
        });
    }

    // Text files: read + parse for markdown-shaped fields.
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

    let fm = extract_frontmatter(&content);

    Ok(DocumentMetadata {
        binary,
        comment_count: Some(comment_count),
        frontmatter: fm,
        last_activity,
        line_count: Some(line_count),
        mime: mime_type,
        path: resolved,
        pending_count: Some(pending_count),
        pending_for,
        size_bytes,
    })
}

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
