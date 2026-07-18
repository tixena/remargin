//! Cross-document text search engine.
//!
//! Search across markdown documents for text matches in document body,
//! comment content, or both. Supports literal and regex patterns with
//! scope filtering and context lines.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use serde_json::{Value, json};

use tixschema::model_schema;

use crate::document::allowlist;
use crate::parser;

/// Compact match-row column names for the base (no-context) arity.
///
/// Emitted once per response in the envelope's `match_cols` header;
/// [`to_compact_row`] fills the positions in this order. `comment_id`
/// is `null` for body matches.
pub const MATCH_COLS: [&str; 4] = ["line", "location", "text", "comment_id"];

/// [`MATCH_COLS`] widened with `before` / `after` (both string arrays),
/// selected when context lines were requested.
pub const MATCH_COLS_CTX: [&str; 6] = ["line", "location", "text", "comment_id", "before", "after"];

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

/// Lowercase `location` for the MCP `search` output.
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
#[model_schema]
pub enum SearchHitLocation {
    Body,
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

/// A bounded page of matches plus the exact total.
///
/// `total` is the full match count from a complete scan (no short-circuit);
/// `matches` is the clamped `[offset .. offset + limit]` window. An offset
/// past the end yields empty `matches` with a truthful `total`, so a caller
/// can always render "showing N of total" and never mistake a page for the
/// whole set.
#[derive(Debug)]
#[non_exhaustive]
pub struct SearchResults {
    /// The clamped page of matches.
    pub matches: Vec<SearchMatch>,
    /// Total matches across the corpus, before offset/limit.
    pub total: usize,
}

/// One compact match row: `[line, location, text, comment_id]`.
///
/// Only the base (4-column) arity is codegen'd; with context the row
/// widens with `before` / `after` at runtime, and the self-describing
/// `match_cols` header covers the widened shape. `location` keeps the MCP
/// lowercase convention (`body` / `comment`), NOT the CLI's `PascalCase`;
/// `comment_id` serializes `null` for body matches.
#[model_schema(name = "CompactMatchRow")]
pub type CompactMatchRow = (usize, SearchHitLocation, String, Option<String>);

/// Schema anchor for one compact per-file match group.
///
/// Mirrors the runtime `{path, matches}` shape but its `matches` are
/// positional [`CompactMatchRow`]s. Exists so xtask emits the TS / Zod
/// types the LLM consumer reads; the runtime builds the shape in
/// [`group_compact`] and the enclosing envelope (`total`, `match_cols`,
/// `effective_limit`, `files`) is assembled by each surface.
#[model_schema]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct CompactFileMatches {
    pub matches: Vec<CompactMatchRow>,
    pub path: PathBuf,
}

/// Options for a search operation.
#[derive(Debug)]
#[non_exhaustive]
pub struct SearchOptions {
    /// Number of context lines around each match.
    pub context_lines: usize,
    /// Case-insensitive matching.
    pub ignore_case: bool,
    /// Page size: return at most this many matches. `None` returns all.
    pub limit: Option<usize>,
    /// Number of matches to skip before the returned page.
    pub offset: usize,
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

    /// Set the page size (max matches returned). `None` returns all.
    #[must_use]
    pub const fn limit(mut self, limit: Option<usize>) -> Self {
        self.limit = limit;
        self
    }

    /// Create a new set of search options.
    #[must_use]
    pub const fn new(pattern: String) -> Self {
        Self {
            context_lines: 0,
            ignore_case: false,
            limit: None,
            offset: 0,
            pattern,
            regex: false,
            scope: SearchScope::All,
        }
    }

    /// Skip this many matches before the returned page.
    #[must_use]
    pub const fn offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
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

/// Column header naming the [`to_compact_row`] positions, widened when
/// context lines were requested.
#[must_use]
pub const fn match_cols(with_context: bool) -> &'static [&'static str] {
    if with_context {
        &MATCH_COLS_CTX
    } else {
        &MATCH_COLS
    }
}

/// Project one verbose [`SearchMatch`] onto its compact positional row.
///
/// `before` / `after` are appended only when context lines were requested;
/// `location` is lowercased to the MCP convention and `comment_id`
/// serializes `null` for body matches.
#[must_use]
pub fn to_compact_row(m: &SearchMatch, with_context: bool) -> Value {
    let location = match m.location {
        MatchLocation::Body => SearchHitLocation::Body,
        MatchLocation::Comment => SearchHitLocation::Comment,
    };
    let mut row = vec![
        json!(m.line),
        json!(location),
        json!(m.text),
        json!(m.comment_id),
    ];
    if with_context {
        row.push(json!(m.before));
        row.push(json!(m.after));
    }
    Value::Array(row)
}

/// Group a page of matches by file into the compact `files` array.
///
/// `path` is stated once per file; a file's rows are contiguous and files
/// appear in order of their first match within the page. A single pass
/// suffices because the scan already emits a file's matches consecutively
/// — this preserves that order rather than re-sorting.
#[must_use]
pub fn group_compact(matches: &[SearchMatch], with_context: bool) -> Vec<Value> {
    let mut files: Vec<Value> = Vec::new();
    for m in matches {
        let path = m.path.display().to_string();
        let row = to_compact_row(m, with_context);
        let continues = files
            .last()
            .and_then(|f| f.get("path"))
            .and_then(Value::as_str)
            .is_some_and(|p| p == path);
        if continues {
            if let Some(arr) = files
                .last_mut()
                .and_then(|f| f.get_mut("matches"))
                .and_then(Value::as_array_mut)
            {
                arr.push(row);
            }
        } else {
            files.push(json!({ "path": path, "matches": [row] }));
        }
    }
    files
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
) -> Result<SearchResults> {
    if options.pattern.is_empty() {
        bail!("search pattern cannot be empty");
    }

    let matcher = build_matcher(options)?;

    // A path naming one file is searched directly; walking a file as a
    // directory yields no entries (silent zero matches). Mirrors query.
    if system.is_file(search_dir).unwrap_or(false) {
        let mut results = Vec::new();
        if let Ok(content) = system.read_to_string(search_dir) {
            let relative = search_dir
                .strip_prefix(base_dir)
                .unwrap_or(search_dir)
                .to_path_buf();
            search_file(&content, &relative, &matcher, options, &mut results);
        }
        return Ok(paginate(results, options));
    }

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

    Ok(paginate(results, options))
}

/// Clamp the full match set to `[offset .. offset + limit]` while
/// reporting the exact total. The scan is always complete, so `total`
/// is honest even when the requested page is empty (offset past end).
fn paginate(all: Vec<SearchMatch>, options: &SearchOptions) -> SearchResults {
    let total = all.len();
    let limit = options.limit.unwrap_or(usize::MAX);
    let matches = all.into_iter().skip(options.offset).take(limit).collect();
    SearchResults { matches, total }
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
/// Each comment's source line span (`sl`, `el`) is recorded at parse time
/// from the exact block bytes, so this works purely in line space — no byte
/// slicing, no re-serialization, no drift.
fn build_line_attribution(content: &str, doc: &parser::ParsedDocument) -> Vec<LineAttribution> {
    let total_lines = content.lines().count() + usize::from(content.ends_with('\n'));
    let mut attribution = vec![LineAttribution::Body; total_lines];

    for cm in doc.comments() {
        // In-memory comments carry no source position.
        let (Some(sl), Some(el)) = (cm.sl, cm.el) else {
            continue;
        };
        let end = el.min(attribution.len());
        for slot in attribution.iter_mut().take(end).skip(sl - 1) {
            *slot = LineAttribution::Comment(cm.id.clone());
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
