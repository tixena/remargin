//! Tests for the plan projection: the comment partition, changed line ranges, the subset gate
//! and the warn-tier notes.

use std::path::{Path, PathBuf};

use os_shim::mock::MemorySystem;

use super::{
    PlanIdentity, PlanRequest, diff_comment_sets, dispatch, project_doc_report, project_report,
    whole_file_checksum,
};
use crate::advice::OpAdvice;
use crate::comment_style;
use crate::config::{Mode, ResolvedConfig};
use crate::operations::projections::{ProjectBatchOp, ProjectCommentParams};
use crate::parser;
use crate::parser::AuthorType;
use crate::writer::InsertPosition;

// Keeps the checksum of DOC_ONE_COMMENT over edited content, so its row is `checksum_ok = false`.
const DOC_AAA_BAD_CHECKSUM: &str = "# Test\n\nSome body text here.\n\n```remargin\n---\nid: aaa\nauthor: alice\ntype: human\nts: 2026-04-06T10:00:00-04:00\nchecksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb\n---\nFirst comment, edited.\n```\n";

// Same id as DOC_ONE_COMMENT, with new content and its recomputed checksum.
const DOC_AAA_EDITED: &str = "# Test\n\nSome body text here.\n\n```remargin\n---\nid: aaa\nauthor: alice\ntype: human\nts: 2026-04-06T10:00:00-04:00\nchecksum: sha256:be02ec5d99642fe8cb4aa92cf85b1c7a05673353e7e4e8069ca3ce5a227162a6\n---\nFirst comment, edited.\n```\n";

const DOC_ONE_COMMENT: &str = "# Test\n\nSome body text here.\n\n```remargin\n---\nid: aaa\nauthor: alice\ntype: human\nts: 2026-04-06T10:00:00-04:00\nchecksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb\n---\nFirst comment.\n```\n";

const DOC_TWO_COMMENTS: &str = "# Test\n\nSome body text here.\n\n```remargin\n---\nid: aaa\nauthor: alice\ntype: human\nts: 2026-04-06T10:00:00-04:00\nchecksum: sha256:0a1b103c177bc33566af5d168667a855f3ffa3c3fd9748424bfa3b3512e6bfdb\n---\nFirst comment.\n```\n\n```remargin\n---\nid: bbb\nauthor: bob\ntype: human\nts: 2026-04-06T11:00:00-04:00\nchecksum: sha256:91f4d2a3dce415f7e893f7d93f37be404da42b1a7a1133ef759ab3fe747ad726\n---\nSecond comment.\n```\n";

/// A reference by id, which the gate reports and never refuses.
const WARNING_BODY: &str = "The recipient list is derived from the parent, as in ow6.\n";

fn open_config() -> ResolvedConfig {
    ResolvedConfig {
        trusted_roots: Vec::new(),
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
        source_path: None,
        unrestricted: false,
    }
}

fn test_identity() -> PlanIdentity {
    PlanIdentity {
        author_type: Some(String::from("human")),
        name: Some(String::from("eduardo")),
        would_sign: false,
    }
}

#[test]
fn whole_file_checksum_matches_known_sha256() {
    let checksum = whole_file_checksum("hello\n");
    assert_eq!(
        checksum,
        "sha256:5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
    );
}

#[test]
fn noop_plan_reports_empty_line_ranges_and_matching_checksums() {
    let before = parser::parse(DOC_ONE_COMMENT).unwrap();
    let after = parser::parse(DOC_ONE_COMMENT).unwrap();

    let report = project_report("write", &before, &after, &open_config(), test_identity()).unwrap();

    assert!(report.noop, "identical inputs must be a noop: {report:?}");
    assert_eq!(report.changed_line_ranges, [] as [[usize; 2]; 0]);
    assert_eq!(report.checksum_before, report.checksum_after);
    assert_eq!(report.op, "write");
    assert!(report.would_commit);
    assert!(report.reject_reason.is_none());
}

#[test]
fn added_comment_lands_in_added_bucket() {
    let before = parser::parse(DOC_ONE_COMMENT).unwrap();
    let after = parser::parse(DOC_TWO_COMMENTS).unwrap();

    let diff = diff_comment_sets(&before, &after);

    assert_eq!(diff.added, vec![String::from("bbb")]);
    assert_eq!(diff.destroyed, Vec::<String>::new());
    assert_eq!(diff.modified, Vec::<String>::new());
    assert_eq!(diff.preserved, vec![String::from("aaa")]);
}

#[test]
fn destroyed_comment_lands_in_destroyed_bucket() {
    let before = parser::parse(DOC_TWO_COMMENTS).unwrap();
    let after = parser::parse(DOC_ONE_COMMENT).unwrap();

    let diff = diff_comment_sets(&before, &after);

    assert_eq!(diff.added, Vec::<String>::new());
    assert_eq!(diff.destroyed, vec![String::from("bbb")]);
    assert_eq!(diff.modified, Vec::<String>::new());
    assert_eq!(diff.preserved, vec![String::from("aaa")]);
}

#[test]
fn modified_checksum_lands_in_modified_bucket() {
    let before = parser::parse(DOC_ONE_COMMENT).unwrap();
    let after = parser::parse(DOC_AAA_EDITED).unwrap();

    let diff = diff_comment_sets(&before, &after);

    assert_eq!(diff.added, Vec::<String>::new());
    assert_eq!(diff.destroyed, Vec::<String>::new());
    assert_eq!(diff.modified, vec![String::from("aaa")]);
    assert_eq!(diff.preserved, Vec::<String>::new());
}

#[test]
fn plan_report_includes_verify_rows_for_every_after_comment() {
    let before = parser::parse(DOC_ONE_COMMENT).unwrap();
    let after = parser::parse(DOC_TWO_COMMENTS).unwrap();

    let report = project_report("write", &before, &after, &open_config(), test_identity()).unwrap();

    assert_eq!(report.verify_after.rows.len(), 2);
    let ids: Vec<&str> = report
        .verify_after
        .rows
        .iter()
        .map(|row| row.id.as_str())
        .collect();
    assert_eq!(ids, vec!["aaa", "bbb"]);
}

#[test]
fn bad_checksum_drives_would_commit_false_with_reason() {
    let before = parser::parse(DOC_ONE_COMMENT).unwrap();
    let after = parser::parse(DOC_AAA_BAD_CHECKSUM).unwrap();

    let report = project_report("write", &before, &after, &open_config(), test_identity()).unwrap();

    assert!(!report.would_commit);
    assert!(
        report
            .reject_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("aaa")),
        "reject_reason should name the bad-checksum id: {:?}",
        report.reject_reason
    );
}

#[test]
fn changed_line_ranges_coalesce_contiguous_runs() {
    let before = parser::parse("# Title\n\nbody a\nbody b\nbody c\n").unwrap();
    let after = parser::parse("# Title\n\nbody A\nbody B\nbody c\n").unwrap();

    let report = project_report("write", &before, &after, &open_config(), test_identity()).unwrap();

    assert!(!report.noop);
    assert_eq!(report.changed_line_ranges, vec![[3_usize, 4_usize]]);
}

#[test]
fn project_doc_report_clean_projection_has_no_subset_gate() {
    let before = parser::parse(DOC_ONE_COMMENT).unwrap();
    let after = parser::parse(DOC_TWO_COMMENTS).unwrap();
    let path = Path::new("/d/file.md");
    let system = MemorySystem::new()
        .with_file(path, DOC_ONE_COMMENT.as_bytes())
        .unwrap();

    let report = project_doc_report(
        &system,
        path,
        "write",
        &before,
        &after,
        &open_config(),
        test_identity(),
    )
    .unwrap();

    assert!(report.would_commit);
    assert!(report.reject_reason.is_none());
    assert!(report.subset_gate.is_none());
}

#[test]
fn project_doc_report_introduced_anomaly_populates_subset_gate() {
    let before = parser::parse(DOC_ONE_COMMENT).unwrap();
    let after = parser::parse(DOC_AAA_BAD_CHECKSUM).unwrap();
    let path = Path::new("/d/file.md");
    let system = MemorySystem::new()
        .with_file(path, DOC_ONE_COMMENT.as_bytes())
        .unwrap();

    let report = project_doc_report(
        &system,
        path,
        "write",
        &before,
        &after,
        &open_config(),
        test_identity(),
    )
    .unwrap();

    assert!(!report.would_commit);
    let gate = report.subset_gate.unwrap();
    assert_eq!(gate.path, path);
    assert_eq!(gate.mode, "open");
    assert_eq!(gate.introduced.len(), 1);
    assert_eq!(gate.introduced[0].id, "aaa");
    assert_eq!(gate.introduced[0].kind, "checksum_invalid");
    assert!(gate.headline.contains("1 new anomaly"));
    assert!(gate.headline.contains("file.md"));
    assert!(gate.hint.contains("remargin verify"));
}

#[test]
fn project_doc_report_pre_existing_anomaly_does_not_refuse() {
    let before = parser::parse(DOC_AAA_BAD_CHECKSUM).unwrap();
    let after = parser::parse(DOC_AAA_BAD_CHECKSUM).unwrap();
    let path = Path::new("/d/file.md");
    let system = MemorySystem::new()
        .with_file(path, DOC_AAA_BAD_CHECKSUM.as_bytes())
        .unwrap();

    let report = project_doc_report(
        &system,
        path,
        "write",
        &before,
        &after,
        &open_config(),
        test_identity(),
    )
    .unwrap();

    assert!(report.would_commit);
    assert!(report.subset_gate.is_none());
}

#[test]
fn project_doc_report_escalates_mode_to_realm_yaml() {
    // An introduced bad checksum trips the gate in any mode, so the gate's `mode` field is what
    // proves the realm escalation ran.
    let path = Path::new("/d/file.md");
    let system = MemorySystem::new()
        .with_file(Path::new("/d/.remargin.yaml"), b"mode: strict\n")
        .unwrap()
        .with_file(
            Path::new("/d/.remargin-registry.yaml"),
            b"participants:\n  eduardo:\n    type: human\n    status: active\n    pubkeys: []\n",
        )
        .unwrap()
        .with_file(path, DOC_ONE_COMMENT.as_bytes())
        .unwrap();

    let before = parser::parse(DOC_ONE_COMMENT).unwrap();
    let after = parser::parse(DOC_AAA_BAD_CHECKSUM).unwrap();

    let report = project_doc_report(
        &system,
        path,
        "write",
        &before,
        &after,
        &open_config(),
        test_identity(),
    )
    .unwrap();

    let gate = report.subset_gate.unwrap();
    assert_eq!(gate.mode, "strict");
    assert!(!report.would_commit);
}

fn system_with_one_comment() -> MemorySystem {
    MemorySystem::new()
        .with_file(Path::new("/docs/test.md"), DOC_ONE_COMMENT.as_bytes())
        .unwrap()
}

#[test]
fn plan_of_a_comment_reports_the_warn_tier_notes_the_live_write_reports() {
    let position = InsertPosition::Append;
    let request = PlanRequest::Comment {
        path: PathBuf::from("/docs/test.md"),
        params: ProjectCommentParams::new(WARNING_BODY, &position),
    };

    let report = dispatch(
        &system_with_one_comment(),
        Path::new("/docs"),
        &open_config(),
        &request,
    )
    .unwrap();

    let notes: Vec<_> = report.warnings.iter().map(|w| w.note.clone()).collect();
    assert_eq!(notes, comment_style::notes(WARNING_BODY));
    assert!(report.warnings.iter().all(|w| w.op == 0));
}

#[test]
fn plan_of_an_edit_reports_the_warn_tier_notes_the_live_edit_reports() {
    let request = PlanRequest::Edit {
        path: PathBuf::from("/docs/test.md"),
        id: "aaa",
        content: WARNING_BODY,
    };

    let report = dispatch(
        &system_with_one_comment(),
        Path::new("/docs"),
        &open_config(),
        &request,
    )
    .unwrap();

    let notes: Vec<_> = report.warnings.iter().map(|w| w.note.clone()).collect();
    assert_eq!(notes, comment_style::notes(WARNING_BODY));
}

#[test]
fn plan_of_a_batch_tags_each_warn_tier_note_with_its_sub_op() {
    let request = PlanRequest::Batch {
        path: PathBuf::from("/docs/test.md"),
        ops: vec![
            ProjectBatchOp::new(String::from("One clean line.\n")),
            ProjectBatchOp::new(String::from(WARNING_BODY)),
        ],
    };

    let report = dispatch(
        &system_with_one_comment(),
        Path::new("/docs"),
        &open_config(),
        &request,
    )
    .unwrap();

    assert_eq!(
        report.warnings.len(),
        comment_style::notes(WARNING_BODY).len()
    );
    assert!(report.warnings.iter().all(|w| w.op == 1));
}

#[test]
fn plan_of_a_clean_body_reports_no_warnings() {
    let position = InsertPosition::Append;
    let request = PlanRequest::Comment {
        path: PathBuf::from("/docs/test.md"),
        params: ProjectCommentParams::new("One clean line.\n", &position),
    };

    let report = dispatch(
        &system_with_one_comment(),
        Path::new("/docs"),
        &open_config(),
        &request,
    )
    .unwrap();

    assert_eq!(report.warnings, [] as [OpAdvice; 0]);
}
