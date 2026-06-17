//! Outbound link extraction for `remargin get`.
//!
//! Scans a document body for the links it points at — wikilinks, embeds,
//! markdown links/images, reference links, autolinks/bare URLs, anchors,
//! and the frontmatter `up:` / `related:` relations — and returns them
//! deduped by target. The scan runs over body text only: comment blocks,
//! checksums, and signatures live in `Segment::Comment` and are masked out
//! by the caller before this function sees the text, so they are excluded
//! for free. Links inside fenced or inline code spans are skipped here.
//!
//! Resolution is same-folder only: an internal target is resolved against
//! the document's own directory and nowhere else. A resolvable internal
//! link gets its on-disk `path` and the target document's own `title`; a
//! broken internal link (no same-folder file) is dropped entirely.
//! External URLs are dropped entirely: only local links are returned.

#[cfg(test)]
mod tests;

use std::path::Path;

use os_shim::System;
use serde::{Deserialize, Serialize};

use tixschema::model_schema;

use crate::frontmatter;

/// Frontmatter properties recognized as relations in v1.
///
/// FUTURE-CONFIGURABLE: this set is hardcoded for now. A follow-up makes
/// the recognized-property set a config knob (see rem-339p "Out of
/// scope"); until then `up` and `related` are the only frontmatter keys
/// whose values count as links.
const RELATION_PROPERTIES: &[&str] = &["up", "related"];

/// A single outbound link from a document, deduped by target.
///
/// One `Link` exists per distinct `target`; every occurrence of that
/// target in the scanned text contributes a [`LinkRef`] to `references`
/// and bumps `count`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct Link {
    /// Display text the link used, when it carried one
    /// (`[[X|the model]]` -> `"the model"`, `[text](url)` -> `"text"`).
    /// Omitted when the link had no distinct display text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Number of occurrences (`references.len()`).
    pub count: usize,
    /// Same-folder resolved file. Always present: only locally-resolving
    /// links are returned (external URLs are dropped).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Every occurrence of this target (dedup detail).
    pub references: Vec<LinkRef>,
    /// The link target: a note name / relative file for the local link.
    pub target: String,
    /// One-hop metadata: the target document's own title, when the link
    /// resolves to a readable same-folder document. Omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// A single occurrence of a [`Link`]'s target.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct LinkRef {
    /// 1-indexed line of this occurrence. Slice-relative when the `get`
    /// that produced it was sliced; whole-file-relative otherwise.
    pub line: usize,
}

/// A link occurrence before dedup + resolution.
struct RawLink {
    alias: Option<String>,
    /// `true` when the target is an external URL (dropped: only
    /// locally-resolving links are returned).
    external: bool,
    line: usize,
    target: String,
}

/// A resolved same-folder target.
struct Resolved {
    path: String,
    title: Option<String>,
}

/// Extract every outbound link from `body`, deduped by target.
///
/// `body` is the already-masked scan text: comment blocks must have been
/// blanked out by the caller (newline-preserving) so line numbers stay
/// aligned with what the caller read. Internal targets resolve against
/// `base_dir` (the document's own folder) only; resolvable links get a
/// `path` and the target document's `title`, broken internal links are
/// dropped, and external URLs are dropped entirely (local links only).
///
/// Lines are 1-indexed relative to the start of `body` — slice-relative
/// when the caller passed a slice, whole-file-relative otherwise.
#[must_use]
pub fn extract_links(body: &str, base_dir: &Path, system: &dyn System) -> Vec<Link> {
    let mut raw: Vec<RawLink> = Vec::new();

    collect_frontmatter_links(body, &mut raw);
    collect_body_links(body, &mut raw);

    dedup_and_resolve(raw, base_dir, system)
}

/// Collapse raw occurrences into deduped [`Link`] entries, resolving
/// internal targets same-folder only and dropping broken internal links
/// and every external URL (local links only).
fn dedup_and_resolve(raw: Vec<RawLink>, base_dir: &Path, system: &dyn System) -> Vec<Link> {
    let mut out: Vec<Link> = Vec::new();

    for occurrence in raw {
        // Local-only: drop external URLs before the dedup-append branch so
        // an external target never creates an entry a later occurrence
        // could append to.
        if occurrence.external {
            continue;
        }

        if let Some(existing) = out.iter_mut().find(|link| link.target == occurrence.target) {
            existing.references.push(LinkRef {
                line: occurrence.line,
            });
            existing.count = existing.references.len();
            if existing.alias.is_none() {
                existing.alias = occurrence.alias;
            }
            continue;
        }

        let (path, title) = match resolve_internal(&occurrence.target, base_dir, system) {
            // Broken internal link: drop the occurrence entirely.
            None => continue,
            Some(resolved) => (Some(resolved.path), resolved.title),
        };

        out.push(Link {
            alias: occurrence.alias,
            count: 1,
            path,
            references: vec![LinkRef {
                line: occurrence.line,
            }],
            target: occurrence.target,
            title,
        });
    }

    out
}

/// Resolve `target` against `base_dir` only. Returns `None` when no
/// same-folder file backs the link (broken internal link).
///
/// Targets carry no `#heading` / `^block` suffix at this point (stripped
/// upstream). A bare note name with no extension resolves to `<name>.md`;
/// a name that already carries an extension (an embed like `img.png`) is
/// taken verbatim. An anchor-only target (`#heading`, empty after the
/// strip) is a self-reference and resolves to the document's own folder
/// marker, which has no file — so it is dropped.
fn resolve_internal(target: &str, base_dir: &Path, system: &dyn System) -> Option<Resolved> {
    if target.is_empty() {
        return None;
    }

    let has_extension = Path::new(target).extension().is_some();
    let relative = if has_extension {
        target.to_owned()
    } else {
        format!("{target}.md")
    };

    let candidate = base_dir.join(&relative);
    if !system.exists(&candidate).unwrap_or(false) {
        return None;
    }

    let title = read_target_title(system, &candidate);
    Some(Resolved {
        path: relative,
        title,
    })
}

/// Read the target document's own title: prefer the frontmatter `title:`
/// field, fall back to the first `# heading`. Returns `None` for
/// non-markdown targets or when neither is present.
fn read_target_title(system: &dyn System, path: &Path) -> Option<String> {
    let content = system.read_to_string(path).ok()?;

    if let Some(title) = frontmatter_title(&content) {
        return Some(title);
    }

    let body = strip_frontmatter(&content);
    frontmatter::extract_title_from_heading(body)
}

/// Pull the `title:` value out of a document's YAML frontmatter, when
/// present and non-empty.
fn frontmatter_title(content: &str) -> Option<String> {
    let body = content.trim_start();
    if !body.starts_with("---") {
        return None;
    }
    let lines: Vec<&str> = content.split('\n').collect();
    let opener = lines.iter().position(|line| line.trim() == "---")?;
    let closer = lines
        .iter()
        .enumerate()
        .skip(opener + 1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(i, _)| i)?;
    let yaml_str: String = lines[opener + 1..closer].join("\n");
    let value: serde_yaml::Value = serde_yaml::from_str(&yaml_str).ok()?;
    let title = value.get("title")?.as_str()?.trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_owned())
    }
}

/// Strip a leading YAML frontmatter block, returning the remaining body.
fn strip_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }
    // Find the closing `---` line and return everything after it.
    let mut seen_open = false;
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        let stripped = line.strip_suffix('\n').unwrap_or(line);
        if stripped.trim() == "---" {
            if seen_open {
                return &content[offset + line.len()..];
            }
            seen_open = true;
        }
        offset += line.len();
    }
    content
}

/// Collect frontmatter relation links (`up:` / `related:`).
///
/// Scalar (`up: Some Note`) and sequence (`related: [A, B]` or a YAML
/// block list) forms are both supported. Each value is recorded at the
/// 1-indexed line of the property key, treated as an internal link.
/// Wikilink wrapping (`up: "[[Note]]"`) is unwrapped to the bare target.
fn collect_frontmatter_links(body: &str, raw: &mut Vec<RawLink>) {
    let trimmed = body.trim_start();
    if !trimmed.starts_with("---") {
        return;
    }
    let lines: Vec<&str> = body.split('\n').collect();
    let Some(opener) = lines.iter().position(|line| line.trim() == "---") else {
        return;
    };
    let Some(closer) = lines
        .iter()
        .enumerate()
        .skip(opener + 1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(i, _)| i)
    else {
        return;
    };

    let mut idx = opener + 1;
    while idx < closer {
        let line = lines[idx];
        let Some((raw_key, rest)) = line.split_once(':') else {
            idx += 1;
            continue;
        };
        let key = raw_key.trim();
        if !RELATION_PROPERTIES.contains(&key) {
            idx += 1;
            continue;
        }

        let key_line = idx + 1;
        let value = rest.trim();

        if value.is_empty() {
            // Block sequence: subsequent `- item` lines until dedent.
            idx += 1;
            while idx < closer {
                let item_line = lines[idx];
                let item = item_line.trim();
                let Some(entry) = item.strip_prefix("- ") else {
                    break;
                };
                push_relation(entry.trim(), key_line, raw);
                idx += 1;
            }
            continue;
        }

        if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
            // Inline flow sequence: `[A, B, C]`.
            for entry in inner.split(',') {
                push_relation(entry.trim(), key_line, raw);
            }
        } else {
            push_relation(value, key_line, raw);
        }
        idx += 1;
    }
}

/// Record one frontmatter relation value as an internal link, unwrapping
/// surrounding quotes and `[[...]]` wikilink syntax.
fn push_relation(value: &str, line: usize, raw: &mut Vec<RawLink>) {
    let unquoted = value.trim_matches('"').trim_matches('\'').trim();
    let bare = unquoted
        .strip_prefix("[[")
        .and_then(|v| v.strip_suffix("]]"))
        .unwrap_or(unquoted)
        .trim();
    if bare.is_empty() {
        return;
    }
    // Drop any alias / heading / block suffix; relations point at a note.
    let target = strip_target_suffix(bare);
    if target.is_empty() {
        return;
    }
    raw.push(RawLink {
        alias: None,
        external: false,
        line,
        target,
    });
}

/// Scan the body (post-frontmatter) for inline links, skipping fenced and
/// inline code spans.
fn collect_body_links(body: &str, raw: &mut Vec<RawLink>) {
    let mut in_fence = false;
    let mut fence_ticks = 0_usize;
    let mut ref_definitions: Vec<(String, String)> = Vec::new();
    let mut pending_refs: Vec<(String, usize, Option<String>)> = Vec::new();

    for (idx, line) in body.split('\n').enumerate() {
        let line_no = idx + 1;
        let trimmed = line.trim_start();

        // Fenced code spans: a line opening/closing a ``` (or longer) fence.
        if let Some(ticks) = fence_marker(trimmed) {
            if in_fence {
                if ticks == fence_ticks {
                    in_fence = false;
                    fence_ticks = 0;
                }
            } else {
                in_fence = true;
                fence_ticks = ticks;
            }
            continue;
        }
        if in_fence {
            continue;
        }

        // Reference definition: `[ref]: url`.
        if let Some((label, url)) = parse_ref_definition(line) {
            ref_definitions.push((label.to_ascii_lowercase(), url));
            continue;
        }

        scan_line(line, line_no, raw, &mut pending_refs);
    }

    // Resolve `[text][ref]` against collected definitions; a ref with no
    // definition is dropped.
    for (label, line, alias) in pending_refs {
        if let Some((_, url)) = ref_definitions
            .iter()
            .find(|(def_label, _)| *def_label == label)
        {
            push_target(url, line, alias, raw);
        }
    }
}

/// Number of opening backticks if `trimmed` is a fence marker line
/// (3+ backticks followed only by an optional info string), else `None`.
fn fence_marker(trimmed: &str) -> Option<usize> {
    let ticks = trimmed.chars().take_while(|&c| c == '`').count();
    (ticks >= 3).then_some(ticks)
}

/// Parse a reference-link definition line `[label]: url`. Returns the
/// label and URL when the line matches.
fn parse_ref_definition(line: &str) -> Option<(&str, String)> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix('[')?;
    let close = rest.find("]:")?;
    let label = &rest[..close];
    if label.is_empty() {
        return None;
    }
    let url = rest[close + 2..].trim();
    if url.is_empty() {
        return None;
    }
    Some((label, url.to_owned()))
}

/// Scan a single (non-fence, non-ref-definition) body line for link
/// occurrences, masking inline code spans so links inside backticks are
/// not detected. Pending `[text][ref]` occurrences are appended to
/// `pending_refs` for a second pass.
fn scan_line(
    line: &str,
    line_no: usize,
    raw: &mut Vec<RawLink>,
    pending_refs: &mut Vec<(String, usize, Option<String>)>,
) {
    let masked = mask_inline_code(line);
    let chars: Vec<char> = masked.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if wikilink_open(&chars, i)
            && let Some(next) = scan_wikilink(&chars, i, line_no, raw)
        {
            i = next;
            continue;
        }
        if chars[i] == '['
            && let Some(next) = scan_md_link(&chars, i, line_no, raw, pending_refs)
        {
            i = next;
            continue;
        }
        if chars[i] == '<'
            && let Some(next) = scan_autolink(&chars, i, line_no, raw)
        {
            i = next;
            continue;
        }
        if is_bare_url_start(&chars, i)
            && let Some(next) = scan_bare_url(&chars, i, line_no, raw)
        {
            i = next;
            continue;
        }
        i += 1;
    }
}

/// True when position `i` opens a wikilink (`[[`), accounting for an
/// optional embed `!` immediately before.
fn wikilink_open(chars: &[char], i: usize) -> bool {
    chars.get(i) == Some(&'[') && chars.get(i + 1) == Some(&'[')
}

/// Scan a wikilink / embed starting at `i` (`[[...]]`, optionally embed
/// `![[...]]`). Returns the index just past the closing `]]`.
fn scan_wikilink(
    chars: &[char],
    i: usize,
    line_no: usize,
    raw: &mut Vec<RawLink>,
) -> Option<usize> {
    let inner_start = i + 2;
    let close = find_subslice(chars, inner_start, &[']', ']'])?;
    let inner: String = chars[inner_start..close].iter().collect();
    let end = close + 2;

    // `[[target|alias]]` -> alias; `[[target#h]]` / `[[target^b]]` ->
    // heading/block suffix stripped from the resolution target.
    let (target_part, alias) = match inner.split_once('|') {
        Some((t, a)) => (t.trim().to_owned(), Some(a.trim().to_owned())),
        None => (inner.trim().to_owned(), None),
    };
    let target = strip_target_suffix(&target_part);
    if target.is_empty() {
        // Pure self-anchor (`[[#heading]]`): no document target. Skip.
        return Some(end);
    }

    raw.push(RawLink {
        alias: alias.filter(|a| !a.is_empty()),
        external: false,
        line: line_no,
        target,
    });
    Some(end)
}

/// Scan a markdown link/image `[text](target)` / `![alt](src)` or a
/// reference link `[text][ref]` starting at `i`. Returns the index just
/// past the construct, or `None` when `i` does not open one.
fn scan_md_link(
    chars: &[char],
    i: usize,
    line_no: usize,
    raw: &mut Vec<RawLink>,
    pending_refs: &mut Vec<(String, usize, Option<String>)>,
) -> Option<usize> {
    let text_close = find_subslice(chars, i + 1, &[']'])?;
    let text: String = chars[i + 1..text_close].iter().collect();
    let after = text_close + 1;

    match chars.get(after) {
        Some('(') => {
            let target_close = find_subslice(chars, after + 1, &[')'])?;
            let raw_target: String = chars[after + 1..target_close].iter().collect();
            let target = clean_md_target(&raw_target);
            let alias = (!text.trim().is_empty()).then(|| text.trim().to_owned());
            push_target(&target, line_no, alias, raw);
            Some(target_close + 1)
        }
        Some('[') => {
            let ref_close = find_subslice(chars, after + 1, &[']'])?;
            let raw_label: String = chars[after + 1..ref_close].iter().collect();
            let label = if raw_label.trim().is_empty() {
                text.trim().to_owned()
            } else {
                raw_label.trim().to_owned()
            };
            let alias = (!text.trim().is_empty()).then(|| text.trim().to_owned());
            pending_refs.push((label.to_ascii_lowercase(), line_no, alias));
            Some(ref_close + 1)
        }
        _ => None,
    }
}

/// Scan an autolink `<https://…>` starting at `i`.
fn scan_autolink(
    chars: &[char],
    i: usize,
    line_no: usize,
    raw: &mut Vec<RawLink>,
) -> Option<usize> {
    let close = find_subslice(chars, i + 1, &['>'])?;
    let inner: String = chars[i + 1..close].iter().collect();
    let trimmed = inner.trim();
    if !is_url(trimmed) {
        return None;
    }
    raw.push(RawLink {
        alias: None,
        external: true,
        line: line_no,
        target: trimmed.to_owned(),
    });
    Some(close + 1)
}

/// True when a bare URL (`http://` / `https://`) starts at `i` and is at a
/// word boundary (preceded by start-of-line or whitespace).
fn is_bare_url_start(chars: &[char], i: usize) -> bool {
    if i > 0 && !chars[i - 1].is_whitespace() {
        return false;
    }
    let rest: String = chars[i..].iter().collect();
    rest.starts_with("http://") || rest.starts_with("https://")
}

/// Scan a bare URL starting at `i`, consuming until whitespace or a
/// closing bracket. Trailing punctuation is trimmed.
fn scan_bare_url(
    chars: &[char],
    i: usize,
    line_no: usize,
    raw: &mut Vec<RawLink>,
) -> Option<usize> {
    let mut end = i;
    while end < chars.len() && !chars[end].is_whitespace() && !matches!(chars[end], ')' | '>' | ']')
    {
        end += 1;
    }
    let collected: String = chars[i..end].iter().collect();
    let url = collected.trim_end_matches(['.', ',', ';', ':', '!', '?']);
    if url.is_empty() {
        return None;
    }
    raw.push(RawLink {
        alias: None,
        external: true,
        line: line_no,
        target: url.to_owned(),
    });
    Some(end)
}

/// Push a markdown-link target, classifying internal vs external and
/// dropping pure in-document anchors (`#heading`).
fn push_target(raw_target: &str, line: usize, alias: Option<String>, raw: &mut Vec<RawLink>) {
    let target = raw_target.trim();
    if target.is_empty() || target.starts_with('#') {
        // Pure self-anchor: not an outbound document/URL link.
        return;
    }
    if is_url(target) {
        raw.push(RawLink {
            alias,
            external: true,
            line,
            target: target.to_owned(),
        });
        return;
    }
    let internal = strip_target_suffix(target);
    if internal.is_empty() {
        return;
    }
    raw.push(RawLink {
        alias,
        external: false,
        line,
        target: internal,
    });
}

/// Strip an Obsidian `#heading` / `^block` suffix and percent-decoding
/// noise from an internal target, returning the bare note/file name.
fn strip_target_suffix(target: &str) -> String {
    let no_heading = target.split('#').next().unwrap_or(target);
    let no_block = no_heading.split('^').next().unwrap_or(no_heading);
    no_block.trim().to_owned()
}

/// Normalize a markdown-link target: drop a `"title"` suffix and angle
/// brackets, then percent-decode spaces.
fn clean_md_target(raw_target: &str) -> String {
    let trimmed = raw_target.trim();
    // `[t](url "title")` — drop the quoted title.
    let without_title = trimmed.split_once(" \"").map_or(trimmed, |(url, _)| url);
    without_title
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .replace("%20", " ")
}

/// True when `s` looks like an external URL (has a scheme).
fn is_url(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("mailto:")
        || s.starts_with("ftp://")
}

/// Replace inline code spans (backtick-delimited) with spaces of equal
/// length so links inside them are not detected while column offsets are
/// preserved. Unterminated backticks are left as-is.
fn mask_inline_code(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<char> = chars.clone();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '`' {
            let ticks = chars[i..].iter().take_while(|&&c| c == '`').count();
            let span_start = i;
            let content_start = i + ticks;
            if let Some(close) = find_closing_ticks(&chars, content_start, ticks) {
                for slot in out.iter_mut().take(close + ticks).skip(span_start) {
                    *slot = ' ';
                }
                i = close + ticks;
                continue;
            }
        }
        i += 1;
    }
    out.into_iter().collect()
}

/// Find the start index of a run of exactly `ticks` backticks at or after
/// `from`, closing an inline code span.
fn find_closing_ticks(chars: &[char], from: usize, ticks: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == '`' {
            let run = chars[i..].iter().take_while(|&&c| c == '`').count();
            if run == ticks {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

/// Find the start index of the first occurrence of `needle` in
/// `chars[from..]`, returning the absolute index.
fn find_subslice(chars: &[char], from: usize, needle: &[char]) -> Option<usize> {
    if needle.is_empty() || from >= chars.len() {
        return None;
    }
    let mut i = from;
    while i + needle.len() <= chars.len() {
        if chars[i..i + needle.len()] == *needle {
            return Some(i);
        }
        i += 1;
    }
    None
}
