//! Cross-document text search engine.
//!
//! Search across markdown documents for text matches in document body,
//! comment content, or both. Supports literal and regex patterns with
//! scope filtering and context lines.

#[cfg(test)]
mod tests;

use core::fmt::Write as _;
use core::iter::repeat_n;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use regex::{Regex, RegexBuilder};
use serde::Serialize;

use tixschema::model_schema;

use crate::document::allowlist;
use crate::parser::{self, Segment, required_fence_depth};
use crate::reactions::{ReactionsExt as _, format_reaction_entry_block, quote_emoji_key};

/// Segment attribution for a single line in the document.
#[derive(Debug, Clone)]
enum LineAttribution {
    /// This line is body text.
    Body,
    /// This line is inside a comment with this ID.
    Comment(String),
}

/// Where a match was found within the document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[model_schema]
pub enum MatchLocation {
    /// In document body text (outside any comment block).
    Body,
    /// Inside a comment block.
    Comment,
}

/// A compiled pattern matcher (wraps a `Regex`).
struct Matcher {
    regex: Regex,
}

/// A single search match.
///
/// Serializes to JSON that matches the `SearchMatch` tixschema:
/// `path` as a string, `location` as `"Body"` or `"Comment"`, and
/// `comment_id` skipped when `None`.
#[derive(Debug, Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct SearchMatch {
    /// Context lines after the match.
    pub after: Vec<String>,
    /// Context lines before the match.
    pub before: Vec<String>,
    /// If the match is inside a comment, the comment ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment_id: Option<String>,
    /// 1-indexed line number of the match within the file.
    pub line: usize,
    /// Whether the match is in body text or a comment.
    pub location: MatchLocation,
    /// Relative file path.
    pub path: PathBuf,
    /// The full text of the matching line.
    pub text: String,
}

/// Options for a search operation.
#[derive(Debug)]
#[non_exhaustive]
pub struct SearchOptions {
    /// Number of context lines around each match.
    pub context_lines: usize,
    /// Case-insensitive matching.
    pub ignore_case: bool,
    /// The search pattern (literal or regex).
    pub pattern: String,
    /// Treat the pattern as a regex.
    pub regex: bool,
    /// What to search: body, comments, or all.
    pub scope: SearchScope,
}

/// Search scope: what parts of the document to search.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SearchScope {
    /// Search everything (body + comments).
    All,
    /// Search only document body text.
    Body,
    /// Search only comment content.
    Comments,
}

impl Matcher {
    fn is_match(&self, text: &str) -> bool {
        self.regex.is_match(text)
    }
}

impl SearchOptions {
    /// Set the number of context lines around matches.
    #[must_use]
    pub const fn context_lines(mut self, n: usize) -> Self {
        self.context_lines = n;
        self
    }

    /// Enable case-insensitive matching.
    #[must_use]
    pub const fn ignore_case(mut self, yes: bool) -> Self {
        self.ignore_case = yes;
        self
    }

    /// Create a new set of search options.
    #[must_use]
    pub const fn new(pattern: String) -> Self {
        Self {
            context_lines: 0,
            ignore_case: false,
            pattern,
            regex: false,
            scope: SearchScope::All,
        }
    }

    /// Enable regex mode.
    #[must_use]
    pub const fn regex(mut self, yes: bool) -> Self {
        self.regex = yes;
        self
    }

    /// Set the search scope.
    #[must_use]
    pub const fn scope(mut self, scope: SearchScope) -> Self {
        self.scope = scope;
        self
    }
}

/// Search across markdown documents in a directory tree.
///
/// Walks the directory tree, reads each visible markdown file, and searches
/// for text matches based on the provided options.
///
/// # Errors
///
/// Returns an error if:
/// - The directory cannot be walked
/// - The pattern is an invalid regex (when `options.regex` is true)
pub fn search(
    system: &dyn System,
    base_dir: &Path,
    search_dir: &Path,
    options: &SearchOptions,
) -> Result<Vec<SearchMatch>> {
    if options.pattern.is_empty() {
        bail!("search pattern cannot be empty");
    }

    let matcher = build_matcher(options)?;

    let entries = system
        .walk_dir(search_dir, false, false)
        .with_context(|| format!("walking directory {}", search_dir.display()))?;

    let mut results = Vec::new();

    for entry in &entries {
        if !entry.is_file {
            continue;
        }

        let has_md_ext = entry
            .path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if !has_md_ext || !allowlist::is_visible(&entry.path, false) {
            continue;
        }

        let Ok(content) = system.read_to_string(&entry.path) else {
            continue;
        };

        let relative = entry
            .path
            .strip_prefix(base_dir)
            .unwrap_or(&entry.path)
            .to_path_buf();

        search_file(&content, &relative, &matcher, options, &mut results);
    }

    Ok(results)
}

/// Build a `Matcher` from the search options.
fn build_matcher(options: &SearchOptions) -> Result<Matcher> {
    let pattern = if options.regex {
        options.pattern.clone()
    } else {
        regex::escape(&options.pattern)
    };

    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(options.ignore_case)
        .build()
        .with_context(|| format!("invalid regex pattern: {}", options.pattern))?;

    Ok(Matcher { regex })
}

/// Build a per-line attribution map from a parsed document.
///
/// For each line in the raw content, determine whether it is body text
/// or inside a specific comment. We achieve this by re-serializing
/// segments and tracking line counts.
fn build_line_attribution(content: &str, doc: &parser::ParsedDocument) -> Vec<LineAttribution> {
    let total_lines = content.lines().count() + usize::from(content.ends_with('\n'));
    let mut attribution = vec![LineAttribution::Body; total_lines];

    // Walk through segments, tracking byte position to compute line offsets.
    let mut byte_pos: usize = 0;

    for seg in &doc.segments {
        match seg {
            Segment::Body(text) => {
                byte_pos += text.len();
            }
            Segment::Comment(cm) => {
                let start_line = content[..byte_pos].matches('\n').count();
                let mut comment_text = String::new();
                serialize_comment_block(cm, &mut comment_text);
                let comment_lines = comment_text.matches('\n').count();
                for i in 0..comment_lines {
                    let idx = start_line + i;
                    if idx < attribution.len() {
                        attribution[idx] = LineAttribution::Comment(cm.id.clone());
                    }
                }
                byte_pos += comment_text.len();
            }
            Segment::LegacyComment(lc) => {
                let start_line = content[..byte_pos].matches('\n').count();
                let mut lc_text = String::new();
                serialize_legacy_block(lc, &mut lc_text);
                let lc_lines = lc_text.matches('\n').count();
                for i in 0..lc_lines {
                    let idx = start_line + i;
                    if idx < attribution.len() {
                        attribution[idx] = LineAttribution::Comment(String::new());
                    }
                }
                byte_pos += lc_text.len();
            }
        }
    }

    attribution
}

/// Search a single file's content for matches.
fn search_file(
    content: &str,
    relative_path: &Path,
    matcher: &Matcher,
    options: &SearchOptions,
    results: &mut Vec<SearchMatch>,
) {
    let lines: Vec<&str> = content.lines().collect();

    // Parse and build attribution for scope filtering and comment ID attribution.
    let attribution = parser::parse(content).map_or_else(
        |_| vec![LineAttribution::Body; lines.len()],
        |doc| build_line_attribution(content, &doc),
    );

    for (idx, line) in lines.iter().enumerate() {
        if !matcher.is_match(line) {
            continue;
        }

        // Check scope filter.
        let attr = attribution
            .get(idx)
            .cloned()
            .unwrap_or(LineAttribution::Body);
        let (location, comment_id) = match &attr {
            LineAttribution::Body => (MatchLocation::Body, None),
            LineAttribution::Comment(id) => {
                let cid = if id.is_empty() {
                    None
                } else {
                    Some(id.clone())
                };
                (MatchLocation::Comment, cid)
            }
        };

        match options.scope {
            SearchScope::Body if location == MatchLocation::Comment => continue,
            SearchScope::Comments if location == MatchLocation::Body => continue,
            SearchScope::All | SearchScope::Body | SearchScope::Comments => {}
        }

        // Collect context lines.
        let start = idx.saturating_sub(options.context_lines);
        let end = (idx + options.context_lines + 1).min(lines.len());

        let before: Vec<String> = lines[start..idx].iter().map(|s| String::from(*s)).collect();
        let after: Vec<String> = if idx + 1 < end {
            lines[idx + 1..end]
                .iter()
                .map(|s| String::from(*s))
                .collect()
        } else {
            Vec::new()
        };

        results.push(SearchMatch {
            after,
            before,
            comment_id,
            line: idx + 1, // 1-indexed
            location,
            path: relative_path.to_path_buf(),
            text: String::from(*line),
        });
    }
}

/// Reconstruct a comment block's text representation for line counting.
fn serialize_comment_block(cm: &parser::Comment, out: &mut String) {
    let fence_depth = required_fence_depth(&cm.content);
    let fence: String = repeat_n('`', fence_depth).collect();
    let _ = writeln!(out, "{fence}remargin");
    out.push_str("---\n");
    let _ = writeln!(out, "id: {}", cm.id);
    let _ = writeln!(out, "author: {}", cm.author);
    let _ = writeln!(out, "type: {}", cm.author_type.as_str());
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
        for (emoji, entries) in cm.reactions.entries_by_emoji() {
            let _ = writeln!(out, "  {}:", quote_emoji_key(&emoji));
            for entry in &entries {
                out.push_str(&format_reaction_entry_block("    ", entry));
            }
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

/// Reconstruct a legacy comment block's text representation for line counting.
fn serialize_legacy_block(lc: &parser::LegacyComment, out: &mut String) {
    let fence: String = repeat_n('`', lc.fence_depth).collect();
    let _ = writeln!(out, "{fence}{}", lc.raw_tag);
    out.push_str(&lc.content);
    if !lc.content.is_empty() && !lc.content.ends_with('\n') {
        out.push('\n');
    }
    let _ = writeln!(out, "{fence}");
}
