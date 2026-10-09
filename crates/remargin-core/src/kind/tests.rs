//! Tests for kind validation, the kind filter and the canonical order used for hashing.

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
