//! Comment classification tags (`remargin_kind`).
//!
//! Comments carry an optional list of short classification strings so
//! downstream tools (UI filters, agent routing, query) can group work
//! without re-inspecting the free-text content. The field is
//! intentionally permissive — any non-surprising identifier passes —
//! but the grammar is tight enough that a malformed value cannot crash
//! downstream consumers or change the YAML wire format.
//!
//! ## Validation grammar
//!
//! Each kind:
//!
//! - Matches [`VALID_KIND_REGEX`], i.e. 1..=[`MAX_KIND_LENGTH`] characters
//!   drawn from `[A-Za-z0-9_ \-]`.
//! - Does not begin or end with a space — embedded spaces are allowed
//!   (so `action item` is legal) but leading/trailing space would round-trip
//!   badly through YAML flow sequences.
//! - Is distinct from every other kind on the same comment (case sensitive
//!   for now: `Question` and `question` are two different tags).
//!
//! A comment carries at most [`MAX_KINDS_PER_COMMENT`] kinds.
//!
//! ## Wire-format note
//!
//! The YAML serializer emits `remargin_kind: [a, b, c]` when the vector
//! is non-empty; empty vectors are dropped from the output entirely so
//! pre-`remargin_kind` comments round-trip byte-for-byte. See
//! [`crate::parser`] for the serializer and [`crate::crypto`] for the
//! matching "empty adds nothing" contribution to checksum and
//! signature payloads — that is the back-compat lynchpin that keeps
//! pre-existing comments verifiable.

use anyhow::{Result, bail};

/// Hard upper bound on kind string length.
///
/// Matches the acceptance criteria for rem-n4x7 (`[a-zA-Z0-9_\- ]{1,15}`).
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
/// mutating operation (create, edit, migrate, batch) before the
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
/// design doc for rem-49w0 explicitly calls out the previous divergence
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

const fn is_allowed_char(ch: char) -> bool {
    matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '-' | ' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(value: &str) -> String {
        value.to_owned()
    }

    #[test]
    fn accepts_simple_identifiers() {
        validate_kinds(&[s("question"), s("action-item"), s("v1_0")]).unwrap();
    }

    #[test]
    fn accepts_embedded_space() {
        validate_kinds(&[s("action item"), s("to review")]).unwrap();
    }

    #[test]
    fn rejects_empty_entry() {
        let err = validate_kinds(&[s("")]).unwrap_err();
        assert!(err.to_string().contains("is empty"));
    }

    #[test]
    fn rejects_over_length_entry() {
        let long = "a".repeat(MAX_KIND_LENGTH + 1);
        let err = validate_kinds(&[long]).unwrap_err();
        assert!(err.to_string().contains("longer than"));
    }

    #[test]
    fn rejects_leading_or_trailing_space() {
        assert!(validate_kinds(&[s(" q")]).is_err());
        assert!(validate_kinds(&[s("q ")]).is_err());
    }

    #[test]
    fn rejects_disallowed_characters() {
        assert!(validate_kinds(&[s("hello!")]).is_err());
        assert!(validate_kinds(&[s("foo,bar")]).is_err());
        assert!(validate_kinds(&[s("a\nb")]).is_err());
    }

    #[test]
    fn rejects_duplicates() {
        let err = validate_kinds(&[s("q"), s("q")]).unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn rejects_too_many() {
        let many: Vec<String> = (0..=MAX_KINDS_PER_COMMENT)
            .map(|i| format!("k{i}"))
            .collect();
        let err = validate_kinds(&many).unwrap_err();
        assert!(err.to_string().contains("at most"));
    }

    #[test]
    fn canonical_kinds_sorts_and_dedups() {
        let input = vec![s("b"), s("a"), s("b"), s("c")];
        assert_eq!(canonical_kinds(&input), vec![s("a"), s("b"), s("c")]);
    }

    #[test]
    fn matches_kind_filter_empty_is_always_true() {
        assert!(matches_kind_filter(&[], &[]));
        assert!(matches_kind_filter(&[s("question")], &[]));
    }

    #[test]
    fn matches_kind_filter_uses_or_semantics() {
        let kinds = vec![s("question"), s("todo")];
        let want = vec![s("todo"), s("blocker")];
        // Matches because `todo` is in both.
        assert!(matches_kind_filter(&kinds, &want));
    }

    #[test]
    fn matches_kind_filter_rejects_disjoint_sets() {
        let kinds = vec![s("question")];
        let want = vec![s("todo"), s("blocker")];
        assert!(!matches_kind_filter(&kinds, &want));
    }

    #[test]
    fn matches_kind_filter_no_match_when_comment_has_no_kinds() {
        let kinds: Vec<String> = Vec::new();
        let want = vec![s("question")];
        assert!(!matches_kind_filter(&kinds, &want));
    }
}
