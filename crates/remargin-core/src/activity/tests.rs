//! Unit tests for [`crate::activity::gather_activity`].

use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset};
use os_shim::mock::MockSystem;

use crate::activity::{Change, gather_activity};

const REALM_YAML: &str = "identity: alice\ntype: human\n";

fn ts(s: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(s).unwrap()
}

fn realm_with(files: &[(&str, &str)]) -> MockSystem {
    let mut system = MockSystem::new()
        .with_file(Path::new("/r/.remargin.yaml"), REALM_YAML.as_bytes())
        .unwrap();
    for (path, body) in files {
        system = system.with_file(Path::new(path), body.as_bytes()).unwrap();
    }
    system
}

/// Build a managed `.md` body with one comment matching the
/// supplied parameters. Checksum is a placeholder; the activity
/// path does not verify content.
fn doc_with_comment(
    id: &str,
    author: &str,
    ts_str: &str,
    edited_at: Option<&str>,
    ack_lines: &[&str],
) -> String {
    use core::fmt::Write as _;
    let edited_line = edited_at.map_or_else(String::new, |s| format!("edited_at: {s}\n"));
    let mut ack_block = String::new();
    if !ack_lines.is_empty() {
        ack_block.push_str("ack:\n");
        for line in ack_lines {
            let _ = writeln!(ack_block, "  - {line}");
        }
    }
    format!(
        "---\ntitle: t\n---\n\n# Body\n\n```remargin\n---\nid: {id}\nauthor: {author}\ntype: human\nts: {ts_str}\n{edited_line}checksum: sha256:test\n{ack_block}---\nBody.\n```\n"
    )
}

fn doc_with_sandbox(entries: &[&str]) -> String {
    use core::fmt::Write as _;
    let mut sandbox_block = String::new();
    if entries.is_empty() {
        sandbox_block.push_str("sandbox: []\n");
    } else {
        sandbox_block.push_str("sandbox:\n");
        for line in entries {
            let _ = writeln!(sandbox_block, "  - {line}");
        }
    }
    format!("---\ntitle: t\n{sandbox_block}---\n\n# Body.\n")
}

/// Scenario 1: file with no comments and no sandbox returns no
/// changes; the file is omitted from the result entirely.
#[test]
fn empty_file_is_omitted() {
    let body = "---\ntitle: t\n---\n\n# Body.\n";
    let system = realm_with(&[("/r/note.md", body)]);
    let result = gather_activity(&system, Path::new("/r/note.md"), None, "alice").unwrap();
    assert!(result.files.is_empty());
    assert!(result.newest_ts_overall.is_none());
}

/// Scenario 2: caller has no prior activity in the file → the
/// initial-touch fallback returns every change.
#[test]
fn initial_touch_fallback_returns_everything() {
    let body = doc_with_comment("c1", "bob", "2026-04-06T12:00:00-04:00", None, &[]);
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let result = gather_activity(&system, Path::new("/r/note.md"), None, "alice").unwrap();
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].changes.len(), 1);
    assert!(matches!(result.files[0].changes[0], Change::Comment { .. }));
}

/// Scenario 3: an explicit `since` cutoff surfaces changes after
/// the cutoff.
#[test]
fn explicit_since_cutoff_surfaces_after_only() {
    let body = doc_with_comment("c1", "bob", "2026-04-06T12:00:00-04:00", None, &[]);
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let cutoff = ts("2026-04-06T11:00:00-04:00");
    let result = gather_activity(&system, Path::new("/r/note.md"), Some(cutoff), "alice").unwrap();
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].changes.len(), 1);
}

/// Scenario 4: a comment that pre-dates `since` is dropped.
#[test]
fn comment_before_since_is_dropped() {
    let body = doc_with_comment("c1", "bob", "2026-04-06T12:00:00-04:00", None, &[]);
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let cutoff = ts("2026-04-06T13:00:00-04:00");
    let result = gather_activity(&system, Path::new("/r/note.md"), Some(cutoff), "alice").unwrap();
    assert!(result.files.is_empty());
}

/// Scenario 5: an edited comment surfaces with the carried ts =
/// `edited_at` when the edit is past the cutoff.
#[test]
fn edited_comment_surfaces_with_edited_ts() {
    let body = doc_with_comment(
        "c1",
        "bob",
        "2026-04-06T12:00:00-04:00",
        Some("2026-04-06T15:00:00-04:00"),
        &[],
    );
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let cutoff = ts("2026-04-06T13:00:00-04:00");
    let result = gather_activity(&system, Path::new("/r/note.md"), Some(cutoff), "alice").unwrap();
    assert_eq!(result.files.len(), 1);
    let change = &result.files[0].changes[0];
    assert!(matches!(change, Change::Comment { .. }));
    if let Change::Comment {
        ts: change_ts,
        comment_id,
        ..
    } = change
    {
        assert_eq!(comment_id, "c1");
        assert_eq!(change_ts, &ts("2026-04-06T15:00:00-04:00"));
    }
}

/// Scenario 6: an edit that pre-dates `since` is dropped.
#[test]
fn edit_before_since_is_dropped() {
    let body = doc_with_comment(
        "c1",
        "bob",
        "2026-04-06T12:00:00-04:00",
        Some("2026-04-06T13:00:00-04:00"),
        &[],
    );
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let cutoff = ts("2026-04-06T14:00:00-04:00");
    let result = gather_activity(&system, Path::new("/r/note.md"), Some(cutoff), "alice").unwrap();
    assert!(result.files.is_empty());
}

/// Scenario 7: an ack on a comment surfaces independently of the
/// comment itself.
#[test]
fn ack_surfaces_after_cutoff() {
    let body = doc_with_comment(
        "c1",
        "bob",
        "2026-04-06T12:00:00-04:00",
        None,
        &["bob@2026-04-06T13:00:00-04:00"],
    );
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let cutoff = ts("2026-04-06T12:30:00-04:00");
    let result = gather_activity(&system, Path::new("/r/note.md"), Some(cutoff), "alice").unwrap();
    let ack_count = result.files[0]
        .changes
        .iter()
        .filter(|c| matches!(c, Change::Ack { .. }))
        .count();
    assert_eq!(ack_count, 1);
}

/// Scenario 8: multiple acks on the same comment produce
/// per-author `Change::Ack` entries.
#[test]
fn multiple_acks_each_produce_a_change() {
    let body = doc_with_comment(
        "c1",
        "bob",
        "2026-04-06T11:00:00-04:00",
        None,
        &[
            "bob@2026-04-06T12:00:00-04:00",
            "carol@2026-04-06T13:00:00-04:00",
        ],
    );
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let cutoff = ts("2026-04-06T11:30:00-04:00");
    let result = gather_activity(&system, Path::new("/r/note.md"), Some(cutoff), "alice").unwrap();
    let ack_count = result.files[0]
        .changes
        .iter()
        .filter(|c| matches!(c, Change::Ack { .. }))
        .count();
    assert_eq!(ack_count, 2);
}

/// Scenario 9: a sandbox-roster entry surfaces as `Change::Sandbox`.
#[test]
fn sandbox_entry_surfaces() {
    let body = doc_with_sandbox(&["bob@2026-04-06T12:00:00-04:00"]);
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let cutoff = ts("2026-04-06T11:00:00-04:00");
    let result = gather_activity(&system, Path::new("/r/note.md"), Some(cutoff), "alice").unwrap();
    let sandbox_count = result.files[0]
        .changes
        .iter()
        .filter(|c| matches!(c, Change::Sandbox { .. }))
        .count();
    assert_eq!(sandbox_count, 1);
}

/// Scenario 11: caller's last action drives the cutoff. Alice
/// commented at T1, acked at T3, and sandboxed at T2; the cutoff
/// is T3 (the latest), so a bob comment at T2 is dropped but a
/// new comment at T4 is surfaced.
#[test]
fn caller_last_action_derives_cutoff() {
    let prefix = "---\ntitle: t\nsandbox:\n  - alice@2026-04-06T13:00:00-04:00\n---\n\n# Body\n";
    let c1 = "```remargin\n---\nid: a1\nauthor: alice\ntype: human\nts: 2026-04-06T12:00:00-04:00\nchecksum: sha256:t\nack:\n  - alice@2026-04-06T14:00:00-04:00\n---\nMine.\n```";
    let c2 = "```remargin\n---\nid: b2\nauthor: bob\ntype: human\nts: 2026-04-06T13:30:00-04:00\nchecksum: sha256:t\n---\nDropped.\n```";
    let c3 = "```remargin\n---\nid: b3\nauthor: bob\ntype: human\nts: 2026-04-06T15:00:00-04:00\nchecksum: sha256:t\n---\nKept.\n```";
    let body = format!("{prefix}\n{c1}\n{c2}\n{c3}");
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let result = gather_activity(&system, Path::new("/r/note.md"), None, "alice").unwrap();
    let comment_ids: Vec<&str> = result.files[0]
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::Comment { comment_id, .. } => Some(comment_id.as_str()),
            Change::Ack { .. } | Change::Sandbox { .. } => None,
        })
        .collect();
    assert_eq!(
        comment_ids,
        vec!["b3"],
        "expected only b3 past 14:00 cutoff"
    );
}

/// Scenario 13: directory walk returns one `FileChanges` per file
/// with activity.
#[test]
fn directory_walk_returns_one_entry_per_file_with_activity() {
    let a_body = doc_with_comment("a1", "bob", "2026-04-06T12:00:00-04:00", None, &[]);
    let b_body = "---\ntitle: t\n---\n\n# empty\n";
    let c_body = doc_with_comment("c1", "bob", "2026-04-06T13:00:00-04:00", None, &[]);
    let system = realm_with(&[
        ("/r/a.md", a_body.as_str()),
        ("/r/b.md", b_body),
        ("/r/c.md", c_body.as_str()),
    ]);
    let result = gather_activity(&system, Path::new("/r"), None, "alice").unwrap();
    assert_eq!(result.files.len(), 2);
    assert_eq!(result.files[0].path, PathBuf::from("/r/a.md"));
    assert_eq!(result.files[1].path, PathBuf::from("/r/c.md"));
}

/// Scenario 16: a path outside any realm errors with a clear
/// message.
#[test]
fn path_outside_realm_errors() {
    let system = MockSystem::new()
        .with_file(Path::new("/elsewhere/note.md"), b"# hi")
        .unwrap();
    let err = gather_activity(&system, Path::new("/elsewhere/note.md"), None, "alice").unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("outside any .remargin.yaml"),
        "expected outside-realm error, got: {msg}"
    );
}

/// Scenario 17: tie-breaker sorts by kind then id when timestamps
/// match.
#[test]
fn tie_breaker_sorts_by_kind_then_id() {
    let a = "```remargin\n---\nid: zzz\nauthor: bob\ntype: human\nts: 2026-04-06T12:00:00-04:00\nchecksum: sha256:t\n---\nA.\n```";
    let b = "```remargin\n---\nid: aaa\nauthor: bob\ntype: human\nts: 2026-04-06T12:00:00-04:00\nchecksum: sha256:t\n---\nB.\n```";
    let body = format!("---\ntitle: t\n---\n\n# Body\n\n{a}\n\n{b}");
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let result = gather_activity(&system, Path::new("/r/note.md"), None, "alice").unwrap();
    let ids: Vec<&str> = result.files[0]
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::Comment { comment_id, .. } => Some(comment_id.as_str()),
            Change::Ack { .. } | Change::Sandbox { .. } => None,
        })
        .collect();
    assert_eq!(ids, vec!["aaa", "zzz"]);
}

/// Scenario 19: a comment without `reply_to` serialises without
/// the field.
#[test]
fn comment_without_reply_to_omits_field_in_json() {
    let body = doc_with_comment("c1", "bob", "2026-04-06T12:00:00-04:00", None, &[]);
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let result = gather_activity(&system, Path::new("/r/note.md"), None, "alice").unwrap();
    let json = serde_json::to_string(&result.files[0].changes[0]).unwrap();
    assert!(!json.contains("reply_to"), "{json}");
}

/// Every change-kind exposes the actor under the field name
/// `author` (uniform JSON shape) so consumers do not case-analyse on
/// `kind`.
#[test]
fn every_change_kind_serialises_actor_as_author() {
    let prefix = "---\ntitle: t\nsandbox:\n  - bob@2026-04-06T13:00:00-04:00\n---\n\n# Body\n";
    let comment = "```remargin\n---\nid: c1\nauthor: bob\ntype: human\nts: 2026-04-06T12:00:00-04:00\nchecksum: sha256:t\nack:\n  - carol@2026-04-06T14:00:00-04:00\n---\nBody.\n```";
    let body = format!("{prefix}\n{comment}\n");
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let result = gather_activity(&system, Path::new("/r/note.md"), None, "alice").unwrap();
    let value = serde_json::to_value(&result.files[0].changes).unwrap();
    let array = value.as_array().unwrap();
    assert_eq!(array.len(), 3, "expected one of each kind, got {value}");
    for change in array {
        let json = change.as_object().unwrap();
        let kind = json
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap();
        assert!(
            json.contains_key("author"),
            "kind={kind} missing author field: {change}"
        );
        assert!(
            !json.contains_key("by"),
            "kind={kind} still emits legacy `by` field: {change}"
        );
    }
}

/// When the registry resolves the actor, sandbox and ack records
/// carry `author_type`. When the registry is silent, the field is
/// omitted (skipped on serialise) rather than guessed.
#[test]
fn sandbox_and_ack_carry_author_type_when_registry_resolves() {
    let registry =
        "participants:\n  bob:\n    type: human\n  carol:\n    type: agent\n    status: active\n";
    let prefix = "---\ntitle: t\nsandbox:\n  - bob@2026-04-06T13:00:00-04:00\n  - dave@2026-04-06T13:30:00-04:00\n---\n\n# Body\n";
    let comment = "```remargin\n---\nid: c1\nauthor: bob\ntype: human\nts: 2026-04-06T12:00:00-04:00\nchecksum: sha256:t\nack:\n  - carol@2026-04-06T14:00:00-04:00\n  - eve@2026-04-06T14:30:00-04:00\n---\nBody.\n```";
    let body = format!("{prefix}\n{comment}\n");
    let system = MockSystem::new()
        .with_file(Path::new("/r/.remargin.yaml"), REALM_YAML.as_bytes())
        .unwrap()
        .with_file(Path::new("/r/.remargin-registry.yaml"), registry.as_bytes())
        .unwrap()
        .with_file(Path::new("/r/note.md"), body.as_bytes())
        .unwrap();
    let result = gather_activity(&system, Path::new("/r/note.md"), None, "alice").unwrap();
    let mut bob_sandbox: Option<String> = None;
    let mut dave_sandbox: Option<Option<String>> = None;
    let mut carol_ack: Option<String> = None;
    let mut eve_ack: Option<Option<String>> = None;
    for change in &result.files[0].changes {
        match change {
            Change::Sandbox {
                author,
                author_type,
                ..
            } if author == "bob" => bob_sandbox = author_type.clone(),
            Change::Sandbox {
                author,
                author_type,
                ..
            } if author == "dave" => dave_sandbox = Some(author_type.clone()),
            Change::Ack {
                author,
                author_type,
                ..
            } if author == "carol" => carol_ack = author_type.clone(),
            Change::Ack {
                author,
                author_type,
                ..
            } if author == "eve" => eve_ack = Some(author_type.clone()),
            Change::Ack { .. } | Change::Comment { .. } | Change::Sandbox { .. } => {}
        }
    }
    assert_eq!(bob_sandbox.as_deref(), Some("human"));
    assert_eq!(carol_ack.as_deref(), Some("agent"));
    assert_eq!(
        dave_sandbox,
        Some(None),
        "registry-missing dave should yield None"
    );
    assert_eq!(
        eve_ack,
        Some(None),
        "registry-missing eve should yield None"
    );
}

/// Implicit cutoff for a caller whose most recent action was an
/// edit pins to `edited_at`, not the original `ts`. Earlier
/// activity from the same caller is excluded from the cutoff fold.
#[test]
fn cutoff_uses_edited_at_when_caller_last_action_was_an_edit() {
    let alice_edit = "```remargin\n---\nid: a1\nauthor: alice\ntype: human\nts: 2026-04-06T08:00:00-04:00\nedited_at: 2026-04-06T16:00:00-04:00\nchecksum: sha256:t\n---\nMine.\n```";
    let bob_between = "```remargin\n---\nid: b1\nauthor: bob\ntype: human\nts: 2026-04-06T13:00:00-04:00\nchecksum: sha256:t\n---\nBefore.\n```";
    let bob_after = "```remargin\n---\nid: b2\nauthor: bob\ntype: human\nts: 2026-04-06T17:00:00-04:00\nchecksum: sha256:t\n---\nAfter.\n```";
    let body =
        format!("---\ntitle: t\n---\n\n# Body\n\n{alice_edit}\n\n{bob_between}\n\n{bob_after}\n");
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let result = gather_activity(&system, Path::new("/r/note.md"), None, "alice").unwrap();
    let file = &result.files[0];
    assert_eq!(
        file.cutoff_applied,
        Some(ts("2026-04-06T16:00:00-04:00")),
        "cutoff should be alice's edited_at, got {:?}",
        file.cutoff_applied
    );
    let comment_ids: Vec<&str> = file
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::Comment { comment_id, .. } => Some(comment_id.as_str()),
            Change::Ack { .. } | Change::Sandbox { .. } => None,
        })
        .collect();
    assert_eq!(
        comment_ids,
        vec!["b2"],
        "only b2 (after the 16:00 cutoff) should surface; got {comment_ids:?}"
    );
}

/// Explicit `--since` propagates `cutoff_explicit=true` onto the
/// result and the same explicit cutoff lands on every per-file
/// record.
#[test]
fn explicit_since_marks_result_cutoff_explicit() {
    let body = doc_with_comment("c1", "bob", "2026-04-06T12:00:00-04:00", None, &[]);
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let cutoff = ts("2026-04-06T11:00:00-04:00");
    let result = gather_activity(&system, Path::new("/r/note.md"), Some(cutoff), "alice").unwrap();
    assert!(result.cutoff_explicit, "expected cutoff_explicit=true");
    assert_eq!(result.files[0].cutoff_applied, Some(cutoff));
}

/// Implicit cutoff with no prior caller activity surfaces
/// `cutoff_applied=None` (the initial-touch fallback) and
/// `cutoff_explicit=false`.
#[test]
fn implicit_initial_touch_records_no_cutoff() {
    let body = doc_with_comment("c1", "bob", "2026-04-06T12:00:00-04:00", None, &[]);
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let result = gather_activity(&system, Path::new("/r/note.md"), None, "alice").unwrap();
    assert!(!result.cutoff_explicit);
    assert_eq!(result.files[0].cutoff_applied, None);
}

/// Scenario 21: per-file `newest_ts` matches the largest ts in
/// the changes list.
#[test]
fn newest_ts_matches_largest_change_ts() {
    let a = "```remargin\n---\nid: a1\nauthor: bob\ntype: human\nts: 2026-04-06T12:00:00-04:00\nchecksum: sha256:t\n---\nA.\n```";
    let b = "```remargin\n---\nid: b1\nauthor: bob\ntype: human\nts: 2026-04-06T15:00:00-04:00\nchecksum: sha256:t\n---\nB.\n```";
    let c = "```remargin\n---\nid: c1\nauthor: bob\ntype: human\nts: 2026-04-06T13:00:00-04:00\nchecksum: sha256:t\n---\nC.\n```";
    let body = format!("---\ntitle: t\n---\n\n# Body\n\n{a}\n\n{b}\n\n{c}");
    let system = realm_with(&[("/r/note.md", body.as_str())]);
    let result = gather_activity(&system, Path::new("/r/note.md"), None, "alice").unwrap();
    assert_eq!(
        result.files[0].newest_ts,
        Some(ts("2026-04-06T15:00:00-04:00"))
    );
    assert_eq!(
        result.newest_ts_overall,
        Some(ts("2026-04-06T15:00:00-04:00"))
    );
}

/// Compact projection: each kind fills its columns and nulls the rest.
/// `kind` keeps the serde tag values; comment-only columns are null for
/// acks / sandboxes; sandboxes also null `comment_id`.
#[test]
fn compact_row_columns_per_kind() {
    use crate::activity::{CHANGE_COLS, to_compact_row};

    assert_eq!(
        CHANGE_COLS,
        [
            "ts",
            "kind",
            "author",
            "author_type",
            "comment_id",
            "line_start",
            "line_end",
            "reply_to",
            "to",
        ]
    );

    let t = ts("2026-04-06T12:00:00-04:00");

    let comment = Change::Comment {
        author: String::from("bob"),
        author_type: String::from("human"),
        comment_id: String::from("c1"),
        line_end: 12,
        line_start: 9,
        reply_to: Some(String::from("c0")),
        to: vec![String::from("alice")],
        ts: t,
    };
    let comment_row = to_compact_row(&comment);
    assert_eq!(comment_row.0, t);
    assert_eq!(comment_row.1, "comment");
    assert_eq!(comment_row.2, "bob");
    assert_eq!(comment_row.3.as_deref(), Some("human"));
    assert_eq!(comment_row.4.as_deref(), Some("c1"));
    assert_eq!(comment_row.5, Some(9));
    assert_eq!(comment_row.6, Some(12));
    assert_eq!(comment_row.7.as_deref(), Some("c0"));
    assert_eq!(comment_row.8, Some(vec![String::from("alice")]));

    let ack = Change::Ack {
        author: String::from("bob"),
        author_type: None,
        comment_id: String::from("c1"),
        ts: t,
    };
    let ack_row = to_compact_row(&ack);
    assert_eq!(ack_row.1, "ack");
    assert_eq!(ack_row.4.as_deref(), Some("c1"));
    assert!(ack_row.3.is_none(), "unknown author_type null");
    assert!(ack_row.5.is_none() && ack_row.6.is_none(), "ack lines null");
    assert!(ack_row.7.is_none(), "ack reply_to null");
    assert!(ack_row.8.is_none(), "ack to not-applicable is null");

    let sandbox = Change::Sandbox {
        author: String::from("alice"),
        author_type: Some(String::from("human")),
        ts: t,
    };
    let sandbox_row = to_compact_row(&sandbox);
    assert_eq!(sandbox_row.1, "sandbox");
    assert_eq!(sandbox_row.3.as_deref(), Some("human"));
    assert!(sandbox_row.4.is_none(), "sandbox comment_id null");
    assert!(
        sandbox_row.5.is_none() && sandbox_row.6.is_none(),
        "sandbox lines null"
    );
    assert!(sandbox_row.7.is_none(), "sandbox reply_to null");
    assert!(sandbox_row.8.is_none(), "sandbox to null");
}

/// A broadcast comment (empty `to`) compacts to `Some([])`, preserving the
/// broadcast signal against acks / sandboxes whose `to` is not-applicable
/// (`null`).
#[test]
fn compact_row_broadcast_to_is_some_empty_vs_null() {
    use crate::activity::to_compact_row;

    let t = ts("2026-04-06T12:00:00-04:00");
    let broadcast = Change::Comment {
        author: String::from("bob"),
        author_type: String::from("human"),
        comment_id: String::from("c1"),
        line_end: 3,
        line_start: 1,
        reply_to: None,
        to: Vec::new(),
        ts: t,
    };
    let row = to_compact_row(&broadcast);
    assert_eq!(row.8, Some(Vec::new()), "broadcast to = Some([])");
    assert!(row.7.is_none(), "no reply_to is null");

    let ack = Change::Ack {
        author: String::from("bob"),
        author_type: None,
        comment_id: String::from("c1"),
        ts: t,
    };
    assert!(
        to_compact_row(&ack).8.is_none(),
        "ack to null, distinct from broadcast Some([])"
    );
}

/// End-to-end envelope: comment + ack + sandbox in one file surface as
/// three positional rows under one `change_cols` header; per-file summary
/// keys stay named, `cutoff_applied` is named under an explicit `since`.
#[test]
fn compact_activity_envelope_shape() {
    use crate::activity::{CHANGE_COLS, to_compact_activity};

    let body = "---\ntitle: t\nsandbox:\n  - alice@2026-04-06T17:00:00-04:00\n---\n\n# Body\n\n```remargin\n---\nid: c1\nauthor: bob\ntype: human\nts: 2026-04-06T12:00:00-04:00\nto: [carol]\nchecksum: sha256:test\nack:\n  - carol@2026-04-06T14:00:00-04:00\n---\nHello.\n```\n";
    let system = realm_with(&[("/r/note.md", body)]);
    let since = ts("2026-01-01T00:00:00-04:00");
    let result = gather_activity(&system, Path::new("/r/note.md"), Some(since), "alice").unwrap();
    let compact = to_compact_activity(&result);

    assert_eq!(compact["cutoff_explicit"], serde_json::json!(true));
    assert_eq!(compact["change_cols"], serde_json::json!(CHANGE_COLS));
    assert!(compact["newest_ts_overall"].is_string());

    let files = compact["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    let file = &files[0];
    assert!(file["path"].as_str().unwrap().ends_with("note.md"));
    assert!(file["newest_ts"].is_string());
    assert_eq!(
        file["cutoff_applied"].as_str().unwrap(),
        "2026-01-01T00:00:00-04:00"
    );

    let rows = file["changes"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    // Sorted by ts ascending: comment (12:00), ack (14:00), sandbox (17:00).
    let comment = rows[0].as_array().unwrap();
    assert_eq!(comment.len(), 9);
    assert_eq!(comment[1], "comment");
    assert_eq!(comment[4], "c1");
    assert_eq!(comment[8], serde_json::json!(["carol"]));
    let ack = rows[1].as_array().unwrap();
    assert_eq!(ack.len(), 9);
    assert_eq!(ack[1], "ack");
    assert_eq!(ack[4], "c1");
    assert!(ack[5].is_null() && ack[6].is_null());
    assert!(ack[8].is_null(), "ack to null");
    let sandbox = rows[2].as_array().unwrap();
    assert_eq!(sandbox.len(), 9);
    assert_eq!(sandbox[1], "sandbox");
    assert!(sandbox[4].is_null(), "sandbox comment_id null");
    assert!(sandbox[8].is_null(), "sandbox to null");
}

/// Codegen contract: the compact change-row alias renders its `Option`
/// tuple columns as nullable in TS and Zod (relies on the pinned tixschema
/// nullable-in-tuple support), and the per-file record carries the row by
/// reference.
#[test]
fn compact_change_row_schema_renders_nullable_columns() {
    use crate::activity::{compact_change_row_schema, compact_file_changes_schema};

    let row_ts = compact_change_row_schema::Schema::ts_definition();
    assert!(
        row_ts.contains("string | null"),
        "TS nullable columns: {row_ts}"
    );
    let row_zod = compact_change_row_schema::Schema::zod_schema();
    assert!(
        row_zod.contains("z.nullable(z.string())"),
        "Zod nullable columns: {row_zod}"
    );

    let file_ts = compact_file_changes_schema::Schema::ts_definition();
    assert!(
        file_ts.contains("Array<CompactChangeRow>"),
        "record references row type: {file_ts}"
    );
    let file_zod = compact_file_changes_schema::Schema::zod_schema();
    assert!(
        file_zod.contains("z.array(CompactChangeRow$Schema)"),
        "record Zod references row schema: {file_zod}"
    );
}
