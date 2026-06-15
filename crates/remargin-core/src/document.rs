//! Document access layer: ls, get, write, rm, metadata.
//!
//! All filesystem access goes through path sandboxing, file type allowlisting,
//! and dotfile hiding.

pub mod allowlist;
pub mod get_image;
pub mod mime;

#[cfg(test)]
mod tests;

use core::cmp::Reverse;
use std::collections::HashSet;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use os_shim::System;
use serde::Serialize;
use serde_json::{Value, json};

use tixschema::model_schema;

use crate::config::ResolvedConfig;
use crate::frontmatter;
use crate::operations::verify::commit_with_verify;
use crate::parser;
use crate::permissions::op_guard::pre_mutate_check_for_caller;
use crate::writer::ensure_not_forbidden_target;

/// Bytes + mime + size for a binary file read.
///
/// Returned by [`read_binary`]. Callers decide how to surface the
/// payload: base64 in JSON, raw bytes to stdout, or written to a caller-named
/// file. The helper deliberately does not special-case image/* vs other
/// binary mimes — that is an adapter-level concern (e.g. MCP returning an
/// image content block).
#[derive(Debug)]
#[non_exhaustive]
pub struct BinaryPayload {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    /// Resolved (canonical) path of the file that was read.
    pub path: PathBuf,
    pub size_bytes: u64,
}

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

/// Result of a single-file removal operation.
#[derive(Debug)]
#[non_exhaustive]
pub struct RmResult {
    pub existed: bool,
    pub path: PathBuf,
}

impl RmResult {
    /// `deleted` echoes the caller-supplied path verbatim so the
    /// response round-trips through the same surface.
    #[must_use]
    pub fn to_json(&self, requested_path: &str) -> Value {
        json!({
            "deleted": requested_path,
            "existed": self.existed,
        })
    }
}

/// Report of a recursive directory removal.
///
/// Produced by the directory branch of [`rm`]. Every listed resource
/// passed the readability + per-file gate pre-flight (a failure aborts
/// before any deletion, so a populated report is always a fully-applied
/// delete). Paths are the resolved (canonical) on-disk paths the walk
/// observed.
///
/// `folders_left_behind` records directories remargin could not remove
/// without force because they still held entries remargin cannot list
/// (hidden / non-visible files, or a nested `.remargin.yaml`). This is
/// not an error — remargin deleted everything it could see and left the
/// rest intact.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct RmDirReport {
    /// Visible files removed, deepest-first.
    pub files_deleted: Vec<PathBuf>,
    /// Directories left in place because a no-force remove failed: they
    /// still hold entries remargin cannot list.
    pub folders_left_behind: Vec<PathBuf>,
    /// Directories removed, deepest-first.
    pub folders_removed: Vec<PathBuf>,
}

impl RmDirReport {
    fn render_text(&self, requested_path: &str) -> String {
        use core::fmt::Write as _;

        let mut out = format!(
            "deleted directory: {requested_path} ({} file(s), {} folder(s) removed)",
            self.files_deleted.len(),
            self.folders_removed.len()
        );
        if !self.folders_left_behind.is_empty() {
            let _ = write!(
                out,
                "\n{} folder(s) left behind (not empty / unlistable contents):",
                self.folders_left_behind.len()
            );
            for path in &self.folders_left_behind {
                let _ = write!(out, "\n  {}", path.display());
            }
        }
        out
    }

    fn to_json(&self, requested_path: &str) -> Value {
        json!({
            "deleted": requested_path,
            "is_directory": true,
            "files_deleted": display_paths(&self.files_deleted),
            "folders_left_behind": display_paths(&self.folders_left_behind),
            "folders_removed": display_paths(&self.folders_removed),
        })
    }
}

/// Outcome of an [`rm`] call.
///
/// `rm` deletes a single file or, when pointed at a directory, removes
/// the directory tree recursively. The two cases surface different
/// reports; this enum lets callers render each without the directory
/// report leaking into the long-standing single-file JSON shape.
#[derive(Debug)]
#[non_exhaustive]
pub enum RmOutcome {
    /// A directory was removed recursively.
    Directory(RmDirReport),
    /// A single file was removed (or was already absent).
    File(RmResult),
}

impl RmOutcome {
    /// Human-readable one-or-more-line summary for non-JSON output.
    #[must_use]
    pub fn render_text(&self, requested_path: &str) -> String {
        match self {
            Self::Directory(report) => report.render_text(requested_path),
            Self::File(result) => {
                if result.existed {
                    format!("deleted: {requested_path}")
                } else {
                    format!("already absent: {requested_path}")
                }
            }
        }
    }

    /// `requested_path` echoes the caller-supplied path verbatim so the
    /// response round-trips through the same surface. The `File` variant
    /// emits the long-standing `{deleted, existed}` shape unchanged.
    #[must_use]
    pub fn to_json(&self, requested_path: &str) -> Value {
        match self {
            Self::Directory(report) => report.to_json(requested_path),
            Self::File(result) => result.to_json(requested_path),
        }
    }
}

/// Result of a `write` call.
///
/// `noop == true` means the prospective content was byte-identical to the
/// on-disk file and no disk write was performed. Retries and idempotent
/// re-submits of the same content settle here, keeping file mtime stable
/// and avoiding downstream watcher/reload noise.
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub struct WriteOutcome {
    pub noop: bool,
}

impl WriteOutcome {
    /// `raw` is forced true for binary writes (they skip the frontmatter
    /// / comment-preservation pass), so callers don't need to OR it in.
    #[must_use]
    pub fn to_json(self, written: &str, binary: bool, raw: bool) -> Value {
        json!({
            "written": written,
            "binary": binary,
            "raw": raw || binary,
            "noop": self.noop,
        })
    }
}

/// Metadata for a single document.
///
/// Shape-switches on file type:
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

impl DocumentMetadata {
    /// CLI passes `include_frontmatter=false` (round-tripping via
    /// `get`/`write`); MCP passes `true` so agents get one round trip.
    #[must_use]
    pub fn to_json(&self, include_frontmatter: bool) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("binary".into(), json!(self.binary));
        map.insert("mime".into(), json!(self.mime));
        map.insert("path".into(), json!(self.path));
        map.insert("size_bytes".into(), json!(self.size_bytes));
        if let Some(count) = self.comment_count {
            map.insert("comment_count".into(), json!(count));
        }
        if let Some(count) = self.line_count {
            map.insert("line_count".into(), json!(count));
        }
        if let Some(count) = self.pending_count {
            map.insert("pending_count".into(), json!(count));
        }
        if !self.pending_for.is_empty() {
            map.insert("pending_for".into(), json!(self.pending_for));
        }
        if let Some(last) = &self.last_activity {
            map.insert("last_activity".into(), json!(last));
        }
        if include_frontmatter && let Some(fm) = &self.frontmatter {
            map.insert("frontmatter".into(), json!(fm));
        }
        Value::Object(map)
    }
}

/// Options for the `write` function.
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub struct WriteOptions {
    /// Base64-encoded binary data; implies `raw`.
    pub binary: bool,
    /// Missing parent dirs are created; the file itself must not already exist.
    pub create: bool,
    /// When set, replace only lines `[start..=end]` (1-indexed, inclusive)
    /// with the provided content and leave the rest of the file
    /// byte-identical. Incompatible with `create`, `raw`, and `binary`.
    ///
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

/// Projection result for a planned `write` op.
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
    let resolved = allowlist::resolve_sandboxed(
        system,
        base_dir,
        path,
        config.unrestricted,
        &config.trusted_roots,
    )?;

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

        let has_md_extension = is_markdown_extension(Path::new(filename));
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
    trusted_roots: &[PathBuf],
) -> Result<String> {
    let resolved =
        allowlist::resolve_sandboxed(system, base_dir, path, unrestricted, trusted_roots)?;

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

/// Read a file as raw bytes, enforcing the same sandbox + visibility rules as
/// [`get`]. Rejects markdown files so the comment-preservation pipeline is
/// never bypassed through the binary path.
///
/// Mime is derived from the file extension; unknown extensions map to
/// `application/octet-stream`. This is symmetric with [`write`]'s `binary`
/// option and is the shared core for CLI `get --binary` and MCP `get` with
/// `binary: true`.
///
/// # Errors
///
/// Returns an error if:
/// - The path escapes the sandbox.
/// - The file is a dotfile or has a disallowed extension.
/// - The file is markdown (`.md`) — use the text `get` path instead.
/// - The file cannot be opened or read.
pub fn read_binary(
    system: &dyn System,
    base_dir: &Path,
    path: &Path,
    unrestricted: bool,
    trusted_roots: &[PathBuf],
) -> Result<BinaryPayload> {
    let resolved =
        allowlist::resolve_sandboxed(system, base_dir, path, unrestricted, trusted_roots)?;

    if !allowlist::is_visible(&resolved, false) {
        bail!("file not visible: {}", path.display());
    }

    // Never bypass comment-preservation through the binary surface. Symmetric
    // with `write`'s markdown-rejects-binary behaviour.
    if is_markdown_extension(&resolved) {
        bail!(
            "cannot fetch markdown file as binary: {} (use `get` without --binary)",
            path.display()
        );
    }

    let size_bytes = system
        .metadata(&resolved)
        .with_context(|| format!("stat {}", resolved.display()))?
        .len;

    let mut reader = system
        .open(&resolved)
        .with_context(|| format!("opening {}", resolved.display()))?;
    let mut bytes = Vec::with_capacity(usize::try_from(size_bytes).unwrap_or(0));
    reader
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", resolved.display()))?;

    let mime = mime::mime_for_extension(&resolved);

    Ok(BinaryPayload {
        bytes,
        mime,
        path: resolved,
        size_bytes,
    })
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

/// Remove a file, or — when pointed at a directory — remove the
/// directory tree recursively.
///
/// Directory removal is always recursive (there is no `--recursive`
/// flag) and is driven by what remargin can *see*: it walks the tree,
/// keeps only the resources `ls` would list (visible extensions,
/// dotfiles hidden), pre-flights readability + the per-file gate over
/// every one of them, then deletes bottom-up. The pre-flight is
/// all-or-nothing — if any listed resource fails, nothing is deleted and
/// the error names the blocking path. Each directory is removed with a
/// no-force rmdir: a directory that still holds entries remargin could
/// not list (hidden files, a nested `.remargin.yaml`) is left in place
/// and recorded in the report, with no error. A nested realm's
/// `.remargin.yaml` is a dotfile, so its folder always looks non-empty
/// to the no-force remove and survives.
///
/// Idempotent: deleting a file that does not exist returns the `File`
/// variant with `existed: false`.
///
/// # Errors
///
/// Returns an error if:
/// - The path escapes the sandbox
/// - The path is a forbidden target (e.g. `.remargin.yaml`)
/// - The path is a single file that is a dotfile or otherwise not visible
/// - (Directory case) any listed resource fails the readability /
///   per-file gate pre-flight
pub fn rm(
    system: &dyn System,
    base_dir: &Path,
    path: &Path,
    config: &ResolvedConfig,
) -> Result<RmOutcome> {
    ensure_not_forbidden_target(path)?;
    let resolved = allowlist::resolve_sandboxed(
        system,
        base_dir,
        path,
        config.unrestricted,
        &config.trusted_roots,
    )?;
    ensure_not_forbidden_target(&resolved)?;

    if system.is_dir(&resolved).unwrap_or(false) {
        return rm_directory(system, &resolved).map(RmOutcome::Directory);
    }

    if !allowlist::is_visible(&resolved, false) {
        bail!("file not visible: {}", path.display());
    }

    // Existence is a filesystem fact — NOT "are the bytes valid UTF-8".
    // `read_to_string` fails on binary files (e.g. PNGs), which made `rm`
    // report `existed: false` and skip the unlink for any non-text file.
    let existed = system.exists(&resolved).unwrap_or(false);

    if existed {
        system
            .remove_file(&resolved)
            .with_context(|| format!("removing {}", resolved.display()))?;
    }

    Ok(RmOutcome::File(RmResult {
        existed,
        path: path.to_path_buf(),
    }))
}

/// Recursive directory removal. `resolved` is the canonical directory
/// path (already past the sandbox + forbidden-target guards).
///
/// 1. Walk the tree and keep only the resources `ls` would list
///    (`allowlist::is_visible`) — these are the resources remargin can
///    see. The directories under `resolved` are kept regardless (we
///    remove them bottom-up); the root itself is removed last.
/// 2. Pre-flight every visible file: forbidden-target guard + readability
///    (`system.metadata`). A failure aborts before any deletion.
/// 3. Delete visible files deepest-first, then directories deepest-first
///    via a no-force rmdir (a directory that still holds entries
///    remargin could not list is left in place and recorded).
fn rm_directory(system: &dyn System, resolved: &Path) -> Result<RmDirReport> {
    let entries = system
        .walk_dir(resolved, false, true)
        .with_context(|| format!("walking {}", resolved.display()))?;

    // The resources remargin can see: visible files, plus every
    // subdirectory (directories are always visible for navigation, but
    // dot-directories are not — and ls would not descend into them).
    let mut visible_files: Vec<PathBuf> = Vec::new();
    let mut directories: Vec<PathBuf> = Vec::new();
    for entry in &entries {
        if !allowlist::is_visible(&entry.path, entry.is_dir) {
            continue;
        }
        if entry.is_dir {
            directories.push(entry.path.clone());
        } else {
            visible_files.push(entry.path.clone());
        }
    }

    // Pre-flight (all-or-nothing): every visible file must pass the
    // per-file gate (config-file guard) and be readable before we delete
    // anything. Readability is an actual open — a `stat`-only check would
    // pass for a `000`-mode file whose bytes cannot be read.
    for file in &visible_files {
        ensure_not_forbidden_target(file)?;
        if system.open(file).is_err() {
            bail!("cannot read {}: aborting, nothing deleted", file.display());
        }
    }

    let mut report = RmDirReport::default();

    // Files first, deepest-first.
    visible_files.sort_by_key(|path| Reverse(depth_of(path)));
    for file in visible_files {
        system
            .remove_file(&file)
            .with_context(|| format!("removing {}", file.display()))?;
        report.files_deleted.push(file);
    }

    // Then directories deepest-first, finishing with the root itself.
    directories.push(resolved.to_path_buf());
    directories.sort_by_key(|path| Reverse(depth_of(path)));
    for dir in directories {
        if remove_dir_no_force(system, &dir) {
            report.folders_removed.push(dir);
        } else {
            report.folders_left_behind.push(dir);
        }
    }

    Ok(report)
}

/// No-force directory removal. Returns `true` when the directory was
/// removed. The `System` trait only exposes a recursive `remove_dir_all`;
/// to emulate `rmdir` (which refuses a non-empty directory) we read the
/// directory first and only remove it when it has no remaining entries.
/// A directory still holding entries remargin could not list (hidden
/// files, a nested `.remargin.yaml`) is reported back as `false` and left
/// in place — no error.
fn remove_dir_no_force(system: &dyn System, dir: &Path) -> bool {
    match system.read_dir(dir) {
        Ok(entries) if entries.is_empty() => system.remove_dir_all(dir).is_ok(),
        _ => false,
    }
}

/// Component count of a path — used to order deepest-first deletes.
fn depth_of(path: &Path) -> usize {
    path.components().count()
}

/// Display-form of every path, for the directory-report JSON arrays.
fn display_paths(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect()
}

/// Write document contents with comment preservation enforcement.
///
/// The payload must include all existing comment blocks with their original
/// IDs, checksums, and signatures intact.
///
/// When `create` is true, the file is expected to be new: missing parent
/// directories are created automatically (subject to the sandbox check), and
/// the file itself must not already exist. Comment preservation checks are
/// skipped since there is no existing file to compare against.
///
/// # Errors
///
/// Returns an error if:
/// - The path escapes the sandbox
/// - Comments were added, removed, or modified (when `create` is false)
/// - `create` is true but the file already exists
/// - `create` is true and creating the parent directories would escape the sandbox
/// - The file cannot be written
pub fn write(
    system: &dyn System,
    base_dir: &Path,
    path: &Path,
    content: &str,
    config: &ResolvedConfig,
    opts: WriteOptions,
) -> Result<WriteOutcome> {
    ensure_not_forbidden_target(path)?;
    pre_mutate_check_for_caller(system, "write", path, &config.caller_info())?;
    validate_write_opts(path, &opts)?;

    let resolved = if opts.create {
        let target = allowlist::resolve_sandboxed_create(
            system,
            base_dir,
            path,
            config.unrestricted,
            &config.trusted_roots,
        )?;
        if system.read_to_string(&target).is_ok() {
            bail!(
                "file already exists (use write without --create): {}",
                path.display()
            );
        }
        target
    } else {
        allowlist::resolve_sandboxed(
            system,
            base_dir,
            path,
            config.unrestricted,
            &config.trusted_roots,
        )?
    };
    ensure_not_forbidden_target(&resolved)?;

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

    // Non-markdown extensions skip parse / ensure_frontmatter / verify
    // and write the payload as-is. Partial-line writes still splice
    // textually; frontmatter injection is markdown-only.
    if !is_markdown_extension(&resolved) {
        let bytes: Vec<u8> = if let Some((start, end)) = opts.lines {
            let existing = system
                .read_to_string(&resolved)
                .with_context(|| format!("reading {} for partial write", resolved.display()))?;
            splice_lines(&existing, start, end, content).into_bytes()
        } else {
            content.as_bytes().to_vec()
        };
        if is_byte_identical(system, &resolved, &bytes) {
            return Ok(WriteOutcome { noop: true });
        }
        system
            .write(&resolved, &bytes)
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

    commit_markdown(system, config, &resolved, &content_to_parse, opts.create)
}

/// Shared markdown-commit tail for whole-document mutations.
///
/// Runs the exact integrity pipeline `write` uses on its markdown path:
/// parse the candidate content, enforce comment preservation against the
/// on-disk document (skipped when `create`), normalize frontmatter,
/// short-circuit byte-identical no-ops, then commit through the
/// post-verify subset gate ([`commit_with_verify`]). `resolved` must be a
/// path that has already passed sandbox resolution, the forbidden-target
/// check, and the visibility check; the op-guard (`pre_mutate_check_*`)
/// must already have run under the caller's own op name. This factoring
/// lets `replace` reuse the identical commit semantics without inheriting
/// `write`'s op name (so `deny write / allow replace` policies stay
/// independent).
///
/// # Errors
///
/// Surfaces parse failures, comment-preservation violations, frontmatter
/// errors, and the subset-gate refusal — the same diagnostics `write`
/// raises on its markdown path.
pub(crate) fn commit_markdown(
    system: &dyn System,
    config: &ResolvedConfig,
    resolved: &Path,
    content_to_parse: &str,
    create: bool,
) -> Result<WriteOutcome> {
    let new_doc = parser::parse(content_to_parse).context("parsing incoming content")?;

    // Comment preservation: only check when overwriting an existing file.
    if !create && let Ok(old_content) = system.read_to_string(resolved) {
        let old_doc = parser::parse(&old_content).context("parsing existing document")?;
        check_comment_preservation(&old_doc, &new_doc)?;
    }

    let mut final_doc = new_doc;
    frontmatter::ensure_frontmatter(&mut final_doc, config)?;

    // No-op detection: if the canonical output would be
    // byte-identical to what is already on disk, skip both the disk
    // write AND the post-write verify gate. The verify gate is
    // semantically satisfied: the file is already in the verified state
    // this payload would produce. `create` never no-ops (the path is
    // guaranteed not to exist, ruled out above).
    let final_content = final_doc.to_markdown()?;
    if !create && is_byte_identical(system, resolved, final_content.as_bytes()) {
        return Ok(WriteOutcome { noop: true });
    }

    commit_with_verify(system, &final_doc, config, resolved, |verified_doc| {
        let serialized = verified_doc.to_markdown()?;
        system
            .write(resolved, serialized.as_bytes())
            .with_context(|| format!("writing {}", resolved.display()))
    })?;

    Ok(WriteOutcome { noop: false })
}

/// No-write projection of [`commit_markdown`].
///
/// Runs the identical integrity pipeline — parse, comment-preservation,
/// frontmatter normalization, byte-identical no-op detection, and the
/// post-verify subset gate — against an **existing** file, but never
/// touches disk. Returns `true` when the candidate content would change
/// the on-disk bytes, `false` when it is a no-op. Powers `replace
/// --dry-run`: a gate refusal surfaces as an `Err` exactly as it would on
/// a real commit, but no byte is written. `resolved` must already have
/// passed sandbox resolution, the forbidden-target check, and the
/// visibility check.
///
/// # Errors
///
/// Surfaces the same diagnostics [`commit_markdown`] raises on its
/// pre-write path: parse failures, comment-preservation violations,
/// frontmatter errors, and the subset-gate refusal.
pub(crate) fn project_commit_markdown(
    system: &dyn System,
    config: &ResolvedConfig,
    resolved: &Path,
    content_to_parse: &str,
) -> Result<bool> {
    let new_doc = parser::parse(content_to_parse).context("parsing incoming content")?;

    if let Ok(old_content) = system.read_to_string(resolved) {
        let old_doc = parser::parse(&old_content).context("parsing existing document")?;
        check_comment_preservation(&old_doc, &new_doc)?;
    }

    let mut final_doc = new_doc;
    frontmatter::ensure_frontmatter(&mut final_doc, config)?;

    let final_content = final_doc.to_markdown()?;
    if is_byte_identical(system, resolved, final_content.as_bytes()) {
        return Ok(false);
    }

    // Run the subset gate without writing: the no-op closure proves the
    // candidate passes `Q ⊆ P`, so a damaging dry-run reports the same
    // refusal a real commit would.
    commit_with_verify(system, &final_doc, config, resolved, |_verified_doc| Ok(()))?;

    Ok(true)
}

/// Pure projection of a `write` op: runs the same pipeline [`write`]
/// does up through `ensure_frontmatter`, but stops before invoking
/// [`commit_with_verify`] and never calls `system.write`.
///
/// Used by the `remargin plan write` subcommand to feed a
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
    ensure_not_forbidden_target(path)?;
    validate_write_opts(path, &opts)?;

    let resolved = if opts.create {
        let target = allowlist::resolve_sandboxed_create(
            system,
            base_dir,
            path,
            config.unrestricted,
            &config.trusted_roots,
        )?;
        if system.read_to_string(&target).is_ok() {
            bail!(
                "file already exists (use write without --create): {}",
                path.display()
            );
        }
        target
    } else {
        allowlist::resolve_sandboxed(
            system,
            base_dir,
            path,
            config.unrestricted,
            &config.trusted_roots,
        )?
    };
    ensure_not_forbidden_target(&resolved)?;

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

    let after_bytes = after.to_markdown()?;
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
/// True when the path's extension marks it as part of the markdown family
/// (`.md` or `.mdx`, case-insensitive). Frontmatter injection,
/// comment-preservation, and the post-mutation verify gate apply only to
/// these files.
fn is_markdown_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            let lower = ext.to_ascii_lowercase();
            lower == "md" || lower == "mdx"
        })
}

fn is_byte_identical(system: &dyn System, path: &Path, new_bytes: &[u8]) -> bool {
    system
        .read_to_string(path)
        .is_ok_and(|existing| existing.as_bytes() == new_bytes)
}

/// Validate mutually-exclusive `WriteOptions` combinations up front so
/// both CLI and MCP callers surface identical diagnostics.
fn validate_write_opts(path: &Path, opts: &WriteOptions) -> Result<()> {
    let is_md = is_markdown_extension(path);

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
    trusted_roots: &[PathBuf],
) -> Result<DocumentMetadata> {
    let resolved =
        allowlist::resolve_sandboxed(system, base_dir, path, unrestricted, trusted_roots)?;

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
    let pending_count = comments.iter().filter(|cm| cm.is_pending()).count();

    let mut pending_for: Vec<String> = Vec::new();
    for cm in &comments {
        if !cm.is_pending() {
            continue;
        }
        for recipient in &cm.to {
            if cm.is_pending_for(recipient) && !pending_for.contains(recipient) {
                pending_for.push(recipient.clone());
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

    let pending = comments.iter().filter(|cm| cm.is_pending()).count();
    let last_activity = comments
        .iter()
        .map(|cm| cm.ts)
        .max()
        .map(|ts| ts.to_rfc3339());

    (u32::try_from(pending).ok(), last_activity)
}
