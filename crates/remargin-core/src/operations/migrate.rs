//! Old-format migration.
//!
//! Convert legacy inline comments (`user comments` / `agent comments`) to the
//! Remargin format with proper IDs, checksums, and metadata.

#[cfg(test)]
mod tests;

extern crate alloc;

use alloc::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context as _, Result};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, TimeZone as _, Utc};
use os_shim::System;

use crate::config::ResolvedConfig;
use crate::crypto::compute_checksum;
use crate::frontmatter;
use crate::id;
use crate::operations::verify::commit_with_verify;
use crate::parser::{self, Acknowledgment, AuthorType, Comment, LegacyRole, Segment};
use crate::writer;

/// Record of a migrated comment.
#[derive(Debug)]
#[non_exhaustive]
pub struct MigratedComment {
    /// The new Remargin ID assigned.
    pub new_id: String,
    /// The original role (user or agent).
    pub original_role: String,
}

/// Migrate all legacy comments in a document to Remargin format.
///
/// If `backup` is true, writes a `.bak` copy before modifying.
///
/// Callers who want to preview the outcome without writing should use
/// `remargin plan migrate` (rem-0ry dropped the per-op `--dry-run` flag
/// in favour of the uniform plan projection).
///
/// # Errors
///
/// Returns an error if:
/// - The file cannot be read or written
/// - The document cannot be parsed
pub fn migrate(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    backup: bool,
) -> Result<Vec<MigratedComment>> {
    let mut doc = parser::parse_file(system, path)?;

    let legacy_count = doc.legacy_comments().len();
    if legacy_count == 0 {
        return Ok(Vec::new());
    }

    // Collect migration results.
    let mut results = Vec::new();

    // Create backup if requested.
    if backup {
        let backup_path = path.with_extension("md.bak");
        let content = system
            .read_to_string(path)
            .context("reading file for backup")?;
        system
            .write(&backup_path, content.as_bytes())
            .context("writing backup")?;
    }

    // Convert each legacy comment to a Remargin comment.
    let now = Utc::now().fixed_offset();
    let mut new_segments: Vec<Segment> = Vec::new();

    for seg in &doc.segments {
        match seg {
            Segment::LegacyComment(lc) => {
                let existing_ids = collect_ids_from_segments(&new_segments);
                let new_id = id::generate(&existing_ids);

                let (author, author_type) = match lc.role {
                    LegacyRole::User => (String::from("legacy-user"), AuthorType::Human),
                    LegacyRole::Agent => (String::from("legacy-agent"), AuthorType::Agent),
                };

                let ack = lc
                    .done_date
                    .as_ref()
                    .and_then(|date_str| parse_done_date(date_str))
                    .map(|ts| {
                        let ack_author = match lc.role {
                            LegacyRole::User => "legacy-agent",
                            LegacyRole::Agent => "legacy-user",
                        };
                        vec![Acknowledgment {
                            author: String::from(ack_author),
                            ts,
                        }]
                    })
                    .unwrap_or_default();

                let checksum = compute_checksum(&lc.content);

                let role_str = match lc.role {
                    LegacyRole::User => "user",
                    LegacyRole::Agent => "agent",
                };

                results.push(MigratedComment {
                    new_id: new_id.clone(),
                    original_role: String::from(role_str),
                });

                let comment = Comment {
                    ack,
                    attachments: Vec::new(),
                    author,
                    author_type,
                    checksum,
                    content: lc.content.clone(),
                    id: new_id,
                    line: 0, // Will be recomputed on next parse.
                    reactions: BTreeMap::default(),
                    reply_to: None,
                    signature: None,
                    thread: None,
                    to: Vec::new(),
                    ts: now,
                };

                new_segments.push(Segment::Comment(Box::new(comment)));
            }
            Segment::Body(text) => {
                new_segments.push(Segment::Body(text.clone()));
            }
            Segment::Comment(cm) => {
                new_segments.push(Segment::Comment(cm.clone()));
            }
        }
    }

    doc.segments = new_segments;

    frontmatter::ensure_frontmatter(&mut doc, config)?;

    // Write.
    let added_ids: HashSet<String> = results.iter().map(|r| r.new_id.clone()).collect();
    let removed: HashSet<String> = HashSet::new();
    commit_with_verify(&doc, config, |verified_doc| {
        writer::write_document(system, path, verified_doc, &added_ids, &removed)
    })?;

    Ok(results)
}

/// Collect comment IDs from segments built so far.
fn collect_ids_from_segments(segments: &[Segment]) -> HashSet<&str> {
    segments
        .iter()
        .filter_map(|seg| match seg {
            Segment::Comment(cm) => Some(cm.id.as_str()),
            Segment::Body(_) | Segment::LegacyComment(_) => None,
        })
        .collect()
}

/// Parse a `[done:DATE]` date string into a timestamp.
fn parse_done_date(date_str: &str) -> Option<DateTime<FixedOffset>> {
    let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d").ok()?;
    let naive_dt = date.and_time(NaiveTime::from_hms_opt(0, 0, 0)?);
    Some(Utc.from_utc_datetime(&naive_dt).fixed_offset())
}
