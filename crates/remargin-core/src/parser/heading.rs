//! Heading-anchored insertion: resolve `--after-heading` paths to a
//! 1-indexed line number in a parsed document.
//!
//! Callers (CLI + MCP `comment` and `batch`) want to say "put this
//! comment after `### P3.`" without having to track line numbers. This
//! module walks the document's markdown looking for ATX headings and
//! returns the line of the first heading whose text matches the
//! requested path.
//!
//! ## Match rules (rem-5oqx)
//!
//! - Only ATX headings (`#` … `######`) outside fenced code blocks and
//!   outside the YAML frontmatter participate in the walk. Setext
//!   underline headings are NOT supported in v1.
//! - Match is a case-sensitive prefix comparison on the heading's
//!   stripped text. Trailing `#` characters and surrounding whitespace
//!   are removed before comparing, per the `CommonMark` ATX spec.
//! - Paths use `>` as the segment separator. Each segment is a prefix
//!   match against a heading. Segments after the first must be at a
//!   STRICTLY DEEPER level than the previous segment, and must appear
//!   before the next heading at the previous segment's level (i.e.
//!   within the parent's section). Levels can skip — `# A > ### C`
//!   resolves so long as no `# A` sibling closes the section first.
//! - First match wins. Ambiguity is resolved by adding more path
//!   segments, not by an error.
//!
//! ## Storage
//!
//! The resolver is a lookup convenience. It returns the heading's
//! 1-indexed line; the caller forwards that to the existing
//! [`InsertPosition::AfterLine`](crate::writer::InsertPosition::AfterLine)
//! pipeline, so the comment's stored `line` field is unchanged.

use anyhow::{Result, bail};

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
    let markdown = doc.to_markdown();
    let headings = scan_headings(&markdown);
    match_path(&headings, &segments).ok_or_else(|| anyhow::anyhow!("no heading matched {path:?}"))
}

/// Count the leading backtick run at the start of `line`. Returns
/// `Some(count)` when `line` starts with one or more `` ` ``, `None`
/// otherwise. ATX headings can contain backticks but never start with
/// them, so this is a sound discriminator for fence-opener lines.
fn leading_backtick_count(line: &str) -> Option<usize> {
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
mod tests {
    use super::*;
    use crate::parser::parse;

    fn doc(markdown: &str) -> ParsedDocument {
        parse(markdown).unwrap()
    }

    #[test]
    fn empty_path_errors() {
        let md = "# A\n";
        let parsed = doc(md);
        resolve_heading_path(&parsed, "").unwrap_err();
        resolve_heading_path(&parsed, "   ").unwrap_err();
    }

    #[test]
    fn first_match_wins_at_same_path() {
        let md = "\
## Section\n\
### Item\n\
\n\
## Other\n\
### Item\n\
";
        let parsed = doc(md);
        // Bare path returns the first ### Item in document order.
        assert_eq!(resolve_heading_path(&parsed, "Item").unwrap(), 2);
        assert_eq!(resolve_heading_path(&parsed, "Other > Item").unwrap(), 5);
    }

    #[test]
    fn full_text_also_matches_via_prefix() {
        let md = "### P3. `deny_ops` (operation-level)\n";
        let parsed = doc(md);
        assert_eq!(
            resolve_heading_path(&parsed, "P3. `deny_ops` (operation-level)").unwrap(),
            1
        );
    }

    #[test]
    fn malformed_path_separators_error() {
        let md = "# A\n";
        let parsed = doc(md);
        resolve_heading_path(&parsed, "> A").unwrap_err();
        resolve_heading_path(&parsed, "A >").unwrap_err();
        resolve_heading_path(&parsed, "A > > B").unwrap_err();
    }

    #[test]
    fn missing_heading_errors() {
        let md = "# Title\n";
        let parsed = doc(md);
        let err = resolve_heading_path(&parsed, "Z9.").unwrap_err();
        assert!(format!("{err:#}").contains("Z9."));
    }

    #[test]
    fn path_can_skip_levels() {
        let md = "# A\n\n### C\n";
        let parsed = doc(md);
        assert_eq!(resolve_heading_path(&parsed, "A > C").unwrap(), 3);
    }

    #[test]
    fn path_disambiguates_duplicate_headings() {
        let md = "\
## Activity epic tests\n\
### A10. MCP / CLI parity\n\
\n\
## Permissions epic tests\n\
### P11. MCP / CLI parity\n\
";
        let parsed = doc(md);
        let line = resolve_heading_path(&parsed, "Activity epic tests > A10.").unwrap();
        assert_eq!(line, 2);
    }

    #[test]
    fn resolves_simple_prefix_match() {
        let md = "# Title\n\n### P3. deny_ops\n\nbody\n";
        let parsed = doc(md);
        assert_eq!(resolve_heading_path(&parsed, "P3.").unwrap(), 3);
    }

    #[test]
    fn same_level_child_terminates_parent_section() {
        let md = "## A\n\n## B\n";
        let parsed = doc(md);
        let err = resolve_heading_path(&parsed, "A > B").unwrap_err();
        assert!(format!("{err:#}").contains("A > B"));
    }

    #[test]
    fn skips_headings_inside_code_fences() {
        let md = "\
### Real\n\
\n\
```text\n\
### Fake\n\
```\n\
";
        let parsed = doc(md);
        assert_eq!(resolve_heading_path(&parsed, "Real").unwrap(), 1);
        let err = resolve_heading_path(&parsed, "Fake").unwrap_err();
        assert!(format!("{err:#}").contains("Fake"));
    }

    #[test]
    fn skips_yaml_frontmatter() {
        let md = "\
---\n\
title: doc\n\
remargin_kind: question\n\
---\n\
\n\
### P3. real heading\n\
";
        let parsed = doc(md);
        assert_eq!(resolve_heading_path(&parsed, "P3.").unwrap(), 6);
        let err = resolve_heading_path(&parsed, "remargin_kind").unwrap_err();
        assert!(format!("{err:#}").contains("remargin_kind"));
    }

    #[test]
    fn trailing_atx_hashes_are_stripped() {
        let md = "### Foo ###\n";
        let parsed = doc(md);
        assert_eq!(resolve_heading_path(&parsed, "Foo").unwrap(), 1);
    }
}
