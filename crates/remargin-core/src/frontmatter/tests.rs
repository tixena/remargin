//! Tests for frontmatter management.

extern crate alloc;

use std::path::Path;

use chrono::{DateTime, FixedOffset};
use os_shim::System as _;
use os_shim::mock::MemorySystem;
use regex::Regex;
use serde_json::json;
use serde_yaml::{Mapping, Value};

use crate::config::identity::IdentityFlags;
use crate::config::{Mode, ResolvedConfig};
use crate::document::{get_with_links, metadata};
use crate::frontmatter::{
    Fence, add_sandbox_entry_for, ensure_frontmatter, ensure_frontmatter_authored,
    extract_title_from_heading, fence_of, parse_existing_frontmatter, populate_user_fields,
    read_author, read_sandbox_entries, update_remargin_fields, write_sandbox_entries,
};
use crate::linter::lint_doc;
use crate::mcp;
use crate::operations::purge::purge;
use crate::operations::{CreateCommentParams, ack_comments, create_comment};
use crate::parser::{
    self, Acknowledgment, AuthorType, Comment, ParsedDocument, SandboxEntry, Segment, rfc3339_z,
};
use crate::reactions::Reactions;
use crate::writer::InsertPosition;

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

// Opt-in `+++` TOML frontmatter (R1). The golden fixtures under `tests/golden/` were captured
// with the pre-change binary: `remargin comment <fixture> "golden"` and the MCP `write` of the
// fixture's own bytes, in an open-mode vault whose `.remargin.yaml` declares `identity: golden`
// and `type: agent`. Comment ids and timestamps are replaced by `<ID>` / `<TS>`.
const GOLDEN_CONFIG: &str = "mode: open\nidentity: golden\ntype: agent\n";
const TOML_CONFIG: &str = "mode: open\nidentity: golden\ntype: agent\ntoml_frontmatter: true\n";
const TOML_NOTE: &str = include_str!("tests/golden/toml_note.md");

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

fn vault(config_yaml: &str, file: &str, content: &str) -> MemorySystem {
    MemorySystem::new()
        .with_file(Path::new("/vault/.remargin.yaml"), config_yaml.as_bytes())
        .unwrap()
        .with_file(Path::new("/vault").join(file), content.as_bytes())
        .unwrap()
}

fn resolve(system: &MemorySystem) -> ResolvedConfig {
    ResolvedConfig::resolve(system, Path::new("/vault"), &IdentityFlags::default(), None).unwrap()
}

/// Mirror of `remargin comment <file> "<content>"`; returns the file after the write with the
/// new id replaced by `<ID>`.
fn comment(system: &MemorySystem, config: &ResolvedConfig, file: &str, content: &str) -> String {
    let path = Path::new("/vault").join(file);
    let id = create_comment(
        system,
        &path,
        config,
        &CreateCommentParams::new(content, &InsertPosition::Append),
    )
    .unwrap();
    let after = system.read_to_string(&path).unwrap();
    after.replace(&format!("id: {id}\n"), "id: <ID>\n")
}

/// Mirror of the MCP `write` of the file's own bytes.
fn rewrite(system: &MemorySystem, config: &ResolvedConfig, file: &str) -> String {
    let path = Path::new("/vault").join(file);
    let content = system.read_to_string(&path).unwrap();
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1_i32,
        "method": "tools/call",
        "params": {"name": "write", "arguments": {"path": file, "content": content}},
    });
    let response = mcp::process_request(system, Path::new("/vault"), config, &request.to_string())
        .unwrap()
        .unwrap();
    assert!(!response.contains("\"error\""), "write failed: {response}");
    system.read_to_string(&path).unwrap()
}

fn mask_timestamps(text: &str) -> String {
    Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})")
        .unwrap()
        .replace_all(text, "<TS>")
        .into_owned()
}

fn assert_golden(fixture: &str, comment_golden: &str, write_golden: &str) {
    let commented = {
        let system = vault(GOLDEN_CONFIG, "note.md", fixture);
        comment(&system, &resolve(&system), "note.md", "golden")
    };
    assert_eq!(mask_timestamps(&commented), comment_golden);

    let written = {
        let system = vault(GOLDEN_CONFIG, "note.md", fixture);
        rewrite(&system, &resolve(&system), "note.md")
    };
    assert_eq!(mask_timestamps(&written), write_golden);
}

/// The `+++` block at the top of `text`, parsed as TOML. Panics when there is none.
fn toml_block(text: &str) -> toml::Table {
    let rest = text.strip_prefix("+++\n").unwrap();
    let end = rest.find("\n+++\n").unwrap();
    rest[..end].parse::<toml::Table>().unwrap()
}

/// The text from the first comment block on, with timestamps masked.
fn comment_blocks(text: &str) -> String {
    mask_timestamps(&text[text.find("```remargin").unwrap()..])
}

#[test]
fn a_yaml_file_is_written_exactly_as_before() {
    assert_golden(
        include_str!("tests/golden/yaml_note.md"),
        include_str!("tests/golden/yaml_note.comment.after.md"),
        include_str!("tests/golden/yaml_note.write.after.md"),
    );
}

#[test]
fn a_file_without_frontmatter_is_written_exactly_as_before() {
    assert_golden(
        include_str!("tests/golden/plain_note.md"),
        include_str!("tests/golden/plain_note.comment.after.md"),
        include_str!("tests/golden/plain_note.write.after.md"),
    );
}

#[test]
fn a_toml_file_without_the_opt_in_is_written_exactly_as_before() {
    assert_golden(
        TOML_NOTE,
        include_str!("tests/golden/toml_note.comment.after.md"),
        include_str!("tests/golden/toml_note.write.after.md"),
    );
}

#[test]
fn a_toml_file_keeps_toml_after_a_comment() {
    let system = vault(TOML_CONFIG, "note.md", TOML_NOTE);
    let after = comment(&system, &resolve(&system), "note.md", "golden");

    assert!(after.starts_with("+++\n"), "{after}");
    let before_comment = &after[..after.find("```remargin").unwrap()];
    assert!(!before_comment.lines().any(|line| line == "---"), "{after}");
    assert!(before_comment.contains("# Toml note\n\nBody text under TOML frontmatter.\n"));

    let table = toml_block(&after);
    assert_eq!(table["title"].as_str(), Some("Toml note"));
    assert_eq!(table["tags"].as_array().unwrap().len(), 2);
    assert_eq!(table["created"].as_str(), Some("2026-01-02T03:04:05Z"));
    assert_eq!(table["author"].as_str(), Some("golden"));
    assert_eq!(table["remargin_pending"].as_integer(), Some(1));
}

#[test]
fn a_toml_comment_line_is_not_taken_for_a_heading() {
    // A TOML comment that looks like a heading must not catch an after-heading insert.
    let shadowed = "+++\n# Toml note\ntitle = \"Toml note\"\n+++\n\n# Toml note\n\nBody.\n";
    let system = vault(TOML_CONFIG, "note.md", shadowed);
    let path = Path::new("/vault/note.md");
    let position = InsertPosition::AfterHeading(String::from("Toml note"));
    create_comment(
        &system,
        path,
        &resolve(&system),
        &CreateCommentParams::new("here", &position),
    )
    .unwrap();

    let after = system.read_to_string(path).unwrap();
    let body_heading = after.find("+++\n\n# Toml note\n").unwrap();
    assert!(after.find("```remargin").unwrap() > body_heading, "{after}");
    assert_eq!(toml_block(&after)["title"].as_str(), Some("Toml note"));
}

#[test]
fn pending_counts_are_recomputed_in_toml() {
    let system = vault(TOML_CONFIG, "note.md", TOML_NOTE);
    let config = resolve(&system);
    let path = Path::new("/vault/note.md");
    let append = InsertPosition::Append;
    let first = create_comment(
        &system,
        path,
        &config,
        &CreateCommentParams::new("one", &append),
    )
    .unwrap();
    let second = create_comment(
        &system,
        path,
        &config,
        &CreateCommentParams::new("two", &append),
    )
    .unwrap();
    let two_pending = toml_block(&system.read_to_string(path).unwrap());
    assert_eq!(two_pending["remargin_pending"].as_integer(), Some(2));
    assert_eq!(
        two_pending["remargin_pending_for"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    ack_comments(
        &system,
        path,
        &config,
        &[first.as_str(), second.as_str()],
        false,
    )
    .unwrap();
    let acked = system.read_to_string(path).unwrap();
    let none_pending = toml_block(&acked);
    assert!(!none_pending.contains_key("remargin_pending"), "{acked}");
    assert!(
        !none_pending.contains_key("remargin_pending_for"),
        "{acked}"
    );
    assert!(none_pending["remargin_last_activity"].as_str().is_some());

    purge(&system, path, &config).unwrap();
    let purged = system.read_to_string(path).unwrap();
    let cleaned = toml_block(&purged);
    assert!(
        !cleaned.keys().any(|key| key.starts_with("remargin_")),
        "{purged}"
    );
    assert_eq!(cleaned["title"].as_str(), Some("Toml note"));
}

#[test]
fn a_new_file_uses_yaml_unless_configured_toml() {
    let plain = include_str!("tests/golden/plain_note.md");

    for (config_yaml, opener) in [
        (String::from(TOML_CONFIG), "---\n"),
        (format!("{GOLDEN_CONFIG}frontmatter: toml\n"), "---\n"),
        (format!("{TOML_CONFIG}frontmatter: yaml\n"), "---\n"),
        (format!("{TOML_CONFIG}frontmatter: toml\n"), "+++\n"),
    ] {
        let system = vault(&config_yaml, "note.md", plain);
        let after = comment(&system, &resolve(&system), "note.md", "golden");
        assert!(after.starts_with(opener), "{config_yaml}\n{after}");
        if opener == "+++\n" {
            assert_eq!(toml_block(&after)["title"].as_str(), Some("Plain note"));
        }
    }
}

#[test]
fn a_toml_datetime_reads_as_the_string_yaml_would_store() {
    let text = "+++\ncreated = 2026-01-02T03:04:05+02:00\nzulu = 2026-01-02T03:04:05.5Z\nday = 2026-01-02\n+++\n\nBody.\n";
    let doc = parser::parse(text).unwrap();
    let mapping = parse_existing_frontmatter(&doc, true).unwrap();
    let utc = DateTime::parse_from_rfc3339("2026-01-02T01:04:05Z").unwrap();
    assert_eq!(
        mapping.get("created").and_then(Value::as_str),
        Some(rfc3339_z(&utc).as_str())
    );
    assert_eq!(
        mapping.get("zulu").and_then(Value::as_str),
        Some("2026-01-02T03:04:05.500Z")
    );
    assert_eq!(
        mapping.get("day").and_then(Value::as_str),
        Some("2026-01-02")
    );
    assert!(parse_existing_frontmatter(&doc, false).unwrap().is_empty());

    let system = vault(TOML_CONFIG, "note.md", TOML_NOTE);
    let meta = metadata(
        &system,
        Path::new("/vault"),
        Path::new("note.md"),
        &resolve(&system),
    )
    .unwrap();
    assert_eq!(
        meta.frontmatter
            .unwrap()
            .get("created")
            .and_then(Value::as_str),
        Some("2026-01-02T03:04:05Z")
    );
}

#[test]
fn the_opt_in_is_found_by_walk_up_for_a_manual_identity() {
    let system = MemorySystem::new()
        .with_file(
            Path::new("/vault/.remargin.yaml"),
            b"mode: open\ntoml_frontmatter: true\n",
        )
        .unwrap()
        .with_file(Path::new("/vault/sub/note.md"), TOML_NOTE.as_bytes())
        .unwrap();
    let flags = IdentityFlags {
        author_type: Some(AuthorType::Agent),
        identity: Some(String::from("hub")),
        ..IdentityFlags::default()
    };
    let config = ResolvedConfig::resolve(&system, Path::new("/"), &flags, None).unwrap();
    assert!(config.source_path.is_none());

    let after = comment(&system, &config, "sub/note.md", "golden");
    assert!(after.starts_with("+++\n"), "{after}");
    assert_eq!(toml_block(&after)["author"].as_str(), Some("hub"));
}

#[test]
fn comment_blocks_are_byte_identical_in_both_formats() {
    let body = "\n# Note\n\nBody.\n";

    let yaml_system = vault(
        TOML_CONFIG,
        "note.md",
        &format!("---\ntitle: Note\n---\n{body}"),
    );
    let yaml_after = comment(
        &yaml_system,
        &resolve(&yaml_system),
        "note.md",
        "same words",
    );

    let toml_system = vault(
        TOML_CONFIG,
        "note.md",
        &format!("+++\ntitle = \"Note\"\n+++\n{body}"),
    );
    let toml_after = comment(
        &toml_system,
        &resolve(&toml_system),
        "note.md",
        "same words",
    );

    assert!(yaml_after.starts_with("---\n"));
    assert!(toml_after.starts_with("+++\n"));
    assert_eq!(comment_blocks(&yaml_after), comment_blocks(&toml_after));
}

#[test]
fn links_and_lint_read_toml_frontmatter() {
    let note = "+++\nup = \"Parent\"\nrelated = [\"Sibling\"]\n+++\n\n# Note\n";
    let system = vault(TOML_CONFIG, "note.md", note)
        .with_file(
            Path::new("/vault/Parent.md"),
            b"+++\ntitle = \"The parent\"\n+++\n\nBody.\n",
        )
        .unwrap()
        .with_file(Path::new("/vault/Sibling.md"), b"# Sibling\n")
        .unwrap();
    let config = resolve(&system);
    let got = get_with_links(
        &system,
        Path::new("/vault"),
        Path::new("note.md"),
        None,
        false,
        &config,
    )
    .unwrap();
    let found: Vec<(&str, &[usize], Option<&str>)> = got
        .links
        .iter()
        .map(|link| {
            (
                link.target.as_str(),
                link.lines.as_slice(),
                link.title.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        found,
        vec![
            ("Parent", &[2_usize][..], Some("The parent")),
            ("Sibling", &[3_usize][..], Some("Sibling")),
        ]
    );

    let broken = "+++\ntitle = \n+++\n\nBody.\n";
    let unclosed = "+++\ntitle = \"x\"\n\nBody.\n";
    for (text, expected) in [
        (broken, "invalid TOML in frontmatter"),
        (unclosed, "unclosed TOML frontmatter"),
    ] {
        let opted_in = vault(TOML_CONFIG, "note.md", text);
        let found_errors = lint_doc(&opted_in, Path::new("/vault/note.md"), &config).unwrap();
        assert!(
            found_errors
                .errors
                .iter()
                .any(|err| err.message.starts_with(expected)),
            "{text}: {:?}",
            found_errors.errors
        );

        let not_opted_in = vault(GOLDEN_CONFIG, "note.md", text);
        let clean = lint_doc(&not_opted_in, Path::new("/vault/note.md"), &config).unwrap();
        assert!(clean.errors.is_empty(), "{:?}", clean.errors);
    }

    assert_eq!(fence_of("+++\na = 1\n+++\n", true), Some(Fence::Toml));
    assert_eq!(fence_of("+++\na = 1\n+++\n", false), None);
    assert_eq!(fence_of("---\na: 1\n---\n", true), Some(Fence::Yaml));
    assert_eq!(fence_of("# Plain\n", true), None);
}

#[test]
fn unreadable_frontmatter_settings_leave_every_file_as_before() {
    // No `mode:` here, so the walk carries on to the ancestor's wrong-typed keys too. An ancestor
    // that is not YAML at all already fails `ResolvedConfig::resolve` (its permissions walk).
    let noisy_config =
        "identity: golden\ntype: agent\nfrontmatter: obsidian\ntoml_frontmatter: \"yes\"\n";
    let fixtures = [
        (
            include_str!("tests/golden/yaml_note.md"),
            include_str!("tests/golden/yaml_note.comment.after.md"),
            include_str!("tests/golden/yaml_note.write.after.md"),
        ),
        (
            include_str!("tests/golden/plain_note.md"),
            include_str!("tests/golden/plain_note.comment.after.md"),
            include_str!("tests/golden/plain_note.write.after.md"),
        ),
        (
            TOML_NOTE,
            include_str!("tests/golden/toml_note.comment.after.md"),
            include_str!("tests/golden/toml_note.write.after.md"),
        ),
    ];
    let noisy_vault = |fixture: &str| {
        vault(noisy_config, "note.md", fixture)
            .with_file(
                Path::new("/.remargin.yaml"),
                b"toml_frontmatter: 7\nfrontmatter: [toml]\n",
            )
            .unwrap()
    };
    for (fixture, comment_golden, write_golden) in fixtures {
        let commented = {
            let system = noisy_vault(fixture);
            comment(&system, &resolve(&system), "note.md", "golden")
        };
        assert_eq!(mask_timestamps(&commented), comment_golden);

        let written = {
            let system = noisy_vault(fixture);
            rewrite(&system, &resolve(&system), "note.md")
        };
        assert_eq!(mask_timestamps(&written), write_golden);
    }
}

#[test]
fn the_opt_in_above_the_realm_root_is_ignored() {
    let system = vault(GOLDEN_CONFIG, "note.md", TOML_NOTE)
        .with_file(Path::new("/.remargin.yaml"), b"toml_frontmatter: true\n")
        .unwrap();
    let after = comment(&system, &resolve(&system), "note.md", "golden");
    assert_eq!(
        mask_timestamps(&after),
        include_str!("tests/golden/toml_note.comment.after.md")
    );
}
