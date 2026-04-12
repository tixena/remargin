//! Tests for the comment block parser.

use std::path::Path;

use os_shim::mock::MockSystem;

use super::{AuthorType, LegacyRole, Segment, parse, parse_file};

/// Build a minimal valid remargin block.
fn minimal_block(id: &str) -> String {
    format!(
        "```remargin\n\
         ---\n\
         id: {id}\n\
         author: testuser\n\
         type: human\n\
         ts: 2026-04-06T14:32:00-04:00\n\
         checksum: sha256:abc123\n\
         ---\n\
         ```\n"
    )
}

/// Build a remargin block with custom content.
fn block_with_content(id: &str, content: &str) -> String {
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

#[test]
fn test_simple_comment() {
    let doc = minimal_block("abc");
    let parsed = parse(&doc).unwrap();
    let comments = parsed.comments();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, "abc");
    assert_eq!(comments[0].author, "testuser");
    assert_eq!(comments[0].author_type, AuthorType::Human);
    assert_eq!(comments[0].checksum, "sha256:abc123");
    // fence_depth is no longer stored on Comment; it is computed at serialization time.
}

#[test]
fn test_multiple_comments_with_body() {
    let doc = format!(
        "# Title\n\nSome intro text.\n\n{}\n\nMiddle paragraph.\n\n{}\n\nEnd.\n\n{}\n",
        minimal_block("a01"),
        minimal_block("b02"),
        minimal_block("c03"),
    );
    let parsed = parse(&doc).unwrap();
    let comments = parsed.comments();
    assert_eq!(comments.len(), 3);
    assert_eq!(comments[0].id, "a01");
    assert_eq!(comments[1].id, "b02");
    assert_eq!(comments[2].id, "c03");

    // Body segments should exist between comments.
    let body_count = parsed
        .segments
        .iter()
        .filter(|s| matches!(s, Segment::Body(_)))
        .count();
    assert!(body_count >= 3, "expected at least 3 body segments");
}

#[test]
fn test_required_fields_only() {
    let doc = minimal_block("xyz");
    let parsed = parse(&doc).unwrap();
    let c = parsed.comments()[0];
    assert!(c.to.is_empty());
    assert!(c.reply_to.is_none());
    assert!(c.thread.is_none());
    assert!(c.signature.is_none());
    assert!(c.attachments.is_empty());
    assert!(c.ack.is_empty());
    assert!(c.reactions.is_empty());
}

#[test]
fn test_all_fields_present() {
    let doc = "\
````remargin
---
id: full
author: eduardo
type: agent
ts: 2026-04-06T14:32:00-04:00
checksum: sha256:deadbeef
to: [jorge, claude]
reply-to: abc
thread: t01
attachments: [diagram.png, notes.pdf]
reactions:
  thumbsup: [eduardo, jorge]
  heart: [claude]
ack:
  - jorge@2026-04-06T15:00:00-04:00
  - claude@2026-04-06T15:05:00-04:00
signature: ed25519:base64signature==
---
This is the comment body.
````
";
    let parsed = parse(doc).unwrap();
    let c = parsed.comments()[0];
    assert_eq!(c.id, "full");
    assert_eq!(c.author, "eduardo");
    assert_eq!(c.author_type, AuthorType::Agent);
    assert_eq!(c.checksum, "sha256:deadbeef");
    assert_eq!(c.to, vec!["jorge", "claude"]);
    assert_eq!(c.reply_to.as_deref(), Some("abc"));
    assert_eq!(c.thread.as_deref(), Some("t01"));
    assert_eq!(c.attachments, vec!["diagram.png", "notes.pdf"]);
    assert_eq!(c.reactions.len(), 2);
    assert_eq!(c.reactions["thumbsup"], vec!["eduardo", "jorge"]);
    assert_eq!(c.reactions["heart"], vec!["claude"]);
    assert_eq!(c.ack.len(), 2);
    assert_eq!(c.ack[0].author, "jorge");
    assert_eq!(c.ack[1].author, "claude");
    assert_eq!(c.signature.as_deref(), Some("ed25519:base64signature=="));
    // fence_depth is no longer stored on Comment; it is computed at serialization time.
    assert_eq!(c.content, "This is the comment body.");
}

#[test]
fn test_empty_content() {
    let doc = minimal_block("empty");
    let parsed = parse(&doc).unwrap();
    let c = parsed.comments()[0];
    assert!(c.content.is_empty());
}

#[test]
fn test_legacy_user_comment() {
    let doc = "\
```user comments
This is legacy feedback.
```
";
    let parsed = parse(doc).unwrap();
    let legacy = parsed.legacy_comments();
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].role, LegacyRole::User);
    assert!(legacy[0].done_date.is_none());
    assert_eq!(legacy[0].content, "This is legacy feedback.\n");
}

#[test]
fn test_legacy_done_marker() {
    let doc = "\
```user comments [done:2026-04-05]
Addressed in previous revision.
```
";
    let parsed = parse(doc).unwrap();
    let legacy = parsed.legacy_comments();
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].role, LegacyRole::User);
    assert_eq!(legacy[0].done_date.as_deref(), Some("2026-04-05"));
}

#[test]
fn test_legacy_agent_comment() {
    let doc = "\
```agent comments
This is an agent response.
```
";
    let parsed = parse(doc).unwrap();
    let legacy = parsed.legacy_comments();
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].role, LegacyRole::Agent);
    assert!(legacy[0].done_date.is_none());
}

#[test]
fn test_mixed_old_and_new() {
    let doc = format!(
        "# Document\n\n{}\nSome text.\n\n```user comments\nOld feedback.\n```\n",
        minimal_block("new1"),
    );
    let parsed = parse(&doc).unwrap();
    assert_eq!(parsed.comments().len(), 1);
    assert_eq!(parsed.legacy_comments().len(), 1);
}

#[test]
fn test_round_trip_simple() {
    let doc = format!(
        "# Hello\n\nIntro text.\n\n{}\nMore text.\n",
        block_with_content("rt1", "Body of the comment."),
    );
    let parsed = parse(&doc).unwrap();
    let reconstructed = parsed.to_markdown();
    // Re-parse the reconstructed document and verify it matches.
    let reparsed = parse(&reconstructed).unwrap();
    assert_eq!(reparsed.comments().len(), 1);
    assert_eq!(reparsed.comments()[0].id, "rt1");
    assert_eq!(reparsed.comments()[0].content, "Body of the comment.");
}

#[test]
fn test_malformed_yaml_error() {
    let doc = "\
```remargin
---
this is not: [valid: yaml: at: all
---
```
";
    let err = parse(doc).unwrap_err();
    let err_msg = format!("{err:#}");
    assert!(
        err_msg.contains("failed to parse YAML"),
        "expected descriptive error, got: {err_msg}"
    );
}

#[test]
fn test_four_backtick_wrapper() {
    let doc = "\
````remargin
---
id: deep
author: testuser
type: human
ts: 2026-04-06T14:32:00-04:00
checksum: sha256:abc123
---
Here is a code block inside the comment:

```python
print(\"hello\")
```

End of comment.
````
";
    let parsed = parse(doc).unwrap();
    let c = parsed.comments()[0];
    assert_eq!(c.id, "deep");
    // fence_depth is no longer stored on Comment; it is computed at serialization time.
    assert!(c.content.contains("```python"));
    assert!(c.content.contains("print(\"hello\")"));
}

#[test]
fn test_six_backtick_wrapper() {
    let doc = "\
``````remargin
---
id: v6wrap
author: testuser
type: human
ts: 2026-04-06T14:32:00-04:00
checksum: sha256:abc123
---
Quoting a remargin block:

`````remargin
This is quoted content, not a real block.
`````

Done quoting.
``````
";
    let parsed = parse(doc).unwrap();
    let comments = parsed.comments();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, "v6wrap");
    // fence_depth is no longer stored on Comment; it is computed at serialization time.
    assert!(comments[0].content.contains("`````remargin"));
}

#[test]
fn test_three_backtick_minimal() {
    let doc = minimal_block("min3");
    let parsed = parse(&doc).unwrap();
    // fence_depth is no longer stored on Comment; verify the comment parsed successfully.
    assert_eq!(parsed.comments()[0].id, "min3");
}

#[test]
fn test_same_depth_not_confused() {
    // A 4-backtick wrapper; the inner ``` (3 backticks) is content, not a closer.
    let doc = "\
````remargin
---
id: sd4
author: testuser
type: human
ts: 2026-04-06T14:32:00-04:00
checksum: sha256:abc123
---
Inner code:

```
some code
```

More text.
````
";
    let parsed = parse(doc).unwrap();
    let c = parsed.comments()[0];
    assert_eq!(c.id, "sd4");
    // fence_depth is no longer stored on Comment; it is computed at serialization time.
    assert!(c.content.contains("```"));
    assert!(c.content.contains("some code"));
}

#[test]
fn test_no_comments() {
    let doc = "# Just a Title\n\nSome text.\n\n```python\nprint('hello')\n```\n";
    let parsed = parse(doc).unwrap();
    assert!(parsed.comments().is_empty());
    assert!(parsed.legacy_comments().is_empty());
}

#[test]
fn test_parse_file_with_mock_system() {
    let content = minimal_block("file1");
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), content.as_bytes())
        .unwrap();
    let parsed = parse_file(&system, Path::new("/docs/test.md")).unwrap();
    assert_eq!(parsed.comments().len(), 1);
    assert_eq!(parsed.comments()[0].id, "file1");
}

#[test]
fn test_comment_ids() {
    let doc = format!("{}{}", minimal_block("aa1"), minimal_block("bb2"));
    let parsed = parse(&doc).unwrap();
    let ids = parsed.comment_ids();
    assert!(ids.contains("aa1"));
    assert!(ids.contains("bb2"));
    assert_eq!(ids.len(), 2);
}

#[test]
fn test_find_comment() {
    let doc = format!("{}{}", minimal_block("fc1"), minimal_block("fc2"));
    let parsed = parse(&doc).unwrap();
    assert!(parsed.find_comment("fc1").is_some());
    assert!(parsed.find_comment("fc2").is_some());
    assert!(parsed.find_comment("nonexistent").is_none());
}

#[test]
fn test_legacy_singular_form() {
    let doc = "\
```user comment
Singular form feedback.
```
";
    let parsed = parse(doc).unwrap();
    let legacy = parsed.legacy_comments();
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].role, LegacyRole::User);
}

#[test]
fn test_content_multiline() {
    let doc = "\
```remargin
---
id: ml1
author: testuser
type: human
ts: 2026-04-06T14:32:00-04:00
checksum: sha256:abc123
---
Line one.

Line three after blank.
```
";
    let parsed = parse(doc).unwrap();
    let c = parsed.comments()[0];
    assert_eq!(c.content, "Line one.\n\nLine three after blank.");
}

#[test]
fn test_parse_file_missing() {
    let system = MockSystem::new();
    let result = parse_file(&system, Path::new("/nonexistent.md"));
    result.unwrap_err();
}

#[test]
fn test_line_number_at_start() {
    let doc = minimal_block("ln1");
    let parsed = parse(&doc).unwrap();
    let c = parsed.comments()[0];
    assert_eq!(c.line, 1, "comment at start of file should be line 1");
}

#[test]
fn test_line_number_after_body() {
    // "# Title\n\nBody text.\n\n" = 4 lines, comment starts on line 5
    let doc = format!("# Title\n\nBody text.\n\n{}", minimal_block("ln2"));
    let parsed = parse(&doc).unwrap();
    let c = parsed.comments()[0];
    assert_eq!(c.line, 5, "comment after 4 lines of body should be line 5");
}

#[test]
fn test_line_numbers_multiple_comments() {
    // First comment at line 1.
    // minimal_block produces 9 lines (opening fence, ---, 5 YAML fields, ---, closing fence).
    // So second comment starts at line 10 (after a blank line separator at line 10? let's compute)
    let block1 = minimal_block("m1");
    let block2 = minimal_block("m2");
    let doc = format!("{block1}\n{block2}");
    let parsed = parse(&doc).unwrap();
    let comments = parsed.comments();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].line, 1);
    // block1 is 9 lines (ends with \n) + 1 blank separator line = line 11
    assert_eq!(comments[1].line, 11);
}

#[test]
fn test_legacy_comment_line_number() {
    let doc = "# Title\n\nSome text.\n\n```user comments\nOld feedback.\n```\n";
    let parsed = parse(doc).unwrap();
    let legacy = parsed.legacy_comments();
    assert_eq!(legacy.len(), 1);
    // "# Title\n\nSome text.\n\n" = 4 lines, legacy starts at line 5
    assert_eq!(legacy[0].line, 5);
}

#[test]
fn test_line_number_round_trip() {
    let doc = format!(
        "# Hello\n\n{}\nMore text.\n",
        block_with_content("rt2", "Body.")
    );
    let parsed = parse(&doc).unwrap();
    let original_line = parsed.comments()[0].line;
    assert_eq!(original_line, 3);

    // Round-trip through serialize and re-parse.
    let reconstructed = parsed.to_markdown();
    let reparsed = parse(&reconstructed).unwrap();
    assert_eq!(
        reparsed.comments()[0].line,
        original_line,
        "line number should be recomputed identically after round-trip"
    );
}

#[test]
fn test_comment_json_shape_matches_schema() {
    // Build a block that exercises every optional field: `reply_to`,
    // `thread`, `signature`, plus a non-empty `ack`, `reactions`,
    // `attachments`, and `to` list.
    let doc = "```remargin\n\
         ---\n\
         id: full\n\
         author: alice\n\
         type: human\n\
         ts: 2026-04-06T14:32:00-04:00\n\
         checksum: sha256:abc123\n\
         to: [bob, carol]\n\
         reply-to: abc\n\
         thread: t1\n\
         attachments: [file.png]\n\
         reactions:\n\
           \"+1\": [bob]\n\
         ack:\n\
           - bob@2026-04-06T15:00:00-04:00\n\
         signature: ed25519:deadbeef\n\
         ---\n\
         Hello world.\n\
         ```\n";
    let parsed = parse(doc).unwrap();
    let comment = parsed.comments()[0].clone();

    // Serialize the comment through serde (which is what the CLI's
    // `--json comments` output relies on) and inspect the resulting
    // JSON object.
    let value = serde_json::to_value(&comment).unwrap();
    let obj = value.as_object().unwrap();

    // Required keys must always be present.
    for key in [
        "ack",
        "attachments",
        "author",
        "author_type",
        "checksum",
        "content",
        "id",
        "line",
        "reactions",
        "to",
        "ts",
    ] {
        assert!(
            obj.contains_key(key),
            "required key `{key}` missing from serialized Comment"
        );
    }

    // `author_type` must use the lowercase enum value the schema
    // declares (`human`/`agent`) — matching the fence wire format,
    // checksum input, and human display. Not the legacy `type` key
    // the CLI used to hand-write.
    assert_eq!(obj["author_type"], serde_json::json!("human"));
    assert!(
        !obj.contains_key("type"),
        "legacy `type` key must not appear in serialized Comment"
    );

    // Optional fields with values should be present.
    assert_eq!(obj["reply_to"], serde_json::json!("abc"));
    assert_eq!(obj["thread"], serde_json::json!("t1"));
    assert_eq!(obj["signature"], serde_json::json!("ed25519:deadbeef"));

    // Timestamp must be RFC3339 (the format the generated
    // `z.iso.datetime()` schema expects).
    assert_eq!(obj["ts"], serde_json::json!("2026-04-06T14:32:00-04:00"));
}

#[test]
fn test_minimal_comment_json_skips_none_and_defaults_collections() {
    // A block with only the required YAML fields should still
    // serialize to all required schema keys, including empty
    // collections for `ack`, `attachments`, `to`, and `reactions`.
    let doc = minimal_block("abc");
    let parsed = parse(&doc).unwrap();
    let comment = parsed.comments()[0].clone();

    let value = serde_json::to_value(&comment).unwrap();
    let obj = value.as_object().unwrap();

    assert_eq!(obj["ack"], serde_json::json!([]));
    assert_eq!(obj["attachments"], serde_json::json!([]));
    assert_eq!(obj["to"], serde_json::json!([]));
    assert_eq!(obj["reactions"], serde_json::json!({}));

    // Optional fields with no value should be omitted entirely so the
    // Zod `strictObject` schema accepts them as `undefined` instead of
    // rejecting an explicit `null`.
    for key in ["reply_to", "thread", "signature"] {
        assert!(
            !obj.contains_key(key),
            "optional key `{key}` should be skipped when None, \
             but was present in {obj:?}"
        );
    }
}

#[test]
fn test_author_type_serializes_lowercase() {
    let human = super::AuthorType::Human;
    let agent = super::AuthorType::Agent;
    assert_eq!(
        serde_json::to_value(&human).unwrap(),
        serde_json::json!("human")
    );
    assert_eq!(
        serde_json::to_value(&agent).unwrap(),
        serde_json::json!("agent")
    );
}
