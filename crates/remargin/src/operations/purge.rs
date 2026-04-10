//! Comment stripping and purge operations.
//!
//! Remove all Remargin comment blocks from a document, producing a clean
//! document with only body content and user-owned frontmatter.

#[cfg(test)]
mod tests;

use std::path::Path;

use core::mem;

use anyhow::{Context as _, Result};
use os_shim::System;
use serde_yaml::Value;

use crate::config::ResolvedConfig;
use crate::parser::{self, Segment};

/// Result of a purge operation.
#[derive(Debug)]
#[non_exhaustive]
pub struct PurgeResult {
    /// Number of attachment files cleaned up.
    pub attachments_cleaned: usize,
    /// Number of comment blocks removed.
    pub comments_removed: usize,
}

/// Remove all Remargin comment blocks from a document.
///
/// If `dry_run` is true, returns stats without writing.
///
/// # Errors
///
/// Returns an error if:
/// - The file cannot be read or written
/// - The document cannot be parsed
pub fn purge(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    dry_run: bool,
) -> Result<PurgeResult> {
    let mut doc = parser::parse_file(system, path)?;

    // Count comments and collect attachment paths.
    let comments = doc.comments();
    let comments_removed = comments.len();

    let attachment_paths: Vec<String> = comments
        .iter()
        .flat_map(|cm| cm.attachments.clone())
        .collect();

    if dry_run {
        return Ok(PurgeResult {
            attachments_cleaned: attachment_paths.len(),
            comments_removed,
        });
    }

    // Remove all Comment and LegacyComment segments.
    doc.segments.retain(|seg| matches!(seg, Segment::Body(_)));

    // Collapse consecutive empty Body segments and normalize double blank lines.
    collapse_body_segments(&mut doc.segments);

    // Clean up remargin_* frontmatter fields.
    clean_frontmatter(&mut doc);

    // Clean up orphaned attachments.
    let doc_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut attachments_cleaned: usize = 0;
    for attachment in &attachment_paths {
        let attachment_path = doc_dir
            .join(&config.assets_dir)
            .join(Path::new(attachment).file_name().unwrap_or_default());
        if system.remove_file(&attachment_path).is_ok() {
            attachments_cleaned += 1;
        }
    }

    // Write the clean document.
    let markdown = doc.to_markdown();
    system
        .write(path, markdown.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;

    Ok(PurgeResult {
        attachments_cleaned,
        comments_removed,
    })
}

/// Clean up remargin_* fields from frontmatter.
fn clean_frontmatter(doc: &mut parser::ParsedDocument) {
    let Some(Segment::Body(text)) = doc.segments.first() else {
        return;
    };

    let trimmed = text.trim_start();
    if !trimmed.starts_with("---") {
        return;
    }

    let lines: Vec<&str> = text.split('\n').collect();
    let opener = lines.iter().position(|line| line.trim() == "---");
    let Some(opener_idx) = opener else {
        return;
    };

    let closer = lines
        .iter()
        .enumerate()
        .skip(opener_idx + 1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(i, _)| i);

    let Some(closer_idx) = closer else {
        return;
    };

    let yaml_str: String = lines[opener_idx + 1..closer_idx].join("\n");
    let parsed: Result<Value, _> = serde_yaml::from_str(&yaml_str);
    let Ok(Value::Mapping(mut mapping)) = parsed else {
        return;
    };

    // Remove remargin_* fields.
    let keys_to_remove: Vec<Value> = mapping
        .keys()
        .filter(|key| key.as_str().is_some_and(|s| s.starts_with("remargin_")))
        .cloned()
        .collect();

    for key in &keys_to_remove {
        mapping.remove(key);
    }

    // Rebuild frontmatter.
    if mapping.is_empty() {
        // No fields left -- remove frontmatter entirely.
        let remaining = lines[closer_idx + 1..].join("\n");
        let cleaned = remaining.trim_start_matches('\n');
        doc.segments[0] = Segment::Body(String::from(cleaned));
    } else {
        let new_yaml = serde_yaml::to_string(&Value::Mapping(mapping)).unwrap_or_default();
        let before_fm = "";
        let after_fm = lines[closer_idx + 1..].join("\n");
        let new_body = format!("{before_fm}---\n{new_yaml}---\n{after_fm}");
        doc.segments[0] = Segment::Body(new_body);
    }
}

/// Collapse consecutive Body segments and remove excessive blank lines.
fn collapse_body_segments(segments: &mut Vec<Segment>) {
    // First pass: merge consecutive Body segments.
    let mut merged = Vec::new();
    let mut current_body = String::new();

    for seg in segments.drain(..) {
        match seg {
            Segment::Body(text) => current_body.push_str(&text),
            other @ (Segment::Comment(_) | Segment::LegacyComment(_)) => {
                if !current_body.is_empty() {
                    merged.push(Segment::Body(mem::take(&mut current_body)));
                }
                merged.push(other);
            }
        }
    }
    if !current_body.is_empty() {
        merged.push(Segment::Body(current_body));
    }

    // Second pass: normalize excessive blank lines in Body segments.
    for seg in &mut merged {
        if let Segment::Body(text) = seg {
            // Replace 3+ consecutive newlines with 2 (max one blank line between paragraphs).
            let mut normalized = String::new();
            let mut consecutive_newlines: usize = 0;
            for ch in text.chars() {
                if ch == '\n' {
                    consecutive_newlines += 1;
                    if consecutive_newlines <= 2 {
                        normalized.push(ch);
                    }
                } else {
                    consecutive_newlines = 0;
                    normalized.push(ch);
                }
            }
            *text = normalized;
        }
    }

    *segments = merged;
}
