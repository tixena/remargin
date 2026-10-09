//! YAML frontmatter management for `remargin_*` fields.
//!
//! User-owned fields (`title`, `description`, `created`) are
//! auto-populated on first save but never overwritten. Tool-managed fields
//! (`remargin_pending`, `remargin_pending_for`, `remargin_last_activity`)
//! are recomputed on every write operation. `author` is user-owned on the
//! shared [`ensure_frontmatter`] path but authenticated against the doc's
//! realm on the write/replace path ([`ensure_frontmatter_authored`]):
//! stamped from the caller identity on create, gated by mode on edit.
//!
//! The `sandbox` key is user-written volatile state: a list of
//! `author@timestamp` strings identifying participants who have staged the
//! file. It is preserved across writes but is not part of comment checksums
//! or signatures (those are computed per comment and do not include any
//! document-level frontmatter).

#[cfg(test)]
mod tests;

use anyhow::{Context as _, Result};
use chrono::{DateTime, FixedOffset, Utc};
use serde_yaml::{Mapping, Value};

use crate::config::{Mode, ResolvedConfig};
use crate::parser::{self, Comment, ParsedDocument, SandboxEntry, Segment, rfc3339_z};

/// Not prefixed with `remargin_`: that prefix is reserved for tool-managed fields, and `sandbox`
/// is user-written state.
pub const SANDBOX_KEY: &str = "sandbox";

const FRONTMATTER_DELIMITER: &str = "---";

/// Ensure a document has frontmatter. If missing, add it.
/// If present, merge (add missing user fields, recompute remargin fields).
///
/// # Errors
///
/// Returns an error if existing frontmatter YAML cannot be parsed.
pub fn ensure_frontmatter(doc: &mut ParsedDocument, config: &ResolvedConfig) -> Result<()> {
    let comments: Vec<&Comment> = doc.comments();
    let body = extract_body_text(doc);

    let mut mapping = parse_existing_frontmatter(doc)?;

    populate_user_fields(&mut mapping, &body, config);
    update_remargin_fields(&mut mapping, &comments);

    let yaml = Value::Mapping(mapping);
    write_frontmatter_to_doc(doc, &yaml);

    Ok(())
}

/// Authored-write normalization for the `write`/`replace` seams.
///
/// Also covers their plan/dry-run projections. Behaves identically to
/// [`ensure_frontmatter`] for every field except `author`, which is
/// authenticated against the realm the document lives in:
///
/// - `create`: the caller's supplied `author` is ignored and overwritten
///   with the authenticated caller identity (dropped entirely when the
///   caller is anonymous).
/// - edit: an unchanged or omitted `author` passes; a changed `author` is
///   gated by [`Mode`] (open allows anything; registered requires an
///   active participant; strict forbids changing an existing author and
///   only lets an authorless doc gain the caller's own identity).
///
/// `config` MUST already be escalated to the doc's realm so `mode` and
/// `registry` reflect the realm, not the caller.
///
/// # Errors
///
/// Returns an error if existing frontmatter YAML cannot be parsed, or if
/// the author change is rejected by the realm's mode gate.
pub fn ensure_frontmatter_authored(
    doc: &mut ParsedDocument,
    config: &ResolvedConfig,
    create: bool,
    old_author: Option<&str>,
) -> Result<()> {
    let comments: Vec<&Comment> = doc.comments();
    let body = extract_body_text(doc);

    let mut mapping = parse_existing_frontmatter(doc)?;

    // Judge `author` as written, before the generic fill can back-fill it from the caller
    // identity.
    authenticate_author(&mut mapping, config, create, old_author)?;
    populate_non_author_fields(&mut mapping, &body);
    update_remargin_fields(&mut mapping, &comments);

    let yaml = Value::Mapping(mapping);
    write_frontmatter_to_doc(doc, &yaml);

    Ok(())
}

/// Read the document-level `author` frontmatter value, if any.
///
/// Returns `None` when the key is absent or is not a string scalar.
///
/// # Errors
///
/// Returns an error if existing frontmatter YAML cannot be parsed.
pub(crate) fn read_author(doc: &ParsedDocument) -> Result<Option<String>> {
    let mapping = parse_existing_frontmatter(doc)?;
    Ok(mapping
        .get("author")
        .and_then(Value::as_str)
        .map(str::to_owned))
}

/// Authenticate the `author` frontmatter key in place. `config` must already be escalated to the
/// doc's realm.
fn authenticate_author(
    mapping: &mut Mapping,
    config: &ResolvedConfig,
    create: bool,
    old_author: Option<&str>,
) -> Result<()> {
    let key = Value::String(String::from("author"));
    let supplied = mapping
        .get("author")
        .and_then(Value::as_str)
        .map(str::to_owned);

    if create {
        // Ignore-and-override: authorship is the authenticated caller, never
        // whatever the payload claimed. Anonymous callers get no author key.
        match &config.identity {
            Some(id) => {
                mapping.insert(key, Value::String(id.clone()));
            }
            None => {
                mapping.remove(&key);
            }
        }
        return Ok(());
    }

    match supplied {
        // Omitted on an edit is not a change: preserve the on-disk author.
        None => {
            match old_author {
                Some(prev) => {
                    mapping.insert(key, Value::String(prev.to_owned()));
                }
                None => {
                    mapping.remove(&key);
                }
            }
            Ok(())
        }
        Some(new) if old_author == Some(new.as_str()) => Ok(()),
        Some(new) => match config.mode {
            Mode::Open => Ok(()),
            Mode::Registered => {
                let reg = config
                    .registry
                    .as_ref()
                    .context("mode is registered but no registry found")?;
                if reg.is_active(&new) {
                    Ok(())
                } else {
                    anyhow::bail!(
                        "author change to {new:?} rejected: not an active registry participant"
                    );
                }
            }
            Mode::Strict => match old_author {
                Some(_) => {
                    anyhow::bail!("author is immutable in strict mode: {old_author:?} -> {new:?}")
                }
                None => {
                    if config.identity.as_deref() == Some(new.as_str()) {
                        Ok(())
                    } else {
                        anyhow::bail!(
                            "strict mode: a first-time author must be your own identity: \
                             {new:?} != caller {:?}",
                            config.identity
                        );
                    }
                }
            },
        },
    }
}

/// Read the current `sandbox` frontmatter list as `SandboxEntry` values.
///
/// Returns an empty vector when the key is absent, null, or an empty
/// sequence — self-healing for the common case where a user wrote bare
/// `sandbox:` in the frontmatter. Unparseable entries are surfaced as
/// errors so callers can decide how to respond (the CLI reports the file
/// as failed rather than silently dropping state).
///
/// # Errors
///
/// Returns an error if existing frontmatter YAML is invalid, if the
/// sandbox value is present but neither null nor a sequence, or if a
/// sandbox entry is not a well-formed `author@timestamp` string.
pub fn read_sandbox_entries(doc: &ParsedDocument) -> Result<Vec<SandboxEntry>> {
    let mapping = parse_existing_frontmatter(doc)?;
    let items = read_sequence_or_empty(&mapping, SANDBOX_KEY)?;

    let mut entries = Vec::with_capacity(items.len());
    for item in items {
        let Value::String(raw) = item else {
            anyhow::bail!("frontmatter `{SANDBOX_KEY}` entry is not a string");
        };
        entries.push(parser::parse_sandbox_entry(&raw)?);
    }
    Ok(entries)
}

/// Read a sequence-typed frontmatter value, coercing missing keys and null
/// values to an empty sequence.
///
/// This is the defensive parser shared by any caller that treats a
/// frontmatter key as a list. It accepts:
///
/// - key absent → empty sequence
/// - key present with null value (e.g. bare `sandbox:`) → empty sequence
/// - key present as a sequence → the sequence
///
/// Any other type (scalar, mapping, tagged) is rejected so a misuse is
/// surfaced rather than silently dropped.
///
/// # Errors
///
/// Returns an error when the key is present but its value is neither null
/// nor a YAML sequence.
fn read_sequence_or_empty(mapping: &Mapping, key: &str) -> Result<Vec<Value>> {
    let key_value = Value::String(String::from(key));
    let Some(value) = mapping.get(&key_value) else {
        return Ok(Vec::new());
    };

    match value {
        Value::Null => Ok(Vec::new()),
        Value::Sequence(items) => Ok(items.clone()),
        Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Mapping(_)
        | Value::Tagged(_) => {
            anyhow::bail!("frontmatter `{key}` is not a sequence");
        }
    }
}

/// Write the given sandbox entries back into the document, preserving all
/// other frontmatter. An empty slice removes the `sandbox` key entirely.
///
/// # Errors
///
/// Returns an error if existing frontmatter YAML cannot be parsed.
pub fn write_sandbox_entries(doc: &mut ParsedDocument, entries: &[SandboxEntry]) -> Result<()> {
    let mut mapping = parse_existing_frontmatter(doc)?;
    set_sandbox_on_mapping(&mut mapping, entries);
    let yaml = Value::Mapping(mapping);
    write_frontmatter_to_doc(doc, &yaml);
    Ok(())
}

/// Set the `sandbox` key on a frontmatter mapping in place.
///
/// Empty entry lists delete the key entirely so clean documents stay clean.
pub fn set_sandbox_on_mapping(mapping: &mut Mapping, entries: &[SandboxEntry]) {
    let key = Value::String(String::from(SANDBOX_KEY));
    if entries.is_empty() {
        mapping.remove(&key);
        return;
    }
    let values: Vec<Value> = entries
        .iter()
        .map(|e| Value::String(parser::format_sandbox_entry(e)))
        .collect();
    mapping.insert(key, Value::Sequence(values));
}

/// Append a sandbox entry for `identity`, OR refresh the existing
/// entry's timestamp when it already exists.
///
/// Returns:
/// - `true` when the entries vector was mutated (push OR ts update).
/// - `false` only when an existing entry's `ts` already equals `now`
///   (preserves the test-friendly "no clock advance, no rewrite"
///   noop invariant the op layer relies on).
///
/// The roster stays one-entry-per-identity. Position is preserved
/// across timestamp refreshes — only the matching entry's `ts`
/// field is mutated; surrounding entries are left untouched.
pub fn add_sandbox_entry_for(
    entries: &mut Vec<SandboxEntry>,
    identity: &str,
    now: DateTime<FixedOffset>,
) -> bool {
    if let Some(existing) = entries.iter_mut().find(|e| e.author == identity) {
        if existing.ts == now {
            return false;
        }
        existing.ts = now;
        return true;
    }
    entries.push(SandboxEntry {
        author: String::from(identity),
        ts: now,
    });
    true
}

/// Remove any sandbox entry matching `identity`. Returns `true` when an
/// entry was removed, `false` when none existed for the caller.
pub fn remove_sandbox_entry_for(entries: &mut Vec<SandboxEntry>, identity: &str) -> bool {
    let before = entries.len();
    entries.retain(|e| e.author != identity);
    entries.len() != before
}

/// Auto-populate user-owned fields. Only fills missing fields, never overwrites.
///
/// - `title`: from first `# heading` in the document, or empty
/// - `description`: empty string (user fills in)
/// - `author`: from config identity if available
/// - `created`: current timestamp
pub fn populate_user_fields(mapping: &mut Mapping, doc_body: &str, config: &ResolvedConfig) {
    populate_non_author_fields(mapping, doc_body);

    let author_key = Value::String(String::from("author"));
    if !mapping.contains_key(&author_key)
        && let Some(identity) = &config.identity
    {
        mapping.insert(author_key, Value::String(identity.clone()));
    }
}

/// Auto-populate every user-owned field except `author`.
///
/// The authored write path ([`ensure_frontmatter_authored`]) authenticates
/// `author` separately, so it must not be back-filled from the caller
/// identity here.
fn populate_non_author_fields(mapping: &mut Mapping, doc_body: &str) {
    let title_key = Value::String(String::from("title"));
    if !mapping.contains_key(&title_key) {
        let title = extract_title_from_heading(doc_body).unwrap_or_default();
        mapping.insert(title_key, Value::String(title));
    }

    let desc_key = Value::String(String::from("description"));
    if !mapping.contains_key(&desc_key) {
        mapping.insert(desc_key, Value::String(String::new()));
    }

    let created_key = Value::String(String::from("created"));
    if !mapping.contains_key(&created_key) {
        let now = rfc3339_z(&Utc::now());
        mapping.insert(created_key, Value::String(now));
    }
}

/// Recompute `remargin_*` fields from the current comment state.
/// Always overwrites these fields (they are tool-managed); with nothing
/// pending, `remargin_pending` and `remargin_pending_for` are dropped.
pub fn update_remargin_fields(mapping: &mut Mapping, comments: &[&Comment]) {
    let pending_count = comments.iter().filter(|cm| cm.is_pending()).count();

    // A broadcast surfaces under `<unassigned>`, which no registered identity can collide with; a
    // directed comment contributes only the recipients who have not acked.
    let mut pending_for: Vec<String> = Vec::new();
    let unassigned_sentinel = String::from("<unassigned>");
    for cm in comments {
        if !cm.is_pending() {
            continue;
        }
        if cm.to.is_empty() {
            if !pending_for.contains(&unassigned_sentinel) {
                pending_for.push(unassigned_sentinel.clone());
            }
        } else {
            for recipient in &cm.to {
                if cm.is_pending_for(recipient) && !pending_for.contains(recipient) {
                    pending_for.push(recipient.clone());
                }
            }
        }
    }
    pending_for.sort();

    let last_activity = find_last_activity(comments);

    // With nothing pending both fields are removed, not stamped `0` / `[]`; removal also sheds a
    // stale value.
    let pending_key = Value::String(String::from("remargin_pending"));
    let pending_for_key = Value::String(String::from("remargin_pending_for"));
    if pending_count == 0 {
        mapping.remove(&pending_key);
        mapping.remove(&pending_for_key);
    } else {
        mapping.insert(
            pending_key,
            Value::Number(serde_yaml::Number::from(pending_count as u64)),
        );

        let pending_for_values: Vec<Value> = pending_for.into_iter().map(Value::String).collect();
        mapping.insert(pending_for_key, Value::Sequence(pending_for_values));
    }

    let last_activity_key = Value::String(String::from("remargin_last_activity"));
    match last_activity {
        Some(ts) => {
            mapping.insert(last_activity_key, Value::String(rfc3339_z(&ts)));
        }
        None => {
            mapping.insert(last_activity_key, Value::Null);
        }
    }
}

/// Extract the body text from a document (all body segments concatenated).
fn extract_body_text(doc: &ParsedDocument) -> String {
    let mut body = String::new();
    for seg in &doc.segments {
        if let Segment::Body(text) = seg {
            body.push_str(text);
        }
    }
    body
}

/// Extract the title from the first `# heading` line in the body.
#[must_use]
pub fn extract_title_from_heading(body: &str) -> Option<String> {
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let title = rest.trim();
            if !title.is_empty() {
                return Some(String::from(title));
            }
        }
    }
    None
}

/// Find the most recent activity timestamp across all comments, acks, and reactions.
fn find_last_activity(comments: &[&Comment]) -> Option<DateTime<FixedOffset>> {
    let mut latest: Option<DateTime<FixedOffset>> = None;

    for cm in comments {
        latest = Some(max_ts(latest, cm.ts));

        for ack in &cm.ack {
            latest = Some(max_ts(latest, ack.ts));
        }
    }

    latest
}

fn max_ts(
    current: Option<DateTime<FixedOffset>>,
    candidate: DateTime<FixedOffset>,
) -> DateTime<FixedOffset> {
    match current {
        Some(existing) if existing > candidate => existing,
        _ => candidate,
    }
}

/// Parse existing frontmatter from the document's first body segment.
/// Returns a `Mapping` (possibly empty if no frontmatter exists).
pub(crate) fn parse_existing_frontmatter(doc: &ParsedDocument) -> Result<Mapping> {
    let first_body = match doc.segments.first() {
        Some(Segment::Body(text)) => text.as_str(),
        _ => return Ok(Mapping::new()),
    };

    let trimmed = first_body.trim_start();
    if !trimmed.starts_with(FRONTMATTER_DELIMITER) {
        return Ok(Mapping::new());
    }

    let lines: Vec<&str> = first_body.split('\n').collect();
    let opener = lines
        .iter()
        .position(|line| line.trim() == FRONTMATTER_DELIMITER);
    let Some(opener_idx) = opener else {
        return Ok(Mapping::new());
    };

    let closer = lines
        .iter()
        .enumerate()
        .skip(opener_idx + 1)
        .find(|(_, line)| line.trim() == FRONTMATTER_DELIMITER)
        .map(|(i, _)| i);

    let Some(closer_idx) = closer else {
        return Ok(Mapping::new());
    };

    let yaml_str: String = lines[opener_idx + 1..closer_idx].join("\n");
    let value: Value =
        serde_yaml::from_str(&yaml_str).context("failed to parse existing frontmatter YAML")?;

    match value {
        Value::Mapping(m) => Ok(m),
        Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Sequence(_)
        | Value::Tagged(_) => Ok(Mapping::new()),
    }
}

fn strip_frontmatter(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let opener = lines
        .iter()
        .position(|line| line.trim() == FRONTMATTER_DELIMITER);
    let Some(opener_idx) = opener else {
        return String::from(text);
    };

    let closer = lines
        .iter()
        .enumerate()
        .skip(opener_idx + 1)
        .find(|(_, line)| line.trim() == FRONTMATTER_DELIMITER)
        .map(|(i, _)| i);

    let Some(closer_idx) = closer else {
        return String::from(text);
    };

    let remaining_lines = &lines[closer_idx + 1..];
    remaining_lines.join("\n")
}

/// Write the frontmatter YAML back into the document's first body segment.
fn write_frontmatter_to_doc(doc: &mut ParsedDocument, yaml: &Value) {
    let yaml_str = serde_yaml::to_string(yaml).unwrap_or_default();
    let frontmatter_block = format!("{FRONTMATTER_DELIMITER}\n{yaml_str}{FRONTMATTER_DELIMITER}\n");

    if let Some(Segment::Body(text)) = doc.segments.first() {
        let trimmed = text.trim_start();
        if trimmed.starts_with(FRONTMATTER_DELIMITER) {
            let remaining = strip_frontmatter(text);
            let new_body = format!("{frontmatter_block}{remaining}");
            doc.segments[0] = Segment::Body(new_body);
            return;
        }
    }

    match doc.segments.first() {
        Some(Segment::Body(text)) => {
            let new_body = format!("{frontmatter_block}\n{text}");
            doc.segments[0] = Segment::Body(new_body);
        }
        _ => {
            doc.segments.insert(0, Segment::Body(frontmatter_block));
        }
    }
}
