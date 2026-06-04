//! Copy a tracked file without touching the source.
//!
//! `remargin cp <src> <dst>` is the remargin-blessed alternative to Bash
//! `cp` on a restricted realm. The **source is always left untouched** —
//! the defining difference from `mv`. For markdown the copy is
//! **body + frontmatter only**: comment blocks are stripped so the
//! duplicate starts a clean history, avoiding both cross-tree comment-ID
//! ambiguity and broken signatures (the signature payload binds `id`).
//! Non-markdown and comment-free markdown copy verbatim.
//!
//! Both endpoints flow through the same sandbox / forbidden-target guards
//! every other mutating op uses. The destination additionally requires a
//! write-side `trusted_roots` check. The source is gated only by
//! `deny_ops`-for-`cp` so a sensitive doc can be readable (`get`) yet
//! non-duplicable.
//!
//! Single-file only; directory copy is out of scope for v1.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use serde::{Deserialize, Serialize};
use tixschema::model_schema;

use crate::config::ResolvedConfig;
use crate::document::allowlist;
use crate::frontmatter;
use crate::parser::{self, Segment};
use crate::permissions::op_guard::pre_mutate_check_for_caller;
use crate::writer::ensure_not_forbidden_target;

/// Which copy path ran. Carried in [`CpOutcome`] so callers can distinguish
/// without inspecting fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
#[model_schema]
pub enum CpKind {
    /// Comment-bearing markdown: body + frontmatter copied, comments
    /// dropped so the duplicate starts a clean history.
    BodyOnly,
    /// `src` and `dst` resolved to the same canonical path; nothing
    /// written.
    Noop,
    /// Non-markdown, or comment-free markdown: copied byte-for-byte.
    Verbatim,
}

/// Outcome of a successful [`cp`] call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
#[model_schema]
pub struct CpOutcome {
    /// Size of the source file in bytes. `0` for [`CpKind::Noop`].
    pub bytes_copied: u64,
    /// Number of comment blocks dropped from the markdown source.
    /// Non-zero only when `kind == BodyOnly`; `0` otherwise.
    pub comments_dropped: usize,
    /// Canonical absolute destination path.
    pub dst_absolute: PathBuf,
    /// Which copy variant ran.
    pub kind: CpKind,
    /// `true` when `dst` existed before the call and was overwritten.
    /// Only ever `true` when [`CpArgs::force`] was set.
    pub overwritten: bool,
    /// Canonical absolute source path.
    pub src_absolute: PathBuf,
}

/// Inputs to [`cp`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CpArgs {
    /// Destination path (relative to `base_dir` or absolute).
    pub dst: PathBuf,
    /// Allow overwriting `dst` when it exists.
    pub force: bool,
    /// Source path (relative to `base_dir` or absolute).
    pub src: PathBuf,
}

impl CpArgs {
    /// Constructor that takes ownership of both paths and defaults
    /// `force` to `false`.
    #[must_use]
    pub const fn new(src: PathBuf, dst: PathBuf) -> Self {
        Self {
            dst,
            force: false,
            src,
        }
    }

    /// Builder-style mutator: opt the args into `--force` overwrite
    /// semantics.
    #[must_use]
    pub const fn with_force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }
}

/// Copy a single file from `args.src` to `args.dst`.
///
/// The source is **never** modified. For markdown the copy carries the body
/// and frontmatter only (comment blocks are dropped); for non-markdown and
/// comment-free markdown the copy is verbatim bytes.
///
/// Both endpoints flow through:
///
/// - [`ensure_not_forbidden_target`] — refuses reserved basenames.
/// - [`allowlist::resolve_sandboxed`] (src) and
///   [`allowlist::resolve_sandboxed_create`] (dst) — sandbox boundary
///   enforcement.
///
/// The destination additionally passes [`pre_mutate_check_for_caller`] (must
/// be inside `trusted_roots` / not blocked by `deny_ops` for `cp`). The
/// source passes only the `deny_ops`-for-`cp` check — it is not required to
/// be inside `trusted_roots` because `cp` does not mutate it.
///
/// # Errors
///
/// Returns an error when:
///
/// - Either endpoint is a forbidden target.
/// - Either endpoint escapes the sandbox.
/// - The destination is outside `trusted_roots` for the caller.
/// - `deny_ops: cp` is set on the source.
/// - `args.src` is missing.
/// - `args.src` is a directory (recursive copy is out of scope for v1).
/// - `args.src` is a file and `args.dst` is an existing directory.
/// - `args.dst` already exists and `args.force` is `false`.
/// - The underlying copy or write operation fails.
pub fn cp(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    args: &CpArgs,
) -> Result<CpOutcome> {
    let (src_resolved, dst_resolved) = resolve_endpoints(system, base_dir, config, args)?;

    if src_resolved == dst_resolved {
        return Ok(CpOutcome {
            bytes_copied: 0,
            comments_dropped: 0,
            dst_absolute: dst_resolved,
            kind: CpKind::Noop,
            overwritten: false,
            src_absolute: src_resolved,
        });
    }

    let caller = config.caller_info();
    pre_mutate_check_for_caller(system, "cp", &dst_resolved, &caller)?;
    pre_mutate_check_for_caller(system, "cp", &src_resolved, &caller)?;

    let dst_pre_existed = system.exists(&dst_resolved).unwrap_or(false);
    if dst_pre_existed && !args.force {
        bail!(
            "destination exists: {} (pass --force to overwrite)",
            args.dst.display()
        );
    }

    let bytes_copied = file_size(system, &src_resolved);
    perform_copy(
        system,
        config,
        &src_resolved,
        &dst_resolved,
        bytes_copied,
        dst_pre_existed,
    )
}

/// Validate source/destination shapes and resolve both endpoints through the
/// sandbox boundary and forbidden-target check. Returns
/// `(src_resolved, dst_resolved)`.
fn resolve_endpoints(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    args: &CpArgs,
) -> Result<(PathBuf, PathBuf)> {
    ensure_not_forbidden_target(&args.src)?;
    ensure_not_forbidden_target(&args.dst)?;

    let src_lexical = lexical_join(base_dir, &args.src);
    let dst_lexical = lexical_join(base_dir, &args.dst);

    if !system.exists(&src_lexical).unwrap_or(false) {
        bail!("source not found: {}", args.src.display());
    }
    if system.is_dir(&src_lexical).unwrap_or(false) {
        bail!(
            "source is a directory: {} (recursive copy is not supported in v1; pass an explicit file path)",
            args.src.display()
        );
    }
    if system.is_dir(&dst_lexical).unwrap_or(false) {
        bail!(
            "destination is a directory: {} (this op copies a single file; pass an explicit destination path)",
            args.dst.display()
        );
    }

    let src_resolved = allowlist::resolve_sandboxed(
        system,
        base_dir,
        &args.src,
        config.unrestricted,
        &config.trusted_roots,
    )?;
    ensure_not_forbidden_target(&src_resolved)?;

    let dst_resolved = allowlist::resolve_sandboxed_create(
        system,
        base_dir,
        &args.dst,
        config.unrestricted,
        &config.trusted_roots,
    )?;
    ensure_not_forbidden_target(&dst_resolved)?;

    Ok((src_resolved, dst_resolved))
}

/// Execute the actual copy after all preflights have passed. Decides between
/// non-markdown verbatim byte copy, comment-free markdown copy, and
/// comment-bearing body-only copy.
fn perform_copy(
    system: &dyn System,
    config: &ResolvedConfig,
    src: &Path,
    dst: &Path,
    bytes_copied: u64,
    dst_pre_existed: bool,
) -> Result<CpOutcome> {
    if !is_markdown_extension(src) {
        system
            .copy(src, dst)
            .with_context(|| format!("copying {} -> {}", src.display(), dst.display()))?;
        return Ok(CpOutcome {
            bytes_copied,
            comments_dropped: 0,
            dst_absolute: dst.to_path_buf(),
            kind: CpKind::Verbatim,
            overwritten: dst_pre_existed,
            src_absolute: src.to_path_buf(),
        });
    }

    let src_content = system
        .read_to_string(src)
        .with_context(|| format!("reading {}", src.display()))?;
    let parsed = parser::parse(&src_content)
        .with_context(|| format!("parsing source document {}", src.display()))?;

    let comment_count = parsed.comments().len();

    if comment_count == 0 {
        write_markdown_copy(system, dst, &src_content, config)?;
        return Ok(CpOutcome {
            bytes_copied,
            comments_dropped: 0,
            dst_absolute: dst.to_path_buf(),
            kind: CpKind::Verbatim,
            overwritten: dst_pre_existed,
            src_absolute: src.to_path_buf(),
        });
    }

    let mut body_only_doc = parsed;
    let comments_dropped = body_only_doc.comments().len();
    body_only_doc
        .segments
        .retain(|seg| matches!(seg, Segment::Body(_)));

    let body_text = body_only_doc
        .to_markdown()
        .with_context(|| format!("reassembling body-only content from {}", src.display()))?;

    write_markdown_copy(system, dst, &body_text, config)?;

    Ok(CpOutcome {
        bytes_copied,
        comments_dropped,
        dst_absolute: dst.to_path_buf(),
        kind: CpKind::BodyOnly,
        overwritten: dst_pre_existed,
        src_absolute: src.to_path_buf(),
    })
}

/// Write markdown content to `dst` through the frontmatter pipeline.
///
/// Runs `ensure_frontmatter` (which recomputes `remargin_pending`,
/// `remargin_pending_for`, and `remargin_last_activity` from the comments in
/// `content`) then clears the `sandbox` key so the copy starts with no
/// pending/sandbox state. Returns `(CpKind::Verbatim, 0)` — the
/// comment-bearing branch is handled by the caller which substitutes
/// `CpKind::BodyOnly`.
fn write_markdown_copy(
    system: &dyn System,
    dst: &Path,
    content: &str,
    config: &ResolvedConfig,
) -> Result<()> {
    let mut doc = parser::parse(content)
        .with_context(|| format!("parsing markdown content for {}", dst.display()))?;
    // Recompute remargin_pending / _pending_for / _last_activity from the
    // (possibly empty) comment list in `doc`.
    frontmatter::ensure_frontmatter(&mut doc, config)?;
    // Clear the sandbox key: the copy is a fresh document with no staged
    // participants.
    frontmatter::write_sandbox_entries(&mut doc, &[])?;

    let serialized = doc
        .to_markdown()
        .with_context(|| format!("serializing markdown for {}", dst.display()))?;
    system
        .write(dst, serialized.as_bytes())
        .with_context(|| format!("writing {}", dst.display()))
}

/// Read the file at `path` and return its size in bytes, or `0` when
/// the read fails (informational; failure here doesn't affect the copy).
fn file_size(system: &dyn System, path: &Path) -> u64 {
    let Ok(content) = system.read_to_string(path) else {
        return 0;
    };
    u64::try_from(content.len()).unwrap_or(0)
}

/// Lexical join: relative paths are appended to `base`, absolute paths
/// pass through.
fn lexical_join(base: &Path, requested: &Path) -> PathBuf {
    if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        base.join(requested)
    }
}

/// `true` when `path` has a markdown extension (`.md` / `.mdx`).
fn is_markdown_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            let lower = ext.to_ascii_lowercase();
            lower == "md" || lower == "mdx"
        })
}

/// Render a [`CpOutcome`] as a human-readable one-line summary.
///
/// `src` and `dst` are the display forms of the paths (caller-supplied
/// strings, not the resolved canonicals).
#[must_use]
pub fn render_cp_outcome(src: &str, dst: &str, outcome: &CpOutcome) -> String {
    let overwrite_suffix = if outcome.overwritten {
        ", overwrote destination"
    } else {
        ""
    };
    match outcome.kind {
        CpKind::Noop => format!("no-op: {src} (same canonical path)"),
        CpKind::Verbatim => format!(
            "copied: {src} -> {dst} ({} bytes{overwrite_suffix})",
            outcome.bytes_copied,
        ),
        CpKind::BodyOnly => format!(
            "copied: {src} -> {dst} ({} bytes, dropped {} comment{}{overwrite_suffix})",
            outcome.bytes_copied,
            outcome.comments_dropped,
            if outcome.comments_dropped == 1 {
                ""
            } else {
                "s"
            },
        ),
    }
}
