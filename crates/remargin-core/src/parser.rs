//! Comment block parser: extract remargin blocks from markdown.

pub mod heading;

extern crate alloc;

use alloc::collections::BTreeMap;
use core::fmt::Display;
use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context as _, Result};
use chrono::{DateTime, FixedOffset, SecondsFormat, TimeZone};
use os_shim::System;
use serde::Serialize;
use tixschema::model_schema;

use crate::kind::validate_kinds;
use crate::on_disk_comment::{OnDiskComment, comment_from_on_disk};
use crate::reactions::{ReactionEntry, reaction_entry_schema};
use crate::writer::serialize_comment;

/// An acknowledgment of a comment by another participant.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct Acknowledgment {
    pub author: String,
    pub ts: DateTime<FixedOffset>,
}

/// A single sandbox entry attached to a document's frontmatter.
///
/// Wire format mirrors [`Acknowledgment`]: a single `author@timestamp`
/// string. Sandbox entries represent "this participant has staged this
/// file for attention" — they are volatile user state and excluded from
/// comment-level checksums and signatures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct SandboxEntry {
    pub author: String,
    pub ts: DateTime<FixedOffset>,
}

/// Whether the comment author is a human or an AI agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
#[model_schema]
pub enum AuthorType {
    Agent,
    Human,
}

impl AuthorType {
    /// Canonical lowercase name for the author type, matching the YAML
    /// representation and the CLI / MCP JSON output.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Human => "human",
        }
    }
}

/// A parsed Remargin comment with all metadata fields.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct Comment {
    pub ack: Vec<Acknowledgment>,
    pub attachments: Vec<String>,
    pub author: String,
    pub author_type: AuthorType,
    pub checksum: String,
    pub content: String,
    /// Set by [`crate::operations::edit_comment`] on every successful
    /// edit. `None` for comments that have never
    /// been edited. Pretty-print + the activity command surface this
    /// when present. Deliberately NOT included in the signed payload
    /// (see [`crate::crypto::signature_payload`] for the rationale).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<DateTime<FixedOffset>>,
    /// 1-indexed last source line of the comment block (closing fence).
    /// `None` for in-memory comments with no source position. Runtime-only:
    /// never written to disk (the on-disk form is [`OnDiskComment`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub el: Option<usize>,
    pub id: String,
    /// 1-indexed line number of the opening fence in the source document.
    /// Zero means "not yet placed" (e.g. newly created, before write).
    pub line: usize,
    pub reactions: BTreeMap<String, Vec<ReactionEntry>>,
    /// Comment classification tags. Absent by default; each entry is a
    /// short lowercase-friendly label (e.g. `question`, `action item`)
    /// matching [`crate::kind::VALID_KIND_REGEX`] and bounded by
    /// [`crate::kind::MAX_KINDS_PER_COMMENT`].
    ///
    /// The field is additive: comments created before the `remargin_kind`
    /// field existed continue to round-trip, verify, and serialize
    /// identically because `None` contributes no bytes to the
    /// checksum or the signature payload (see [`crate::crypto`]).
    /// Prefer [`Comment::kinds`] for reads — it returns `&[]` when the
    /// field is absent so call sites do not have to branch on the
    /// Option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remargin_kind: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// 1-indexed first source line of the comment block (opening fence).
    /// Mirrors [`Comment::line`]; `None` for in-memory comments. Runtime-only:
    /// never written to disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sl: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    pub to: Vec<String>,
    pub ts: DateTime<FixedOffset>,
}

impl Comment {
    /// `max(ts, edited_at)`: the timestamp to use when deciding whether the comment is newer than a
    /// cutoff, so an edited comment surfaces under its edit time.
    #[must_use]
    pub fn effective_ts(&self) -> DateTime<FixedOffset> {
        match self.edited_at {
            Some(edited) if edited > self.ts => edited,
            _ => self.ts,
        }
    }

    /// True when at least one named recipient has not acknowledged
    /// (or, for broadcasts, when nobody has). The single source of
    /// truth used by frontmatter, metadata, search, and query so the
    /// surfaces never disagree.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        is_pending(&self.to, &self.ack)
    }

    /// True for broadcast (`to:` empty) comments written by someone
    /// else that the caller has not personally acked. Distinct from
    /// [`Self::is_pending_for`]: for broadcasts, a personal ack closes
    /// the conversation from the caller's view even when other
    /// participants have not, and the caller's own broadcasts never
    /// count.
    #[must_use]
    pub fn is_pending_broadcast_for(&self, me: &str) -> bool {
        is_pending_broadcast_for(&self.author, &self.to, &self.ack, me)
    }

    /// True when `target` is in `to:` and has not acknowledged yet.
    #[must_use]
    pub fn is_pending_for(&self, target: &str) -> bool {
        is_pending_for(&self.to, &self.ack, target)
    }

    /// Borrow the comment's classification tags as a slice. Returns
    /// `&[]` when the field is absent, so callers that do not care
    /// about the `Some(empty)` vs `None` distinction can iterate,
    /// check emptiness, or pass through to helpers like
    /// [`crate::crypto::compute_checksum`] without unwrapping the
    /// `Option` at each site.
    #[must_use]
    pub fn kinds(&self) -> &[String] {
        self.remargin_kind.as_deref().unwrap_or(&[])
    }
}

/// Sequence of body segments and comment blocks in document order. Preserves
/// the exact structure for round-tripping.
#[derive(Debug)]
#[non_exhaustive]
pub struct ParsedDocument {
    pub segments: Vec<Segment>,
}

/// One piece of a parsed document: raw markdown or a comment block.
#[derive(Debug)]
#[non_exhaustive]
pub enum Segment {
    Body(String),
    Comment(Box<Comment>),
}

/// A fenced code block found in the source, with its byte range, info tag and inner text.
#[derive(Debug)]
struct FencedBlock {
    /// Byte offset one past the last character of the closing fence line.
    end: usize,
    inner: String,
    /// Byte offset of the first character of the opening fence line.
    start: usize,
    tag: String,
}

impl ParsedDocument {
    #[must_use]
    pub fn comment_ids(&self) -> HashSet<&str> {
        self.comments().iter().map(|cm| cm.id.as_str()).collect()
    }

    /// All Remargin comments in document order.
    #[must_use]
    pub fn comments(&self) -> Vec<&Comment> {
        self.segments
            .iter()
            .filter_map(|seg| match seg {
                Segment::Comment(cm) => Some(cm.as_ref()),
                Segment::Body(_) => None,
            })
            .collect()
    }

    #[must_use]
    pub fn find_comment(&self, id: &str) -> Option<&Comment> {
        self.comments().into_iter().find(|cm| cm.id == id)
    }

    /// Build a document from segments — for in-memory construction
    /// (writer, projections, tests). Comments built this way carry no
    /// source span (`sl`/`el` are `None`); only [`parse`] records spans.
    #[must_use]
    pub const fn from_segments(segments: Vec<Segment>) -> Self {
        Self { segments }
    }

    /// Round-trip: parse -> modify -> serialize back to a markdown string.
    ///
    /// # Errors
    ///
    /// Propagates [`serialize_comment`]'s `serde_yaml::Error` if any
    /// comment fails to serialize. Unreachable in practice (the
    /// underlying Serialize impl on [`crate::on_disk_comment::OnDiskComment`]
    /// emits only primitive types), but the `Result` lets the chain
    /// surface a programming-error signal instead of panicking.
    pub fn to_markdown(&self) -> Result<String, serde_yaml::Error> {
        let mut out = String::new();
        for seg in &self.segments {
            match seg {
                Segment::Body(text) => out.push_str(text),
                Segment::Comment(cm) => out.push_str(&serialize_comment(cm)?),
            }
        }
        Ok(out)
    }
}

fn byte_offset_to_line(content: &str, offset: usize) -> usize {
    content[..offset].matches('\n').count() + 1
}

/// True when `author` appears in `ack`. Shared by `Comment` and any
/// other carrier of `(to, ack)` (e.g. `ExpandedComment`).
#[must_use]
pub fn is_acked_by(ack: &[Acknowledgment], author: &str) -> bool {
    ack.iter().any(|a| a.author == author)
}

/// Pending predicate over the `(to, ack)` shape. Directed comments
/// are pending when at least one named recipient has not acked;
/// broadcasts (`to:` empty) are pending when nobody has.
#[must_use]
pub fn is_pending(to: &[String], ack: &[Acknowledgment]) -> bool {
    if to.is_empty() {
        return ack.is_empty();
    }
    to.iter().any(|recipient| !is_acked_by(ack, recipient))
}

/// Pending-for-`target` predicate: `target` must be in `to` and not
/// already in `ack`.
#[must_use]
pub fn is_pending_for(to: &[String], ack: &[Acknowledgment], target: &str) -> bool {
    to.iter().any(|t| t == target) && !is_acked_by(ack, target)
}

/// Pending-broadcast-for-`me` predicate: `to` empty, `me` is not the
/// author, and `me` has not personally acked. Writing a broadcast is
/// not owing one — the author never sees their own in this set.
#[must_use]
pub fn is_pending_broadcast_for(
    author: &str,
    to: &[String],
    ack: &[Acknowledgment],
    me: &str,
) -> bool {
    to.is_empty() && author != me && !is_acked_by(ack, me)
}

/// Parse a markdown string into a structured document.
///
/// # Errors
///
/// Returns an error if a Remargin block contains malformed YAML or invalid
/// metadata (e.g. unparseable timestamp, unknown author type).
pub fn parse(content: &str) -> Result<ParsedDocument> {
    let blocks = scan_fences(content);
    let mut segments = Vec::new();
    let mut last_end: usize = 0;

    for block in &blocks {
        if block.start > last_end {
            segments.push(Segment::Body(content[last_end..block.start].to_owned()));
        }

        let line = byte_offset_to_line(content, block.start);

        if block.tag == "remargin" {
            let mut comment = parse_remargin_block(&block.inner, line)
                .with_context(|| format!("in remargin block starting at byte {}", block.start))?;
            // Exact source line span from the block's byte range — never
            // re-serialized, so it cannot drift.
            let block_text = &content[block.start..block.end];
            let block_lines =
                block_text.matches('\n').count() + usize::from(!block_text.ends_with('\n'));
            comment.sl = Some(line);
            comment.el = Some(line + block_lines - 1);
            segments.push(Segment::Comment(Box::new(comment)));
        } else {
            segments.push(Segment::Body(content[block.start..block.end].to_owned()));
        }

        last_end = block.end;
    }

    if last_end < content.len() {
        segments.push(Segment::Body(content[last_end..].to_owned()));
    }

    Ok(ParsedDocument { segments })
}

/// # Errors
///
/// Returns an error if the file cannot be read or contains malformed
/// remargin blocks.
pub fn parse_file(system: &dyn System, path: &Path) -> Result<ParsedDocument> {
    let content = system
        .read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    parse(&content)
}

/// Determine the minimum fence depth needed to wrap content that may
/// contain backtick sequences.  Returns at least 3.
pub(crate) fn required_fence_depth(content: &str) -> usize {
    let mut max_backticks: usize = 0;

    for line in content.split('\n') {
        let mut current: usize = 0;
        for ch in line.chars() {
            if ch == '`' {
                current += 1;
                if current > max_backticks {
                    max_backticks = current;
                }
            } else {
                current = 0;
            }
        }
    }

    let min_depth = max_backticks + 1;
    if min_depth < 3 { 3 } else { min_depth }
}

fn scan_fences(source: &str) -> Vec<FencedBlock> {
    let mut blocks = Vec::new();
    let mut pos: usize = 0;
    let bytes = source.as_bytes();
    let len = bytes.len();

    while pos < len {
        let line_start = pos;

        let mut tick_count: usize = 0;
        let mut idx = pos;
        while idx < len && bytes[idx] == b'`' {
            tick_count += 1;
            idx += 1;
        }

        if tick_count >= 3 {
            let tag_start = idx;
            while idx < len && bytes[idx] != b'\n' {
                idx += 1;
            }
            let tag = source[tag_start..idx].trim().to_owned();

            if idx < len {
                idx += 1;
            }

            let content_start = idx;
            let mut found_close = false;

            while idx < len {
                let close_line_start = idx;

                let mut close_ticks: usize = 0;
                while idx < len && bytes[idx] == b'`' {
                    close_ticks += 1;
                    idx += 1;
                }

                if close_ticks == tick_count {
                    let rest_start = idx;
                    while idx < len && bytes[idx] != b'\n' {
                        idx += 1;
                    }
                    let rest = source[rest_start..idx].trim();
                    if rest.is_empty() {
                        if idx < len {
                            idx += 1;
                        }
                        blocks.push(FencedBlock {
                            end: idx,
                            inner: source[content_start..close_line_start].to_owned(),
                            start: line_start,
                            tag: tag.clone(),
                        });
                        found_close = true;
                        break;
                    }
                } else {
                    while idx < len && bytes[idx] != b'\n' {
                        idx += 1;
                    }
                }

                if idx < len {
                    idx += 1;
                }
            }

            if found_close {
                pos = idx;
            } else {
                pos = line_start;
                while pos < len && bytes[pos] != b'\n' {
                    pos += 1;
                }
                if pos < len {
                    pos += 1;
                }
            }
        } else {
            while pos < len && bytes[pos] != b'\n' {
                pos += 1;
            }
            if pos < len {
                pos += 1;
            }
        }
    }

    blocks
}

/// Parses entries like `"alice@2026-04-11T12:34:56+00:00"`: author, literal `@`, RFC 3339
/// timestamp, the same shape as the on-disk `ack:` entries.
///
/// # Errors
///
/// Returns an error if the entry lacks the `@` separator or the timestamp is not RFC 3339.
pub fn parse_sandbox_entry(entry: &str) -> Result<SandboxEntry> {
    let at_pos = entry
        .find('@')
        .with_context(|| format!("sandbox entry missing '@': {entry}"))?;
    let author = entry[..at_pos].to_owned();
    let ts_str = &entry[at_pos + 1..];
    let ts = DateTime::parse_from_rfc3339(ts_str)
        .with_context(|| format!("invalid sandbox timestamp: {ts_str}"))?;
    Ok(SandboxEntry { author, ts })
}

/// Serialize a [`SandboxEntry`] to its compact `author@timestamp` wire form.
#[must_use]
pub fn format_sandbox_entry(entry: &SandboxEntry) -> String {
    format!("{}@{}", entry.author, rfc3339_z(&entry.ts))
}

/// Canonical RFC 3339 rendering for every displayed or on-disk timestamp:
/// `SecondsFormat::AutoSi` plus `Z` for zero offset, exactly what chrono's
/// serde impl emits, so hand-formatted and serde-serialized fields agree
/// byte for byte. Crypto canonicalization payloads deliberately do NOT use
/// this.
pub(crate) fn rfc3339_z<Tz>(ts: &DateTime<Tz>) -> String
where
    Tz: TimeZone,
    Tz::Offset: Display,
{
    ts.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

fn parse_remargin_block(inner: &str, line: usize) -> Result<Comment> {
    let mut parts = inner.splitn(3, "---\n");

    let _prefix = parts.next().unwrap_or("");

    let yaml_str = parts
        .next()
        .context("missing YAML header (no opening --- found)")?;

    let content_str = parts.next().unwrap_or("");

    // The newline before the closing fence is structural, not part of the content.
    let content = content_str
        .strip_suffix('\n')
        .unwrap_or(content_str)
        .to_owned();

    let on_disk: OnDiskComment = serde_yaml::from_str(yaml_str)
        .with_context(|| format!("failed to parse YAML header:\n{yaml_str}"))?;

    validate_kinds(&on_disk.remargin_kind)
        .with_context(|| format!("invalid remargin_kind in block starting near line {line}"))?;

    comment_from_on_disk(on_disk, content, line)
}

#[cfg(test)]
mod tests;
