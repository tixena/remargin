//! Comment operations: create, ack, react, delete, edit.

pub mod batch;
pub mod migrate;
pub mod plan;
pub mod projections;
pub mod purge;
pub mod query;
pub mod sandbox;
pub mod search;
pub mod sign;
pub mod threading;
pub mod verify;

#[cfg(test)]
mod tests;

extern crate alloc;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use os_shim::System;

use crate::config::ResolvedConfig;
use crate::crypto::{compute_checksum, compute_reaction_checksum, compute_signature};
use crate::frontmatter;
use crate::id;
use crate::kind::validate_kinds;
use crate::linter;
use crate::operations::verify::commit_with_verify;
use crate::parser::{self, Acknowledgment, AuthorType, Comment, ParsedDocument, Segment};
use crate::permissions::op_guard::pre_mutate_check;
use crate::reactions::Reactions;
use crate::writer::{self, InsertPosition};

/// Parameters for creating a new comment.
#[non_exhaustive]
pub struct CreateCommentParams<'params> {
    pub attachments: &'params [PathBuf],
    /// Automatically acknowledge the parent comment when replying.
    pub auto_ack: bool,
    pub content: &'params str,
    pub position: &'params InsertPosition,
    /// Optional classification tags for the new comment. Validated
    /// against [`crate::kind::validate_kinds`] before the comment is
    /// written; an invalid entry surfaces as a pre-write error so the
    /// document is never mutated with a malformed tag.
    pub remargin_kind: &'params [String],
    pub reply_to: Option<&'params str>,
    /// Atomically stage the file in the caller's sandbox in the same
    /// write cycle as the comment insert. If the caller already has a
    /// sandbox entry on the document, the existing timestamp is kept
    /// (idempotent with the standalone `sandbox add` command).
    pub sandbox: bool,
    pub to: &'params [String],
}

impl<'params> CreateCommentParams<'params> {
    #[must_use]
    pub const fn new(content: &'params str, position: &'params InsertPosition) -> Self {
        Self {
            attachments: &[],
            auto_ack: false,
            content,
            position,
            remargin_kind: &[],
            reply_to: None,
            sandbox: false,
            to: &[],
        }
    }
}

/// Create a new comment in a document.
///
/// Returns the generated comment ID.
///
/// # Errors
///
/// Returns an error if:
/// - The author is not allowed to post (mode enforcement)
/// - Attachment files do not exist
/// - The file cannot be read or written
/// - The linter detects structural issues
pub fn create_comment(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    params: &CreateCommentParams<'_>,
) -> Result<String> {
    writer::ensure_not_forbidden_target(path)?;
    pre_mutate_check(system, "comment", path)?;

    let identity = config
        .identity
        .as_deref()
        .context("identity is required to create a comment")?;

    // Registry membership and strict-mode key presence are validated at
    // `ResolvedConfig::resolve` time (rem-xc8x); the op just reads the
    // signing key it needs.
    let signing_key = config.resolve_signing_key(identity);

    if params.auto_ack && params.reply_to.is_none() {
        bail!("--auto-ack requires --reply-to");
    }

    let author_type = config.author_type.clone().unwrap_or(AuthorType::Human);

    let mut doc = parser::parse_file(system, path)?;

    let markdown_before = doc.to_markdown();
    linter::lint_or_fail(&markdown_before)
        .context("document has structural issues before write")?;

    let existing_ids = doc.comment_ids();
    let new_id = id::generate(&existing_ids);

    // rem-49w0: accept `remargin_kind` from params. Validate before
    // doing any work so a malformed tag never touches the document.
    // An empty slice becomes `None` on the comment so the YAML writer
    // emits no `remargin_kind:` line — preserving pre-kind comments
    // byte-for-byte.
    validate_kinds(params.remargin_kind).context("invalid remargin_kind")?;
    let remargin_kind: Option<Vec<String>> = if params.remargin_kind.is_empty() {
        None
    } else {
        Some(params.remargin_kind.to_vec())
    };
    let checksum = compute_checksum(params.content, params.remargin_kind);

    let thread = params
        .reply_to
        .map(|parent_id| resolve_thread(&doc, parent_id));

    let resolved_attachments = copy_attachments(system, path, config, params.attachments)
        .context("copying attachments")?;

    // Reply invariant: the parent's author is always first in `to:`.
    // Additional recipients passed by the caller are appended in input
    // order, with duplicates (including duplicates of the parent author)
    // removed. Root comments (no `reply_to`) use `params.to` verbatim.
    let effective_to: Vec<String> = {
        let parent_author = params
            .reply_to
            .and_then(|pid| doc.find_comment(pid))
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
    let mut comment = Comment {
        ack: Vec::new(),
        attachments: resolved_attachments,
        author: String::from(identity),
        author_type,
        checksum,
        content: String::from(params.content),
        edited_at: None,
        id: new_id.clone(),
        line: 0, // Placeholder; updated after document write and re-parse.
        reactions: Reactions::new(),
        remargin_kind,
        reply_to: params.reply_to.map(String::from),
        signature: None,
        thread,
        to: effective_to,
        ts: now,
    };

    if let Some(key_path) = signing_key {
        let sig = compute_signature(&comment, key_path, system)?;
        comment.signature = Some(sig);
    }

    writer::insert_comment(&mut doc, comment, params.position)?;

    // Auto-ack the parent comment in the same write cycle.
    if params.auto_ack
        && let Some(parent_id) = params.reply_to
    {
        let parent = find_comment_mut(&mut doc, parent_id)
            .with_context(|| format!("auto-ack: parent comment {parent_id:?} not found"))?;
        parent.ack.push(Acknowledgment {
            author: String::from(identity),
            ts: now,
        });
    }

    frontmatter::ensure_frontmatter(&mut doc, config)?;

    // Atomic comment+sandbox composite write: append the caller's sandbox
    // entry (idempotent) in the same write cycle. This runs *after*
    // `ensure_frontmatter` so recomputed `remargin_*` fields are preserved.
    if params.sandbox {
        let mut entries = frontmatter::read_sandbox_entries(&doc)?;
        // The bool result is intentionally ignored: idempotent re-add
        // preserves the existing timestamp, and the comment write still
        // happens either way.
        let _added = frontmatter::add_sandbox_entry_for(&mut entries, identity, now);
        frontmatter::write_sandbox_entries(&mut doc, &entries)?;
    }

    let expected_added: HashSet<String> = HashSet::from([new_id.clone()]);
    let expected_removed: HashSet<String> = HashSet::new();

    let markdown_after = doc.to_markdown();
    linter::lint_or_fail(&markdown_after).context("document has structural issues after write")?;

    commit_with_verify(&doc, config, |verified_doc| {
        writer::write_document(
            system,
            path,
            verified_doc,
            &expected_added,
            &expected_removed,
        )
    })?;

    Ok(new_id)
}

/// Acknowledge (or un-acknowledge) one or more comments.
///
/// When `remove` is true, every acknowledgment authored by the current
/// identity is stripped from each matching comment. Comments with no
/// matching ack entry are left alone (no error); the only failure mode
/// for removals is a missing comment ID.
///
/// # Errors
///
/// Returns an error if:
/// - The author is not allowed to post
/// - A comment ID does not exist
/// - Writing fails
pub fn ack_comments(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    comment_ids: &[&str],
    remove: bool,
) -> Result<()> {
    writer::ensure_not_forbidden_target(path)?;
    pre_mutate_check(system, "ack", path)?;

    let identity = config
        .identity
        .as_deref()
        .context("identity is required to ack")?;

    let mut doc = parser::parse_file(system, path)?;
    let now = Utc::now().fixed_offset();

    for comment_id in comment_ids {
        let found = find_comment_mut(&mut doc, comment_id);
        let Some(cm) = found else {
            bail!("comment {comment_id:?} not found");
        };

        // Self-heal: keep only the first Acknowledgment per author so
        // repeated acks or pre-dirty input converge to a single entry
        // (preserving the original timestamp).
        let mut seen: HashSet<String> = HashSet::new();
        cm.ack.retain(|a| seen.insert(a.author.clone()));

        if remove {
            cm.ack.retain(|a| a.author != identity);
        } else if cm.ack.iter().any(|a| a.author == identity) {
            // Idempotent: identity already acked (possibly from a
            // pre-dedup duplicate above) — nothing to push.
        } else {
            cm.ack.push(Acknowledgment {
                author: String::from(identity),
                ts: now,
            });
        }
    }

    frontmatter::ensure_frontmatter(&mut doc, config)?;

    let empty: HashSet<String> = HashSet::new();
    commit_with_verify(&doc, config, |verified_doc| {
        writer::write_document(system, path, verified_doc, &empty, &empty)
    })?;

    Ok(())
}

/// Add or remove an emoji reaction.
///
/// # Errors
///
/// Returns an error if:
/// - The author is not allowed to post
/// - The comment ID does not exist
/// - Writing fails
pub fn react(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    comment_id: &str,
    emoji: &str,
    remove: bool,
) -> Result<()> {
    writer::ensure_not_forbidden_target(path)?;
    pre_mutate_check(system, "react", path)?;

    let identity = config
        .identity
        .as_deref()
        .context("identity is required to react")?;

    let mut doc = parser::parse_file(system, path)?;

    let cm = find_comment_mut(&mut doc, comment_id)
        .with_context(|| format!("comment {comment_id:?} not found"))?;

    let now = Utc::now().fixed_offset();
    if remove {
        let _was_removed = cm.reactions.remove(emoji, identity);
    } else {
        let _was_added = cm.reactions.add(emoji, identity, now);
    }

    let _reaction_checksum = compute_reaction_checksum(&cm.reactions);

    frontmatter::ensure_frontmatter(&mut doc, config)?;

    let empty: HashSet<String> = HashSet::new();
    commit_with_verify(&doc, config, |verified_doc| {
        writer::write_document(system, path, verified_doc, &empty, &empty)
    })?;

    Ok(())
}

/// Delete one or more comments.
///
/// # Errors
///
/// Returns an error if:
/// - A comment ID does not exist
/// - Writing fails
pub fn delete_comments(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    comment_ids: &[&str],
) -> Result<()> {
    writer::ensure_not_forbidden_target(path)?;
    pre_mutate_check(system, "delete", path)?;
    let mut doc = parser::parse_file(system, path)?;

    let deleted_attachments: Vec<String> = comment_ids
        .iter()
        .filter_map(|cid| doc.find_comment(cid))
        .flat_map(|cm| cm.attachments.clone())
        .collect();

    for comment_id in comment_ids {
        if doc.find_comment(comment_id).is_none() {
            bail!("comment {comment_id:?} not found for deletion");
        }
    }

    let id_set: HashSet<&str> = comment_ids.iter().copied().collect();
    doc.segments
        .retain(|seg| !matches!(seg, Segment::Comment(cm) if id_set.contains(cm.id.as_str())));

    collapse_body_segments(&mut doc.segments);

    let remaining_attachments: HashSet<String> = doc
        .comments()
        .iter()
        .flat_map(|cm| cm.attachments.clone())
        .collect();

    for attachment in &deleted_attachments {
        if !remaining_attachments.contains(attachment) {
            let attachment_path = path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(attachment);
            // Best-effort deletion; ignore errors if the file is already gone.
            let _: Result<(), _> = system.remove_file(&attachment_path);
        }
    }

    frontmatter::ensure_frontmatter(&mut doc, config)?;

    let expected_added: HashSet<String> = HashSet::new();
    let expected_removed: HashSet<String> = comment_ids.iter().map(|s| String::from(*s)).collect();
    commit_with_verify(&doc, config, |verified_doc| {
        writer::write_document(
            system,
            path,
            verified_doc,
            &expected_added,
            &expected_removed,
        )
    })?;

    Ok(())
}

/// Edit a comment's content. Cascading consequences:
/// - Recomputes checksum and signature
/// - Clears all ack entries on the edited comment
/// - Clears all ack entries on all child comments (entire reply chain)
///
/// When `new_kinds` is `Some`, the comment's `remargin_kind` list is
/// replaced wholesale (validated first). When `None`, the stored kinds
/// are preserved — this lets content-only edits continue to work without
/// callers having to round-trip the tag list.
///
/// # Errors
///
/// Returns an error if:
/// - The comment ID does not exist
/// - `new_kinds` is present but invalid
/// - Writing fails
pub fn edit_comment(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    comment_id: &str,
    new_content: &str,
    new_kinds: Option<&[String]>,
) -> Result<()> {
    writer::ensure_not_forbidden_target(path)?;
    pre_mutate_check(system, "edit", path)?;
    let identity = config.identity.as_deref();

    // Strict-mode key presence is validated at resolve time (rem-xc8x);
    // the op just reads the key when it needs one.
    let signing_key = identity.and_then(|author| config.resolve_signing_key(author));

    // Validate replacement kinds before any document mutation so the
    // file stays byte-identical on invalid input.
    if let Some(kinds) = new_kinds {
        validate_kinds(kinds).context("invalid remargin_kind")?;
    }

    let mut doc = parser::parse_file(system, path)?;

    let cm = find_comment_mut(&mut doc, comment_id)
        .with_context(|| format!("comment {comment_id:?} not found"))?;

    cm.content = String::from(new_content);
    if let Some(kinds) = new_kinds {
        cm.remargin_kind = if kinds.is_empty() {
            None
        } else {
            Some(kinds.to_vec())
        };
    }
    // rem-n4x7: rehash against the (possibly replaced, possibly
    // preserved) kinds so the fresh checksum stays consistent with
    // the persisted YAML. `kinds()` returns `&[]` when the field is
    // absent, matching the pre-kind back-compat hinge in
    // [`compute_checksum`].
    cm.checksum = compute_checksum(new_content, cm.kinds());

    // rem-g3sy.2 / T32: stamp the edit time so the activity command
    // can surface this edit as a distinct event. Original `ts`
    // (creation time) is preserved; `edited_at` is the new field.
    // The signature payload deliberately excludes `edited_at` so
    // pre-edit signatures stay valid against the canonical metadata
    // (see [`crate::crypto::signature_payload`]).
    cm.edited_at = Some(Utc::now().fixed_offset());

    cm.ack.clear();

    if let Some(key_path) = signing_key {
        let sig = compute_signature(cm, key_path, system)?;
        cm.signature = Some(sig);
    }

    // Cascade ack invalidation through reply chain.
    let descendants = collect_descendants(&doc, comment_id);
    for descendant_id in &descendants {
        if let Some(child) = find_comment_mut(&mut doc, descendant_id) {
            child.ack.clear();
        }
    }

    frontmatter::ensure_frontmatter(&mut doc, config)?;

    let empty: HashSet<String> = HashSet::new();
    commit_with_verify(&doc, config, |verified_doc| {
        writer::write_document(system, path, verified_doc, &empty, &empty)
    })?;

    Ok(())
}

/// Merge adjacent `Body` segments and collapse runs of 3+ consecutive
/// newlines down to 2 (at most one blank line).  This prevents surplus
/// blank lines after comment deletion.
///
/// The tricky part: comment serialization already appends `\n` via
/// `writeln!` on the closing fence.  So a `Body("\n\n")` between two
/// comments produces three consecutive newlines in the final output
/// (`\n` from fence + `\n\n` from body).  We handle this by also
/// considering the surrounding context when normalizing whitespace-only
/// body segments.
pub(crate) fn collapse_body_segments(segments: &mut Vec<Segment>) {
    // 1. Merge adjacent Body segments into one.
    let mut idx = 0;
    while idx + 1 < segments.len() {
        if matches!(segments[idx], Segment::Body(_))
            && matches!(segments[idx + 1], Segment::Body(_))
        {
            if let Segment::Body(next_text) = segments.remove(idx + 1)
                && let Segment::Body(ref mut text) = segments[idx]
            {
                text.push_str(&next_text);
            }
        } else {
            idx += 1;
        }
    }

    // 2. Normalize excessive newlines in Body segments.
    //
    // Two passes: first a general normalization (collapse 3+ newlines
    // to 2), then a context-aware pass that accounts for the `\n` that
    // comment serialization already appends via `writeln!` on the
    // closing fence.
    for seg in segments.iter_mut() {
        if let Segment::Body(text) = seg {
            while text.contains("\n\n\n") {
                *text = text.replace("\n\n\n", "\n\n");
            }
        }
    }

    // Context-aware pass: a whitespace-only body segment adjacent to a
    // comment only needs a single `\n` because the comment block itself
    // already ends with `\n`.  Without this, `Body("\n\n")` between
    // two comments produces three consecutive newlines in the output.
    let len = segments.len();
    for pos in 0..len {
        let is_whitespace_body = matches!(&segments[pos], Segment::Body(t) if t.trim().is_empty());
        if !is_whitespace_body {
            continue;
        }

        let preceded_by_comment = pos > 0
            && matches!(
                segments[pos - 1],
                Segment::Comment(_) | Segment::LegacyComment(_)
            );
        let followed_by_comment = pos + 1 < len
            && matches!(
                segments[pos + 1],
                Segment::Comment(_) | Segment::LegacyComment(_)
            );

        if (preceded_by_comment || followed_by_comment)
            && let Segment::Body(text) = &mut segments[pos]
        {
            *text = String::from("\n");
        }
    }
}

/// Collect all descendant comment IDs in the reply chain (depth-first).
pub(super) fn collect_descendants(doc: &ParsedDocument, root_id: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut stack = vec![String::from(root_id)];

    while let Some(parent_id) = stack.pop() {
        for cm in doc.comments() {
            if cm.reply_to.as_deref() == Some(parent_id.as_str()) && !result.contains(&cm.id) {
                result.push(cm.id.clone());
                stack.push(cm.id.clone());
            }
        }
    }

    result
}

/// Copy attachment files to the assets directory.
fn copy_attachments(
    system: &dyn System,
    doc_path: &Path,
    config: &ResolvedConfig,
    attachments: &[PathBuf],
) -> Result<Vec<String>> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }

    let doc_dir = doc_path.parent().unwrap_or_else(|| Path::new("."));
    let assets_dir = doc_dir.join(&config.assets_dir);

    system
        .create_dir_all(&assets_dir)
        .context("creating assets directory")?;

    let mut result = Vec::new();
    for src_path in attachments {
        if !system.exists(src_path).unwrap_or(false) {
            bail!("attachment not found: {}", src_path.display());
        }

        let filename = src_path
            .file_name()
            .context("attachment has no filename")?
            .to_str()
            .context("attachment filename is not valid UTF-8")?;

        let dest_path = assets_dir.join(filename);
        system
            .copy(src_path, &dest_path)
            .with_context(|| format!("copying attachment {}", src_path.display()))?;

        let relative = format!("{}/{filename}", config.assets_dir);
        result.push(relative);
    }

    Ok(result)
}

pub(crate) fn find_comment_mut<'doc>(
    doc: &'doc mut ParsedDocument,
    id: &str,
) -> Option<&'doc mut Comment> {
    doc.segments.iter_mut().find_map(|seg| match seg {
        Segment::Comment(cm) if cm.id == id => Some(cm.as_mut()),
        Segment::Body(_) | Segment::Comment(_) | Segment::LegacyComment(_) => None,
    })
}

/// Resolve the thread field for a reply comment.
///
/// If the parent has a thread, inherit it. Otherwise, use the parent's ID
/// as the thread root.
pub(super) fn resolve_thread(doc: &ParsedDocument, parent_id: &str) -> String {
    doc.find_comment(parent_id)
        .and_then(|parent| parent.thread.clone())
        .unwrap_or_else(|| String::from(parent_id))
}
