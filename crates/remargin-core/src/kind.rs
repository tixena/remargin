//! Comment classification tags (`remargin_kind`).
//!
//! Permissive grammar, tight enough that malformed values cannot
//! change the YAML wire format. Empty vectors are dropped from output
//! so pre-`remargin_kind` comments round-trip byte-for-byte.

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value};

/// Short enough to render as a compact chip and too short to carry free-text content.
pub const MAX_KIND_LENGTH: usize = 15;

/// Bounds the YAML line and the signature payload; eight still fits on one chip row.
pub const MAX_KINDS_PER_COMMENT: usize = 8;

/// Shown in error messages only; the check itself is [`validate_single`].
pub const VALID_KIND_REGEX: &str = r"^[A-Za-z0-9_ \-]{1,15}$";

/// Validates the kinds of one comment. The parser calls it on every block it reads, and every
/// mutating operation calls it before the checksum is computed.
///
/// # Errors
///
/// Returns an error naming the offending value when the list is too long, repeats a value, or
/// holds an entry [`validate_single`] refuses.
pub fn validate_kinds(kinds: &[String]) -> Result<()> {
    if kinds.len() > MAX_KINDS_PER_COMMENT {
        bail!(
            "remargin_kind has {} entries; at most {} allowed",
            kinds.len(),
            MAX_KINDS_PER_COMMENT
        );
    }
    for (index, kind) in kinds.iter().enumerate() {
        validate_single(kind)?;
        // Quadratic, but bounded by MAX_KINDS_PER_COMMENT.
        for other in &kinds[index + 1..] {
            if other == kind {
                bail!("remargin_kind has duplicate value {kind:?}");
            }
        }
    }
    Ok(())
}

/// Validates one kind: 1 to [`MAX_KIND_LENGTH`] characters from `[A-Za-z0-9_ -]`, with no leading
/// or trailing space.
///
/// # Errors
///
/// Returns an error naming the entry when it is empty, too long, space-padded, or holds a
/// disallowed character.
pub fn validate_single(kind: &str) -> Result<()> {
    if kind.is_empty() {
        bail!("remargin_kind entry is empty");
    }
    if kind.len() > MAX_KIND_LENGTH {
        bail!("remargin_kind entry {kind:?} is longer than {MAX_KIND_LENGTH} characters");
    }
    if kind.starts_with(' ') || kind.ends_with(' ') {
        bail!("remargin_kind entry {kind:?} has leading or trailing space");
    }
    for ch in kind.chars() {
        if !is_allowed_char(ch) {
            bail!(
                "remargin_kind entry {kind:?} contains invalid character {ch:?}; allowed: {VALID_KIND_REGEX}"
            );
        }
    }
    Ok(())
}

/// The `--kind` filter shared by `comments` and `query`: `true` when `filter` is empty or when
/// `comment_kinds` holds at least one of its values.
#[must_use]
pub fn matches_kind_filter(comment_kinds: &[String], filter: &[String]) -> bool {
    if filter.is_empty() {
        return true;
    }
    filter.iter().any(|wanted| comment_kinds.contains(wanted))
}

/// The kinds as they are hashed: sorted and de-duplicated, so `[a, b]` and `[b, a]` produce the
/// same checksum and signature. The stored order is left alone.
#[must_use]
pub fn canonical_kinds(kinds: &[String]) -> Vec<String> {
    let mut out: Vec<String> = kinds.to_vec();
    out.sort();
    out.dedup();
    out
}

/// Read the `kind` tags off one JSON operation object. Absent or `null`
/// yields no tags; anything but an array of strings is refused.
///
/// # Errors
///
/// Returns an error when the value is present but is not an array of
/// strings.
pub fn kinds_from_json(obj: &Map<String, Value>) -> Result<Vec<String>> {
    match obj.get("kind") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => value
            .as_array()
            .and_then(|arr| {
                arr.iter()
                    .map(|v| v.as_str().map(String::from))
                    .collect::<Option<Vec<_>>>()
            })
            .context("`kind` must be an array of strings"),
    }
}

const fn is_allowed_char(ch: char) -> bool {
    matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '-' | ' ')
}

#[cfg(test)]
mod tests;
