//! Unit tests for [`crate::activity::gather_activity`] (rem-g3sy.3 /
//! T33).
//!
//! Covers scenarios 1-21 from the rem-g3sy.3 plan.

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
