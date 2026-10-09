//! Tests for the advisory review pass: it must catch a hand-wrapped paragraph and stay silent on
//! every construct whose newlines are genuine content.

use serde_json::json;

use super::{Advice, OpAdvice, attach_op_notes, format_op_text, format_text, offset_lines, review};

#[test]
fn flags_a_hard_wrapped_paragraph_at_its_first_line() {
    let content = "Goal. Three actions per revision, Accept (metadata-only, no new version), Manual Edit\n\
                   (MarkdownEditor sandbox, no DB writes until accept), Accept & Apply (send markdown, replace\n\
                   content in the Word doc, save a new version).\n";

    let notes = review(content);

    assert_eq!(notes.len(), 1, "one paragraph, one note: {notes:?}");
    assert_eq!(notes[0].line, 1);
    assert!(
        notes[0].message.contains("hard-wrapped across 3 lines"),
        "note counts the span: {}",
        notes[0].message
    );
    assert!(
        notes[0].message.contains("one continuous line"),
        "note says what to do instead: {}",
        notes[0].message
    );
}

#[test]
fn stays_silent_on_continuous_prose() {
    let content = "A single paragraph that runs as long as it likes, because the viewer is what wraps it.\n\
                   \n\
                   Another one, also on its own line.\n";

    assert_eq!(review(content), Vec::new());
}

#[test]
fn flags_each_wrapped_paragraph_separately() {
    let content = "First paragraph broken\nacross two lines.\n\
                   \n\
                   Second paragraph broken\nacross two lines as well.\n";

    let notes = review(content);

    assert_eq!(notes.len(), 2, "{notes:?}");
    assert_eq!(notes[0].line, 1);
    assert_eq!(notes[1].line, 4);
}

#[test]
fn never_flags_fenced_code_or_remargin_blocks() {
    let content = "```remargin\nid: abc\nauthor: someone\ntype: comment\n```\n\
                   \n\
                   ```rust\nlet a = 1;\nlet b = 2;\n```\n\
                   \n\
                   ~~~\ntilde fenced\nstill code\n~~~\n";

    assert_eq!(review(content), Vec::new());
}

#[test]
fn never_flags_frontmatter() {
    let content = "---\ntitle: A doc\nauthor: someone\ndescription: ''\n---\n\
                   \n\
                   Body on one line.\n";

    assert_eq!(review(content), Vec::new());
}

#[test]
fn unterminated_frontmatter_yields_no_notes() {
    let content = "---\ntitle: A doc\nauthor: someone\n";

    assert_eq!(review(content), Vec::new());
}

#[test]
fn never_flags_lists_tables_quotes_or_headings() {
    let content = "# A heading\n## Another heading\n\
                   \n\
                   - first item\n- second item\n- third item\n\
                   \n\
                   1. one\n2. two\n\
                   \n\
                   | a | b |\n| - | - |\n| 1 | 2 |\n\
                   \n\
                   > quoted line\n> another quoted line\n\
                   \n\
                   <div>\n<span>html</span>\n</div>\n";

    assert_eq!(review(content), Vec::new());
}

#[test]
fn never_flags_indented_blocks() {
    let content = "    indented code\n    more indented code\n";

    assert_eq!(review(content), Vec::new());
}

#[test]
fn respects_explicit_markdown_hard_breaks() {
    let content = "Address line one  \nAddress line two  \nAddress line three\n\
                   \n\
                   Backslash break\\\nsecond line\n";

    assert_eq!(review(content), Vec::new());
}

#[test]
fn never_flags_a_setext_heading_underline() {
    let content = "A title\n=======\n\nBody text on one line.\n";

    assert_eq!(review(content), Vec::new());
}

#[test]
fn prose_after_a_fence_is_still_reviewed() {
    let content = "```\ncode\n```\n\nWrapped prose\nafter the fence.\n";

    let notes = review(content);

    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].line, 5);
}

#[test]
fn unclosed_fence_swallows_the_rest_without_notes() {
    let content = "```\ncode\nmore code\nnever closed\n";

    assert_eq!(review(content), Vec::new());
}

#[test]
fn offset_lines_points_notes_at_the_real_file() {
    let mut notes = vec![Advice {
        line: 1,
        message: String::from("note"),
    }];

    offset_lines(&mut notes, 409);

    assert_eq!(notes[0].line, 410);
}

#[test]
fn format_text_is_empty_when_there_is_nothing_to_say() {
    assert_eq!(format_text(&[]), "");
}

#[test]
fn format_text_labels_each_note_as_advice() {
    let notes = vec![Advice {
        line: 411,
        message: String::from("say something"),
    }];

    assert_eq!(format_text(&notes), "advice: line 411: say something\n");
}

#[test]
fn format_op_text_names_the_operation_ahead_of_the_line() {
    let notes = vec![OpAdvice::new(
        1,
        Advice {
            line: 3,
            message: String::from("say something"),
        },
    )];

    assert_eq!(
        format_op_text(&notes),
        "advice: op 1, line 3: say something\n"
    );
}

#[test]
fn format_op_text_is_empty_when_there_is_nothing_to_say() {
    assert_eq!(format_op_text(&[]), "");
}

#[test]
fn attach_op_notes_carries_the_op_alongside_line_and_message() {
    let mut result = json!({ "ids": ["a1x", "b2y"] });

    attach_op_notes(
        &mut result,
        &[OpAdvice::new(
            1,
            Advice {
                line: 3,
                message: String::from("say something"),
            },
        )],
    );

    assert_eq!(
        result,
        json!({
            "ids": ["a1x", "b2y"],
            "warnings": [{ "op": 1_i32, "line": 3_i32, "message": "say something" }],
        })
    );
}

#[test]
fn attach_op_notes_leaves_a_clean_result_untouched() {
    let mut result = json!({ "ids": ["a1x"] });

    attach_op_notes(&mut result, &[]);

    assert_eq!(result, json!({ "ids": ["a1x"] }));
}
