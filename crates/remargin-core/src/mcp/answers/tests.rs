//! The MCP answers serialize to the JSON the tools have always written.

extern crate alloc;

use alloc::collections::BTreeMap;

use chrono::{DateTime, FixedOffset};
use serde_json::json;

use std::path::PathBuf;

use super::{
    ActivityPage, CommentRow, CommentsPage, GetAnswer, GetContent, GetLines, IdentityCreated,
    LsAnswer, MatchRow, QueryFile, QueryPage, ReportSpillAnswer, SandboxListAnswer, SearchFile,
    SearchPage, Timed,
};
use crate::document::ListEntry;
use crate::operations::sandbox::SandboxListEntry;
use crate::operations::search::SearchHitLocation;
use crate::parser::AuthorType;
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
fn timed_adds_elapsed_ms_beside_the_fields_of_its_success() {
    let timed = Timed {
        elapsed_ms: 12,
        value: CommentsPage {
            comment_cols: Vec::new(),
            comments: Vec::new(),
            effective_limit: None,
            total: 0,
        },
    };

    assert_eq!(
        serde_json::to_value(timed).unwrap(),
        json!({ "comment_cols": [], "comments": [], "elapsed_ms": 12_u64, "total": 0_u64 })
    );
    assert_eq!(
        serde_json::to_value(Timed {
            elapsed_ms: 3,
            value: CommentCreated {
                id: String::from("abc"),
                warnings: Vec::new(),
            },
        })
        .unwrap(),
        json!({ "elapsed_ms": 3_u64, "id": "abc" })
    );
}

#[test]
fn search_rows_have_4_or_6_columns() {
    let page = |row| SearchPage {
        effective_limit: None,
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
        files: Vec::new(),
        newest_ts_overall: None,
        total: 0,
    };

    assert_eq!(
        serde_json::to_value(page).unwrap(),
        json!({
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
        identity: String::from("alice"),
        key: None,
        yaml: String::from("identity: alice\ntype: human\n"),
    };

    assert_eq!(
        serde_json::to_value(created).unwrap(),
        json!({
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
        inferred: true,
        previous_cap: 40_000,
        reported_size: 1000,
        spill_cap: 1000,
    };

    assert_eq!(
        serde_json::to_value(answer).unwrap(),
        json!({
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
    let expected = json!({ "entries": entries });

    let typed = LsAnswer { entries };

    assert_eq!(serde_json::to_value(&typed).unwrap(), expected);
}

#[test]
fn sandbox_list_answers_its_files() {
    let files = vec![SandboxListEntry {
        path: String::from("notes/a.md"),
        since: String::from("2026-01-02T03:04:05Z"),
    }];
    let expected = json!({ "files": files });

    let typed = SandboxListAnswer { files };

    assert_eq!(serde_json::to_value(&typed).unwrap(), expected);
}
