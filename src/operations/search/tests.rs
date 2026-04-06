//! Tests for the cross-document text search engine.

use std::path::Path;

use os_shim::mock::MockSystem;

use super::{MatchLocation, SearchOptions, SearchScope, search};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build minimal search options for a literal pattern.
fn literal_opts(pattern: &str) -> SearchOptions {
    SearchOptions {
        context_lines: 0,
        ignore_case: false,
        pattern: String::from(pattern),
        regex: false,
        scope: SearchScope::All,
    }
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

// ---------------------------------------------------------------------------
// Test 1: Literal match in body text
// ---------------------------------------------------------------------------

#[test]
fn literal_match_in_body() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(
            Path::new("/docs/test.md"),
            b"# Title\n\nThe notification system works.\n",
        )
        .unwrap();

    let results = search(&system, base, base, &literal_opts("notification")).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].line, 3);
    assert_eq!(results[0].location, MatchLocation::Body);
    assert!(results[0].text.contains("notification"));
}

// ---------------------------------------------------------------------------
// Test 2: Literal match in comment
// ---------------------------------------------------------------------------

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

    let results = search(&system, base, base, &literal_opts("bd ready")).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].location, MatchLocation::Comment);
    assert_eq!(results[0].comment_id.as_deref(), Some("abc"));
}

// ---------------------------------------------------------------------------
// Test 3: Scope body only
// ---------------------------------------------------------------------------

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

    let results = search(&system, base, base, &opts).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].location, MatchLocation::Body);
}

// ---------------------------------------------------------------------------
// Test 4: Scope comments only
// ---------------------------------------------------------------------------

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

    let results = search(&system, base, base, &opts).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].location, MatchLocation::Comment);
}

// ---------------------------------------------------------------------------
// Test 5: Regex pattern
// ---------------------------------------------------------------------------

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
        pattern: String::from("bd (ready|list)"),
        regex: true,
        scope: SearchScope::All,
    };

    let results = search(&system, base, base, &opts).unwrap();
    assert_eq!(results.len(), 2);
}

// ---------------------------------------------------------------------------
// Test 6: Case insensitive
// ---------------------------------------------------------------------------

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

    let results = search(&system, base, base, &opts).unwrap();
    assert_eq!(results.len(), 2);
}

// ---------------------------------------------------------------------------
// Test 7: Context lines
// ---------------------------------------------------------------------------

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

    let results = search(&system, base, base, &opts).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].before, vec!["line 2"]);
    assert_eq!(results[0].after, vec!["line 4"]);
}

// ---------------------------------------------------------------------------
// Test 8: No matches
// ---------------------------------------------------------------------------

#[test]
fn no_matches() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), b"# Hello\n\nWorld.\n")
        .unwrap();

    let results = search(&system, base, base, &literal_opts("nonexistent")).unwrap();
    assert!(results.is_empty());
}

// ---------------------------------------------------------------------------
// Test 9: Non-markdown files skipped
// ---------------------------------------------------------------------------

#[test]
fn non_markdown_skipped() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.txt"), b"notification in txt\n")
        .unwrap()
        .with_file(Path::new("/docs/test.md"), b"notification in md\n")
        .unwrap();

    let results = search(&system, base, base, &literal_opts("notification")).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].path.to_str().unwrap(), "test.md");
}

// ---------------------------------------------------------------------------
// Test 10: Empty pattern rejected
// ---------------------------------------------------------------------------

#[test]
fn empty_pattern_rejected() {
    let base = Path::new("/docs");
    let system = MockSystem::new();

    let result = search(&system, base, base, &literal_opts(""));
    result.unwrap_err();
}

// ---------------------------------------------------------------------------
// Test 11: Multiple files
// ---------------------------------------------------------------------------

#[test]
fn multiple_files() {
    let base = Path::new("/docs");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/a.md"), b"hello world\n")
        .unwrap()
        .with_file(Path::new("/docs/b.md"), b"hello there\n")
        .unwrap();

    let results = search(&system, base, base, &literal_opts("hello")).unwrap();
    assert_eq!(results.len(), 2);
}
