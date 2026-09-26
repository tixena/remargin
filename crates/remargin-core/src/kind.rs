//! Comment classification tags (`remargin_kind`).
//!
//! Permissive grammar, tight enough that malformed values cannot
//! change the YAML wire format. Empty vectors are dropped from output
//! so pre-`remargin_kind` comments round-trip byte-for-byte.

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value};

/// Hard upper bound on kind string length.
///
/// Matches the acceptance criteria for (`[a-zA-Z0-9_\- ]{1,15}`).
/// Keeps tags short enough to render as compact chips in the Obsidian
/// sidebar and to discourage abuse as free-text mini-content.
pub const MAX_KIND_LENGTH: usize = 15;

/// Maximum number of kind tags a single comment may carry.
///
/// Chosen to keep both the YAML line and the signature payload bounded
/// and to discourage "kind-stuffing" as an alternative to proper
/// threading. Eight is comfortably above the handful of categories the
/// product design doc calls out and still fits on one on-screen chip row.
pub const MAX_KINDS_PER_COMMENT: usize = 8;

/// Human-readable restatement of the validation grammar. Referenced
/// by error messages so operators can copy-paste the exact shape into
/// their tooling without digging through source.
pub const VALID_KIND_REGEX: &str = r"^[A-Za-z0-9_ \-]{1,15}$";

/// Validate a slice of proposed `remargin_kind` values.
///
/// Called from the parser for every block on read, and from every
/// mutating operation (create, edit, batch) before the
/// checksum is computed. Keeping validation centralised means a
/// malformed tag cannot sneak in through a parser edge-case and break
/// signature verification downstream.
///
/// # Errors
///
/// Returns an error describing the offending value when:
///
/// - The list has more than [`MAX_KINDS_PER_COMMENT`] entries.
/// - An entry is empty, longer than [`MAX_KIND_LENGTH`], contains a
///   disallowed character, or starts/ends with a space.
/// - Two entries are equal (duplicates).
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
        // Duplicates are detected by looking forward from the current
        // position; the inner loop is bounded by MAX_KINDS_PER_COMMENT
        // so the quadratic scan is a rounding error.
        for other in &kinds[index + 1..] {
            if other == kind {
                bail!("remargin_kind has duplicate value {kind:?}");
            }
        }
    }
    Ok(())
}

/// Per-element validation. Exposed for callers that only need to check
/// a single incoming string (e.g. an MCP param validator before
/// assembling the full vector).
///
/// # Errors
///
/// See the error conditions enumerated on [`validate_kinds`].
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

/// Shared `--kind` filter matcher used by `comments` and `query`.
///
/// Returns `true` when `filter` is empty (no filter active) or when
/// `comment_kinds` contains at least one of the values in `filter`
/// (OR semantics).
///
/// Kept in this module so the `comments` list-a-single-file path and
/// the `query` walk-the-tree path share a single implementation — the
/// design doc for explicitly calls out the previous divergence
/// between those two surfaces as a bug.
#[must_use]
pub fn matches_kind_filter(comment_kinds: &[String], filter: &[String]) -> bool {
    if filter.is_empty() {
        return true;
    }
    filter.iter().any(|wanted| comment_kinds.contains(wanted))
}

/// Return the set of kinds canonicalised for hashing:
///
/// - De-duplicated (validator already enforces this, but the helper
///   stays defensive so a future caller that skips validation cannot
///   silently desync checksum and signature).
/// - Sorted lexicographically so `[a, b]` and `[b, a]` hash to the
///   same value. Storage order is preserved in the YAML; only the
///   hashed representation is canonicalised.
#[must_use]
pub fn canonical_kinds(kinds: &[String]) -> Vec<String> {
    let mut out: Vec<String> = kinds.to_vec();
    out.sort();
    out.dedup();
    out
}

/// Read the kind tags off one JSON operation object.
///
/// `kind` is an alias of `remargin_kind`, as on the single-comment surfaces.
/// Absent or `null` yields no tags; anything but an array of strings is
/// refused.
///
/// # Errors
///
/// Returns an error when the value is present but is not an array of
/// strings.
pub fn kinds_from_json(obj: &Map<String, Value>) -> Result<Vec<String>> {
    match obj.get("remargin_kind").or_else(|| obj.get("kind")) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => value
            .as_array()
            .and_then(|arr| {
                arr.iter()
                    .map(|v| v.as_str().map(String::from))
                    .collect::<Option<Vec<_>>>()
            })
            .context("`remargin_kind`/`kind` must be an array of strings"),
    }
}

const fn is_allowed_char(ch: char) -> bool {
    matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '-' | ' ')
}

#[cfg(test)]
mod tests;
