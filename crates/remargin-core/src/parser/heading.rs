//! Heading-anchored insertion.
//!
//! Resolves `--after-heading` paths to a 1-indexed line number. ATX
//! headings only; case-sensitive prefix match; `>` separates path
//! segments which must descend strictly deeper. The resolver returns
//! a line number; downstream uses
//! [`InsertPosition::AfterLine`](crate::writer::InsertPosition::AfterLine)
//! so the comment's stored `line` field is unchanged.

use anyhow::{Context as _, Result, bail};

use crate::parser::ParsedDocument;

/// One ATX heading found by [`scan_headings`].
#[derive(Debug, Clone)]
struct Heading {
    level: usize,
    /// 1-indexed line number of the heading in the source markdown.
    line: usize,
    /// Heading text after the leading `#`s and any matching trailing
    /// `#`s have been stripped, with surrounding whitespace trimmed.
    text: String,
}

#[derive(Debug)]
struct FenceState {
    ticks: usize,
}

/// Locate the line number of a heading addressed by `path`.
///
/// Returns the 1-indexed line of the heading itself; the caller decides
/// where to insert relative to it (existing convention: insertion is
/// "after line N", which means line N+1).
///
/// `path` is a `>`-separated sequence of segments. Each segment is
/// matched as a prefix of the heading's stripped text. Walks through
/// ATX headings in document order; segments after the first must be at
/// a deeper level than the previous one and must appear within the
/// previous segment's section (i.e. before the next heading at the
/// same-or-higher level).
///
/// # Errors
///
/// Returns an error when the path is empty or malformed (leading,
/// trailing or doubled `>` separators), or when no matching heading is
/// found in the document.
pub fn resolve_heading_path(doc: &ParsedDocument, path: &str) -> Result<usize> {
    let segments = parse_path(path)?;
    let markdown = doc
        .to_markdown()
        .context("serializing document to markdown for heading resolution")?;
    let headings = scan_headings(&markdown);
    match_path(&headings, &segments).ok_or_else(|| anyhow::anyhow!("no heading matched {path:?}"))
}

/// Count the leading backtick run at the start of `line`. Returns
/// `Some(count)` when `line` starts with one or more `` ` ``, `None`
/// otherwise. ATX headings can contain backticks but never start with
/// them, so this is a sound discriminator for fence-opener lines.
const fn leading_backtick_count(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut count: usize = 0;
    while count < bytes.len() && bytes[count] == b'`' {
        count += 1;
    }
    if count == 0 { None } else { Some(count) }
}

/// Walk `headings` in order and resolve `segments` per the path rules.
///
/// Returns the line number of the heading matching the FINAL segment,
/// scoped within the section opened by the preceding segments.
fn match_path(headings: &[Heading], segments: &[String]) -> Option<usize> {
    if segments.is_empty() {
        return None;
    }
    let mut start_idx: usize = 0;
    let mut end_idx: usize = headings.len();
    let mut parent_level: Option<usize> = None;
    let mut last_match_idx: Option<usize> = None;

    for segment in segments {
        let mut found: Option<usize> = None;
        for (offset, heading) in headings[start_idx..end_idx].iter().enumerate() {
            // Each non-root segment must sit STRICTLY DEEPER than the
            // previous segment's heading.
            if let Some(parent) = parent_level
                && heading.level <= parent
            {
                continue;
            }
            if heading.text.starts_with(segment.as_str()) {
                found = Some(start_idx + offset);
                break;
            }
        }
        let idx = found?;
        last_match_idx = Some(idx);
        parent_level = Some(headings[idx].level);
        // Restrict the next segment's search to the matched section:
        // from the heading after the match through the next heading
        // at the matched level or shallower.
        start_idx = idx + 1;
        end_idx = headings[start_idx..]
            .iter()
            .position(|h| h.level <= headings[idx].level)
            .map_or(headings.len(), |off| start_idx + off);
    }

    last_match_idx.map(|idx| headings[idx].line)
}

/// Parse an ATX heading line per `CommonMark`.
///
/// Up to three leading spaces of indentation are allowed; the `#` run
/// is 1-6 characters and must be followed by a space (or be the entire
/// line, which yields empty text). Trailing `#`s preceded by whitespace
/// are stripped along with surrounding whitespace.
fn parse_atx_heading(line: &str) -> Option<(usize, String)> {
    let bytes = line.as_bytes();
    let mut idx: usize = 0;
    let mut spaces: usize = 0;
    while idx < bytes.len() && bytes[idx] == b' ' && spaces < 3 {
        idx += 1;
        spaces += 1;
    }
    if idx == bytes.len() || bytes[idx] != b'#' {
        return None;
    }

    let hash_start = idx;
    while idx < bytes.len() && bytes[idx] == b'#' {
        idx += 1;
    }
    let level = idx - hash_start;
    if !(1..=6).contains(&level) {
        return None;
    }

    // ATX requires either EOL right after the hashes, or at least one
    // space/tab separating the hashes from the heading text.
    if idx == bytes.len() {
        return Some((level, String::new()));
    }
    if bytes[idx] != b' ' && bytes[idx] != b'\t' {
        return None;
    }

    let rest = line[idx..].trim();
    let stripped = strip_trailing_atx_hashes(rest);
    Some((level, String::from(stripped)))
}

/// Parse a `>`-separated heading path into prefix segments.
///
/// `"P3."` → `vec!["P3."]`; `"Activity > A10."` → `vec!["Activity", "A10."]`.
/// Leading, trailing, or doubled `>` separators are rejected as
/// malformed (they would imply an empty segment).
fn parse_path(path: &str) -> Result<Vec<String>> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        bail!("after_heading: path is empty");
    }
    let mut segments: Vec<String> = Vec::new();
    for raw in trimmed.split('>') {
        let segment = raw.trim();
        if segment.is_empty() {
            bail!("after_heading: empty segment in path {path:?}");
        }
        segments.push(String::from(segment));
    }
    Ok(segments)
}

/// Walk `markdown` and return every ATX heading outside the YAML
/// frontmatter and outside fenced code blocks, in document order.
fn scan_headings(markdown: &str) -> Vec<Heading> {
    let mut headings: Vec<Heading> = Vec::new();
    let mut in_frontmatter = false;
    let mut frontmatter_done = false;
    let mut fence: Option<FenceState> = None;

    for (idx, raw_line) in markdown.split('\n').enumerate() {
        let line_no = idx + 1;
        let trimmed_full = raw_line.trim();

        // YAML frontmatter handling: only at the very top of the file,
        // a line of exactly `---` opens the block; the next exact `---`
        // closes it. Anything inside is YAML, not markdown body.
        if !frontmatter_done {
            if idx == 0 && trimmed_full == "---" {
                in_frontmatter = true;
                continue;
            }
            if in_frontmatter {
                if trimmed_full == "---" {
                    in_frontmatter = false;
                    frontmatter_done = true;
                }
                continue;
            }
            // First non-frontmatter line locks frontmatter detection
            // off so a stray `---` deeper in the doc cannot reopen it.
            if !trimmed_full.is_empty() {
                frontmatter_done = true;
            }
        }

        // Fenced code block tracking. Match by exact tick count; a
        // closing fence must use at least as many ticks as the opener
        // and have no info string. Mirrors the rule in
        // [`crate::parser::scan_fences`] for the body walker.
        if let Some(state) = &fence {
            if let Some(close_ticks) = leading_backtick_count(raw_line)
                && close_ticks >= state.ticks
                && raw_line[close_ticks..].trim().is_empty()
            {
                fence = None;
            }
            continue;
        }
        if let Some(open_ticks) = leading_backtick_count(raw_line)
            && open_ticks >= 3
        {
            fence = Some(FenceState { ticks: open_ticks });
            continue;
        }

        if let Some((level, text)) = parse_atx_heading(raw_line) {
            headings.push(Heading {
                level,
                line: line_no,
                text,
            });
        }
    }
    headings
}

/// Strip a run of trailing `#` characters per `CommonMark` ATX rules:
/// the trailing `#`s must be preceded by at least one space (or be
/// the entire stripped portion). After stripping, surrounding
/// whitespace is trimmed.
fn strip_trailing_atx_hashes(text: &str) -> &str {
    let trimmed = text.trim_end();
    let mut end = trimmed.len();
    let bytes = trimmed.as_bytes();
    let mut hash_run: usize = 0;
    while end > 0 && bytes[end - 1] == b'#' {
        end -= 1;
        hash_run += 1;
    }
    if hash_run == 0 {
        return text.trim();
    }
    if end == 0 {
        // Heading text is just `#`s — keep them; nothing to strip.
        return text.trim();
    }
    if bytes[end - 1] == b' ' || bytes[end - 1] == b'\t' {
        return trimmed[..end].trim();
    }
    text.trim()
}

#[cfg(test)]
mod tests;
