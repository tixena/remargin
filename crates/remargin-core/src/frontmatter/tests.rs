//! Tests for frontmatter management.

extern crate alloc;

use chrono::{DateTime, FixedOffset};
use serde_yaml::{Mapping, Value};

use crate::config::{Mode, ResolvedConfig};
use crate::frontmatter::{
    add_sandbox_entry_for, ensure_frontmatter, ensure_frontmatter_authored,
    extract_title_from_heading, populate_user_fields, read_author, read_sandbox_entries,
    update_remargin_fields, write_sandbox_entries,
};
use crate::parser::{
    self, Acknowledgment, AuthorType, Comment, ParsedDocument, SandboxEntry, Segment,
};
use crate::reactions::Reactions;

const AUTHOR_GATE_REGISTRY_YAML: &str = "\
participants:
  alice:
    type: human
    status: active
    pubkeys: []
  bob:
    type: human
    status: revoked
    pubkeys: []
";

fn test_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
        source_path: None,
        trusted_roots: Vec::new(),
        unrestricted: false,
    }
}

fn make_comment(id: &str, ts: &str, to: Vec<String>, ack: Vec<Acknowledgment>) -> Comment {
    Comment {
        ack,
        attachments: Vec::new(),
        author: String::from("eduardo"),
        author_type: AuthorType::Human,
        checksum: String::from("sha256:test"),
        content: String::from("Test content."),
        edited_at: None,
        el: None,
        id: String::from(id),
        line: 0,
        reactions: Reactions::new(),
        remargin_kind: None,
        reply_to: None,
        signature: None,
        sl: None,
        thread: None,
        to,
        ts: DateTime::parse_from_rfc3339(ts).unwrap(),
    }
}

fn make_doc(body: &str, comments: Vec<Comment>) -> ParsedDocument {
    let mut segments = vec![Segment::Body(String::from(body))];
    for cm in comments {
        segments.push(Segment::Comment(Box::new(cm)));
        segments.push(Segment::Body(String::from("\n")));
    }
    ParsedDocument::from_segments(segments)
}

fn get_value<'map>(mapping: &'map Mapping, key: &str) -> Option<&'map Value> {
    mapping.get(Value::String(String::from(key)))
}

#[test]
fn no_frontmatter_adds_frontmatter() {
    let config = test_config();
    let mut doc = make_doc("# My Doc\n\nSome text.\n", Vec::new());

    ensure_frontmatter(&mut doc, &config).unwrap();

    let markdown = doc.to_markdown().unwrap();
    assert!(markdown.starts_with("---\n"));
    assert!(markdown.contains("title: My Doc"));
    assert!(markdown.contains("author: eduardo"));
    assert!(markdown.contains("created:"));
    assert!(markdown.contains("remargin_last_activity:"));
    assert!(!markdown.contains("remargin_pending"));
}

#[test]
fn existing_frontmatter_preserved() {
    let config = test_config();
    let body = "---\ntitle: Custom Title\nauthor: alice\n---\n\nSome text.\n";
    let mut doc = make_doc(body, Vec::new());

    ensure_frontmatter(&mut doc, &config).unwrap();

    let markdown = doc.to_markdown().unwrap();
    assert!(markdown.contains("Custom Title"));
    assert!(markdown.contains("alice"));
    assert!(markdown.contains("remargin_last_activity:"));
    assert!(!markdown.contains("remargin_pending"));
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
    assert_eq!(pending.as_u64().unwrap(), 2);
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
    assert_eq!(names, vec!["alice", "eduardo"]);
}

#[test]
fn pending_for_unaddressed_unacked_surfaces_as_unassigned_sentinel() {
    let cm = make_comment("u", "2026-04-06T12:00:00-04:00", Vec::new(), Vec::new());
    let comments: Vec<&Comment> = vec![&cm];
    let mut mapping = Mapping::new();
    update_remargin_fields(&mut mapping, &comments);

    let pending_for = get_value(&mapping, "remargin_pending_for").unwrap();
    let names: Vec<&str> = pending_for
        .as_sequence()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["<unassigned>"]);
}

#[test]
fn pending_for_addressed_and_unaddressed_mix_sorts_with_sentinel() {
    let addressed = make_comment(
        "a",
        "2026-04-06T12:00:00-04:00",
        vec![String::from("eduardo")],
        Vec::new(),
    );
    let unaddressed = make_comment("u", "2026-04-06T13:00:00-04:00", Vec::new(), Vec::new());
    let comments: Vec<&Comment> = vec![&addressed, &unaddressed];
    let mut mapping = Mapping::new();
    update_remargin_fields(&mut mapping, &comments);

    let pending_for = get_value(&mapping, "remargin_pending_for").unwrap();
    let names: Vec<&str> = pending_for
        .as_sequence()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["<unassigned>", "eduardo"]);
}

#[test]
fn pending_for_two_unaddressed_dedupes_sentinel() {
    let cm1 = make_comment("a", "2026-04-06T12:00:00-04:00", Vec::new(), Vec::new());
    let cm2 = make_comment("b", "2026-04-06T13:00:00-04:00", Vec::new(), Vec::new());
    let comments: Vec<&Comment> = vec![&cm1, &cm2];
    let mut mapping = Mapping::new();
    update_remargin_fields(&mut mapping, &comments);

    let pending_for = get_value(&mapping, "remargin_pending_for").unwrap();
    let names: Vec<&str> = pending_for
        .as_sequence()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["<unassigned>"]);
}

#[test]
fn pending_for_unaddressed_acked_does_not_surface_sentinel() {
    let cm = make_comment(
        "a",
        "2026-04-06T12:00:00-04:00",
        Vec::new(),
        vec![Acknowledgment {
            author: String::from("anyone"),
            ts: DateTime::parse_from_rfc3339("2026-04-06T13:00:00-04:00").unwrap(),
        }],
    );
    let comments: Vec<&Comment> = vec![&cm];
    let mut mapping = Mapping::new();
    update_remargin_fields(&mut mapping, &comments);

    assert!(
        get_value(&mapping, "remargin_pending_for").is_none(),
        "acked unaddressed must not surface"
    );
}

#[test]
fn pending_count_directed_with_partial_ack_is_pending() {
    let cm = make_comment(
        "a",
        "2026-04-06T12:00:00-04:00",
        vec![String::from("alice")],
        vec![Acknowledgment {
            author: String::from("bob"),
            ts: DateTime::parse_from_rfc3339("2026-04-06T13:00:00-04:00").unwrap(),
        }],
    );
    let comments: Vec<&Comment> = vec![&cm];
    let mut mapping = Mapping::new();
    update_remargin_fields(&mut mapping, &comments);

    let pending = get_value(&mapping, "remargin_pending").unwrap();
    assert_eq!(
        pending.as_u64().unwrap(),
        1,
        "directed comment whose addressee has not acked must count as pending"
    );
}

#[test]
fn pending_for_directed_with_partial_ack_lists_unacked_recipient() {
    let cm = make_comment(
        "a",
        "2026-04-06T12:00:00-04:00",
        vec![String::from("alice")],
        vec![Acknowledgment {
            author: String::from("bob"),
            ts: DateTime::parse_from_rfc3339("2026-04-06T13:00:00-04:00").unwrap(),
        }],
    );
    let comments: Vec<&Comment> = vec![&cm];
    let mut mapping = Mapping::new();
    update_remargin_fields(&mut mapping, &comments);

    let pending_for = get_value(&mapping, "remargin_pending_for").unwrap();
    let names: Vec<&str> = pending_for
        .as_sequence()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["alice"]);
}

#[test]
fn pending_for_directed_with_one_recipient_acked_excludes_them() {
    let cm = make_comment(
        "a",
        "2026-04-06T12:00:00-04:00",
        vec![String::from("alice"), String::from("eduardo")],
        vec![Acknowledgment {
            author: String::from("alice"),
            ts: DateTime::parse_from_rfc3339("2026-04-06T13:00:00-04:00").unwrap(),
        }],
    );
    let comments: Vec<&Comment> = vec![&cm];
    let mut mapping = Mapping::new();
    update_remargin_fields(&mut mapping, &comments);

    let pending_for = get_value(&mapping, "remargin_pending_for").unwrap();
    let names: Vec<&str> = pending_for
        .as_sequence()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["eduardo"]);
}

#[test]
fn pending_count_self_addressed_with_third_party_ack_is_pending() {
    let cm = make_comment(
        "a",
        "2026-04-06T12:00:00-04:00",
        vec![String::from("eduardo")],
        vec![Acknowledgment {
            author: String::from("agent"),
            ts: DateTime::parse_from_rfc3339("2026-04-06T13:00:00-04:00").unwrap(),
        }],
    );
    let comments: Vec<&Comment> = vec![&cm];
    let mut mapping = Mapping::new();
    update_remargin_fields(&mut mapping, &comments);

    let pending = get_value(&mapping, "remargin_pending").unwrap();
    assert_eq!(pending.as_u64().unwrap(), 1);

    let pending_for = get_value(&mapping, "remargin_pending_for").unwrap();
    let names: Vec<&str> = pending_for
        .as_sequence()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["eduardo"]);
}

#[test]
fn broadcast_comment_with_no_to_and_no_ack_is_pending_for_unassigned() {
    let cm = make_comment("a", "2026-04-06T12:00:00-04:00", Vec::new(), Vec::new());
    let comments: Vec<&Comment> = vec![&cm];
    let mut mapping = Mapping::new();
    update_remargin_fields(&mut mapping, &comments);

    let pending = get_value(&mapping, "remargin_pending").unwrap();
    assert_eq!(
        pending.as_u64().unwrap(),
        1,
        "broadcast unacked comment must count toward remargin_pending"
    );

    let pending_for = get_value(&mapping, "remargin_pending_for").unwrap();
    let names: Vec<&str> = pending_for
        .as_sequence()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["<unassigned>"],
        "broadcast unacked comment must surface under <unassigned>"
    );
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
    assert!(ts_str.contains("16:00:00"));
}

#[test]
fn no_comments_omits_pending_fields() {
    let comments: Vec<&Comment> = Vec::new();
    let mut mapping = Mapping::new();
    update_remargin_fields(&mut mapping, &comments);

    assert!(get_value(&mapping, "remargin_pending").is_none());
    assert!(get_value(&mapping, "remargin_pending_for").is_none());

    let last = get_value(&mapping, "remargin_last_activity").unwrap();
    assert!(last.is_null());
}

#[test]
fn zero_pending_sheds_stale_stored_values() {
    let mut mapping = Mapping::new();
    mapping.insert(
        Value::String(String::from("remargin_pending")),
        Value::Number(serde_yaml::Number::from(2_u64)),
    );
    mapping.insert(
        Value::String(String::from("remargin_pending_for")),
        Value::Sequence(vec![Value::String(String::from("alice"))]),
    );

    let cm = make_comment(
        "a",
        "2026-04-06T12:00:00-04:00",
        vec![String::from("alice")],
        vec![Acknowledgment {
            author: String::from("alice"),
            ts: DateTime::parse_from_rfc3339("2026-04-06T13:00:00-04:00").unwrap(),
        }],
    );
    let comments: Vec<&Comment> = vec![&cm];
    update_remargin_fields(&mut mapping, &comments);

    assert!(get_value(&mapping, "remargin_pending").is_none());
    assert!(get_value(&mapping, "remargin_pending_for").is_none());
    assert!(get_value(&mapping, "remargin_last_activity").is_some());
}

#[test]
fn user_field_preserved() {
    let config = test_config();
    let mut mapping = Mapping::new();

    mapping.insert(
        Value::String(String::from("title")),
        Value::String(String::from("Custom")),
    );

    populate_user_fields(&mut mapping, "# Auto Title\n", &config);

    let title = get_value(&mapping, "title").unwrap();
    assert_eq!(title.as_str().unwrap(), "Custom");
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
        source_path: None,
        trusted_roots: Vec::new(),
        unrestricted: false,
    };
    let mut mapping = Mapping::new();
    populate_user_fields(&mut mapping, "# Doc\n", &config);

    assert!(
        !mapping.contains_key(Value::String(String::from("author"))),
        "author should not be set without identity"
    );
}

fn strict_config(identity: Option<&str>) -> ResolvedConfig {
    ResolvedConfig {
        identity: identity.map(String::from),
        mode: Mode::Strict,
        ..test_config()
    }
}

fn registered_config(identity: &str) -> ResolvedConfig {
    ResolvedConfig {
        identity: Some(String::from(identity)),
        mode: Mode::Registered,
        registry: Some(serde_yaml::from_str(AUTHOR_GATE_REGISTRY_YAML).unwrap()),
        ..test_config()
    }
}

#[test]
fn authored_create_ignores_supplied_author() {
    let config = test_config();
    let mut doc = parser::parse("---\nauthor: someone_else\n---\n\n# Doc\n").unwrap();
    ensure_frontmatter_authored(&mut doc, &config, true, None).unwrap();
    assert_eq!(read_author(&doc).unwrap().as_deref(), Some("eduardo"));
}

#[test]
fn authored_create_stamps_caller_identity() {
    let config = test_config();
    let mut doc = parser::parse("---\ntitle: Doc\n---\n\n# Doc\n").unwrap();
    ensure_frontmatter_authored(&mut doc, &config, true, None).unwrap();
    assert_eq!(read_author(&doc).unwrap().as_deref(), Some("eduardo"));
}

#[test]
fn authored_create_no_identity_drops_author() {
    let config = ResolvedConfig {
        identity: None,
        ..test_config()
    };
    let mut doc = parser::parse("---\nauthor: ghost\n---\n\n# Doc\n").unwrap();
    ensure_frontmatter_authored(&mut doc, &config, true, None).unwrap();
    assert_eq!(read_author(&doc).unwrap(), None);
}

#[test]
fn authored_strict_edit_unchanged_author_ok() {
    let config = strict_config(Some("alice"));
    let mut doc = parser::parse("---\nauthor: alice\n---\n\n# Doc\n").unwrap();
    ensure_frontmatter_authored(&mut doc, &config, false, Some("alice")).unwrap();
    assert_eq!(read_author(&doc).unwrap().as_deref(), Some("alice"));
}

#[test]
fn authored_strict_edit_changed_author_rejected() {
    let config = strict_config(Some("mallory"));
    let mut doc = parser::parse("---\nauthor: mallory\n---\n\n# Doc\n").unwrap();
    let err = ensure_frontmatter_authored(&mut doc, &config, false, Some("alice")).unwrap_err();
    assert!(
        err.to_string()
            .contains("author is immutable in strict mode"),
        "got: {err}"
    );
}

#[test]
fn authored_strict_first_author_matching_caller_allowed() {
    let config = strict_config(Some("alice"));
    let mut doc = parser::parse("---\nauthor: alice\n---\n\n# Doc\n").unwrap();
    ensure_frontmatter_authored(&mut doc, &config, false, None).unwrap();
    assert_eq!(read_author(&doc).unwrap().as_deref(), Some("alice"));
}

#[test]
fn authored_strict_first_author_mismatch_rejected() {
    let config = strict_config(Some("alice"));
    let mut doc = parser::parse("---\nauthor: bob\n---\n\n# Doc\n").unwrap();
    let err = ensure_frontmatter_authored(&mut doc, &config, false, None).unwrap_err();
    assert!(
        err.to_string()
            .contains("a first-time author must be your own identity"),
        "got: {err}"
    );
}

#[test]
fn authored_strict_edit_omit_author_preserved() {
    let config = strict_config(Some("editor"));
    let mut doc = parser::parse("---\ntitle: Doc\n---\n\n# Doc\n").unwrap();
    ensure_frontmatter_authored(&mut doc, &config, false, Some("alice")).unwrap();
    assert_eq!(read_author(&doc).unwrap().as_deref(), Some("alice"));
}

#[test]
fn authored_registered_edit_to_active_ok() {
    let config = registered_config("eduardo");
    let mut doc = parser::parse("---\nauthor: alice\n---\n\n# Doc\n").unwrap();
    ensure_frontmatter_authored(&mut doc, &config, false, Some("eduardo")).unwrap();
    assert_eq!(read_author(&doc).unwrap().as_deref(), Some("alice"));
}

#[test]
fn authored_registered_edit_to_unregistered_rejected() {
    let config = registered_config("eduardo");
    let mut doc = parser::parse("---\nauthor: nobody\n---\n\n# Doc\n").unwrap();
    let err = ensure_frontmatter_authored(&mut doc, &config, false, Some("alice")).unwrap_err();
    assert!(
        err.to_string()
            .contains("not an active registry participant"),
        "got: {err}"
    );
}

/// `is_active` treats a revoked participant as inactive.
#[test]
fn authored_registered_edit_to_revoked_rejected() {
    let config = registered_config("eduardo");
    let mut doc = parser::parse("---\nauthor: bob\n---\n\n# Doc\n").unwrap();
    let err = ensure_frontmatter_authored(&mut doc, &config, false, Some("alice")).unwrap_err();
    assert!(
        err.to_string()
            .contains("not an active registry participant"),
        "got: {err}"
    );
}

#[test]
fn authored_open_edit_changed_author_ok() {
    let config = test_config();
    let mut doc = parser::parse("---\nauthor: anyone\n---\n\n# Doc\n").unwrap();
    ensure_frontmatter_authored(&mut doc, &config, false, Some("alice")).unwrap();
    assert_eq!(read_author(&doc).unwrap().as_deref(), Some("anyone"));
}

#[test]
fn authored_edit_omit_author_preserves_on_disk() {
    let config = test_config();
    let mut doc = parser::parse("---\ntitle: Doc\n---\n\n# Doc\n").unwrap();
    ensure_frontmatter_authored(&mut doc, &config, false, Some("alice")).unwrap();
    assert_eq!(read_author(&doc).unwrap().as_deref(), Some("alice"));
}

/// The editor's identity must not be back-filled, unlike on the create path.
#[test]
fn authored_edit_omit_author_on_authorless_doc_stays_absent() {
    let config = test_config();
    let mut doc = parser::parse("---\ntitle: Doc\n---\n\n# Doc\n").unwrap();
    ensure_frontmatter_authored(&mut doc, &config, false, None).unwrap();
    assert_eq!(read_author(&doc).unwrap(), None);
}

#[test]
fn authored_preserves_non_author_fields() {
    let config = test_config();
    let mut doc = parser::parse("---\ntitle: Custom\nauthor: alice\n---\n\n# Doc\n").unwrap();
    ensure_frontmatter_authored(&mut doc, &config, false, Some("alice")).unwrap();
    let markdown = doc.to_markdown().unwrap();
    assert!(markdown.contains("title: Custom"));
    assert!(markdown.contains("remargin_last_activity:"));
    assert!(!markdown.contains("remargin_pending"));
}

#[test]
fn read_author_reads_value_or_none() {
    let with = parser::parse("---\nauthor: alice\n---\n\nBody\n").unwrap();
    assert_eq!(read_author(&with).unwrap().as_deref(), Some("alice"));
    let without = parser::parse("---\ntitle: Doc\n---\n\nBody\n").unwrap();
    assert_eq!(read_author(&without).unwrap(), None);
}

#[test]
fn sandbox_null_value_reads_as_empty() {
    let body = "---\ntitle: Doc\nsandbox:\n---\n\nBody.\n";
    let doc = make_doc(body, Vec::new());

    let entries = read_sandbox_entries(&doc).unwrap();
    assert_eq!(entries, [] as [parser::SandboxEntry; 0]);
}

#[test]
fn sandbox_missing_key_reads_as_empty() {
    let body = "---\ntitle: Doc\n---\n\nBody.\n";
    let doc = make_doc(body, Vec::new());

    let entries = read_sandbox_entries(&doc).unwrap();
    assert_eq!(entries, [] as [parser::SandboxEntry; 0]);
}

#[test]
fn sandbox_existing_sequence_reads_entries() {
    let body = "---\ntitle: Doc\nsandbox:\n  - alice@2026-04-16T10:00:00-04:00\n---\n\nBody.\n";
    let doc = make_doc(body, Vec::new());

    let entries = read_sandbox_entries(&doc).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].author, "alice");
}

/// A stored `+00:00` reads as the same instant as `Z` and is rewritten as `Z` on the next write.
#[test]
fn sandbox_legacy_zero_offset_reads_like_z_and_converges_on_write() {
    let legacy_body =
        "---\ntitle: Doc\nsandbox:\n  - alice@2026-04-16T14:00:00+00:00\n---\n\nBody.\n";
    let z_body = "---\ntitle: Doc\nsandbox:\n  - alice@2026-04-16T14:00:00Z\n---\n\nBody.\n";

    let mut legacy_doc = make_doc(legacy_body, Vec::new());
    let legacy_entries = read_sandbox_entries(&legacy_doc).unwrap();
    let z_entries = read_sandbox_entries(&make_doc(z_body, Vec::new())).unwrap();
    assert_eq!(legacy_entries.len(), 1);
    assert_eq!(legacy_entries[0].ts, z_entries[0].ts);
    assert_eq!(
        legacy_entries[0].ts,
        DateTime::parse_from_rfc3339("2026-04-16T14:00:00Z").unwrap()
    );

    write_sandbox_entries(&mut legacy_doc, &legacy_entries).unwrap();
    let markdown = legacy_doc.to_markdown().unwrap();
    assert!(
        markdown.contains("- alice@2026-04-16T14:00:00Z"),
        "rewrite must render the sandbox entry with Z:\n{markdown}"
    );
}

#[test]
fn sandbox_null_value_self_heals_on_write() {
    let body = "---\ntitle: Doc\nsandbox:\n---\n\nBody.\n";
    let mut doc = make_doc(body, Vec::new());

    let mut entries = read_sandbox_entries(&doc).unwrap();
    let added = add_sandbox_entry_for(
        &mut entries,
        "eduardo",
        DateTime::parse_from_rfc3339("2026-04-16T10:33:21-04:00").unwrap(),
    );
    assert!(added);
    write_sandbox_entries(&mut doc, &entries).unwrap();

    let markdown = doc.to_markdown().unwrap();
    assert!(markdown.contains("sandbox:"));
    assert!(markdown.contains("- eduardo@2026-04-16T10:33:21"));

    let reparsed = parser::parse(&markdown).unwrap();
    let reread = read_sandbox_entries(&reparsed).unwrap();
    assert_eq!(reread.len(), 1);
    assert_eq!(reread[0].author, "eduardo");
}

#[test]
fn sandbox_non_sequence_errors() {
    let body = "---\ntitle: Doc\nsandbox: alice\n---\n\nBody.\n";
    let doc = make_doc(body, Vec::new());

    let err = read_sandbox_entries(&doc).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("not a sequence"), "got: {msg}");
}

fn t1() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-04-16T10:00:00-04:00").unwrap()
}

fn t2() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-04-16T11:00:00-04:00").unwrap()
}

fn t3() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-04-16T12:00:00-04:00").unwrap()
}

fn t4() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-04-16T13:00:00-04:00").unwrap()
}

#[test]
fn add_sandbox_entry_first_time() {
    let mut entries = Vec::new();
    let mutated = add_sandbox_entry_for(&mut entries, "alice", t1());
    assert!(mutated);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].author, "alice");
    assert_eq!(entries[0].ts, t1());
}

#[test]
fn add_sandbox_entry_refreshes_with_new_ts() {
    let mut entries = vec![SandboxEntry {
        author: String::from("alice"),
        ts: t1(),
    }];
    let mutated = add_sandbox_entry_for(&mut entries, "alice", t2());
    assert!(mutated);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].ts, t2());
}

#[test]
fn add_sandbox_entry_noop_on_identical_ts() {
    let mut entries = vec![SandboxEntry {
        author: String::from("alice"),
        ts: t1(),
    }];
    let mutated = add_sandbox_entry_for(&mut entries, "alice", t1());
    assert!(!mutated);
}

#[test]
fn add_sandbox_entry_appends_second_identity() {
    let mut entries = vec![SandboxEntry {
        author: String::from("alice"),
        ts: t1(),
    }];
    let mutated = add_sandbox_entry_for(&mut entries, "bob", t2());
    assert!(mutated);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].author, "bob");
}

#[test]
fn add_sandbox_entry_refreshes_one_in_multi_roster() {
    let mut entries = vec![
        SandboxEntry {
            author: String::from("alice"),
            ts: t1(),
        },
        SandboxEntry {
            author: String::from("bob"),
            ts: t2(),
        },
    ];
    let mutated = add_sandbox_entry_for(&mut entries, "alice", t3());
    assert!(mutated);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].ts, t3());
    assert_eq!(entries[1].ts, t2());
}

#[test]
fn add_sandbox_entry_preserves_order_across_refresh() {
    let mut entries = vec![
        SandboxEntry {
            author: String::from("alice"),
            ts: t1(),
        },
        SandboxEntry {
            author: String::from("bob"),
            ts: t2(),
        },
        SandboxEntry {
            author: String::from("carol"),
            ts: t3(),
        },
    ];
    add_sandbox_entry_for(&mut entries, "bob", t4());
    assert_eq!(entries[0].author, "alice");
    assert_eq!(entries[1].author, "bob");
    assert_eq!(entries[1].ts, t4());
    assert_eq!(entries[2].author, "carol");
}
