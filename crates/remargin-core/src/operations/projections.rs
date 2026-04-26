//! Projection-only siblings of the lightweight mutating ops (rem-3uo).
//!
//! Each `project_*` function here mirrors the same-named mutating op in
//! [`crate::operations`] up through `ensure_frontmatter`, but stops
//! before [`commit_with_verify`](crate::operations::verify::commit_with_verify)
//! and never calls into the writer. The caller gets back the
//! `(before, after)` pair it can feed into
//! [`crate::operations::plan::project_report`].
//!
//! The projection helpers intentionally run the same identity /
//! permission checks as the mutating ops — a `plan` request that would
//! fail its preflight must fail the same way as the mutating request.
//! They also run the same `frontmatter::ensure_frontmatter` pass so the
//! projected whole-file checksum reflects the exact bytes the mutating
//! op would have written.

extern crate alloc;

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use os_shim::System;
use serde_yaml::Value;

use crate::config::ResolvedConfig;
use crate::crypto::{compute_checksum, compute_signature};
use crate::frontmatter;
use crate::id;
use crate::kind::validate_kinds;
use crate::linter;
use crate::operations::migrate::{self, MigrateIdentities};
use crate::operations::sign;
use crate::operations::{
    collapse_body_segments, collect_descendants, find_comment_mut, resolve_thread,
};
use crate::parser::{self, Acknowledgment, AuthorType, Comment, ParsedDocument, Segment};
use crate::reactions::Reactions;
use crate::writer::{self, InsertPosition};

/// One sub-op inside a [`project_batch`] request: same shape as
/// [`crate::operations::batch::BatchCommentOp`] except attachments become
/// `attachment_filenames` (plan never copies bytes).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ProjectBatchOp {
    pub after_comment: Option<String>,
    pub after_line: Option<usize>,
    /// File names (not paths) to record on the projected comment. Plan
    /// never copies bytes — the caller supplies the basenames it expects
    /// to land in the assets directory.
    pub attachment_filenames: Vec<String>,
    pub auto_ack: bool,
    pub content: String,
    pub reply_to: Option<String>,
    pub to: Vec<String>,
}

impl ProjectBatchOp {
    /// Decode a single plan-batch sub-op JSON object into a
    /// [`ProjectBatchOp`]. Mirrors
    /// [`BatchCommentOp::from_json_object`](crate::operations::batch::BatchCommentOp::from_json_object)
    /// but produces the attachment-filenames (plan never copies bytes).
    ///
    /// `idx` is the zero-based position of the op in its enclosing
    /// array; it is only used to make error messages actionable.
    ///
    /// # Errors
    ///
    /// Returns an error if the required `content` field is missing or
    /// not a string.
    pub fn from_json_object(
        obj: &serde_json::Map<String, serde_json::Value>,
        idx: usize,
    ) -> Result<Self> {
        let content = obj
            .get("content")
            .and_then(serde_json::Value::as_str)
            .with_context(|| format!("plan batch op[{idx}]: missing required field `content`"))?;

        Ok(Self {
            after_comment: obj
                .get("after_comment")
                .and_then(serde_json::Value::as_str)
                .map(String::from),
            after_line: obj
                .get("after_line")
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| usize::try_from(n).ok()),
            attachment_filenames: obj
                .get("attach_names")
                .and_then(serde_json::Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
            auto_ack: obj
                .get("auto_ack")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            content: String::from(content),
            reply_to: obj
                .get("reply_to")
                .and_then(serde_json::Value::as_str)
                .map(String::from),
            to: obj
                .get("to")
                .and_then(serde_json::Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// Minimum-viable sub-op with just content. Other fields default to
    /// empty / false.
    #[must_use]
    pub const fn new(content: String) -> Self {
        Self {
            after_comment: None,
            after_line: None,
            attachment_filenames: Vec::new(),
            auto_ack: false,
            content,
            reply_to: None,
            to: Vec::new(),
        }
    }
}

/// Parameters for [`project_comment`]: mirror of
/// [`crate::operations::CreateCommentParams`] minus `attachments` (which
/// becomes `attachment_filenames` — plan never copies bytes).
#[non_exhaustive]
pub struct ProjectCommentParams<'params> {
    /// File names (not paths) to record in the comment's `attachments`
    /// list. `plan` projects what the `attachments` array would look like
    /// without actually copying any bytes; the caller passes the basenames
    /// they expect to land in the assets directory.
    pub attachment_filenames: &'params [&'params str],
    /// Auto-ack the parent comment (requires `reply_to`).
    pub auto_ack: bool,
    pub content: &'params str,
    pub position: &'params InsertPosition,
    /// Optional classification tags (rem-49w0). Validated before the
    /// projection runs so a malformed tag cannot produce a misleading
    /// preview.
    pub remargin_kind: &'params [String],
    pub reply_to: Option<&'params str>,
    /// Atomically project a sandbox entry for the acting identity. Real
    /// op would stage the file; the projection just rewrites the
    /// frontmatter so callers can see the resulting sandbox list.
    pub sandbox: bool,
    pub to: &'params [String],
}

impl<'params> ProjectCommentParams<'params> {
    /// Build the minimum-viable params (content + position). Other
    /// fields default to empty / false and can be set via the builder
    /// methods below.
    #[must_use]
    pub const fn new(content: &'params str, position: &'params InsertPosition) -> Self {
        Self {
            attachment_filenames: &[],
            auto_ack: false,
            content,
            position,
            remargin_kind: &[],
            reply_to: None,
            sandbox: false,
            to: &[],
        }
    }

    #[must_use]
    pub const fn with_attachment_filenames(mut self, filenames: &'params [&'params str]) -> Self {
        self.attachment_filenames = filenames;
        self
    }

    #[must_use]
    pub const fn with_auto_ack(mut self, auto_ack: bool) -> Self {
        self.auto_ack = auto_ack;
        self
    }

    #[must_use]
    pub const fn with_reply_to(mut self, reply_to: Option<&'params str>) -> Self {
        self.reply_to = reply_to;
        self
    }

    #[must_use]
    pub const fn with_sandbox(mut self, sandbox: bool) -> Self {
        self.sandbox = sandbox;
        self
    }

    #[must_use]
    pub const fn with_to(mut self, to: &'params [String]) -> Self {
        self.to = to;
        self
    }
}

/// Projection sibling of [`crate::operations::ack_comments`].
///
/// Returns the `(before, after)` pair without touching disk. Reads the
/// file once to build `before`, re-parses that same markdown into
/// `after`, and applies the ack mutation + frontmatter normalization in
/// place on the second copy.
///
/// # Errors
///
/// Surfaces the same diagnostics `ack_comments` would on its pre-commit
/// path: missing identity, post-permission rejection, missing comment
/// id, frontmatter issues.
pub fn project_ack(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    comment_ids: &[&str],
    remove: bool,
) -> Result<(ParsedDocument, ParsedDocument)> {
    let identity = config
        .identity
        .as_deref()
        .context("identity is required to ack")?;

    let (before, mut after) = parse_file_twice(system, path)?;

    let now = Utc::now().fixed_offset();

    for comment_id in comment_ids {
        let found = find_comment_mut(&mut after, comment_id);
        let Some(cm) = found else {
            bail!("comment {comment_id:?} not found");
        };

        // Self-heal: keep only the first Acknowledgment per author so
        // repeated acks or pre-dirty input converge to a single entry.
        let mut seen: HashSet<String> = HashSet::new();
        cm.ack.retain(|a| seen.insert(a.author.clone()));

        if remove {
            cm.ack.retain(|a| a.author != identity);
        } else if cm.ack.iter().any(|a| a.author == identity) {
            // Idempotent: identity already acked; nothing to push.
        } else {
            cm.ack.push(Acknowledgment {
                author: String::from(identity),
                ts: now,
            });
        }
    }

    frontmatter::ensure_frontmatter(&mut after, config)?;

    Ok((before, after))
}

/// Projection sibling of [`crate::operations::batch::batch_comment`].
///
/// Applies every sub-op to an in-memory copy of the document in order,
/// using the same `after_line` shift bookkeeping the real op does. No
/// disk writes, no attachment copies, no signatures.
///
/// Per rem-qll, the real `batch_comment` is atomic: if any sub-op fails,
/// nothing is written. The projection mirrors that: the first sub-op
/// whose preflight rejects stops the walk — earlier sub-ops remain
/// applied in the returned `after` so the caller can see the partial
/// state, but the caller is expected to inspect the returned error and
/// act accordingly. Per-sub-op preflight errors are surfaced prefixed
/// with the failing sub-op index so callers can route the rejection
/// back to the offending entry.
///
/// # Errors
///
/// Surfaces the same preflight diagnostics `batch_comment` would:
/// missing identity, post-permission rejection, malformed linter state,
/// any sub-op's `auto_ack` without `reply_to`, or a sub-op's
/// `reply_to` pointing at a missing parent.
pub fn project_batch(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    operations: &[ProjectBatchOp],
) -> Result<(ParsedDocument, ParsedDocument)> {
    let identity = config
        .identity
        .as_deref()
        .context("identity is required to create comments")?;

    let author_type = config.author_type.clone().unwrap_or(AuthorType::Human);

    let (before, mut after) = parse_file_twice(system, path)?;

    let markdown_before = after.to_markdown();
    linter::lint_or_fail(&markdown_before)
        .context("document has structural issues before plan batch")?;

    // Preflight the whole list before any mutation so we surface the
    // failing sub-op index without leaving half the ops applied in
    // `after` on a preventable rejection.
    for (idx, op) in operations.iter().enumerate() {
        if op.auto_ack && op.reply_to.is_none() {
            bail!("batch sub-op {idx}: auto_ack requires reply_to");
        }
    }

    let now = Utc::now().fixed_offset();
    let mut line_shifts: Vec<(usize, usize)> = Vec::new();

    for (idx, op) in operations.iter().enumerate() {
        let existing_ids = after.comment_ids();
        let new_id = id::generate(&existing_ids);
        // rem-n4x7: remargin_kind is not yet wired through the batch
        // projection op surface; rem-49w0 adds it to `BatchCommentOp`.
        // `None` preserves the pre-field checksum shape and leaves the
        // projected YAML without a `remargin_kind:` line.
        let remargin_kind: Option<Vec<String>> = None;
        let checksum = compute_checksum(&op.content, &[]);

        let thread = op.reply_to.as_deref().map(|parent_id| {
            after
                .find_comment(parent_id)
                .and_then(|parent| parent.thread.clone())
                .unwrap_or_else(|| String::from(parent_id))
        });

        let resolved_attachments: Vec<String> = op
            .attachment_filenames
            .iter()
            .map(|fname| format!("{}/{fname}", config.assets_dir))
            .collect();

        let effective_to: Vec<String> = if op.to.is_empty() {
            op.reply_to
                .as_deref()
                .and_then(|pid| after.find_comment(pid))
                .map_or_else(Vec::new, |parent| vec![parent.author.clone()])
        } else {
            op.to.clone()
        };

        let comment = Comment {
            ack: Vec::new(),
            attachments: resolved_attachments,
            author: String::from(identity),
            author_type: author_type.clone(),
            checksum,
            content: op.content.clone(),
            id: new_id,
            line: 0,
            reactions: Reactions::new(),
            remargin_kind,
            reply_to: op.reply_to.clone(),
            signature: None,
            thread,
            to: effective_to,
            ts: now,
        };

        let position = resolve_batch_position(op, &line_shifts);
        let lines_before = after.to_markdown().matches('\n').count();

        writer::insert_comment(&mut after, comment, &position)
            .with_context(|| format!("batch sub-op {idx}: inserting comment"))?;

        if op.auto_ack
            && let Some(parent_id) = op.reply_to.as_deref()
        {
            let lines_before_ack = after.to_markdown().matches('\n').count();
            let parent = find_comment_mut(&mut after, parent_id).with_context(|| {
                format!("batch sub-op {idx}: auto-ack parent {parent_id:?} not found")
            })?;
            parent.ack.push(Acknowledgment {
                author: String::from(identity),
                ts: now,
            });
            let lines_after_ack = after.to_markdown().matches('\n').count();
            let ack_lines_added = lines_after_ack.saturating_sub(lines_before_ack);
            if ack_lines_added > 0
                && let Some(parent_cm) = after.find_comment(parent_id)
            {
                line_shifts.push((parent_cm.line, ack_lines_added));
            }
        }

        if let Some(original_target) = op.after_line {
            let lines_after = after.to_markdown().matches('\n').count();
            let lines_added = lines_after.saturating_sub(lines_before);
            line_shifts.push((original_target, lines_added));
        }
    }

    frontmatter::ensure_frontmatter(&mut after, config)?;

    Ok((before, after))
}

/// Projection sibling of [`crate::operations::create_comment`].
///
/// Returns the `(before, after)` pair without touching disk. Notably,
/// attachment bytes are *not* copied — the caller supplies the expected
/// basenames via `params.attachment_filenames`, and the projection records
/// the same `<assets_dir>/<filename>` strings the real op would write into
/// the comment's `attachments` field. A `plan` consumer acting on the
/// report is responsible for ensuring those source files will exist when
/// the mutating op runs.
///
/// Signatures are also skipped: `plan` must not load the signing key.
/// Per rem-bhk, `would_sign` on the resulting `PlanReport` reports
/// whether a key is configured, not whether signing would succeed.
///
/// # Errors
///
/// Surfaces the same preflight diagnostics `create_comment` would:
/// missing identity, post-permission rejection, malformed linter state,
/// invalid reply target, auto-ack without `reply_to`.
pub fn project_comment(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    params: &ProjectCommentParams<'_>,
) -> Result<(ParsedDocument, ParsedDocument)> {
    let identity = config
        .identity
        .as_deref()
        .context("identity is required to create a comment")?;

    if params.auto_ack && params.reply_to.is_none() {
        bail!("--auto-ack requires --reply-to");
    }

    let author_type = config.author_type.clone().unwrap_or(AuthorType::Human);

    let (before, mut after) = parse_file_twice(system, path)?;

    let markdown_before = after.to_markdown();
    linter::lint_or_fail(&markdown_before).context("document has structural issues before plan")?;

    let existing_ids = after.comment_ids();
    let new_id = id::generate(&existing_ids);

    // rem-49w0: thread remargin_kind through the plan projection so
    // the preview matches the real-op output exactly. Validated before
    // any side-effect work, same as `create_comment`. Empty slice
    // becomes `None` so the projected YAML matches what `create_comment`
    // would actually write.
    validate_kinds(params.remargin_kind).context("invalid remargin_kind")?;
    let remargin_kind: Option<Vec<String>> = if params.remargin_kind.is_empty() {
        None
    } else {
        Some(params.remargin_kind.to_vec())
    };
    let checksum = compute_checksum(params.content, params.remargin_kind);

    let thread = params
        .reply_to
        .map(|parent_id| resolve_thread(&after, parent_id));

    // Plan never copies attachments; just record the expected
    // `<assets_dir>/<filename>` strings.
    let resolved_attachments: Vec<String> = params
        .attachment_filenames
        .iter()
        .map(|fname| format!("{}/{fname}", config.assets_dir))
        .collect();

    // Same reply-invariant `to:` composition as the real op.
    let effective_to: Vec<String> = {
        let parent_author = params
            .reply_to
            .and_then(|pid| after.find_comment(pid))
            .map(|parent| parent.author.clone());

        let mut result: Vec<String> = Vec::new();
        if let Some(author) = parent_author {
            result.push(author);
        }
        for recipient in params.to {
            if !result.contains(recipient) {
                result.push(recipient.clone());
            }
        }
        result
    };

    let now = Utc::now().fixed_offset();
    let comment = Comment {
        ack: Vec::new(),
        attachments: resolved_attachments,
        author: String::from(identity),
        author_type,
        checksum,
        content: String::from(params.content),
        id: new_id,
        line: 0,
        reactions: Reactions::new(),
        remargin_kind,
        reply_to: params.reply_to.map(String::from),
        signature: None,
        thread,
        to: effective_to,
        ts: now,
    };

    writer::insert_comment(&mut after, comment, params.position)?;

    if params.auto_ack
        && let Some(parent_id) = params.reply_to
    {
        let parent = find_comment_mut(&mut after, parent_id)
            .with_context(|| format!("auto-ack: parent comment {parent_id:?} not found"))?;
        parent.ack.push(Acknowledgment {
            author: String::from(identity),
            ts: now,
        });
    }

    frontmatter::ensure_frontmatter(&mut after, config)?;

    if params.sandbox {
        let mut entries = frontmatter::read_sandbox_entries(&after)?;
        let _added = frontmatter::add_sandbox_entry_for(&mut entries, identity, now);
        frontmatter::write_sandbox_entries(&mut after, &entries)?;
    }

    Ok((before, after))
}

/// Projection sibling of [`crate::operations::delete_comments`].
///
/// Returns the `(before, after)` pair without touching disk — in
/// particular, attachment files referenced by the deleted comments are
/// *not* removed. A plan for `delete` tells the caller which comment ids
/// would disappear; acting on that plan means running the real
/// `delete_comments` afterwards.
///
/// # Errors
///
/// Surfaces the same diagnostics `delete_comments` would on its
/// pre-commit path: missing comment id, frontmatter issues.
pub fn project_delete(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    comment_ids: &[&str],
) -> Result<(ParsedDocument, ParsedDocument)> {
    let (before, mut after) = parse_file_twice(system, path)?;

    for comment_id in comment_ids {
        if after.find_comment(comment_id).is_none() {
            bail!("comment {comment_id:?} not found for deletion");
        }
    }

    let id_set: HashSet<&str> = comment_ids.iter().copied().collect();
    after
        .segments
        .retain(|seg| !matches!(seg, Segment::Comment(cm) if id_set.contains(cm.id.as_str())));

    collapse_body_segments(&mut after.segments);

    frontmatter::ensure_frontmatter(&mut after, config)?;

    Ok((before, after))
}

/// Projection sibling of [`crate::operations::edit_comment`].
///
/// Returns the `(before, after)` pair without touching disk. Recomputes
/// the content-derived checksum the same way `edit_comment` does, clears
/// the edited comment's `ack` list, and cascades the ack invalidation to
/// every descendant in the reply chain. Does **not** load the signing
/// key: projections stay side-effect-free.
///
/// # Errors
///
/// Surfaces the same diagnostics `edit_comment` would on its pre-commit
/// path: missing comment id, frontmatter issues. Missing identity is
/// *not* an error here because `edit_comment` only consults identity for
/// signature decisions, which the projection skips.
pub fn project_edit(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    comment_id: &str,
    new_content: &str,
) -> Result<(ParsedDocument, ParsedDocument)> {
    let (before, mut after) = parse_file_twice(system, path)?;

    let cm = find_comment_mut(&mut after, comment_id)
        .with_context(|| format!("comment {comment_id:?} not found"))?;

    cm.content = String::from(new_content);
    // Preserve existing remargin_kind on edit, matching `edit_comment`.
    // `kinds()` returns `&[]` when the field is absent so the
    // pre-kind back-compat checksum branch still fires.
    cm.checksum = compute_checksum(new_content, cm.kinds());
    cm.ack.clear();
    // Mutating `edit_comment` also wipes the signature when content
    // changes (verify would fail otherwise); mirror that here so the
    // projection's post-verify view matches what the real op would see.
    cm.signature = None;

    let descendants = collect_descendants(&after, comment_id);
    for descendant_id in &descendants {
        if let Some(child) = find_comment_mut(&mut after, descendant_id) {
            child.ack.clear();
        }
    }

    frontmatter::ensure_frontmatter(&mut after, config)?;

    Ok((before, after))
}

/// Projection sibling of [`crate::operations::migrate::migrate`].
///
/// Delegates the per-segment building to
/// [`crate::operations::migrate::build_migrated_segments`] so the
/// projection and the real op produce byte-identical comments
/// (threading, identities, signatures, ack timestamps). The real op
/// additionally writes a `.md.bak` when `backup` is set; the projection
/// never copies bytes.
///
/// `after` is byte-identical to `before` when there are no legacy
/// comments, giving a clean `noop` verdict.
///
/// # Errors
///
/// Surfaces the same diagnostics `migrate` would on its pre-commit path:
/// frontmatter issues, or signing failures when `identities` carries a
/// key path that cannot be read.
pub fn project_migrate(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    identities: &MigrateIdentities,
) -> Result<(ParsedDocument, ParsedDocument)> {
    let (before, mut after) = parse_file_twice(system, path)?;

    if after.legacy_comments().is_empty() {
        frontmatter::ensure_frontmatter(&mut after, config)?;
        return Ok((before, after));
    }

    let now = Utc::now().fixed_offset();
    let (new_segments, _results) =
        migrate::build_migrated_segments(system, &after.segments, identities, now)?;
    after.segments = new_segments;

    frontmatter::ensure_frontmatter(&mut after, config)?;

    Ok((before, after))
}

/// Projection sibling of [`crate::operations::purge::purge`].
///
/// Strips every `Comment` and `LegacyComment` segment from the document,
/// collapses the remaining body text, and removes every `remargin_*`
/// frontmatter key. Does *not* delete attachment files from disk — plan
/// stays side-effect-free, and a caller acting on the report is expected
/// to run the real `purge` afterwards.
///
/// # Errors
///
/// Surfaces the same diagnostics `purge` would on its pre-commit path:
/// frontmatter issues.
pub fn project_purge(
    system: &dyn System,
    path: &Path,
    _config: &ResolvedConfig,
) -> Result<(ParsedDocument, ParsedDocument)> {
    let (before, mut after) = parse_file_twice(system, path)?;

    after.segments.retain(|seg| matches!(seg, Segment::Body(_)));

    collapse_body_segments(&mut after.segments);

    clean_remargin_frontmatter(&mut after);

    // `purge` strips all comments and removes `remargin_*` frontmatter
    // keys; we *don't* call `ensure_frontmatter` here because that would
    // re-inject the `remargin_version` key we just removed.
    Ok((before, after))
}

/// Projection sibling of [`crate::operations::react`].
///
/// Returns the `(before, after)` pair without touching disk.
///
/// # Errors
///
/// Surfaces the same diagnostics `react` would on its pre-commit path:
/// missing identity, post-permission rejection, missing comment id,
/// frontmatter issues.
pub fn project_react(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    comment_id: &str,
    emoji: &str,
    remove: bool,
) -> Result<(ParsedDocument, ParsedDocument)> {
    let identity = config
        .identity
        .as_deref()
        .context("identity is required to react")?;

    let (before, mut after) = parse_file_twice(system, path)?;

    let cm = find_comment_mut(&mut after, comment_id)
        .with_context(|| format!("comment {comment_id:?} not found"))?;

    let now = Utc::now().fixed_offset();
    if remove {
        let _was_removed = cm.reactions.remove(emoji, identity);
    } else {
        let _was_added = cm.reactions.add(emoji, identity, now);
    }

    frontmatter::ensure_frontmatter(&mut after, config)?;

    Ok((before, after))
}

/// Projection sibling of [`crate::operations::sign::sign_comments`]
/// (rem-7y3).
///
/// Returns the `(before, after)` pair without touching disk. Unlike the
/// other plan projections, `project_sign` **does** load the signing key
/// — the whole point of `sign` is to attach a cryptographic signature,
/// so a projection that skipped key loading would produce an `after`
/// doc byte-identical to `before` and report a misleading `noop: true`.
/// Reading the key is a pure read; no disk writes occur.
///
/// Pre-flight mirrors [`sign_comments`](crate::operations::sign::sign_comments):
/// - bails when no identity is configured,
/// - bails when no `key_path` is configured (sign without a key has
///   nothing to do — stricter than create/edit),
/// - runs [`sign::classify_candidates`] so `--ids` rejections (unknown
///   id, forgery guard) fire before any signing.
///
/// On success, every target comment in `after` carries a real signature
/// computed with the configured key; already-signed ids listed under
/// `--ids` remain unchanged in `after` (plan surfaces them via the
/// `comments.preserved` bucket, matching the skip semantics of
/// `sign_comments`).
///
/// # Errors
///
/// Surfaces the same preflight diagnostics `sign_comments` would:
/// missing identity, missing key, unknown `--ids` entry, forgery-guard
/// refusal, frontmatter issues.
pub fn project_sign(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    selection: &sign::SignSelection,
) -> Result<(ParsedDocument, ParsedDocument)> {
    let identity = config
        .identity
        .as_deref()
        .context("identity is required to sign comments")?;

    // Match `sign_comments` exactly: sign has no reason to exist without
    // a key, so resolve from `key_path` directly and bail when unset
    // regardless of mode.
    let key_path = match &config.key_path {
        Some(configured) => configured.clone(),
        None => bail!(
            "sign: no signing key resolved for {identity:?} (mode={:?}). \
             Sign requires a key regardless of mode — pass --key or add \
             a `key:` field to .remargin.yaml.",
            config.mode.as_str(),
        ),
    };

    let (before, mut after) = parse_file_twice(system, path)?;

    // Validate `--ids` up front (forgery guard + unknown-id rejection)
    // before touching any comment. The returned skip list is discarded
    // here — plan surfaces already-signed ids via `comments.preserved`.
    // The plan projection treats already-signed ids under `--ids` as
    // skipped regardless of the actual op's `--repair-checksum` flag:
    // this projection does not yet surface a "would re-sign" signal
    // (tracked under rem-7y3).
    let (targets, _skipped) = sign::classify_candidates(&after, identity, selection, false)?;
    let target_ids: HashSet<String> = targets.iter().map(|(id, _)| id.clone()).collect();

    for seg in &mut after.segments {
        if let Segment::Comment(cm) = seg
            && target_ids.contains(&cm.id)
        {
            let sig = compute_signature(cm, &key_path, system)
                .with_context(|| format!("signing comment {:?}", cm.id))?;
            cm.signature = Some(sig);
        }
    }

    frontmatter::ensure_frontmatter(&mut after, config)?;

    Ok((before, after))
}

/// Projection sibling of [`crate::operations::sandbox::add_to_files`]
/// operating on a single document.
///
/// Adds the caller's identity + now timestamp to the `sandbox:`
/// frontmatter list if absent. Idempotent: an existing entry for the
/// caller leaves the file unchanged (projection returns a noop plan).
///
/// Non-markdown paths fail with `not a markdown file` — same as the real
/// op. The projection operates on a single file because `plan` is a
/// pre-commit prediction for one document; callers that want bulk
/// behavior run the plan once per file.
///
/// # Errors
///
/// Surfaces the same preflight diagnostics `add_to_files` would:
/// empty identity, non-markdown path, missing frontmatter.
pub fn project_sandbox_add(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
) -> Result<(ParsedDocument, ParsedDocument)> {
    let identity = config
        .identity
        .as_deref()
        .context("identity is required for sandbox add")?;
    if identity.is_empty() {
        bail!("identity is required for sandbox add");
    }
    ensure_markdown_path(path)?;

    let (before, mut after) = parse_file_twice(system, path)?;

    let now = Utc::now().fixed_offset();
    let mut entries = frontmatter::read_sandbox_entries(&after)?;
    let added = frontmatter::add_sandbox_entry_for(&mut entries, identity, now);
    if added {
        frontmatter::write_sandbox_entries(&mut after, &entries)?;
    }

    frontmatter::ensure_frontmatter(&mut after, config)?;

    Ok((before, after))
}

/// Projection sibling of [`crate::operations::sandbox::remove_from_files`]
/// operating on a single document.
///
/// Removes the caller's sandbox entry if present. Idempotent: a file
/// with no matching entry projects as a noop. When the caller's entry is
/// the last one, the entire `sandbox:` key is removed from the
/// frontmatter (matches the real op's empty-collapse behavior).
///
/// # Errors
///
/// Surfaces the same preflight diagnostics `remove_from_files` would:
/// empty identity, non-markdown path, missing frontmatter.
pub fn project_sandbox_remove(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
) -> Result<(ParsedDocument, ParsedDocument)> {
    let identity = config
        .identity
        .as_deref()
        .context("identity is required for sandbox remove")?;
    if identity.is_empty() {
        bail!("identity is required for sandbox remove");
    }
    ensure_markdown_path(path)?;

    let (before, mut after) = parse_file_twice(system, path)?;

    let mut entries = frontmatter::read_sandbox_entries(&after)?;
    let removed = frontmatter::remove_sandbox_entry_for(&mut entries, identity);
    if removed {
        frontmatter::write_sandbox_entries(&mut after, &entries)?;
    }

    frontmatter::ensure_frontmatter(&mut after, config)?;

    Ok((before, after))
}

/// Strip every `remargin_*` key from the first Body segment if it starts
/// with a YAML frontmatter block. Mirrors the private helper in
/// `purge.rs` so the projection lands the same bytes as the real op.
fn clean_remargin_frontmatter(doc: &mut ParsedDocument) {
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

    let keys_to_remove: Vec<Value> = mapping
        .keys()
        .filter(|key| key.as_str().is_some_and(|s| s.starts_with("remargin_")))
        .cloned()
        .collect();
    for key in &keys_to_remove {
        let _: Option<Value> = mapping.remove(key);
    }

    if mapping.is_empty() {
        let remaining = lines[closer_idx + 1..].join("\n");
        let cleaned = remaining.trim_start_matches('\n');
        doc.segments[0] = Segment::Body(String::from(cleaned));
    } else {
        let new_yaml = serde_yaml::to_string(&Value::Mapping(mapping)).unwrap_or_default();
        let after_fm = lines[closer_idx + 1..].join("\n");
        let new_body = format!("---\n{new_yaml}---\n{after_fm}");
        doc.segments[0] = Segment::Body(new_body);
    }
}

/// Ensure `path` has an `.md` extension. Mirrors the private helper in
/// `sandbox.rs`.
fn ensure_markdown_path(path: &Path) -> Result<()> {
    let is_md = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
    if !is_md {
        bail!("not a markdown file");
    }
    Ok(())
}

/// Resolve the insertion position for a batch sub-op, adjusting any
/// `after_line` target for lines added by previous insertions in the
/// same batch. Mirrors the private helper in
/// [`crate::operations::batch`]. `reply_to` always wins over explicit
/// placement (matches the real op's semantics).
fn resolve_batch_position(op: &ProjectBatchOp, line_shifts: &[(usize, usize)]) -> InsertPosition {
    if let Some(parent_id) = &op.reply_to {
        return InsertPosition::AfterComment(parent_id.clone());
    }
    if let Some(after_comment) = &op.after_comment {
        return InsertPosition::AfterComment(after_comment.clone());
    }
    if let Some(target) = op.after_line {
        let mut adjusted = target;
        for &(prev_target, lines_added) in line_shifts {
            if target >= prev_target {
                adjusted += lines_added;
            }
        }
        return InsertPosition::AfterLine(adjusted);
    }
    InsertPosition::Append
}

/// Parse a file from disk into two independent [`ParsedDocument`] values.
///
/// `ParsedDocument` is intentionally not `Clone` (see the `#[derive]`
/// in `parser.rs`); to produce a `(before, after)` pair we parse the
/// on-disk bytes, then re-parse the same bytes via `to_markdown()` for
/// the mutable `after` copy. The round-trip through `to_markdown()` is
/// stable on any document that parsed cleanly, so `before.to_markdown()
/// == after.to_markdown()` before any mutation is applied.
fn parse_file_twice(system: &dyn System, path: &Path) -> Result<(ParsedDocument, ParsedDocument)> {
    let before = parser::parse_file(system, path)?;
    let markdown = before.to_markdown();
    let after = parser::parse(&markdown).context("re-parsing document for projection")?;
    Ok((before, after))
}
