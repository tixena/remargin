//! YAML frontmatter management for `remargin_*` fields.
//!
//! User-owned fields (`title`, `description`, `author`, `created`) are
//! auto-populated on first save but never overwritten. Tool-managed fields
//! (`remargin_pending`, `remargin_pending_for`, `remargin_last_activity`)
//! are recomputed on every write operation.

#[cfg(test)]
mod tests;

use anyhow::{Context as _, Result};
use chrono::{DateTime, FixedOffset, Utc};
use serde_yaml::{Mapping, Value};

use crate::config::ResolvedConfig;
use crate::parser::{Comment, ParsedDocument, Segment};

/// Frontmatter delimiter.
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

/// Auto-populate user-owned fields. Only fills missing fields, never overwrites.
///
/// - `title`: from first `# heading` in the document, or empty
/// - `description`: empty string (user fills in)
/// - `author`: from config identity if available
/// - `created`: current timestamp
pub fn populate_user_fields(mapping: &mut Mapping, doc_body: &str, config: &ResolvedConfig) {
    // title: from first # heading, only if not already set.
    let title_key = Value::String(String::from("title"));
    if !mapping.contains_key(&title_key) {
        let title = extract_title_from_heading(doc_body).unwrap_or_default();
        mapping.insert(title_key, Value::String(title));
    }

    // description: empty string if not set.
    let desc_key = Value::String(String::from("description"));
    if !mapping.contains_key(&desc_key) {
        mapping.insert(desc_key, Value::String(String::new()));
    }

    // author: from config identity if available and not already set.
    let author_key = Value::String(String::from("author"));
    if !mapping.contains_key(&author_key)
        && let Some(identity) = &config.identity
    {
        mapping.insert(author_key, Value::String(identity.clone()));
    }

    // created: current timestamp if not set.
    let created_key = Value::String(String::from("created"));
    if !mapping.contains_key(&created_key) {
        let now = Utc::now().to_rfc3339();
        mapping.insert(created_key, Value::String(now));
    }
}

/// Recompute `remargin_*` fields from the current comment state.
/// Always overwrites these fields (they are tool-managed).
pub fn update_remargin_fields(mapping: &mut Mapping, comments: &[&Comment]) {
    // Count pending (comments with no ack entries).
    let pending_count = comments.iter().filter(|cm| cm.ack.is_empty()).count();

    // Collect unique "to" recipients on unacked comments.
    let mut pending_for: Vec<String> = Vec::new();
    for cm in comments {
        if cm.ack.is_empty() {
            for recipient in &cm.to {
                if !pending_for.contains(recipient) {
                    pending_for.push(recipient.clone());
                }
            }
        }
    }
    pending_for.sort();

    // Most recent timestamp across all comments, acks, and reactions.
    let last_activity = find_last_activity(comments);

    // Write the fields.
    let pending_key = Value::String(String::from("remargin_pending"));
    mapping.insert(
        pending_key,
        Value::Number(serde_yaml::Number::from(pending_count as u64)),
    );

    let pending_for_key = Value::String(String::from("remargin_pending_for"));
    let pending_for_values: Vec<Value> = pending_for.into_iter().map(Value::String).collect();
    mapping.insert(pending_for_key, Value::Sequence(pending_for_values));

    let last_activity_key = Value::String(String::from("remargin_last_activity"));
    match last_activity {
        Some(ts) => {
            mapping.insert(last_activity_key, Value::String(ts.to_rfc3339()));
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
        // Comment creation timestamp.
        latest = Some(max_ts(latest, cm.ts));

        // Ack timestamps.
        for ack in &cm.ack {
            latest = Some(max_ts(latest, ack.ts));
        }
    }

    latest
}

/// Return the later of an optional timestamp and a new timestamp.
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
fn parse_existing_frontmatter(doc: &ParsedDocument) -> Result<Mapping> {
    let first_body = match doc.segments.first() {
        Some(Segment::Body(text)) => text.as_str(),
        _ => return Ok(Mapping::new()),
    };

    let trimmed = first_body.trim_start();
    if !trimmed.starts_with(FRONTMATTER_DELIMITER) {
        return Ok(Mapping::new());
    }

    // Find the opening and closing --- delimiters.
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

/// Remove the frontmatter block from a body string, returning the rest.
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

    // Everything after the closing --- (including the newline after it).
    let remaining_lines = &lines[closer_idx + 1..];
    remaining_lines.join("\n")
}

/// Write the frontmatter YAML back into the document's first body segment.
fn write_frontmatter_to_doc(doc: &mut ParsedDocument, yaml: &Value) {
    let yaml_str = serde_yaml::to_string(yaml).unwrap_or_default();
    let frontmatter_block = format!("{FRONTMATTER_DELIMITER}\n{yaml_str}{FRONTMATTER_DELIMITER}\n");

    // Check if the first segment is a body with existing frontmatter.
    if let Some(Segment::Body(text)) = doc.segments.first() {
        let trimmed = text.trim_start();
        if trimmed.starts_with(FRONTMATTER_DELIMITER) {
            // Replace existing frontmatter while preserving content after it.
            let remaining = strip_frontmatter(text);
            let new_body = format!("{frontmatter_block}{remaining}");
            doc.segments[0] = Segment::Body(new_body);
            return;
        }
    }

    // No existing frontmatter -- prepend it.
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
