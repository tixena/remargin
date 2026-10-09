//! Advisory review of authored content.
//!
//! The counterpart to [`crate::linter`], and deliberately its opposite in
//! force. Lint is a gate: [`crate::linter::lint_or_fail`] runs before and
//! after every write and rejects the operation. Advice never rejects
//! anything. Its findings ride back on a *successful* result so the author
//! sees the note and decides for themselves; nothing here changes what gets
//! written, and no caller may promote a finding into a failure.
//!
//! That split is the whole point: remargin has an opinion about readable
//! markdown, but the person writing the file has the last word.

#[cfg(test)]
mod tests;

use core::fmt::Write as _;

use serde::Serialize;
use serde_json::{Value, json};

/// One advisory note about authored content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Advice {
    /// 1-indexed.
    pub line: usize,
    pub message: String,
}

/// One advisory note about a batch, tagged with the operation that earned
/// it.
///
/// `op` is the zero-based index into the request's operations array — the
/// one handle the caller holds for every operation, including the ones
/// that produced no id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct OpAdvice {
    pub note: Advice,
    pub op: usize,
}

impl OpAdvice {
    #[must_use]
    pub const fn new(op: usize, note: Advice) -> Self {
        Self { note, op }
    }
}

/// One run of consecutive lines that are the author's own prose.
///
/// Fenced code, blockquotes, tables, lists and headings are not prose and
/// never appear here, so a check built on these blocks cannot fire on
/// something the author merely quoted or demonstrated.
#[derive(Debug)]
pub(crate) struct ProseBlock<'text> {
    /// 1-indexed line of the block's first line.
    pub line: usize,
    pub lines: Vec<&'text str>,
}

/// Review authored markdown and return any advisory notes, in line order.
///
/// Returns an empty vector for content that reads cleanly -- the common
/// case, and the one callers should treat as "say nothing".
#[must_use]
pub fn review(content: &str) -> Vec<Advice> {
    let mut notes = Vec::new();
    check_hard_wrapped_paragraphs(content, &mut notes);
    notes
}

/// Shift every note down by `offset` lines.
///
/// A partial write reviews only the caller's fragment, so its notes start
/// at line 1 of that fragment. Offsetting by `start_line - 1` makes them
/// point at the real lines of the file the caller is editing.
pub fn offset_lines(notes: &mut [Advice], offset: usize) {
    for note in notes {
        note.line += offset;
    }
}

/// The notes as the canonical `warnings` JSON array.
///
/// Built by hand rather than through `serde_json::to_value` so the payload
/// path carries no fallible conversion.
#[must_use]
pub fn to_json(notes: &[Advice]) -> Value {
    Value::Array(
        notes
            .iter()
            .map(|note| json!({ "line": note.line, "message": note.message }))
            .collect(),
    )
}

/// The op-scoped notes as the `warnings` JSON array: the `line`/`message`
/// pair a single-op surface emits, plus the `op` that earned it.
fn op_to_json(notes: &[OpAdvice]) -> Value {
    Value::Array(
        notes
            .iter()
            .map(|entry| {
                json!({
                    "op": entry.op,
                    "line": entry.note.line,
                    "message": entry.note.message,
                })
            })
            .collect(),
    )
}

/// Attach already-assembled notes to a successful result under `warnings`,
/// leaving the result untouched when there is nothing to say.
///
/// Advisory only: no caller may read the added field as a failure signal.
pub fn attach_notes(result: &mut Value, notes: &[Advice]) {
    if !notes.is_empty() {
        result["warnings"] = to_json(notes);
    }
}

/// [`attach_notes`] for a multi-operation result, whose notes each name
/// the operation they came from.
pub fn attach_op_notes(result: &mut Value, notes: &[OpAdvice]) {
    if !notes.is_empty() {
        result["warnings"] = op_to_json(notes);
    }
}

/// Render notes as the human-facing text block, one note per line.
///
/// Empty string when there is nothing to say, so callers can print it
/// unconditionally without emitting a stray blank line.
#[must_use]
pub fn format_text(notes: &[Advice]) -> String {
    let mut buf = String::new();
    for note in notes {
        let _ = writeln!(buf, "advice: line {}: {}", note.line, note.message);
    }
    buf
}

/// [`format_text`] for op-scoped notes, which lead with the operation.
#[must_use]
pub fn format_op_text(notes: &[OpAdvice]) -> String {
    let mut buf = String::new();
    for entry in notes {
        let _ = writeln!(
            buf,
            "advice: op {}, line {}: {}",
            entry.op, entry.note.line, entry.note.message
        );
    }
    buf
}

/// Flag paragraphs whose text is split across several lines.
///
/// In continuous-prose markdown a paragraph occupies exactly one line, so a
/// run of two or more prose lines is a hard wrap the author put there by
/// hand. Everything whose newlines are genuine content is skipped rather
/// than flagged: frontmatter, fenced code (which includes remargin comment
/// blocks, themselves ```` ```remargin ```` fences), headings, tables,
/// blockquotes, list items, HTML, indented blocks, and any line ending in
/// an explicit markdown hard break.
fn check_hard_wrapped_paragraphs(content: &str, notes: &mut Vec<Advice>) {
    for block in prose_blocks(content) {
        let span = block.lines.len();
        if span > 1 {
            notes.push(Advice {
                line: block.line,
                message: format!(
                    "this paragraph is hard-wrapped across {span} lines; write it as one continuous line and let the reader's viewer wrap it"
                ),
            });
        }
    }
}

/// The hard-wrap notes on their own.
///
/// [`review`] folds them in with everything else it accumulates; a caller
/// that must react to this one check — the comment gate promotes it to a
/// rejection — needs it isolated so a later addition to `review` cannot
/// silently change what gets rejected.
pub(crate) fn hard_wrapped_paragraphs(content: &str) -> Vec<Advice> {
    let mut notes = Vec::new();
    check_hard_wrapped_paragraphs(content, &mut notes);
    notes
}

/// Segment `content` into the runs of prose the author wrote themselves.
pub(crate) fn prose_blocks(content: &str) -> Vec<ProseBlock<'_>> {
    let lines: Vec<&str> = content.split('\n').collect();
    let mut idx = skip_frontmatter(&lines);
    let mut blocks = Vec::new();

    while idx < lines.len() {
        if let Some(fence) = fence_run(lines[idx]) {
            idx = skip_fence(&lines, idx, fence);
            continue;
        }
        if !is_prose(lines[idx]) {
            idx += 1;
            continue;
        }

        let start = idx;
        let mut end = idx;
        while end + 1 < lines.len()
            && is_prose(lines[end + 1])
            && fence_run(lines[end + 1]).is_none()
            && !ends_with_hard_break(lines[end])
        {
            end += 1;
        }

        blocks.push(ProseBlock {
            line: start + 1,
            lines: lines[start..=end].to_vec(),
        });
        idx = end + 1;
    }

    blocks
}

/// The first line past a leading `---` frontmatter block.
///
/// An unterminated opener means the whole payload is frontmatter as far as
/// this check is concerned: YAML keys look exactly like wrapped prose, so
/// scanning them would flag every document with metadata.
fn skip_frontmatter(lines: &[&str]) -> usize {
    if lines.first().map(|line| line.trim_end()) != Some("---") {
        return 0;
    }
    lines
        .iter()
        .position(|line| line.trim_end() == "---")
        .filter(|pos| *pos > 0)
        .map_or(lines.len(), |pos| pos + 1)
}

/// The fence character and its run length when `line` opens a code fence.
fn fence_run(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let run = trimmed.chars().take_while(|c| *c == marker).count();
    (run >= 3).then_some((marker, run))
}

/// The first line past the fence opened at `opener`, or the end of input
/// when the fence is never closed.
fn skip_fence(lines: &[&str], opener: usize, fence: (char, usize)) -> usize {
    let (marker, run) = fence;
    for (idx, line) in lines.iter().enumerate().skip(opener + 1) {
        let trimmed = line.trim_start().trim_end();
        let closing = trimmed.chars().take_while(|c| *c == marker).count();
        if closing >= run && trimmed.chars().all(|c| c == marker) {
            return idx + 1;
        }
    }
    lines.len()
}

/// Whether `line` is ordinary paragraph text that should flow continuously.
///
/// Conservative on purpose: only unindented, structurally-plain lines
/// count. Indented text (code blocks, list continuations, nested blocks)
/// carries meaningful newlines, and a false note costs more than a missed
/// one for advice nobody is obliged to follow.
fn is_prose(line: &str) -> bool {
    let text = line.trim_end_matches('\r');
    if text.trim().is_empty() {
        return false;
    }
    if text.starts_with(' ') || text.starts_with('\t') {
        return false;
    }
    !is_structural(text)
}

/// Whether `line` opens a markdown construct that owns its own line breaks.
fn is_structural(line: &str) -> bool {
    if line.starts_with('#')
        || line.starts_with('>')
        || line.starts_with('|')
        || line.starts_with('<')
    {
        return true;
    }
    is_list_item(line) || is_rule_or_setext(line)
}

/// Whether `line` starts a bullet (`-`, `*`, `+`) or ordered (`1.`, `1)`)
/// list item.
fn is_list_item(line: &str) -> bool {
    let mut chars = line.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if matches!(first, '-' | '*' | '+') {
        return chars.next() == Some(' ');
    }
    if !first.is_ascii_digit() {
        return false;
    }
    let mut tail = line
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .chars();
    matches!(tail.next(), Some('.' | ')')) && tail.next() == Some(' ')
}

/// Whether `line` is a thematic break (`---`, `***`, `___`) or a setext
/// heading underline (`===`, `---`), all of which are single-purpose lines.
fn is_rule_or_setext(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.len() < 3 {
        return false;
    }
    let Some(marker) = trimmed.chars().next() else {
        return false;
    };
    matches!(marker, '-' | '*' | '_' | '=') && trimmed.chars().all(|c| c == marker)
}

/// Whether `line` ends in an explicit markdown hard break -- two trailing
/// spaces or a trailing backslash. The author asked for that newline, so it
/// is not a wrap.
fn ends_with_hard_break(line: &str) -> bool {
    let text = line.trim_end_matches('\r');
    text.ends_with("  ") || text.ends_with('\\')
}
