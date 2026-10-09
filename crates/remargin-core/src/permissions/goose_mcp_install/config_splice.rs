//! Line-level editing of remargin's entry inside goose's `config.yaml`.
//!
//! Re-serializing the parsed document is far simpler, but `serde_yaml`
//! models neither comments nor quoting style, so a write reflows the
//! whole file: the notes a user left beside their provider and model,
//! the blank lines grouping the sections, and every quote they chose all
//! disappear on the one install that first adds the entry. This module
//! edits only the lines remargin's entry occupies and leaves the rest of
//! the bytes exactly where they were.
//!
//! The scanner follows block-mapping indentation and nothing else.
//! Anything outside that — a flow `extensions: {...}`, an alias, a root
//! that is not a block mapping — returns `None` so the caller falls back
//! to re-serializing. The caller also re-parses what this module
//! produces and compares it against the mapping it meant to write, so a
//! mislocated edit never reaches the file.

use core::ops::Range;

use serde_yaml::{Mapping, Value};

use super::{EXTENSION_KEY, EXTENSIONS_KEY, insert};

/// Characters a plain scalar key cannot open with: each opens something
/// else — a comment, a sequence item, an anchor, a flow collection.
const INDICATORS: [char; 13] = [
    '#', '-', '?', ':', '&', '*', '!', '|', '>', '%', '@', '`', ',',
];

/// Matches what `serde_yaml` emits, so a written block reads like the rest of the file.
const NEST_INDENT: usize = 2;

/// The change to make to remargin's entry.
pub(super) enum Edit<'entry> {
    Remove,
    Set(&'entry Value),
}

/// A block-mapping key line, split at the colon that ends its key.
struct KeyLine<'line> {
    key: &'line str,
    /// Everything after the colon, trimmed.
    value: &'line str,
    /// Byte offset just past the colon, within the whole line.
    value_at: usize,
}

/// The `extensions` block as the scan found it in the text.
struct Layout {
    /// Lines remargin's entry occupies, when the block declares one.
    entry: Option<Range<usize>>,
    /// Columns the block's entries are indented by.
    indent: usize,
    /// Where a new entry goes: one past the block's last content line.
    insert_at: usize,
    /// Line the `extensions` key sits on.
    key: usize,
    /// Entries the block declares.
    len: usize,
}

/// What the text says about the `extensions` block.
enum Placement {
    /// It declares none, so a new one goes at the end of the file.
    Absent,
    Declared(Layout),
}

impl KeyLine<'_> {
    /// `true` when the value is a nested block rather than something on
    /// this line — the only shape whose entries are lines to edit.
    fn declares_block(&self) -> bool {
        self.value.is_empty() || self.value.starts_with('#')
    }
}

/// `body` with `edit` applied to remargin's entry alone, or `None` when
/// the text is shaped in a way this scanner does not model.
pub(super) fn apply(body: &str, edit: Edit<'_>) -> Option<String> {
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    match (locate(&lines)?, edit) {
        (Placement::Absent, Edit::Remove) => None,
        (Placement::Absent, Edit::Set(entry)) => appended(&lines, entry),
        (Placement::Declared(layout), Edit::Remove) => removed(&lines, &layout),
        (Placement::Declared(layout), Edit::Set(entry)) => replaced(&lines, &layout, entry),
    }
}

/// A whole new block at the end of the file.
fn appended(lines: &[&str], entry: &Value) -> Option<String> {
    let mut edited = lines.concat();
    if !edited.is_empty() && !edited.ends_with('\n') {
        edited.push('\n');
    }
    edited.push_str(EXTENSIONS_KEY);
    edited.push_str(":\n");
    edited.push_str(&rendered(entry, NEST_INDENT)?);
    Some(edited)
}

/// The `extensions` block opened on line `key`, measured by the
/// indentation of the entries under it.
fn block(lines: &[&str], key: usize, root: usize) -> Option<Layout> {
    let mut indent = None;
    let mut starts: Vec<usize> = Vec::new();
    let mut found_entry = None;
    let mut end = key + 1;
    for (index, line) in lines.iter().enumerate().skip(key + 1) {
        let Some(column) = content_indent(line) else {
            continue;
        };
        if column <= root {
            break;
        }
        let child = *indent.get_or_insert(column);
        if column < child {
            return None;
        }
        end = index + 1;
        if column > child {
            continue;
        }
        let parsed = key_line(line)?;
        if parsed.key == EXTENSION_KEY {
            found_entry = Some(starts.len());
        }
        starts.push(index);
    }
    let entry = found_entry.map(|position| {
        let start = starts[position];
        start
            ..trimmed_end(
                lines,
                start,
                starts.get(position + 1).copied().unwrap_or(end),
            )
    });
    Some(Layout {
        entry,
        indent: indent.unwrap_or(root + NEST_INDENT),
        insert_at: end,
        key,
        len: starts.len(),
    })
}

/// The line's indentation in columns, or `None` when it carries no
/// structure at all — blank, or nothing but a comment.
fn content_indent(line: &str) -> Option<usize> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = line[indent..].trim_end();
    if rest.is_empty() || rest.starts_with('#') {
        return None;
    }
    Some(indent)
}

/// The `extensions:` line rewritten to declare an empty mapping. Needed
/// when uninstall removes the block's last entry: the bare key left
/// behind would parse as null, which is not the config the caller means
/// to write.
fn emptied(line: &str) -> Option<String> {
    let parsed = key_line(line)?;
    Some(format!(
        "{} {{}}{}",
        &line[..parsed.value_at],
        &line[parsed.value_at..]
    ))
}

fn key_line(line: &str) -> Option<KeyLine<'_>> {
    let indent = content_indent(line)?;
    let rest = line[indent..].trim_end();
    let (key, at) = split_key(rest)?;
    Some(KeyLine {
        key,
        value: rest[at..].trim(),
        value_at: indent + at,
    })
}

fn locate(lines: &[&str]) -> Option<Placement> {
    let Some(first) = lines.iter().position(|line| content_indent(line).is_some()) else {
        return Some(Placement::Absent);
    };
    let root = content_indent(lines[first])?;
    for (index, line) in lines.iter().enumerate().skip(first) {
        let Some(column) = content_indent(line) else {
            continue;
        };
        if column > root {
            continue;
        }
        // Nothing shallower than the document's own keys is a document
        // this scanner claims to understand.
        if column < root {
            return None;
        }
        let parsed = key_line(line)?;
        if parsed.key != EXTENSIONS_KEY {
            continue;
        }
        if !parsed.declares_block() {
            return None;
        }
        return Some(Placement::Declared(block(lines, index, root)?));
    }
    Some(Placement::Absent)
}

/// The key of a plain (unquoted) scalar and the offset past its colon.
/// The colon that ends the key is the one at the end of the line or
/// followed by whitespace: `http://x: 1` keys on `http://x`.
fn plain_key(rest: &str) -> Option<(&str, usize)> {
    let mut from = 0;
    loop {
        let colon = from + rest[from..].find(':')?;
        let next = rest[colon + 1..].chars().next();
        if matches!(next, None | Some(' ' | '\t')) {
            let key = rest[..colon].trim_end();
            if key.is_empty() || key.starts_with(INDICATORS) {
                return None;
            }
            return Some((key, colon + 1));
        }
        from = colon + 1;
    }
}

/// A quoted key and the offset past its colon. Escapes are not modelled:
/// a key carrying one leaves the caller its fallback.
fn quoted_key(rest: &str, quote: char) -> Option<(&str, usize)> {
    let close = rest[1..].find(quote)? + 1;
    let key = &rest[1..close];
    if key.contains('\\') {
        return None;
    }
    let after = close + 1;
    if !rest[after..].starts_with(':') {
        return None;
    }
    Some((key, after + 1))
}

/// remargin's entry as the lines it occupies at `indent` columns.
fn rendered(entry: &Value, indent: usize) -> Option<String> {
    let mut block = Mapping::new();
    insert(&mut block, EXTENSION_KEY, entry.clone());
    let text = serde_yaml::to_string(&Value::Mapping(block)).ok()?;
    let pad = " ".repeat(indent);
    let mut written = String::new();
    for line in text.lines() {
        if !line.is_empty() {
            written.push_str(&pad);
            written.push_str(line);
        }
        written.push('\n');
    }
    Some(written)
}

fn removed(lines: &[&str], layout: &Layout) -> Option<String> {
    let span = layout.entry.clone()?;
    let mut edited = String::new();
    for (index, line) in lines.iter().enumerate() {
        if span.contains(&index) {
            continue;
        }
        if index == layout.key && layout.len == 1 {
            edited.push_str(&emptied(line)?);
            continue;
        }
        edited.push_str(line);
    }
    Some(edited)
}

fn replaced(lines: &[&str], layout: &Layout, entry: &Value) -> Option<String> {
    let block = rendered(entry, layout.indent)?;
    let span = layout
        .entry
        .clone()
        .unwrap_or(layout.insert_at..layout.insert_at);
    let mut edited = lines[..span.start].concat();
    if !edited.is_empty() && !edited.ends_with('\n') {
        edited.push('\n');
    }
    edited.push_str(&block);
    edited.push_str(&lines[span.end..].concat());
    Some(edited)
}

fn split_key(rest: &str) -> Option<(&str, usize)> {
    if rest.starts_with('"') {
        return quoted_key(rest, '"');
    }
    if rest.starts_with('\'') {
        return quoted_key(rest, '\'');
    }
    plain_key(rest)
}

/// `end` backed up over the blank and comment lines a range ends with,
/// so an edit does not swallow what was written for whatever follows.
fn trimmed_end(lines: &[&str], start: usize, end: usize) -> usize {
    let mut last = end;
    while last > start && content_indent(lines[last - 1]).is_none() {
        last -= 1;
    }
    last
}
