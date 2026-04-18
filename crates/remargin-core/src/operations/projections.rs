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

use alloc::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use os_shim::System;

use crate::config::ResolvedConfig;
use crate::crypto::compute_checksum;
use crate::frontmatter;
use crate::id;
use crate::linter;
use crate::operations::{
    collapse_body_segments, collect_descendants, find_comment_mut, resolve_thread,
};
use crate::parser::{self, Acknowledgment, AuthorType, Comment, ParsedDocument, Segment};
use crate::writer::{self, InsertPosition};

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

    config.can_post(identity)?;

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

    config.can_post(identity)?;

    if params.auto_ack && params.reply_to.is_none() {
        bail!("--auto-ack requires --reply-to");
    }

    let author_type = config.author_type.clone().unwrap_or(AuthorType::Human);

    let (before, mut after) = parse_file_twice(system, path)?;

    let markdown_before = after.to_markdown();
    linter::lint_or_fail(&markdown_before).context("document has structural issues before plan")?;

    let existing_ids = after.comment_ids();
    let new_id = id::generate(&existing_ids);

    let checksum = compute_checksum(params.content);

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
        reactions: BTreeMap::default(),
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
    cm.checksum = compute_checksum(new_content);
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

    config.can_post(identity)?;

    let (before, mut after) = parse_file_twice(system, path)?;

    let cm = find_comment_mut(&mut after, comment_id)
        .with_context(|| format!("comment {comment_id:?} not found"))?;

    if remove {
        if let Some(authors) = cm.reactions.get_mut(emoji) {
            authors.retain(|author| author != identity);
            if authors.is_empty() {
                let _: Option<Vec<String>> = cm.reactions.remove(emoji);
            }
        }
    } else {
        let authors = cm
            .reactions
            .entry(String::from(emoji))
            .or_insert_with(Vec::new);
        if !authors.contains(&String::from(identity)) {
            authors.push(String::from(identity));
        }
    }

    frontmatter::ensure_frontmatter(&mut after, config)?;

    Ok((before, after))
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
