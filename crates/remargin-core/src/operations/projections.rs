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

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use os_shim::System;

use crate::config::ResolvedConfig;
use crate::frontmatter;
use crate::operations::{collapse_body_segments, find_comment_mut};
use crate::parser::{self, Acknowledgment, ParsedDocument, Segment};

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
