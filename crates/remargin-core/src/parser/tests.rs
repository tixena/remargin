//! Tests for the comment block parser.

use std::path::Path;

use chrono::DateTime;
use os_shim::mock::MemorySystem;

use super::{AuthorType, Segment, parse, parse_file};

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
    assert_eq!(c.to, [] as [String; 0]);
    assert!(c.reply_to.is_none());
    assert!(c.thread.is_none());
    assert!(c.signature.is_none());
    assert_eq!(c.attachments, [] as [String; 0]);
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
    let thumbsup_entries = &c.reactions["thumbsup"];
    let thumbsup_authors: Vec<String> = thumbsup_entries.iter().map(|e| e.author.clone()).collect();
    assert_eq!(thumbsup_authors, vec!["eduardo", "jorge"]);
    let heart_entries = &c.reactions["heart"];
    let heart_authors: Vec<String> = heart_entries.iter().map(|e| e.author.clone()).collect();
    assert_eq!(heart_authors, vec!["claude"]);
    assert_eq!(c.ack.len(), 2);
    assert_eq!(c.ack[0].author, "jorge");
    assert_eq!(c.ack[1].author, "claude");
    assert_eq!(c.signature.as_deref(), Some("ed25519:base64signature=="));
    assert_eq!(c.content, "This is the comment body.");
}

#[test]
fn test_empty_content() {
    let doc = minimal_block("empty");
    let parsed = parse(&doc).unwrap();
    let c = parsed.comments()[0];
    assert_eq!(c.content, "");
}

#[test]
fn test_round_trip_simple() {
    let doc = format!(
        "# Hello\n\nIntro text.\n\n{}\nMore text.\n",
        block_with_content("rt1", "Body of the comment."),
    );
    let parsed = parse(&doc).unwrap();
    let reconstructed = parsed.to_markdown().unwrap();
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
    assert!(comments[0].content.contains("`````remargin"));
}

#[test]
fn test_three_backtick_minimal() {
    let doc = minimal_block("min3");
    let parsed = parse(&doc).unwrap();
    assert_eq!(parsed.comments()[0].id, "min3");
}

#[test]
fn test_same_depth_not_confused() {
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
    assert!(c.content.contains("```"));
    assert!(c.content.contains("some code"));
}

#[test]
fn test_no_comments() {
    let doc = "# Just a Title\n\nSome text.\n\n```python\nprint('hello')\n```\n";
    let parsed = parse(doc).unwrap();
    assert!(parsed.comments().is_empty());
}

#[test]
fn test_parse_file_with_mock_system() {
    let content = minimal_block("file1");
    let system = MemorySystem::new()
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
    let system = MemorySystem::new();
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
fn test_line_number_round_trip() {
    let doc = format!(
        "# Hello\n\n{}\nMore text.\n",
        block_with_content("rt2", "Body.")
    );
    let parsed = parse(&doc).unwrap();
    let original_line = parsed.comments()[0].line;
    assert_eq!(original_line, 3);

    let reconstructed = parsed.to_markdown().unwrap();
    let reparsed = parse(&reconstructed).unwrap();
    assert_eq!(
        reparsed.comments()[0].line,
        original_line,
        "line number should be recomputed identically after round-trip"
    );
}

#[test]
fn test_comment_json_shape_matches_schema() {
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

    let value = serde_json::to_value(&comment).unwrap();
    let obj = value.as_object().unwrap();

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

    assert_eq!(obj["author_type"], serde_json::json!("human"));
    assert!(
        !obj.contains_key("type"),
        "legacy `type` key must not appear in serialized Comment"
    );

    assert_eq!(obj["reply_to"], serde_json::json!("abc"));
    assert_eq!(obj["thread"], serde_json::json!("t1"));
    assert_eq!(obj["signature"], serde_json::json!("ed25519:deadbeef"));

    assert_eq!(obj["ts"], serde_json::json!("2026-04-06T14:32:00-04:00"));
}

#[test]
fn test_minimal_comment_json_skips_none_and_defaults_collections() {
    let doc = minimal_block("abc");
    let parsed = parse(&doc).unwrap();
    let comment = parsed.comments()[0].clone();

    let value = serde_json::to_value(&comment).unwrap();
    let obj = value.as_object().unwrap();

    assert_eq!(obj["ack"], serde_json::json!([]));
    assert_eq!(obj["attachments"], serde_json::json!([]));
    assert_eq!(obj["to"], serde_json::json!([]));
    assert_eq!(obj["reactions"], serde_json::json!({}));

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

#[test]
fn test_remargin_kind_absent_parses_to_none() {
    let doc = minimal_block("abc");
    let parsed = parse(&doc).unwrap();
    assert!(
        parsed.comments()[0].remargin_kind.is_none(),
        "pre-field block must parse with remargin_kind=None so serialization omits the line"
    );
    assert!(
        parsed.comments()[0].kinds().is_empty(),
        "accessor must still return an empty slice"
    );
}

/// `remargin_kind: []` on disk parses to `None`, so the next write omits the line.
#[test]
fn test_remargin_kind_explicit_empty_list_normalizes_to_none() {
    let doc = "\
```remargin
---
id: abc
author: testuser
type: human
ts: 2026-04-06T14:32:00-04:00
checksum: sha256:abc123
remargin_kind: []
---
body
```
";
    let parsed = parse(doc).unwrap();
    assert!(
        parsed.comments()[0].remargin_kind.is_none(),
        "empty-list on disk must normalize to None so serialize omits the line"
    );
    let rendered = parsed.to_markdown().unwrap();
    assert!(
        !rendered.contains("remargin_kind:"),
        "serialize must not emit a remargin_kind: line when kinds are absent; got:\n{rendered}"
    );
}

#[test]
fn test_remargin_kind_absent_round_trip_omits_line() {
    let doc = minimal_block("abc");
    let parsed = parse(&doc).unwrap();
    let rendered = parsed.to_markdown().unwrap();
    assert!(
        !rendered.contains("remargin_kind"),
        "absent → None → absent round-trip must not introduce a remargin_kind line; got:\n{rendered}"
    );
}

#[test]
fn test_remargin_kind_round_trip() {
    let doc = "\
```remargin
---
id: abc
author: testuser
type: human
ts: 2026-04-06T14:32:00-04:00
checksum: sha256:abc123
remargin_kind: [question, action item]
---
body text
```
";
    let parsed = parse(doc).unwrap();
    let comments = parsed.comments();
    assert_eq!(comments.len(), 1);
    assert_eq!(
        comments[0].remargin_kind.as_deref(),
        Some(&[String::from("question"), String::from("action item")][..])
    );
    let rendered = parsed.to_markdown().unwrap();
    assert!(
        rendered.contains("remargin_kind: [question, action item]"),
        "round-tripped markdown should preserve the kind list:\n{rendered}"
    );
}

#[test]
fn test_remargin_kind_invalid_value_rejected() {
    let doc = "\
```remargin
---
id: abc
author: testuser
type: human
ts: 2026-04-06T14:32:00-04:00
checksum: sha256:abc123
remargin_kind: [\"bad!value\"]
---
body
```
";
    let err = parse(doc).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("remargin_kind") && msg.contains("invalid character"),
        "expected kind validation error, got {msg}"
    );
}

#[test]
fn legacy_reactions_round_trip_to_new_shape() {
    let doc = "\
```remargin
---
id: legacy
author: eduardo
type: human
ts: 2026-04-26T10:00:00-04:00
checksum: sha256:abc
reactions:
  thumbsup: [eduardo, claude]
---
hello
```
";
    let parsed = parse(doc).unwrap();
    let cm = parsed.comments()[0];

    let entries = &cm.reactions["thumbsup"];
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].author, "eduardo");
    assert_eq!(entries[1].author, "claude");
    assert_eq!(entries[0].ts.to_rfc3339(), "2026-04-26T10:00:00-04:00");
    assert_eq!(entries[1].ts.to_rfc3339(), "2026-04-26T10:00:00-04:00");

    let written = parsed.to_markdown().unwrap();
    assert!(
        written.contains("- author: eduardo"),
        "serialized form missing new-shape author entry:\n{written}"
    );
    assert!(
        written.contains("ts: 2026-04-26T10:00:00-04:00"),
        "serialized form missing the synthesized reaction ts:\n{written}"
    );
    assert!(
        !written.contains("[eduardo, claude]"),
        "serialized form must not retain the legacy flow-list shape:\n{written}"
    );
}

#[test]
fn legacy_reaction_uses_ack_ts_when_author_acked() {
    let doc = "\
```remargin
---
id: ackedreact
author: eduardo
type: human
ts: 2026-04-26T10:00:00-04:00
checksum: sha256:abc
reactions:
  thumbsup: [eduardo]
ack:
  - eduardo@2026-04-26T11:00:00-04:00
---
hi
```
";
    let parsed = parse(doc).unwrap();
    let cm = parsed.comments()[0];
    let entries = &cm.reactions["thumbsup"];
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].ts.to_rfc3339(), "2026-04-26T11:00:00-04:00");
}

/// One block carrying every timestamp the wire format has — `ts`,
/// `edited_at`, a reaction entry, an ack entry — rendered with the given
/// zero-offset spelling.
fn zero_offset_block(offset: &str) -> String {
    format!(
        "\
```remargin
---
id: zeroff
author: eduardo
type: human
ts: 2026-04-26T10:00:00{offset}
edited_at: 2026-04-26T11:00:00{offset}
reactions:
  thumbsup:
    - author: claude
      ts: 2026-04-26T12:00:00{offset}
ack:
  - claude@2026-04-26T13:00:00{offset}
checksum: sha256:abc
---
hello
```
"
    )
}

#[test]
fn legacy_plus_zero_block_parses_to_the_same_instants_as_its_z_twin() {
    let legacy = parse(&zero_offset_block("+00:00")).unwrap();
    let modern = parse(&zero_offset_block("Z")).unwrap();
    let legacy_cm = legacy.comments()[0];
    let modern_cm = modern.comments()[0];

    assert_eq!(legacy_cm.ts, modern_cm.ts);
    assert_eq!(
        legacy_cm.ts,
        DateTime::parse_from_rfc3339("2026-04-26T10:00:00Z").unwrap()
    );
    assert_eq!(legacy_cm.edited_at, modern_cm.edited_at);
    assert_eq!(legacy_cm.ack[0].ts, modern_cm.ack[0].ts);
    assert_eq!(
        legacy_cm.reactions["thumbsup"][0].ts,
        modern_cm.reactions["thumbsup"][0].ts
    );
}

#[test]
fn legacy_plus_zero_block_converges_to_z_on_rewrite() {
    let written = parse(&zero_offset_block("+00:00"))
        .unwrap()
        .to_markdown()
        .unwrap();

    for expected in [
        "ts: 2026-04-26T10:00:00Z",
        "edited_at: 2026-04-26T11:00:00Z",
        "ts: 2026-04-26T12:00:00Z",
        "- claude@2026-04-26T13:00:00Z",
    ] {
        assert!(
            written.contains(expected),
            "rewrite must render {expected:?}:\n{written}"
        );
    }
    assert!(
        !written.contains("+00:00"),
        "rewrite must leave no legacy zero-offset spelling behind:\n{written}"
    );
}

#[test]
fn nested_three_backtick_remargin_truncates_outer_body_at_inner_close() {
    let doc = "\
```remargin
---
id: outer3
author: alice
type: human
ts: 2026-05-25T00:00:00-04:00
checksum: sha256:outer3
---
quoting an inner comment below:
```remargin
---
id: inner3
author: bob
type: human
ts: 2026-05-25T01:00:00-04:00
checksum: sha256:inner3
---
inner body text
```
trailing outer body text
```
";
    let parsed = parse(doc).unwrap();
    let comments = parsed.comments();
    assert_eq!(comments.len(), 1, "3+3-backtick: no stray sibling comment");
    let outer = comments[0];
    assert_eq!(outer.id, "outer3");
    assert!(
        outer.content.starts_with("quoting an inner comment below:"),
        "outer content begins with the prose before the inner fence",
    );
    assert!(
        outer.content.contains("id: inner3"),
        "outer content swallows the inner fence + YAML verbatim",
    );
    assert!(
        outer.content.ends_with("inner body text"),
        "outer content ends at the inner's closing fence -- trailing outer body is lost from the comment",
    );
    let trailing_body: Vec<_> = parsed
        .segments
        .iter()
        .filter_map(|seg| match seg {
            Segment::Body(text) => Some(text.as_str()),
            Segment::Comment(_) => None,
        })
        .collect();
    assert!(
        trailing_body
            .iter()
            .any(|b| b.contains("trailing outer body text")),
        "the text after the inner's closing fence becomes a Body segment, not part of any comment",
    );
}

#[test]
fn nested_four_backtick_outer_three_backtick_inner_preserves_inner_verbatim() {
    let doc = "\
````remargin
---
id: outer4
author: alice
type: human
ts: 2026-05-25T00:00:00-04:00
checksum: sha256:outer4
---
quoting an inner comment below:
```remargin
---
id: inner3
author: bob
type: human
ts: 2026-05-25T01:00:00-04:00
checksum: sha256:inner3
---
inner body text
```
trailing outer body text
````
";
    let parsed = parse(doc).unwrap();
    let comments = parsed.comments();
    assert_eq!(
        comments.len(),
        1,
        "4+3-backtick: clean nesting, one comment"
    );
    let outer = comments[0];
    assert_eq!(outer.id, "outer4");
    assert!(
        outer.content.contains("```remargin\n---\nid: inner3"),
        "outer content preserves the inner fence + YAML verbatim",
    );
    assert!(
        outer.content.contains("inner body text\n```"),
        "outer content preserves the inner's closing fence verbatim",
    );
    assert!(
        outer.content.ends_with("trailing outer body text"),
        "outer content extends through the trailing outer prose, ending at the outer's 4-backtick close",
    );
}

#[test]
fn comment_spans_track_exact_block_lines() {
    let drifting = "```remargin\n\
         ---\n\
         id: aaa\n\
         author: testuser\n\
         type: human\n\
         ts: 2026-04-06T14:32:00-04:00\n\
         checksum: sha256:abc123\n\
         #x\n\
         ---\n\
         first\n\
         ```\n";
    let plain = block_with_content("bbb", "second");
    let doc = format!("intro \u{2014}\n{drifting}mid \u{2014}\n{plain}tail \u{2014}\n");

    let parsed = parse(&doc).unwrap();
    let spans: Vec<(usize, usize)> = parsed
        .comments()
        .iter()
        .map(|cm| (cm.sl.unwrap(), cm.el.unwrap()))
        .collect();
    assert_eq!(spans.len(), 2);

    let drift_lines = drifting.matches('\n').count();
    let plain_lines = plain.matches('\n').count();
    let drift_start = 2;
    let drift_end = drift_start + drift_lines - 1;
    let plain_start = drift_end + 2;
    let plain_end = plain_start + plain_lines - 1;
    assert_eq!(spans[0], (drift_start, drift_end), "drifting block span");
    assert_eq!(spans[1], (plain_start, plain_end), "plain block span");

    let total = doc.lines().count();
    for &(sl, el) in &spans {
        assert!(
            sl <= el && el <= total,
            "span {sl}..={el} within {total} lines"
        );
    }
}

#[test]
fn comment_span_handles_block_at_eof_without_trailing_newline() {
    let block = block_with_content("aaa", "body");
    let doc = format!("intro\n{}", block.trim_end_matches('\n'));

    let parsed = parse(&doc).unwrap();
    let spans: Vec<(usize, usize)> = parsed
        .comments()
        .iter()
        .map(|cm| (cm.sl.unwrap(), cm.el.unwrap()))
        .collect();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].0, 2, "block starts on line 2");
    assert_eq!(
        spans[0].1,
        doc.lines().count(),
        "block ends on the last line"
    );
}

#[test]
fn pending_broadcast_for_exempts_the_author() {
    use crate::parser::is_pending_broadcast_for;
    let broadcast: [String; 0] = [];
    assert!(!is_pending_broadcast_for("alice", &broadcast, &[], "alice"));
    assert!(is_pending_broadcast_for("alice", &broadcast, &[], "bob"));
    let directed = [String::from("bob")];
    assert!(!is_pending_broadcast_for("alice", &directed, &[], "bob"));
}
