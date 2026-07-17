//! Tests for the cross-document text search engine.

use core::fmt::Write as _;
use std::path::Path;

use os_shim::mock::MockSystem;

use crate::parser;

use super::{
    LineAttribution, MatchLocation, SearchOptions, SearchScope, build_line_attribution, search,
};

/// Build minimal search options for a literal pattern.
fn literal_opts(pattern: &str) -> SearchOptions {
    SearchOptions {
        context_lines: 0,
        ignore_case: false,
        limit: None,
        offset: 0,
        pattern: String::from(pattern),
        regex: false,
        scope: SearchScope::All,
    }
}

/// A single body document with `count` lines that each match `needle`.
fn corpus_with_needles(count: usize) -> String {
    let mut doc = String::from("# Title\n\n");
    for i in 1..=count {
        let _ = writeln!(doc, "needle line {i}");
    }
    doc
}

/// Create a minimal remargin comment block.
fn remargin_block(id: &str, content: &str) -> String {
    format!(
        "```remargin\n\
         ---\n\
         id: {id}\n\
         author: testuser\n\
         type: human\n\
         ts: 2026-04-06T14:32:00-04:00\n\
         checksum: sha256:abc123\n\
         ---\n\
         {content}\n\
         ```\n"
    )
}

/// Like [`remargin_block`] but with a YAML comment line that the parser
/// accepts and re-serialization drops, so the stored block is 3 bytes
/// longer than its canonical form.
fn drifting_remargin_block(id: &str, content: &str) -> String {
    format!(
        "```remargin\n\
         ---\n\
         id: {id}\n\
         author: testuser\n\
         type: human\n\
         ts: 2026-04-06T14:32:00-04:00\n\
         checksum: sha256:abc123\n\
         #x\n\
         ---\n\
         {content}\n\
         ```\n"
    )
}

#[test]
fn literal_match_in_body() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(
            Path::new("/docs/test.md"),
            b"# Title\n\nThe notification system works.\n",
        )
        .unwrap();

    let results = search(&system, base, base, &literal_opts("notification"))
        .unwrap()
        .matches;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].line, 3);
    assert_eq!(results[0].location, MatchLocation::Body);
    assert!(results[0].text.contains("notification"));
}

#[test]
fn file_path_searches_that_file() {
    // Regression: a `path` naming a file must search that file, not
    // silently return an empty set (the file-path footgun).
    let base = Path::new("/docs");
    let file = Path::new("/docs/note.md");
    let system = MockSystem::new()
        .with_file(file, b"# Title\n\nThe notification system works.\n")
        .unwrap();

    let results = search(&system, base, file, &literal_opts("notification"))
        .unwrap()
        .matches;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].line, 3);
    assert_eq!(results[0].path, Path::new("note.md"));
}

#[test]
fn literal_match_in_comment() {
    let base = Path::new("/docs");
    let doc = format!(
        "# Title\n\n{}",
        remargin_block("abc", "Run bd ready to check.")
    );
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc.as_bytes())
        .unwrap();

    let results = search(&system, base, base, &literal_opts("bd ready"))
        .unwrap()
        .matches;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].location, MatchLocation::Comment);
    assert_eq!(results[0].comment_id.as_deref(), Some("abc"));
}

#[test]
fn scope_body_only() {
    let base = Path::new("/docs");
    let doc = format!(
        "# Title\n\nNotification in body.\n\n{}",
        remargin_block("abc", "Notification in comment.")
    );
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc.as_bytes())
        .unwrap();

    let mut opts = literal_opts("Notification");
    opts.scope = SearchScope::Body;

    let results = search(&system, base, base, &opts).unwrap().matches;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].location, MatchLocation::Body);
}

#[test]
fn scope_comments_only() {
    let base = Path::new("/docs");
    let doc = format!(
        "# Title\n\nNotification in body.\n\n{}",
        remargin_block("abc", "Notification in comment.")
    );
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc.as_bytes())
        .unwrap();

    let mut opts = literal_opts("Notification");
    opts.scope = SearchScope::Comments;

    let results = search(&system, base, base, &opts).unwrap().matches;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].location, MatchLocation::Comment);
}

#[test]
fn regex_pattern() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(
            Path::new("/docs/test.md"),
            b"# Title\n\nRun bd ready now.\nAlso bd list works.\n",
        )
        .unwrap();

    let opts = SearchOptions {
        context_lines: 0,
        ignore_case: false,
        limit: None,
        offset: 0,
        pattern: String::from("bd (ready|list)"),
        regex: true,
        scope: SearchScope::All,
    };

    let results = search(&system, base, base, &opts).unwrap().matches;
    assert_eq!(results.len(), 2);
}

#[test]
fn case_insensitive() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(
            Path::new("/docs/test.md"),
            b"# Title\n\nNOTIFICATION system.\nnotification handler.\n",
        )
        .unwrap();

    let mut opts = literal_opts("notification");
    opts.ignore_case = true;

    let results = search(&system, base, base, &opts).unwrap().matches;
    assert_eq!(results.len(), 2);
}

#[test]
fn context_lines() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(
            Path::new("/docs/test.md"),
            b"line 1\nline 2\ntarget line\nline 4\nline 5\n",
        )
        .unwrap();

    let mut opts = literal_opts("target");
    opts.context_lines = 1;

    let results = search(&system, base, base, &opts).unwrap().matches;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].before, vec!["line 2"]);
    assert_eq!(results[0].after, vec!["line 4"]);
}

#[test]
fn no_matches() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), b"# Hello\n\nWorld.\n")
        .unwrap();

    let results = search(&system, base, base, &literal_opts("nonexistent"))
        .unwrap()
        .matches;
    assert!(results.is_empty());
}

#[test]
fn non_markdown_skipped() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.txt"), b"notification in txt\n")
        .unwrap()
        .with_file(Path::new("/docs/test.md"), b"notification in md\n")
        .unwrap();

    let results = search(&system, base, base, &literal_opts("notification"))
        .unwrap()
        .matches;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].path.to_str().unwrap(), "test.md");
}

#[test]
fn empty_pattern_rejected() {
    let base = Path::new("/docs");
    let system = MockSystem::new();

    let result = search(&system, base, base, &literal_opts(""));
    result.unwrap_err();
}

#[test]
fn multiple_files() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/a.md"), b"hello world\n")
        .unwrap()
        .with_file(Path::new("/docs/b.md"), b"hello there\n")
        .unwrap();

    let results = search(&system, base, base, &literal_opts("hello"))
        .unwrap()
        .matches;
    assert_eq!(results.len(), 2);
}

#[test]
fn search_match_json_shape_matches_schema() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/body.md"), b"hello world\n")
        .unwrap()
        .with_file(
            Path::new("/docs/comment.md"),
            remargin_block("abc", "hello reviewer").as_bytes(),
        )
        .unwrap();

    let results = search(&system, base, base, &literal_opts("hello"))
        .unwrap()
        .matches;
    assert!(!results.is_empty());

    // The body match has no comment_id; the comment match does.
    let body_match = results
        .iter()
        .find(|m| matches!(m.location, MatchLocation::Body))
        .unwrap();
    let comment_match = results
        .iter()
        .find(|m| matches!(m.location, MatchLocation::Comment))
        .unwrap();

    let body_value = serde_json::to_value(body_match).unwrap();
    let body_obj = body_value.as_object().unwrap();

    for key in ["after", "before", "line", "location", "path", "text"] {
        assert!(
            body_obj.contains_key(key),
            "required key `{key}` missing from serialized SearchMatch"
        );
    }

    // `location` is a PascalCase enum string in the schema.
    assert_eq!(body_obj["location"], serde_json::json!("Body"));
    // `comment_id` is omitted when None.
    assert!(
        !body_obj.contains_key("comment_id"),
        "comment_id must be skipped when None"
    );
    // `path` renders as a plain string.
    assert!(body_obj["path"].is_string());

    let comment_value = serde_json::to_value(comment_match).unwrap();
    let comment_obj = comment_value.as_object().unwrap();
    assert_eq!(comment_obj["location"], serde_json::json!("Comment"));
    assert!(
        comment_obj.contains_key("comment_id"),
        "comment_id must be present for comment matches"
    );
}

#[test]
fn multibyte_body_after_drifted_block_does_not_panic() {
    let base = Path::new("/docs");
    let doc = format!(
        "# Title\n\n{}text \u{2014}\n{}",
        drifting_remargin_block("aaa", "first"),
        remargin_block("bbb", "second")
    );
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc.as_bytes())
        .unwrap();

    let results = search(&system, base, base, &literal_opts("text"))
        .unwrap()
        .matches;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].location, MatchLocation::Body);
    assert_eq!(results[0].comment_id, None);
}

#[test]
fn drifted_block_keeps_following_body_attribution() {
    let base = Path::new("/docs");
    let doc = format!(
        "# Title\n\n{}marker line\n{}",
        drifting_remargin_block("aaa", "first"),
        remargin_block("bbb", "second")
    );
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc.as_bytes())
        .unwrap();

    let all_scope = search(&system, base, base, &literal_opts("marker"))
        .unwrap()
        .matches;
    assert_eq!(all_scope.len(), 1);
    assert_eq!(all_scope[0].location, MatchLocation::Body);
    assert_eq!(all_scope[0].comment_id, None);

    let mut opts = literal_opts("marker");
    opts.scope = SearchScope::Body;
    let body_scope = search(&system, base, base, &opts).unwrap().matches;
    assert_eq!(
        body_scope.len(),
        1,
        "body-scope filter must not hide the marker line"
    );
}

#[test]
fn attribution_matches_stored_block_spans() {
    let block_a = drifting_remargin_block("aaa", "first");
    let block_b = remargin_block("bbb", "second");
    let doc = format!("intro\n{block_a}mid \u{2014}\n{block_b}tail\n");

    let parsed = parser::parse(&doc).unwrap();
    let attribution = build_line_attribution(&doc, &parsed);

    let a_lines = block_a.matches('\n').count();
    let b_lines = block_b.matches('\n').count();
    let mid_idx = 1 + a_lines;
    let b_start = mid_idx + 1;

    for (idx, attr) in attribution.iter().enumerate() {
        let expected = if (1..mid_idx).contains(&idx) {
            Some("aaa")
        } else if (b_start..b_start + b_lines).contains(&idx) {
            Some("bbb")
        } else {
            None
        };
        let ok = match (expected, attr) {
            (None, LineAttribution::Body) => true,
            (Some(want), LineAttribution::Comment(got)) => got.as_str() == want,
            _ => false,
        };
        assert!(ok, "line {idx}: expected {expected:?}, got {attr:?}");
    }
}

#[test]
fn limit_and_offset_return_bounded_window_with_true_total() {
    // Spec example: 320 matches, offset 50 limit 50 -> 50 matches, total 320.
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(
            Path::new("/docs/big.md"),
            corpus_with_needles(320).as_bytes(),
        )
        .unwrap();

    let opts = literal_opts("needle").offset(50).limit(Some(50));
    let results = search(&system, base, base, &opts).unwrap();

    assert_eq!(results.total, 320);
    assert_eq!(results.matches.len(), 50);
    // Window starts at the 51st match (offset 50) and spans 50 matches.
    assert_eq!(results.matches[0].text, "needle line 51");
    assert_eq!(results.matches[49].text, "needle line 100");
}

#[test]
fn offset_past_end_yields_empty_matches_with_true_total() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(
            Path::new("/docs/big.md"),
            corpus_with_needles(320).as_bytes(),
        )
        .unwrap();

    let opts = literal_opts("needle").offset(400).limit(Some(50));
    let results = search(&system, base, base, &opts).unwrap();

    assert!(results.matches.is_empty());
    assert_eq!(results.total, 320);
}

#[test]
fn no_limit_returns_all_matches_and_total_equals_len() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(
            Path::new("/docs/big.md"),
            corpus_with_needles(320).as_bytes(),
        )
        .unwrap();

    let results = search(&system, base, base, &literal_opts("needle")).unwrap();

    assert_eq!(results.total, 320);
    assert_eq!(results.matches.len(), results.total);
}

#[test]
fn limit_larger_than_total_returns_all_matches() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(
            Path::new("/docs/big.md"),
            corpus_with_needles(320).as_bytes(),
        )
        .unwrap();

    let opts = literal_opts("needle").limit(Some(1000));
    let results = search(&system, base, base, &opts).unwrap();

    assert_eq!(results.matches.len(), 320);
    assert_eq!(results.total, 320);
}

#[test]
fn offset_without_limit_returns_tail() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(
            Path::new("/docs/big.md"),
            corpus_with_needles(320).as_bytes(),
        )
        .unwrap();

    let opts = literal_opts("needle").offset(300);
    let results = search(&system, base, base, &opts).unwrap();

    assert_eq!(results.matches.len(), 20);
    assert_eq!(results.total, 320);
    assert_eq!(results.matches[0].text, "needle line 301");
}
