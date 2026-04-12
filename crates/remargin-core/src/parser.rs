//! Comment block parser: extract remargin blocks from markdown.

extern crate alloc;

use alloc::collections::BTreeMap;
use core::fmt::Write as _;
use core::iter::repeat_n;
use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context as _, Result};
use chrono::{DateTime, FixedOffset};
use os_shim::System;
use serde::{Deserialize, Serialize};
use tixschema::model_schema;

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
    pub id: String,
    /// 1-indexed line number of the opening fence in the source document.
    /// Zero means "not yet placed" (e.g. newly created, before write).
    pub line: usize,
    pub reactions: BTreeMap<String, Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    pub to: Vec<String>,
    pub ts: DateTime<FixedOffset>,
}

/// A legacy inline comment block (`user comments` / `agent comments`).
#[derive(Debug)]
#[non_exhaustive]
pub struct LegacyComment {
    pub content: String,
    pub done_date: Option<String>,
    pub fence_depth: usize,
    pub line: usize,
    /// The raw language tag exactly as it appeared (for faithful round-trip).
    pub raw_tag: String,
    pub role: LegacyRole,
}

/// Role of a legacy comment author.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LegacyRole {
    Agent,
    User,
}

/// Sequence of body segments and comment blocks in document order. Preserves
/// the exact structure for round-tripping.
#[derive(Debug)]
#[non_exhaustive]
pub struct ParsedDocument {
    pub segments: Vec<Segment>,
}

pub type Reactions = BTreeMap<String, Vec<String>>;

#[derive(Debug)]
#[non_exhaustive]
pub enum Segment {
    /// Raw markdown text (not a remargin block).
    Body(String),
    /// A parsed Remargin comment block (boxed to reduce enum size).
    Comment(Box<Comment>),
    LegacyComment(LegacyComment),
}

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

#[derive(Debug)]
struct FencedBlock {
    depth: usize,
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
                Segment::Body(_) | Segment::LegacyComment(_) => None,
            })
            .collect()
    }

    #[must_use]
    pub fn find_comment(&self, id: &str) -> Option<&Comment> {
        self.comments().into_iter().find(|cm| cm.id == id)
    }

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

    /// Round-trip: parse -> modify -> serialize back to a markdown string.
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
        if block.start > last_end {
            segments.push(Segment::Body(content[last_end..block.start].to_owned()));
        }

        let line = byte_offset_to_line(content, block.start);

        if block.tag == "remargin" {
            let comment = parse_remargin_block(&block.inner, line)
                .with_context(|| format!("in remargin block starting at byte {}", block.start))?;
            segments.push(Segment::Comment(Box::new(comment)));
        } else if let Some(legacy) = try_parse_legacy(block, line) {
            segments.push(Segment::LegacyComment(legacy));
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

fn serialize_comment(cm: &Comment, out: &mut String) {
    let fence_depth = required_fence_depth(&cm.content);
    let fence: String = repeat_n('`', fence_depth).collect();

    let _ = writeln!(out, "{fence}remargin");
    out.push_str("---\n");

    let _ = writeln!(out, "id: {}", cm.id);
    let _ = writeln!(out, "author: {}", cm.author);
    let type_str = match cm.author_type {
        AuthorType::Human => "human",
        AuthorType::Agent => "agent",
    };
    let _ = writeln!(out, "type: {type_str}");
    let _ = writeln!(out, "ts: {}", cm.ts.to_rfc3339());
    let _ = writeln!(out, "checksum: {}", cm.checksum);

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

    if !cm.content.is_empty() {
        out.push_str(&cm.content);
        if !cm.content.ends_with('\n') {
            out.push('\n');
        }
    }

    let _ = writeln!(out, "{fence}");
}

fn serialize_legacy_comment(lc: &LegacyComment, out: &mut String) {
    let fence: String = repeat_n('`', lc.fence_depth).collect();
    let _ = writeln!(out, "{fence}{}", lc.raw_tag);
    out.push_str(&lc.content);
    if !lc.content.is_empty() && !lc.content.ends_with('\n') {
        out.push('\n');
    }
    let _ = writeln!(out, "{fence}");
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

/// Parses entries like `"eduardo@2026-04-06T15:00:00-04:00"`.
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

/// Parses entries like `"eduardo@2026-04-11T12:34:56+00:00"`.
///
/// Shape-identical to [`parse_ack_entry`]: author, literal `@`, RFC3339
/// timestamp. See [`SandboxEntry`].
///
/// # Errors
///
/// Returns an error if:
/// - The entry is missing the `@` separator
/// - The timestamp cannot be parsed as RFC 3339
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
    format!("{}@{}", entry.author, entry.ts.to_rfc3339())
}

fn parse_remargin_block(inner: &str, line: usize) -> Result<Comment> {
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

/// Returns `None` if the tag does not match a `user comments` or `agent
/// comments` block.
fn try_parse_legacy(block: &FencedBlock, line: usize) -> Option<LegacyComment> {
    let tag = block.tag.trim();

    let (role, rest) = if let Some(rest) = tag.strip_prefix("user comment") {
        (LegacyRole::User, rest)
    } else if let Some(rest) = tag.strip_prefix("agent comment") {
        (LegacyRole::Agent, rest)
    } else {
        return None;
    };

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

#[cfg(test)]
mod tests;
