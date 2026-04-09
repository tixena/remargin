//! Comment block parser: extract remargin blocks from markdown.
//!
//! This module provides a parser that takes a markdown document as input and
//! returns a structured representation of all Remargin comment blocks with
//! their metadata and content, as well as body segments and legacy comments.

extern crate alloc;

use alloc::collections::BTreeMap;
use core::fmt::Write as _;
use core::iter::repeat_n;
use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context as _, Result};
use chrono::{DateTime, FixedOffset};
use os_shim::System;
use serde::Deserialize;
use tixschema::model_schema;

// ---------------------------------------------------------------------------
// Public data structures
// ---------------------------------------------------------------------------

/// An acknowledgment of a comment by another participant.
#[derive(Debug, Clone)]
#[non_exhaustive]
#[model_schema]
pub struct Acknowledgment {
    /// Author who acknowledged.
    pub author: String,
    /// Timestamp of the acknowledgment.
    pub ts: DateTime<FixedOffset>,
}

/// Whether the comment author is a human or an AI agent.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[model_schema]
pub enum AuthorType {
    /// An AI agent participant.
    Agent,
    /// A human participant.
    Human,
}

/// A parsed Remargin comment with all metadata fields.
#[derive(Debug, Clone)]
#[non_exhaustive]
#[model_schema]
pub struct Comment {
    /// Acknowledgments from other participants.
    pub ack: Vec<Acknowledgment>,
    /// Attached file references.
    pub attachments: Vec<String>,
    /// Author name or identifier.
    pub author: String,
    /// Whether the author is human or agent.
    pub author_type: AuthorType,
    /// Content integrity checksum (e.g. "sha256:a1b2c3d4...").
    pub checksum: String,
    /// Comment body text (everything after the YAML `---` separator).
    pub content: String,
    /// Number of backticks in the wrapping fence (3, 4, 5, ...).
    pub fence_depth: usize,
    /// Unique short identifier (e.g. "abc").
    pub id: String,
    /// 1-indexed line number of the opening fence in the source document.
    /// Zero means "not yet placed" (e.g. newly created, before write).
    pub line: usize,
    /// Emoji reactions mapped to lists of author IDs.
    pub reactions: BTreeMap<String, Vec<String>>,
    /// ID of the comment this is replying to.
    pub reply_to: Option<String>,
    /// Cryptographic signature (e.g. "ed25519:base64...").
    pub signature: Option<String>,
    /// Thread identifier grouping related comments.
    pub thread: Option<String>,
    /// Addressees of the comment.
    pub to: Vec<String>,
    /// Timestamp when the comment was created.
    pub ts: DateTime<FixedOffset>,
}

/// A legacy inline comment block (`user comments` / `agent comments`).
#[derive(Debug)]
#[non_exhaustive]
pub struct LegacyComment {
    /// Comment body content.
    pub content: String,
    /// The `[done:DATE]` marker if present.
    pub done_date: Option<String>,
    /// Number of backticks in the wrapping fence.
    pub fence_depth: usize,
    /// 1-indexed line number of the opening fence in the source document.
    pub line: usize,
    /// The raw language tag exactly as it appeared (for faithful round-trip).
    pub raw_tag: String,
    /// Whether this was a user or agent comment.
    pub role: LegacyRole,
}

/// Role of a legacy comment author.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LegacyRole {
    /// The agent (AI).
    Agent,
    /// The user (human).
    User,
}

/// A parsed document: the sequence of body segments and comment blocks
/// in document order.  This preserves the exact structure for round-tripping.
#[derive(Debug)]
#[non_exhaustive]
pub struct ParsedDocument {
    /// Alternating body text and comment blocks in document order.
    pub segments: Vec<Segment>,
}

/// Map of emoji to list of author IDs who reacted with that emoji.
pub type Reactions = BTreeMap<String, Vec<String>>;

/// One piece of a parsed document.
#[derive(Debug)]
#[non_exhaustive]
pub enum Segment {
    /// Raw markdown text (not a remargin block).
    Body(String),
    /// A parsed Remargin comment block (boxed to reduce enum size).
    Comment(Box<Comment>),
    /// An old-format comment block (for migration).
    LegacyComment(LegacyComment),
}

// ---------------------------------------------------------------------------
// YAML header deserialization (serde)
// ---------------------------------------------------------------------------

/// Raw YAML header as deserialized from the `---` block.
#[derive(Deserialize)]
struct RawYamlHeader {
    #[serde(default)]
    ack: Vec<String>,
    #[serde(default)]
    attachments: Vec<String>,
    author: String,
    #[serde(rename = "type")]
    author_type: String,
    checksum: String,
    id: String,
    #[serde(default)]
    reactions: BTreeMap<String, Vec<String>>,
    #[serde(rename = "reply-to")]
    reply_to: Option<String>,
    signature: Option<String>,
    thread: Option<String>,
    #[serde(default)]
    to: Vec<String>,
    ts: String,
}

// ---------------------------------------------------------------------------
// Fence scanner internal structure
// ---------------------------------------------------------------------------

/// A raw fenced code block located in the source text.
#[derive(Debug)]
struct FencedBlock {
    /// Number of backticks in the opening/closing fence.
    depth: usize,
    /// Byte offset one past the last character of the closing fence line.
    end: usize,
    /// The inner content between the opening fence line and closing fence line.
    inner: String,
    /// Byte offset of the first character of the opening fence line.
    start: usize,
    /// Language tag after the opening backticks.
    tag: String,
}

// ---------------------------------------------------------------------------
// ParsedDocument methods
// ---------------------------------------------------------------------------

impl ParsedDocument {
    /// Get all comment IDs as a set.
    #[must_use]
    pub fn comment_ids(&self) -> HashSet<&str> {
        self.comments().iter().map(|cm| cm.id.as_str()).collect()
    }

    /// Get all Remargin comments in document order.
    #[must_use]
    pub fn comments(&self) -> Vec<&Comment> {
        self.segments
            .iter()
            .filter_map(|seg| match seg {
                Segment::Comment(cm) => Some(cm.as_ref()),
                Segment::Body(_) | Segment::LegacyComment(_) => None,
            })
            .collect()
    }

    /// Find a comment by ID.
    #[must_use]
    pub fn find_comment(&self, id: &str) -> Option<&Comment> {
        self.comments().into_iter().find(|cm| cm.id == id)
    }

    /// Get all legacy (old-format) comments.
    #[must_use]
    pub fn legacy_comments(&self) -> Vec<&LegacyComment> {
        self.segments
            .iter()
            .filter_map(|seg| match seg {
                Segment::LegacyComment(lc) => Some(lc),
                Segment::Body(_) | Segment::Comment(_) => None,
            })
            .collect()
    }

    /// Reassemble the document back to a markdown string.
    /// This is the round-trip function: parse -> modify -> serialize.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        for seg in &self.segments {
            match seg {
                Segment::Body(text) => out.push_str(text),
                Segment::Comment(cm) => serialize_comment(cm, &mut out),
                Segment::LegacyComment(lc) => serialize_legacy_comment(lc, &mut out),
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Top-level parse functions
// ---------------------------------------------------------------------------

/// Compute a 1-indexed line number from a byte offset into the source text.
fn byte_offset_to_line(content: &str, offset: usize) -> usize {
    content[..offset].matches('\n').count() + 1
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
        // Emit body text between the end of the previous block and the start of this one.
        if block.start > last_end {
            segments.push(Segment::Body(content[last_end..block.start].to_owned()));
        }

        let line = byte_offset_to_line(content, block.start);

        if block.tag == "remargin" {
            let comment = parse_remargin_block(&block.inner, block.depth, line)
                .with_context(|| format!("in remargin block starting at byte {}", block.start))?;
            segments.push(Segment::Comment(Box::new(comment)));
        } else if let Some(legacy) = try_parse_legacy(block, line) {
            segments.push(Segment::LegacyComment(legacy));
        } else {
            // Not a remargin or legacy block; treat as body text.
            segments.push(Segment::Body(content[block.start..block.end].to_owned()));
        }

        last_end = block.end;
    }

    // Trailing body text after the last block.
    if last_end < content.len() {
        segments.push(Segment::Body(content[last_end..].to_owned()));
    }

    Ok(ParsedDocument { segments })
}

/// Read a file and parse it.
///
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

// ---------------------------------------------------------------------------
// Serialization helpers (for round-tripping)
// ---------------------------------------------------------------------------

/// Serialize a Remargin comment back to its fenced block form.
fn serialize_comment(cm: &Comment, out: &mut String) {
    let fence: String = repeat_n('`', cm.fence_depth).collect();

    // Opening fence.
    let _ = writeln!(out, "{fence}remargin");
    out.push_str("---\n");

    // Required fields.
    let _ = writeln!(out, "id: {}", cm.id);
    let _ = writeln!(out, "author: {}", cm.author);
    let type_str = match cm.author_type {
        AuthorType::Human => "human",
        AuthorType::Agent => "agent",
    };
    let _ = writeln!(out, "type: {type_str}");
    let _ = writeln!(out, "ts: {}", cm.ts.to_rfc3339());
    let _ = writeln!(out, "checksum: {}", cm.checksum);

    // Optional fields (only emit if non-default).
    if !cm.to.is_empty() {
        let _ = writeln!(out, "to: [{}]", cm.to.join(", "));
    }
    if let Some(reply_to) = &cm.reply_to {
        let _ = writeln!(out, "reply-to: {reply_to}");
    }
    if let Some(thread) = &cm.thread {
        let _ = writeln!(out, "thread: {thread}");
    }
    if !cm.attachments.is_empty() {
        let _ = writeln!(out, "attachments: [{}]", cm.attachments.join(", "));
    }
    if !cm.reactions.is_empty() {
        out.push_str("reactions:\n");
        for (emoji, authors) in &cm.reactions {
            let _ = writeln!(out, "  {emoji}: [{}]", authors.join(", "));
        }
    }
    if !cm.ack.is_empty() {
        out.push_str("ack:\n");
        for ack_entry in &cm.ack {
            let _ = writeln!(
                out,
                "  - {}@{}",
                ack_entry.author,
                ack_entry.ts.to_rfc3339()
            );
        }
    }
    if let Some(sig) = &cm.signature {
        let _ = writeln!(out, "signature: {sig}");
    }

    out.push_str("---\n");

    // Content.
    if !cm.content.is_empty() {
        out.push_str(&cm.content);
        if !cm.content.ends_with('\n') {
            out.push('\n');
        }
    }

    let _ = writeln!(out, "{fence}");
}

/// Serialize a legacy comment back to its fenced block form.
fn serialize_legacy_comment(lc: &LegacyComment, out: &mut String) {
    let fence: String = repeat_n('`', lc.fence_depth).collect();
    let _ = writeln!(out, "{fence}{}", lc.raw_tag);
    out.push_str(&lc.content);
    if !lc.content.is_empty() && !lc.content.ends_with('\n') {
        out.push('\n');
    }
    let _ = writeln!(out, "{fence}");
}

// ---------------------------------------------------------------------------
// Fence scanner
// ---------------------------------------------------------------------------

/// Scan a markdown string and identify all fenced code blocks.
fn scan_fences(source: &str) -> Vec<FencedBlock> {
    let mut blocks = Vec::new();
    let mut pos: usize = 0;
    let bytes = source.as_bytes();
    let len = bytes.len();

    while pos < len {
        let line_start = pos;

        // Count leading backticks.
        let mut tick_count: usize = 0;
        let mut idx = pos;
        while idx < len && bytes[idx] == b'`' {
            tick_count += 1;
            idx += 1;
        }

        if tick_count >= 3 {
            // This could be a fence opener.  Read the rest of the line as the tag.
            let tag_start = idx;
            while idx < len && bytes[idx] != b'\n' {
                idx += 1;
            }
            let tag = source[tag_start..idx].trim().to_owned();

            // Advance past the newline.
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
                            depth: tick_count,
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
                // No matching close found.  Treat the opening line as plain text.
                pos = line_start;
                while pos < len && bytes[pos] != b'\n' {
                    pos += 1;
                }
                if pos < len {
                    pos += 1;
                }
            }
        } else {
            // Not a fence line.  Skip to end of line.
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

// ---------------------------------------------------------------------------
// Comment header parsing
// ---------------------------------------------------------------------------

/// Parse an `ack` entry like `"eduardo@2026-04-06T15:00:00-04:00"`.
fn parse_ack_entry(entry: &str) -> Result<Acknowledgment> {
    let at_pos = entry
        .find('@')
        .with_context(|| format!("ack entry missing '@': {entry}"))?;
    let author = entry[..at_pos].to_owned();
    let ts_str = &entry[at_pos + 1..];
    let ts = DateTime::parse_from_rfc3339(ts_str)
        .with_context(|| format!("invalid ack timestamp: {ts_str}"))?;
    Ok(Acknowledgment { author, ts })
}

/// Parse the YAML header and content from a remargin block's inner text.
fn parse_remargin_block(inner: &str, fence_depth: usize, line: usize) -> Result<Comment> {
    // Split on `---` delimiters.
    let mut parts = inner.splitn(3, "---\n");

    // Skip any text before the first `---` (should be empty or whitespace).
    let _prefix = parts.next().unwrap_or("");

    let yaml_str = parts
        .next()
        .context("missing YAML header (no opening --- found)")?;

    let content_str = parts.next().unwrap_or("");

    let header: RawYamlHeader = serde_yaml::from_str(yaml_str)
        .with_context(|| format!("failed to parse YAML header:\n{yaml_str}"))?;

    let ts = DateTime::parse_from_rfc3339(&header.ts)
        .with_context(|| format!("invalid timestamp: {}", header.ts))?;

    let author_type = match header.author_type.as_str() {
        "human" => AuthorType::Human,
        "agent" => AuthorType::Agent,
        other => anyhow::bail!("unknown author type: {other}"),
    };

    let mut ack_list = Vec::with_capacity(header.ack.len());
    for entry in &header.ack {
        ack_list.push(parse_ack_entry(entry)?);
    }

    // Trim a single trailing newline from content if present (the newline
    // before the closing fence is structural, not part of the content).
    let content = content_str
        .strip_suffix('\n')
        .unwrap_or(content_str)
        .to_owned();

    Ok(Comment {
        ack: ack_list,
        attachments: header.attachments,
        author: header.author,
        author_type,
        checksum: header.checksum,
        content,
        fence_depth,
        id: header.id,
        line,
        reactions: header.reactions,
        reply_to: header.reply_to,
        signature: header.signature,
        thread: header.thread,
        to: header.to,
        ts,
    })
}

// ---------------------------------------------------------------------------
// Legacy comment parsing
// ---------------------------------------------------------------------------

/// Try to parse a fenced block as a legacy `user comments` or `agent comments`
/// block.  Returns `None` if the tag does not match.
fn try_parse_legacy(block: &FencedBlock, line: usize) -> Option<LegacyComment> {
    let tag = block.tag.trim();

    let (role, rest) = if let Some(rest) = tag.strip_prefix("user comment") {
        (LegacyRole::User, rest)
    } else if let Some(rest) = tag.strip_prefix("agent comment") {
        (LegacyRole::Agent, rest)
    } else {
        return None;
    };

    // After "user comment" or "agent comment" we can have "s" (plural)
    // and/or whitespace and a done marker.
    let without_plural = rest.strip_prefix('s').unwrap_or(rest);
    let trimmed_rest = without_plural.trim();

    let done_date = if trimmed_rest.starts_with("[done:") {
        let date_start = "[done:".len();
        trimmed_rest
            .find(']')
            .map(|end_bracket| trimmed_rest[date_start..end_bracket].to_owned())
    } else {
        None
    };

    Some(LegacyComment {
        content: block.inner.clone(),
        done_date,
        fence_depth: block.depth,
        line,
        raw_tag: block.tag.clone(),
        role,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
