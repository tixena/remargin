use std::path::{Path, PathBuf};

use os_shim::mock::MockSystem;
use remargin_core::config;
use remargin_core::config::registry::Registry;
use remargin_core::display::render_activity_cutoff_header;
use serde_json::json;

use super::{
    parse_line_range, registry_participant_json, registry_participant_pretty,
    resolve_comment_content,
};

fn ts(s: &str) -> chrono::DateTime<chrono::FixedOffset> {
    chrono::DateTime::parse_from_rfc3339(s).unwrap()
}

/// implicit cutoff with a caller-last-action ts
/// renders as "(since you last touched this file: …)".
#[test]
fn cutoff_header_implicit_with_last_action() {
    let header = render_activity_cutoff_header(false, Some(ts("2026-04-27T02:09:00-04:00")));
    assert_eq!(
        header,
        "(since you last touched this file: 2026-04-27 02:09)"
    );
}

/// implicit cutoff with no prior activity renders
/// the initial-touch fallback message.
#[test]
fn cutoff_header_implicit_initial_touch() {
    let header = render_activity_cutoff_header(false, None);
    assert!(
        header.contains("since the beginning"),
        "unexpected header: {header}"
    );
    assert!(
        header.contains("no prior activity"),
        "unexpected header: {header}"
    );
}

/// explicit `--since` echoes the cutoff with the
/// "(since …)" wording, matching the user's input.
#[test]
fn cutoff_header_explicit_since() {
    let header = render_activity_cutoff_header(true, Some(ts("2026-04-27T02:09:00-04:00")));
    assert_eq!(header, "(since 2026-04-27 02:09)");
}

/// the placeholder string `YOUR-LAST-ACTION` from
/// the design discussion must never reach user-visible output.
#[test]
fn cutoff_header_never_emits_placeholder() {
    for explicit in [true, false] {
        let with_ts =
            render_activity_cutoff_header(explicit, Some(ts("2026-04-27T02:09:00-04:00")));
        let without_ts = render_activity_cutoff_header(explicit, None);
        assert!(!with_ts.contains("YOUR-LAST-ACTION"), "{with_ts}");
        assert!(!without_ts.contains("YOUR-LAST-ACTION"), "{without_ts}");
    }
}

#[test]
fn parse_line_range_accepts_simple_pair() {
    let (s, e) = parse_line_range("10-20").unwrap();
    assert_eq!((s, e), (10, 20));
}

#[test]
fn parse_line_range_accepts_single_line_range() {
    let (s, e) = parse_line_range("7-7").unwrap();
    assert_eq!((s, e), (7, 7));
}

#[test]
fn parse_line_range_rejects_missing_dash() {
    let err = parse_line_range("100").unwrap_err();
    assert!(err.to_string().contains("START-END"));
}

#[test]
fn parse_line_range_rejects_non_numeric() {
    let err = parse_line_range("a-b").unwrap_err();
    assert!(err.to_string().contains("invalid start value"));
}

#[test]
fn parse_line_range_rejects_non_numeric_end() {
    let err = parse_line_range("1-b").unwrap_err();
    assert!(err.to_string().contains("invalid end value"));
}

#[test]
fn content_from_positional_arg() {
    let system = MockSystem::new();
    let cwd = Path::new("/project");
    let content = String::from("Hello from arg");

    let result = resolve_comment_content(&system, cwd, Some(&content), None).unwrap();
    assert_eq!(result, "Hello from arg");
}

#[test]
fn content_from_file() {
    let system = MockSystem::new()
        .with_file(Path::new("/project/comment.txt"), b"Hello from file")
        .unwrap();
    let cwd = Path::new("/project");
    let path = PathBuf::from("comment.txt");

    let result = resolve_comment_content(&system, cwd, None, Some(&path)).unwrap();
    assert_eq!(result, "Hello from file");
}

#[test]
fn content_from_absolute_file_path() {
    let system = MockSystem::new()
        .with_file(Path::new("/elsewhere/note.md"), b"Absolute path content")
        .unwrap();
    let cwd = Path::new("/project");
    let path = PathBuf::from("/elsewhere/note.md");

    let result = resolve_comment_content(&system, cwd, None, Some(&path)).unwrap();
    assert_eq!(result, "Absolute path content");
}

#[test]
fn error_when_neither_content_nor_file() {
    let system = MockSystem::new();
    let cwd = Path::new("/project");

    let err = resolve_comment_content(&system, cwd, None, None).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("comment body required"),
        "unexpected error: {msg}",
    );
}

#[test]
fn error_when_file_not_found() {
    let system = MockSystem::new();
    let cwd = Path::new("/project");
    let path = PathBuf::from("missing.txt");

    let err = resolve_comment_content(&system, cwd, None, Some(&path)).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("reading comment body from"),
        "unexpected error: {msg}",
    );
}

fn registry_with_yaml(yaml: &str) -> Registry {
    let system = MockSystem::new()
        .with_file(
            Path::new("/project/.remargin-registry.yaml"),
            yaml.as_bytes(),
        )
        .unwrap();
    config::load_registry(&system, Path::new("/project"))
        .unwrap()
        .unwrap()
}

#[test]
fn registry_json_includes_display_name_when_set() {
    let registry = registry_with_yaml(
        "\
participants:
  alice:
    display_name: \"Alice Doe\"
    type: human
    status: active
    pubkeys:
      - \"ssh-ed25519 AAAA...\"
",
    );
    let alice = &registry.participants["alice"];
    let value = registry_participant_json("alice", alice);
    assert_eq!(
        value,
        json!({
            "name": "alice",
            "display_name": "Alice Doe",
            "type": "human",
            "status": "active",
            "pubkeys": 1_u64,
        })
    );
}

#[test]
fn registry_json_falls_back_to_name_when_display_name_absent() {
    let registry = registry_with_yaml(
        "\
participants:
  ci-bot:
    type: agent
    status: active
    pubkeys: []
",
    );
    let bot = &registry.participants["ci-bot"];
    let value = registry_participant_json("ci-bot", bot);
    assert_eq!(
        value,
        json!({
            "name": "ci-bot",
            "display_name": "ci-bot",
            "type": "agent",
            "status": "active",
            "pubkeys": 0_u64,
        })
    );
}

#[test]
fn registry_pretty_with_display_name() {
    let registry = registry_with_yaml(
        "\
participants:
  alice:
    display_name: \"Alice Doe\"
    type: human
    status: active
    pubkeys:
      - \"ssh-ed25519 AAAA...\"
",
    );
    let alice = &registry.participants["alice"];
    assert_eq!(
        registry_participant_pretty("alice", alice),
        "\"Alice Doe\" (alice) (human) [active] 1 key(s)",
    );
}

#[test]
fn registry_pretty_without_display_name() {
    let registry = registry_with_yaml(
        "\
participants:
  alice:
    type: human
    status: active
    pubkeys:
      - \"ssh-ed25519 AAAA...\"
",
    );
    let alice = &registry.participants["alice"];
    assert_eq!(
        registry_participant_pretty("alice", alice),
        "alice (human) [active] 1 key(s)",
    );
}
