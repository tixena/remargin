//! The MCP answers serialize to the JSON the tools have always written.

extern crate alloc;

use alloc::collections::BTreeMap;

use chrono::{DateTime, FixedOffset};
use serde_json::json;

use std::path::{Path, PathBuf};

use super::{
    ActivityPage, CommentRow, CommentsPage, GetAnswer, GetContent, GetLines, IdentityCreated,
    LsAnswer, MatchRow, QueryFile, QueryPage, ReportSpillAnswer, SandboxListAnswer, SearchFile,
    SearchPage, TimedResponse as _,
};
use crate::advice::{Advice, OpAdvice};
use crate::config::identity::IdentityReport;
use crate::config::system_prompt::ResolvedSystemPrompt;
use crate::document::{ListEntry, RmDirReport, RmOutcome, RmResult, WriteOutcome};
use crate::linter::{LintErrorView, LintReport, PermissionsLintErrorView};
use crate::operations::mv::{MvAction, MvOutcome, MvTopology};
use crate::operations::plan::{
    CommentDiff, MvDiff, MvExistence, MvState, PlanIdentity, PlanReport, PlanVerifyReport,
    PlanVerifyRow,
};
use crate::operations::prompt::{
    PromptDeleteOutcome, PromptListAnswer, PromptListEntry, PromptSetOutcome,
};
use crate::operations::purge::{
    PurgeAnswer, PurgeBulkFailure, PurgeBulkFile, PurgeBulkResult, PurgeResult,
};
use crate::operations::sandbox::SandboxListEntry;
use crate::operations::sandbox::{SandboxBulkResult, SandboxFailure};
use crate::operations::search::SearchHitLocation;
use crate::operations::sign::{RepairedChecksumEntry, SignResult, SignedEntry, SkippedEntry};
use crate::operations::verify::{
    FileVerifyOutcome, FolderVerifyReport, RecipientStatus, RowStatus, SignatureStatus,
    VerifyAnswer, VerifyReport,
};
use crate::parser::AuthorType;
use crate::permissions::inspect::{
    AllowDotFoldersView, CheckOutput, DenyOpsItemView, DenyOpsView, MatchingRule, ShowOutput,
    TrustedRootView,
};
use crate::responses::CommentCreated;

fn ts() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z").unwrap()
}

fn plain_row() -> CommentRow {
    CommentRow::Plain((
        String::from("c1"),
        4,
        String::from("alice"),
        AuthorType::Human,
        ts(),
        None,
        None,
        Vec::new(),
        Vec::new(),
        BTreeMap::default(),
        None,
        None,
        Vec::new(),
        String::from("hi"),
    ))
}

fn integrity_row() -> CommentRow {
    CommentRow::WithIntegrity((
        String::from("c1"),
        4,
        String::from("alice"),
        AuthorType::Agent,
        ts(),
        Some(String::from("p1")),
        Some(String::from("p1")),
        vec![String::from("bob")],
        vec![String::from("bob@2026-01-02T03:04:05Z")],
        BTreeMap::default(),
        Some(vec![String::from("question")]),
        Some(ts()),
        vec![String::from("a.png")],
        String::from("sha256:x"),
        None,
        String::from("hi"),
    ))
}

#[test]
fn comment_rows_have_14_or_16_columns() {
    let plain = serde_json::to_value(plain_row()).unwrap();
    assert_eq!(
        plain,
        json!([
            "c1",
            4_usize,
            "alice",
            "human",
            "2026-01-02T03:04:05Z",
            null,
            null,
            [],
            [],
            {},
            null,
            null,
            [],
            "hi"
        ])
    );
    assert_eq!(
        serde_json::to_value(integrity_row()).unwrap(),
        json!([
            "c1",
            4_usize,
            "alice",
            "agent",
            "2026-01-02T03:04:05Z",
            "p1",
            "p1",
            ["bob"],
            ["bob@2026-01-02T03:04:05Z"],
            {},
            ["question"],
            "2026-01-02T03:04:05Z",
            ["a.png"],
            "sha256:x",
            null,
            "hi"
        ])
    );
}

#[test]
fn comments_page_writes_effective_limit_only_when_the_cap_trimmed() {
    let page = |effective_limit| CommentsPage {
        comment_cols: vec![String::from("id")],
        comments: vec![plain_row()],
        effective_limit,
        elapsed_ms: 0,
        total: 9,
    };

    let whole = serde_json::to_value(page(None)).unwrap();
    assert!(whole.get("effective_limit").is_none(), "{whole}");
    assert_eq!(whole["total"], json!(9_u64));
    assert_eq!(whole["comment_cols"], json!(["id"]));

    let trimmed = serde_json::to_value(page(Some(1))).unwrap();
    assert_eq!(trimmed["effective_limit"], json!(1_u64));
}

#[test]
fn with_elapsed_ms_sets_elapsed_ms_beside_the_fields_of_its_success() {
    let page = CommentsPage {
        comment_cols: Vec::new(),
        comments: Vec::new(),
        effective_limit: None,
        elapsed_ms: 0,
        total: 0,
    };

    assert_eq!(
        serde_json::to_value(page.with_elapsed_ms(12)).unwrap(),
        json!({ "comment_cols": [], "comments": [], "elapsed_ms": 12_u64, "total": 0_u64 })
    );
    let created = CommentCreated {
        elapsed_ms: 0,
        id: String::from("abc"),
        warnings: Vec::new(),
    };
    assert_eq!(
        serde_json::to_value(created.with_elapsed_ms(3)).unwrap(),
        json!({ "elapsed_ms": 3_u64, "id": "abc" })
    );
}

#[test]
fn search_rows_have_4_or_6_columns() {
    let page = |row| SearchPage {
        effective_limit: None,
        elapsed_ms: 0,
        files: vec![SearchFile {
            matches: vec![row],
            path: String::from("a.md"),
        }],
        match_cols: vec![String::from("line")],
        total: 1,
    };

    assert_eq!(
        serde_json::to_value(page(MatchRow::Plain((
            2,
            SearchHitLocation::Body,
            String::from("needle"),
            None
        ))))
        .unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "files": [{ "matches": [[2_u64, "body", "needle", null]], "path": "a.md" }],
            "match_cols": ["line"],
            "total": 1_u64,
        })
    );
    assert_eq!(
        serde_json::to_value(page(MatchRow::WithContext((
            2,
            SearchHitLocation::Comment,
            String::from("needle"),
            Some(String::from("c1")),
            vec![String::from("before")],
            Vec::new(),
        ))))
        .unwrap()["files"][0]["matches"],
        json!([[2_u64, "comment", "needle", "c1", ["before"], []]])
    );
}

#[test]
fn query_file_omits_comments_in_summary_mode() {
    let file = |comments| QueryFile {
        comment_count: 2,
        comments,
        last_activity: None,
        matched_count: 1,
        path: "a.md".into(),
        pending_count: 0,
        pending_for: None,
    };

    assert_eq!(
        serde_json::to_value(file(None)).unwrap(),
        json!({ "comment_count": 2_u64, "matched_count": 1_u64, "path": "a.md", "pending_count": 0_u64 })
    );
    let page = QueryPage {
        base_path: String::from("./"),
        comment_cols: Vec::new(),
        effective_limit: None,
        elapsed_ms: 0,
        results: vec![file(Some(vec![plain_row()]))],
        total: 1,
    };
    let written = serde_json::to_value(page).unwrap();
    assert_eq!(written["results"][0]["comments"][0][0], json!("c1"));
    assert_eq!(written["base_path"], json!("./"));
}

#[test]
fn activity_page_writes_a_null_newest_ts_overall() {
    let page = ActivityPage {
        change_cols: Vec::new(),
        cutoff_explicit: false,
        effective_limit: None,
        elapsed_ms: 0,
        files: Vec::new(),
        newest_ts_overall: None,
        total: 0,
    };

    assert_eq!(
        serde_json::to_value(page).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "change_cols": [],
            "cutoff_explicit": false,
            "files": [],
            "newest_ts_overall": null,
            "total": 0_u64,
        })
    );
}

#[test]
fn get_answers_are_the_lines_or_the_content() {
    let content = GetAnswer::Content(GetContent {
        content: String::from("a\nb"),
        effective_end_line: None,
        links: Vec::new(),
        links_cols: vec![String::from("alias")],
        total_lines: 2,
    });
    assert_eq!(
        serde_json::to_value(content).unwrap(),
        json!({ "content": "a\nb", "links": [], "links_cols": ["alias"], "total_lines": 2_u64 })
    );

    let lines = GetAnswer::Lines(GetLines {
        effective_end_line: Some(2),
        lines: vec![String::from("a"), String::from("b")],
        links: Vec::new(),
        links_cols: Vec::new(),
        start_line: 1,
        total_lines: 5,
    });
    assert_eq!(
        serde_json::to_value(lines).unwrap(),
        json!({
            "effective_end_line": 2_u64,
            "lines": ["a", "b"],
            "links": [],
            "links_cols": [],
            "start_line": 1_u64,
            "total_lines": 5_u64,
        })
    );
}

#[test]
fn identity_created_writes_a_null_key_and_type() {
    let created = IdentityCreated {
        author_type: String::from("human"),
        elapsed_ms: 0,
        identity: String::from("alice"),
        key: None,
        yaml: String::from("identity: alice\ntype: human\n"),
    };

    assert_eq!(
        serde_json::to_value(created).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "identity": "alice",
            "key": null,
            "type": "human",
            "yaml": "identity: alice\ntype: human\n",
        })
    );
}

#[test]
fn report_spill_answers_the_caps() {
    let answer = ReportSpillAnswer {
        elapsed_ms: 0,
        inferred: true,
        previous_cap: 40_000,
        reported_size: 1000,
        spill_cap: 1000,
    };

    assert_eq!(
        serde_json::to_value(answer).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "inferred": true,
            "previous_cap": 40_000_u64,
            "reported_size": 1000_u64,
            "spill_cap": 1000_u64,
        })
    );
}

#[test]
fn ls_answers_its_entries() {
    let entries = vec![
        ListEntry {
            is_dir: false,
            path: PathBuf::from("notes/a.md"),
            remargin_last_activity: Some(String::from("2026-01-02T03:04:05Z")),
            remargin_pending: Some(2),
            size: Some(120),
        },
        ListEntry {
            is_dir: true,
            path: PathBuf::from("notes"),
            remargin_last_activity: None,
            remargin_pending: None,
            size: None,
        },
    ];
    let expected = json!({ "elapsed_ms": 0_u64, "entries": entries });

    let typed = LsAnswer {
        elapsed_ms: 0,
        entries,
    };

    assert_eq!(serde_json::to_value(&typed).unwrap(), expected);
}

#[test]
fn sandbox_list_answers_its_files() {
    let files = vec![SandboxListEntry {
        path: String::from("notes/a.md"),
        since: String::from("2026-01-02T03:04:05Z"),
    }];
    let expected = json!({ "elapsed_ms": 0_u64, "files": files });

    let typed = SandboxListAnswer {
        elapsed_ms: 0,
        files,
    };

    assert_eq!(serde_json::to_value(&typed).unwrap(), expected);
}

#[test]
fn lint_answers_its_findings_and_ok() {
    let dirty = LintReport {
        errors: vec![LintErrorView {
            line: 3,
            message: String::from("unclosed fence"),
        }],
        permissions: vec![PermissionsLintErrorView {
            column: None,
            line: Some(2),
            message: String::from("bad yaml"),
            source_file: PathBuf::from("/vault/.remargin.yaml"),
        }],
        recipients: vec![LintErrorView {
            line: 9,
            message: String::from("unknown recipient"),
        }],
    };
    assert_eq!(
        serde_json::to_value(dirty.answer()).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "errors": [{ "line": 3_usize, "message": "unclosed fence" }],
            "ok": false,
            "permissions": [{
                "column": null,
                "line": 2_usize,
                "message": "bad yaml",
                "source_file": "/vault/.remargin.yaml",
            }],
            "recipients": [{ "line": 9_usize, "message": "unknown recipient" }],
        })
    );

    let clean = LintReport {
        errors: Vec::new(),
        permissions: Vec::new(),
        recipients: Vec::new(),
    };
    assert_eq!(
        serde_json::to_value(clean.answer()).unwrap(),
        json!({ "elapsed_ms": 0_u64, "errors": [], "ok": true, "permissions": [], "recipients": [] })
    );
}

#[test]
fn mv_answers_its_outcome() {
    let outcome = MvOutcome {
        action: MvAction {
            fallback_copy: true,
            overwritten: false,
        },
        bytes_moved: 12,
        dst_absolute: PathBuf::from("/vault/b.md"),
        nested_files_moved: 3,
        src_absolute: PathBuf::from("/vault/a.md"),
        topology: MvTopology {
            is_directory: true,
            noop_same_path: false,
        },
    };
    assert_eq!(
        serde_json::to_value(outcome.answer()).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "bytes_moved": 12_usize,
            "dst_absolute": "/vault/b.md",
            "fallback_copy": true,
            "is_directory": true,
            "nested_files_moved": 3_usize,
            "noop_same_path": false,
            "overwritten": false,
            "src_absolute": "/vault/a.md",
        })
    );
}

#[test]
fn purge_answers_a_file() {
    let result = PurgeResult {
        attachments_cleaned: 2,
        comments_removed: 5,
    };
    assert_eq!(
        serde_json::to_value(PurgeAnswer::File(result.answer())).unwrap(),
        json!({ "elapsed_ms": 0_u64, "attachments_cleaned": 2_usize, "comments_removed": 5_usize })
    );
}

#[test]
fn purge_answers_a_folder() {
    let result = PurgeBulkResult {
        failed: vec![PurgeBulkFailure {
            path: PathBuf::from("/vault/docs/locked.md"),
            reason: String::from("denied"),
        }],
        purged: vec![
            PurgeBulkFile {
                attachments_cleaned: 1,
                comments_removed: 2,
                path: PathBuf::from("/vault/docs/a.md"),
            },
            PurgeBulkFile {
                attachments_cleaned: 0,
                comments_removed: 4,
                path: PathBuf::from("/elsewhere/b.md"),
            },
        ],
        skipped: vec![PathBuf::from("/vault/docs/clean.md")],
    };
    assert_eq!(
        serde_json::to_value(PurgeAnswer::Directory(result.answer(Path::new("/vault")))).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "attachments_cleaned": 1_usize,
            "comments_removed": 6_usize,
            "failed": [{ "path": "docs/locked.md", "reason": "denied" }],
            "purged": [
                { "attachments_cleaned": 1_usize, "comments_removed": 2_usize, "path": "docs/a.md" },
                { "attachments_cleaned": 0_usize, "comments_removed": 4_usize, "path": "/elsewhere/b.md" },
            ],
            "skipped": ["docs/clean.md"],
        })
    );
}

#[test]
fn rm_answers_a_file() {
    let outcome = RmOutcome::File(RmResult {
        existed: false,
        path: PathBuf::from("/vault/a.md"),
    });
    assert_eq!(
        serde_json::to_value(outcome.answer("a.md")).unwrap(),
        json!({ "elapsed_ms": 0_u64, "deleted": "a.md", "existed": false })
    );
}

#[test]
fn rm_answers_a_directory() {
    let outcome = RmOutcome::Directory(RmDirReport {
        files_deleted: vec![PathBuf::from("/vault/d/a.md")],
        folders_left_behind: vec![PathBuf::from("/vault/d/hidden")],
        folders_removed: vec![PathBuf::from("/vault/d")],
    });
    assert_eq!(
        serde_json::to_value(outcome.answer("d")).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "deleted": "d",
            "files_deleted": ["/vault/d/a.md"],
            "folders_left_behind": ["/vault/d/hidden"],
            "folders_removed": ["/vault/d"],
            "is_directory": true,
        })
    );
}

#[test]
fn sign_answers_its_lists() {
    let result = SignResult {
        elapsed_ms: 0,
        repaired: vec![RepairedChecksumEntry {
            id: String::from("c3"),
            new_checksum: String::from("sha256:new"),
            old_checksum: String::from("sha256:old"),
        }],
        signed: vec![SignedEntry {
            id: String::from("c1"),
            ts: String::from("2026-01-02T03:04:05Z"),
        }],
        skipped: vec![SkippedEntry {
            id: String::from("c2"),
            reason: String::from("already_signed"),
        }],
    };
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "repaired": [{ "id": "c3", "new_checksum": "sha256:new", "old_checksum": "sha256:old" }],
            "signed": [{ "id": "c1", "ts": "2026-01-02T03:04:05Z" }],
            "skipped": [{ "id": "c2", "reason": "already_signed" }],
        })
    );
    assert_eq!(
        serde_json::to_value(SignResult::default()).unwrap(),
        json!({ "elapsed_ms": 0_u64, "repaired": [], "signed": [], "skipped": [] })
    );
}

fn verify_row(id: &str, signature: SignatureStatus, recipients: RecipientStatus) -> RowStatus {
    RowStatus {
        author: String::from("alice"),
        checksum_ok: matches!(signature, SignatureStatus::Valid),
        id: String::from(id),
        line: 7,
        recipients,
        signature,
    }
}

#[test]
fn verify_answers_a_passing_file() {
    let report = VerifyReport {
        ok: true,
        results: vec![verify_row(
            "c1",
            SignatureStatus::Valid,
            RecipientStatus::Ok,
        )],
    };
    assert_eq!(
        serde_json::to_value(VerifyAnswer::File(report.answer())).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "ok": true,
            "results": [{
                "author": "alice",
                "checksum_ok": true,
                "id": "c1",
                "line": 7_usize,
                "recipients": { "status": "ok" },
                "signature": "valid",
            }],
        })
    );
}

#[test]
fn verify_answers_a_failing_file() {
    let report = VerifyReport {
        ok: false,
        results: vec![
            verify_row(
                "c1",
                SignatureStatus::UnknownAuthor,
                RecipientStatus::Unknown(vec![String::from("bob")]),
            ),
            verify_row("c2", SignatureStatus::Invalid, RecipientStatus::Ok),
            verify_row("c3", SignatureStatus::Missing, RecipientStatus::Ok),
        ],
    };
    let value = serde_json::to_value(VerifyAnswer::File(report.answer())).unwrap();
    assert_eq!(value["ok"], json!(false));
    assert_eq!(
        value["results"][0],
        json!({
            "author": "alice",
            "checksum_ok": false,
            "id": "c1",
            "line": 7_usize,
            "recipients": { "status": "unknown", "unresolved": ["bob"] },
            "signature": "unknown_author",
        })
    );
    assert_eq!(value["results"][1]["signature"], json!("invalid"));
    assert_eq!(value["results"][2]["signature"], json!("missing"));
}

#[test]
fn verify_answers_a_folder() {
    let passing = VerifyReport {
        ok: true,
        results: vec![verify_row(
            "c1",
            SignatureStatus::Valid,
            RecipientStatus::Ok,
        )],
    };
    let failing = VerifyReport {
        ok: false,
        results: vec![
            verify_row("c2", SignatureStatus::Valid, RecipientStatus::Ok),
            verify_row("c3", SignatureStatus::Missing, RecipientStatus::Ok),
        ],
    };
    let report = FolderVerifyReport {
        files: vec![
            FileVerifyOutcome {
                error: None,
                path: PathBuf::from("good.md"),
                report: Some(passing),
            },
            FileVerifyOutcome {
                error: None,
                path: PathBuf::from("bad.md"),
                report: Some(failing),
            },
            FileVerifyOutcome {
                error: Some(String::from("parse failed")),
                path: PathBuf::from("broken.md"),
                report: None,
            },
        ],
        ok: false,
    };
    assert_eq!(
        serde_json::to_value(VerifyAnswer::Folder(report.answer())).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "failures": [
                {
                    "bad": [{
                        "author": "alice",
                        "checksum_ok": false,
                        "id": "c3",
                        "line": 7_usize,
                        "recipients": { "status": "ok" },
                        "signature": "missing",
                    }],
                    "comments": 2_usize,
                    "path": "bad.md",
                },
                { "error": "parse failed", "path": "broken.md" },
            ],
            "files_passed": 1_usize,
            "files_verified": 3_usize,
            "ok": false,
        })
    );
}

#[test]
fn write_answers_every_flag_combination() {
    for (binary, raw) in [(false, false), (false, true), (true, false), (true, true)] {
        let outcome = WriteOutcome::default();
        assert_eq!(
            serde_json::to_value(outcome.answer("a.md", binary, raw)).unwrap(),
            json!({
                "elapsed_ms": 0_u64,
                "binary": binary,
                "noop": false,
                "raw": raw || binary,
                "written": "a.md",
            })
        );
    }
}

#[test]
fn write_answers_its_warnings_only_when_there_are_some() {
    let outcome = WriteOutcome {
        noop: true,
        warnings: vec![Advice {
            line: 4,
            message: String::from("heading skipped"),
        }],
    };
    assert_eq!(
        serde_json::to_value(outcome.answer("a.md", false, false)).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "binary": false,
            "noop": true,
            "raw": false,
            "warnings": [{ "line": 4_usize, "message": "heading skipped" }],
            "written": "a.md",
        })
    );
}

fn sandbox_result() -> SandboxBulkResult {
    SandboxBulkResult {
        changed: vec![PathBuf::from("/docs/a.md"), PathBuf::from("/other/b.md")],
        failed: vec![SandboxFailure {
            path: PathBuf::from("/docs/c.txt"),
            reason: String::from("not a markdown file"),
        }],
        skipped: vec![PathBuf::from("/docs/d.md")],
    }
}

#[test]
fn sandbox_add_answers_its_paths() {
    assert_eq!(
        serde_json::to_value(sandbox_result().add_report(Path::new("/docs"))).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "added": ["a.md", "/other/b.md"],
            "failed": [{ "path": "c.txt", "reason": "not a markdown file" }],
            "skipped": ["d.md"],
        })
    );
}

#[test]
fn sandbox_remove_answers_its_paths() {
    assert_eq!(
        serde_json::to_value(sandbox_result().remove_report(Path::new("/docs"))).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "failed": [{ "path": "c.txt", "reason": "not a markdown file" }],
            "removed": ["a.md", "/other/b.md"],
            "skipped": ["d.md"],
        })
    );
}

#[test]
fn prompt_resolve_answers_the_fallback_and_a_found_block() {
    let fallback = ResolvedSystemPrompt {
        elapsed_ms: 0,
        is_default: true,
        name: String::from("default"),
        prompt: String::from("Please process"),
        runner: None,
        source: None,
    };
    assert_eq!(
        serde_json::to_value(&fallback).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "is_default": true,
            "name": "default",
            "prompt": "Please process",
            "runner": null,
            "source": null,
        })
    );
    let found = ResolvedSystemPrompt {
        elapsed_ms: 0,
        is_default: false,
        name: String::from("docs"),
        prompt: String::from("Do it"),
        runner: Some(String::from("claude")),
        source: Some(PathBuf::from("/vault/.remargin.yaml")),
    };
    assert_eq!(
        serde_json::to_value(&found).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "is_default": false,
            "name": "docs",
            "prompt": "Do it",
            "runner": "claude",
            "source": "/vault/.remargin.yaml",
        })
    );
}

#[test]
fn prompt_set_and_delete_answer_their_flags() {
    let set = PromptSetOutcome {
        created: true,
        elapsed_ms: 0,
        noop: false,
        source: PathBuf::from("/vault/.remargin.yaml"),
    };
    assert_eq!(
        serde_json::to_value(&set).unwrap(),
        json!({ "elapsed_ms": 0_u64, "created": true, "noop": false, "source": "/vault/.remargin.yaml" })
    );
    let delete = PromptDeleteOutcome {
        absent: false,
        elapsed_ms: 0,
        left_empty: true,
        source: PathBuf::from("/vault/.remargin.yaml"),
    };
    assert_eq!(
        serde_json::to_value(&delete).unwrap(),
        json!({ "elapsed_ms": 0_u64, "absent": false, "left_empty": true, "source": "/vault/.remargin.yaml" })
    );
}

#[test]
fn prompt_list_answers_its_entries() {
    let entries = vec![
        PromptListEntry {
            folder: PathBuf::from("/vault"),
            name: Some(String::from("docs")),
            prompt: String::from("Do it"),
            runner: None,
            source: PathBuf::from("/vault/.remargin.yaml"),
        },
        PromptListEntry {
            folder: PathBuf::from("/vault/sub"),
            name: None,
            prompt: String::from("Other"),
            runner: Some(String::from("claude")),
            source: PathBuf::from("/vault/sub/.remargin.yaml"),
        },
    ];
    assert_eq!(
        serde_json::to_value(PromptListAnswer {
            elapsed_ms: 0,
            entries
        })
        .unwrap(),
        json!({ "elapsed_ms": 0_u64, "entries": [
            {
                "folder": "/vault",
                "name": "docs",
                "prompt": "Do it",
                "runner": null,
                "source": "/vault/.remargin.yaml",
            },
            {
                "folder": "/vault/sub",
                "name": null,
                "prompt": "Other",
                "runner": "claude",
                "source": "/vault/sub/.remargin.yaml",
            },
        ] })
    );
}

#[test]
fn permissions_show_answers_the_resolved_permissions() {
    let report = ShowOutput {
        allow_dot_folders: vec![AllowDotFoldersView {
            names: vec![String::from(".notes")],
            source_file: PathBuf::from("/vault/.remargin.yaml"),
        }],
        deny_ops: vec![DenyOpsView {
            ops: vec![DenyOpsItemView {
                exceptions: vec![String::from("alice")],
                name: String::from("purge"),
            }],
            path: PathBuf::from("/vault/locked"),
            source_file: PathBuf::from("/vault/.remargin.yaml"),
        }],
        elapsed_ms: 0,
        trusted_roots: vec![
            TrustedRootView {
                absolute_path: Some(PathBuf::from("/vault/src")),
                also_deny_bash: vec![String::from("rm")],
                cli_allowed: true,
                path_text: String::from("src"),
                realm_root: None,
                source_file: PathBuf::from("/vault/.remargin.yaml"),
            },
            TrustedRootView {
                absolute_path: None,
                also_deny_bash: Vec::new(),
                cli_allowed: false,
                path_text: String::from("*"),
                realm_root: Some(PathBuf::from("/vault")),
                source_file: PathBuf::from("/vault/.remargin.yaml"),
            },
        ],
    };
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "allow_dot_folders": [{ "names": [".notes"], "source_file": "/vault/.remargin.yaml" }],
            "deny_ops": [{
                "ops": [{ "exceptions": ["alice"], "name": "purge" }],
                "path": "/vault/locked",
                "source_file": "/vault/.remargin.yaml",
            }],
            "trusted_roots": [
                {
                    "absolute_path": "/vault/src",
                    "also_deny_bash": ["rm"],
                    "cli_allowed": true,
                    "path_text": "src",
                    "realm_root": null,
                    "source_file": "/vault/.remargin.yaml",
                },
                {
                    "absolute_path": null,
                    "also_deny_bash": [],
                    "cli_allowed": false,
                    "path_text": "*",
                    "realm_root": "/vault",
                    "source_file": "/vault/.remargin.yaml",
                },
            ],
        })
    );
}

#[test]
fn permissions_check_answers_with_and_without_a_rule() {
    let plain = CheckOutput {
        elapsed_ms: 0,
        matching_rule: None,
        path: PathBuf::from("/vault/a.md"),
        restricted: false,
    };
    assert_eq!(
        serde_json::to_value(&plain).unwrap(),
        json!({ "elapsed_ms": 0_u64, "matching_rule": null, "path": "/vault/a.md", "restricted": false })
    );
    let restricted = CheckOutput {
        elapsed_ms: 0,
        matching_rule: Some(MatchingRule {
            kind: String::from("trusted_roots"),
            rule_text: String::from("outside trusted_roots"),
            source_file: PathBuf::from("/vault/.remargin.yaml"),
        }),
        path: PathBuf::from("/other/a.md"),
        restricted: true,
    };
    assert_eq!(
        serde_json::to_value(&restricted).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "matching_rule": {
                "kind": "trusted_roots",
                "rule_text": "outside trusted_roots",
                "source_file": "/vault/.remargin.yaml",
            },
            "path": "/other/a.md",
            "restricted": true,
        })
    );
}

fn plan_report(mv_diff: Option<MvDiff>, warnings: Vec<OpAdvice>) -> PlanReport {
    PlanReport {
        changed_line_ranges: vec![[2, 4]],
        checksum_after: String::from("sha256:after"),
        checksum_before: String::from("sha256:before"),
        comments: CommentDiff {
            added: vec![String::from("c9")],
            destroyed: Vec::new(),
            modified: vec![String::from("c2")],
            preserved: vec![String::from("c1")],
        },
        config_diff: None,
        cp_diff: None,
        elapsed_ms: 0,
        identity: PlanIdentity::new(Some(String::from("alice")), None, true),
        mv_diff,
        noop: false,
        op: String::from("write"),
        purge_dir_diff: None,
        reject_reason: None,
        subset_gate: None,
        unprotect_diff: None,
        verify_after: PlanVerifyReport {
            ok: true,
            rows: vec![PlanVerifyRow {
                checksum_ok: true,
                id: String::from("c1"),
                signature: String::from("valid"),
            }],
        },
        warnings,
        would_commit: true,
    }
}

#[test]
fn plan_answers_a_document_projection() {
    let report = plan_report(
        None,
        vec![OpAdvice::new(
            1,
            Advice {
                line: 4,
                message: String::from("heading skipped"),
            },
        )],
    );
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "changed_line_ranges": [[2_usize, 4_usize]],
            "checksum_after": "sha256:after",
            "checksum_before": "sha256:before",
            "comments": {
                "added": ["c9"],
                "destroyed": [],
                "modified": ["c2"],
                "preserved": ["c1"],
            },
            "identity": { "author_type": null, "name": "alice", "would_sign": true },
            "noop": false,
            "op": "write",
            "reject_reason": null,
            "verify_after": {
                "ok": true,
                "rows": [{ "checksum_ok": true, "id": "c1", "signature": "valid" }],
            },
            "warnings": [{ "note": { "line": 4_usize, "message": "heading skipped" }, "op": 1_usize }],
            "would_commit": true,
        })
    );
}

#[test]
fn plan_answers_a_move_projection() {
    let report = plan_report(
        Some(MvDiff {
            dst_absolute: PathBuf::from("/vault/b.md"),
            existence: MvExistence {
                dst_exists: false,
                is_directory: false,
                src_exists: true,
            },
            nested_files_moved: 0,
            src_absolute: PathBuf::from("/vault/a.md"),
            state: MvState {
                idempotent_already_settled: false,
                noop_same_path: false,
            },
        }),
        Vec::new(),
    );
    assert_eq!(
        serde_json::to_value(&report).unwrap()["mv_diff"],
        json!({
            "dst_absolute": "/vault/b.md",
            "dst_exists": false,
            "idempotent_already_settled": false,
            "is_directory": false,
            "nested_files_moved": 0_usize,
            "noop_same_path": false,
            "src_absolute": "/vault/a.md",
            "src_exists": true,
        })
    );
}

#[test]
fn whoami_answers_a_found_and_a_missing_identity() {
    let found = IdentityReport {
        author_type: Some(String::from("human")),
        elapsed_ms: 0,
        found: true,
        identity: Some(String::from("alice")),
        key: Some(String::from("/keys/alice")),
        mode: Some(String::from("strict")),
        path: Some(String::from("/vault/.remargin.yaml")),
    };
    assert_eq!(
        serde_json::to_value(&found).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "author_type": "human",
            "found": true,
            "identity": "alice",
            "key": "/keys/alice",
            "mode": "strict",
            "path": "/vault/.remargin.yaml",
        })
    );
    assert_eq!(
        serde_json::to_value(IdentityReport::not_found()).unwrap(),
        json!({ "elapsed_ms": 0_u64, "found": false })
    );
}
