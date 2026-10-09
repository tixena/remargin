//! Tests for the comment-body style gate: it must refuse an agent's slab of prose and stay out of
//! the way of a human author, a quoted bad example and a warn-tier heuristic.

use super::{Severity, TRAILING_METADATA_OPENERS, gate, gate_edit, notes, review};
use crate::parser::AuthorType;

const CLEAN: &str = "The import form reads its field list from the gateway.\n";

const TRAILING_METADATA: &str = "The gate runs at the write seam.\n\nOne thing: the sidebar shows a badge nobody asked about.\n";

const HARD_WRAPPED: &str = "The import form and the generate form both read their field list from the\n\
                            gateway, so a change to either one has to land in both controllers.\n";

#[test]
fn hard_wrapped_paragraph_is_reject_tier() {
    let findings = review(HARD_WRAPPED);

    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind.severity(), Severity::Reject);
    assert_eq!(findings[0].line, 1);
}

#[test]
fn agent_hard_wrap_is_refused_with_the_line_and_the_fix() {
    let err = gate(HARD_WRAPPED, &AuthorType::Agent)
        .unwrap_err()
        .to_string();

    assert!(err.contains("line 1"), "names the offending line: {err}");
    assert!(err.contains("one continuous line"), "states the fix: {err}");
}

#[test]
fn a_refusal_never_cites_a_rule_number() {
    let err = gate(HARD_WRAPPED, &AuthorType::Agent)
        .unwrap_err()
        .to_string();

    assert!(
        !err.to_lowercase().contains("rule"),
        "the author has to know what to change, not which rule to look up: {err}"
    );
}

#[test]
fn a_human_writes_the_same_body_unchanged() {
    gate(HARD_WRAPPED, &AuthorType::Human).unwrap();
}

#[test]
fn every_trailing_metadata_opener_is_refused() {
    for opener in TRAILING_METADATA_OPENERS {
        let body = format!(
            "Posted as a self-reply, so nothing was acked.\n\n{opener} the sidebar shows a badge that nobody asked about.\n"
        );

        let err = gate(&body, &AuthorType::Agent).unwrap_err().to_string();

        assert!(
            err.contains(opener),
            "the refusal quotes the phrase it matched: {err}"
        );
        assert!(err.contains("cut it"), "the refusal states the fix: {err}");
    }
}

#[test]
fn trailing_metadata_matches_past_a_bold_marker_and_any_case() {
    let body = "The gate is wired at the write seam.\n\n**One Thing**: the batch surface warns without blocking.\n";

    gate(body, &AuthorType::Agent).unwrap_err();
}

#[test]
fn trailing_metadata_inside_a_fenced_block_is_accepted() {
    let body = "Rewrite the closing block of the reply as:\n\n```markdown\nOne thing: the sidebar shows a badge nobody asked about.\n```\n";

    gate(body, &AuthorType::Agent).unwrap();
}

#[test]
fn trailing_metadata_inside_a_blockquote_is_accepted() {
    let body = "**Before**: the answer, then an aside the reader cannot act on:\n\n> Posted as a self-reply, so nothing was acked.\n>\n> One observation I cannot act on: the sidebar shows a badge even though no recipient was passed.\n";

    gate(body, &AuthorType::Agent).unwrap();
}

#[test]
fn a_phrase_introducing_a_trailing_code_block_is_accepted() {
    let body = "The fix is in the gateway controller.\n\nNote that the bindings have to be regenerated:\n\n```bash\njust generate-types\n```\n";

    gate(body, &AuthorType::Agent).unwrap();
}

#[test]
fn a_middle_block_opening_with_an_aside_is_accepted() {
    let body = "The gate is wired at the write seam.\n\nOne thing worth checking is the batch path.\n\nBoth surfaces are covered.\n";

    gate(body, &AuthorType::Agent).unwrap();
}

#[test]
fn a_bare_comment_id_reference_warns_without_blocking() {
    let body = "The recipient list is derived from the parent, as in ow6.\n";

    let findings = review(body);

    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind.severity(), Severity::Warn);
    gate(body, &AuthorType::Agent).unwrap();
    assert_eq!(notes(body).len(), 1, "the warning reaches the author");
}

#[test]
fn a_backticked_reference_warns_even_without_a_digit() {
    let body = "The recipient list is derived from the parent, see `zuv`.\n";

    assert_eq!(review(body).len(), 1);
}

#[test]
fn ordinary_prose_after_a_cue_word_stays_quiet() {
    let body = "See the gateway controller for the field list, and per the design the two forms share it.\n";

    assert_eq!(review(body), Vec::new());
}

#[test]
fn a_dense_body_warns_without_blocking() {
    let body = format!(
        "{} done.\n",
        "The gate is wired at the write seam so every surface is covered.".repeat(10)
    );

    let findings = review(&body);

    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind.severity(), Severity::Warn);
    gate(&body, &AuthorType::Agent).unwrap();
}

#[test]
fn a_long_body_broken_into_blocks_stays_quiet() {
    let paragraph = "The gate is wired at the write seam so every surface is covered.".repeat(5);
    let body = format!("{paragraph}\n\n{paragraph}\n");

    assert_eq!(review(&body), Vec::new());
}

#[test]
fn a_well_shaped_comment_earns_no_findings() {
    let body = "All three anonymous endpoints that serve form definitions live in the API gateway:\n\n\
                | Form type | Route | Implemented at |\n\
                | --- | --- | --- |\n\
                | Import | `GET /app/shared-items/document-types/:id` | `ecm.ts:2424` |\n\
                | Generate | `GET /app/shared-items/template-version/:id/info` | `template.ts:832` |\n\
                \n\
                Paths are relative to `packages/api-gateway/src/controllers/`.\n";

    assert_eq!(review(body), Vec::new());
    gate(body, &AuthorType::Agent).unwrap();
}

#[test]
fn an_empty_body_earns_no_findings() {
    assert_eq!(review(""), Vec::new());
    gate("", &AuthorType::Agent).unwrap();
}

#[test]
fn an_edit_adding_a_hard_wrap_is_refused_with_the_line_and_the_fix() {
    let err = gate_edit(CLEAN, HARD_WRAPPED, &AuthorType::Agent)
        .unwrap_err()
        .to_string();

    assert!(err.contains("line 1"), "names the offending line: {err}");
    assert!(err.contains("one continuous line"), "states the fix: {err}");
}

#[test]
fn a_human_edit_adding_the_same_wrap_is_accepted() {
    gate_edit(CLEAN, HARD_WRAPPED, &AuthorType::Human).unwrap();
}

#[test]
fn an_edit_repairing_the_body_is_accepted() {
    gate_edit(HARD_WRAPPED, CLEAN, &AuthorType::Agent).unwrap();
}

#[test]
fn an_edit_repairing_one_of_two_wraps_is_accepted() {
    let old = format!("{HARD_WRAPPED}\n{HARD_WRAPPED}");
    let new = format!("{HARD_WRAPPED}\n{CLEAN}");

    assert_eq!(review(&old).len(), 2, "the fixture carries two wraps");
    gate_edit(&old, &new, &AuthorType::Agent).unwrap();
}

#[test]
fn an_edit_holding_an_inherited_wrap_in_place_is_accepted() {
    let new = "The import form and the generate form both read their field list from the\ngateway, so either change lands in both controllers.\n";

    assert_eq!(review(new).len(), 1, "the new body still carries the wrap");
    gate_edit(HARD_WRAPPED, new, &AuthorType::Agent).unwrap();
}

#[test]
fn an_edit_repairing_one_kind_and_inheriting_another_is_accepted() {
    let old = format!("{HARD_WRAPPED}\n{TRAILING_METADATA}");
    let new = format!("{CLEAN}\n{TRAILING_METADATA}");

    gate_edit(&old, &new, &AuthorType::Agent).unwrap();
}

#[test]
fn an_edit_adding_a_second_wrap_to_a_body_that_already_had_one_is_refused() {
    let new = format!("{HARD_WRAPPED}\n{HARD_WRAPPED}");

    gate_edit(HARD_WRAPPED, &new, &AuthorType::Agent).unwrap_err();
}

#[test]
fn an_edit_adding_one_kind_is_refused_even_when_another_kind_falls() {
    let old = format!("{HARD_WRAPPED}\n{HARD_WRAPPED}");
    let new = format!("{HARD_WRAPPED}\n{TRAILING_METADATA}");

    let err = gate_edit(&old, &new, &AuthorType::Agent)
        .unwrap_err()
        .to_string();

    assert!(err.contains("one thing"), "quotes the added phrase: {err}");
    assert!(
        !err.contains("one continuous line"),
        "the surviving wrap is inherited, so it is not held against the editor: {err}"
    );
}

#[test]
fn an_edit_earning_only_a_warning_is_accepted() {
    let new = "The recipient list is derived from the parent, as in ow6.\n";

    gate_edit(CLEAN, new, &AuthorType::Agent).unwrap();
    assert_eq!(notes(new).len(), 1, "the warning reaches the editor");
}
