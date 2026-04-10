//! Tests for frontmatter management.

extern crate alloc;

use alloc::collections::BTreeMap;

use chrono::DateTime;
use serde_yaml::{Mapping, Value};

use crate::config::{Mode, ResolvedConfig};
use crate::frontmatter::{
    ensure_frontmatter, extract_title_from_heading, populate_user_fields, update_remargin_fields,
};
use crate::parser::{Acknowledgment, AuthorType, Comment, ParsedDocument, Segment};

/// Create a default `ResolvedConfig` for testing.
fn test_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
        unrestricted: false,
    }
}

/// Create a comment with the given parameters.
fn make_comment(id: &str, ts: &str, to: Vec<String>, ack: Vec<Acknowledgment>) -> Comment {
    Comment {
        ack,
        attachments: Vec::new(),
        author: String::from("eduardo"),
        author_type: AuthorType::Human,
        checksum: String::from("sha256:test"),
        content: String::from("Test content."),
        fence_depth: 3,
        id: String::from(id),
        line: 0,
        reactions: BTreeMap::new(),
        reply_to: None,
        signature: None,
        thread: None,
        to,
        ts: DateTime::parse_from_rfc3339(ts).unwrap(),
    }
}

/// Create a `ParsedDocument` with given body text and comments.
fn make_doc(body: &str, comments: Vec<Comment>) -> ParsedDocument {
    let mut segments = vec![Segment::Body(String::from(body))];
    for cm in comments {
        segments.push(Segment::Comment(Box::new(cm)));
        segments.push(Segment::Body(String::from("\n")));
    }
    ParsedDocument { segments }
}

/// Helper to get a value from a `Mapping` by string key.
fn get_value<'map>(mapping: &'map Mapping, key: &str) -> Option<&'map Value> {
    mapping.get(Value::String(String::from(key)))
}

#[test]
fn no_frontmatter_adds_frontmatter() {
    let config = test_config();
    let mut doc = make_doc("# My Doc\n\nSome text.\n", Vec::new());

    ensure_frontmatter(&mut doc, &config).unwrap();

    let markdown = doc.to_markdown();
    assert!(markdown.starts_with("---\n"));
    assert!(markdown.contains("title: My Doc"));
    assert!(markdown.contains("author: eduardo"));
    assert!(markdown.contains("created:"));
    assert!(markdown.contains("remargin_pending: 0"));
}

#[test]
fn existing_frontmatter_preserved() {
    let config = test_config();
    let body = "---\ntitle: Custom Title\nauthor: alice\n---\n\nSome text.\n";
    let mut doc = make_doc(body, Vec::new());

    ensure_frontmatter(&mut doc, &config).unwrap();

    let markdown = doc.to_markdown();
    // User fields preserved (not overwritten).
    assert!(markdown.contains("Custom Title"));
    assert!(markdown.contains("alice"));
    // Remargin fields added.
    assert!(markdown.contains("remargin_pending: 0"));
}

#[test]
fn title_from_heading() {
    assert_eq!(
        extract_title_from_heading("Some text\n# My Document\nMore text"),
        Some(String::from("My Document"))
    );
}

#[test]
fn title_from_heading_none() {
    assert_eq!(extract_title_from_heading("No heading here"), None);
}

#[test]
fn pending_count() {
    let cm1 = make_comment("a", "2026-04-06T12:00:00-04:00", Vec::new(), Vec::new());
    let cm2 = make_comment("b", "2026-04-06T13:00:00-04:00", Vec::new(), Vec::new());
    let cm3 = make_comment(
        "c",
        "2026-04-06T14:00:00-04:00",
        Vec::new(),
        vec![Acknowledgment {
            author: String::from("alice"),
            ts: DateTime::parse_from_rfc3339("2026-04-06T15:00:00-04:00").unwrap(),
        }],
    );

    let comments: Vec<&Comment> = vec![&cm1, &cm2, &cm3];
    let mut mapping = Mapping::new();
    update_remargin_fields(&mut mapping, &comments);

    let pending = get_value(&mapping, "remargin_pending").unwrap();
    assert_eq!(pending.as_u64().unwrap(), 2); // cm1 and cm2 are unacked
}

#[test]
fn pending_for() {
    let cm1 = make_comment(
        "a",
        "2026-04-06T12:00:00-04:00",
        vec![String::from("eduardo")],
        Vec::new(),
    );
    let cm2 = make_comment(
        "b",
        "2026-04-06T13:00:00-04:00",
        vec![String::from("alice"), String::from("eduardo")],
        Vec::new(),
    );

    let comments: Vec<&Comment> = vec![&cm1, &cm2];
    let mut mapping = Mapping::new();
    update_remargin_fields(&mut mapping, &comments);

    let pending_for = get_value(&mapping, "remargin_pending_for").unwrap();
    let seq = pending_for.as_sequence().unwrap();
    let names: Vec<&str> = seq.iter().map(|v| v.as_str().unwrap()).collect();
    // Sorted and deduplicated.
    assert_eq!(names, vec!["alice", "eduardo"]);
}

#[test]
fn last_activity() {
    let cm1 = make_comment("a", "2026-04-06T12:00:00-04:00", Vec::new(), Vec::new());
    let cm2 = make_comment(
        "b",
        "2026-04-06T13:00:00-04:00",
        Vec::new(),
        vec![Acknowledgment {
            author: String::from("alice"),
            ts: DateTime::parse_from_rfc3339("2026-04-06T16:00:00-04:00").unwrap(),
        }],
    );

    let comments: Vec<&Comment> = vec![&cm1, &cm2];
    let mut mapping = Mapping::new();
    update_remargin_fields(&mut mapping, &comments);

    let last = get_value(&mapping, "remargin_last_activity").unwrap();
    let ts_str = last.as_str().unwrap();
    // The ack at 16:00 is the most recent.
    assert!(ts_str.contains("16:00:00"));
}

#[test]
fn no_comments_zero_pending() {
    let comments: Vec<&Comment> = Vec::new();
    let mut mapping = Mapping::new();
    update_remargin_fields(&mut mapping, &comments);

    let pending = get_value(&mapping, "remargin_pending").unwrap();
    assert_eq!(pending.as_u64().unwrap(), 0);

    let last = get_value(&mapping, "remargin_last_activity").unwrap();
    assert!(last.is_null());
}

#[test]
fn user_field_preserved() {
    let config = test_config();
    let mut mapping = Mapping::new();

    // Pre-set a custom title.
    mapping.insert(
        Value::String(String::from("title")),
        Value::String(String::from("Custom")),
    );

    // Body has a different heading.
    populate_user_fields(&mut mapping, "# Auto Title\n", &config);

    let title = get_value(&mapping, "title").unwrap();
    assert_eq!(title.as_str().unwrap(), "Custom"); // Not overwritten.
}

#[test]
fn author_from_config() {
    let config = test_config();
    let mut mapping = Mapping::new();
    populate_user_fields(&mut mapping, "# Doc\n", &config);

    let author = get_value(&mapping, "author").unwrap();
    assert_eq!(author.as_str().unwrap(), "eduardo");
}

#[test]
fn no_identity_no_author() {
    let config = ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: None,
        identity: None,
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
        unrestricted: false,
    };
    let mut mapping = Mapping::new();
    populate_user_fields(&mut mapping, "# Doc\n", &config);

    assert!(
        !mapping.contains_key(Value::String(String::from("author"))),
        "author should not be set without identity"
    );
}
