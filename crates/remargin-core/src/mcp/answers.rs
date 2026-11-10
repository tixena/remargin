//! The successes the MCP tools answer, as model types whose JSON is the
//! JSON the tools have always written.

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use chrono::{DateTime, FixedOffset};
use serde::Serialize;
use tixschema::model_schema;

use crate::activity::{CompactFileChanges, compact_file_changes_schema};
use crate::config::identity::IdentityReport;
use crate::config::system_prompt::ResolvedSystemPrompt;
use crate::document::{
    ListEntry, RmAnswer, RmDirAnswer, RmFileAnswer, WriteAnswer, list_entry_schema,
};
use crate::linter::LintAnswer;
use crate::operations::cp::CpOutcome;
use crate::operations::links::{CompactLinkRow, compact_link_row_schema};
use crate::operations::mv::MvAnswer;
use crate::operations::plan::PlanReport;
use crate::operations::prompt::{PromptDeleteOutcome, PromptListAnswer, PromptSetOutcome};
use crate::operations::purge::{PurgeAnswer, PurgeDirAnswer, PurgeFileAnswer};
use crate::operations::query::{
    CompactCommentRow, CompactCommentRowWithIntegrity, compact_comment_row_schema,
    compact_comment_row_with_integrity_schema,
};
use crate::operations::replace::ReplaceReport;
use crate::operations::sandbox::{SandboxAddReport, SandboxRemoveReport};
use crate::operations::sandbox::{SandboxListEntry, sandbox_list_entry_schema};
use crate::operations::search::{
    CompactMatchRow, CompactMatchRowWithContext, compact_match_row_schema,
    compact_match_row_with_context_schema,
};
use crate::operations::sign::SignResult;
use crate::operations::verify::{FolderVerifySummaryView, VerifyAnswer, VerifyReportView};
use crate::permissions::doctor::DoctorReport;
use crate::permissions::inspect::{CheckOutput, ShowOutput};
use crate::responses::{
    AckAnswer, BatchAnswer, CommentCreated, CommentEdited, CommentsDeleted, ReactAnswer,
};

/// Implements `TimedResponse` for each listed answer struct by replacing its `elapsed_ms`.
macro_rules! timed_responses {
    ($($answer:ty),* $(,)?) => {$(
        impl TimedResponse for $answer {
            fn with_elapsed_ms(self, elapsed_ms: u64) -> Self {
                Self { elapsed_ms, ..self }
            }
        }
    )*};
}

/// One comment as a positional row: the 14 columns, or 16 with `checksum`
/// and `signature`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum CommentRow {
    Plain(CompactCommentRow),
    WithIntegrity(CompactCommentRowWithIntegrity),
}

/// What `comments` answers.
#[model_schema(enable = ["jsonschema"])]
#[derive(Serialize)]
#[non_exhaustive]
pub struct CommentsPage {
    pub comment_cols: Vec<String>,
    pub comments: Vec<CommentRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_limit: Option<usize>,
    /// How long the call took.
    pub elapsed_ms: u64,
    pub total: usize,
}

/// What `activity` answers.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct ActivityPage {
    pub change_cols: Vec<String>,
    pub cutoff_explicit: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_limit: Option<usize>,
    /// How long the call took.
    pub elapsed_ms: u64,
    pub files: Vec<CompactFileChanges>,
    #[model_schema_prop(nullable)]
    pub newest_ts_overall: Option<DateTime<FixedOffset>>,
    pub total: usize,
}

/// One file of a `query` answer: its counts, and its comments as positional
/// rows unless the query asked for a summary.
#[model_schema(enable = ["jsonschema"])]
#[derive(Serialize)]
#[non_exhaustive]
pub struct QueryFile {
    pub comment_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<Vec<CommentRow>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<DateTime<FixedOffset>>,
    pub matched_count: u32,
    pub path: PathBuf,
    pub pending_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_for: Option<Vec<String>>,
}

/// What `query` answers.
#[model_schema(enable = ["jsonschema"])]
#[derive(Serialize)]
#[non_exhaustive]
pub struct QueryPage {
    pub base_path: String,
    pub comment_cols: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_limit: Option<usize>,
    /// How long the call took.
    pub elapsed_ms: u64,
    pub results: Vec<QueryFile>,
    pub total: usize,
}

/// One match as a positional row: the 4 columns, or 6 with the `before` and
/// `after` context lines.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum MatchRow {
    Plain(CompactMatchRow),
    WithContext(CompactMatchRowWithContext),
}

/// The matches of one file in a `search` answer.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct SearchFile {
    pub matches: Vec<MatchRow>,
    pub path: String,
}

/// What `search` answers.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct SearchPage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_limit: Option<usize>,
    /// How long the call took.
    pub elapsed_ms: u64,
    pub files: Vec<SearchFile>,
    pub match_cols: Vec<String>,
    pub total: usize,
}

/// What `get` answers for a window read with `line_numbers: true`: the
/// lines, numbered from `start_line`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct GetLines {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_end_line: Option<usize>,
    pub lines: Vec<String>,
    pub links: Vec<CompactLinkRow>,
    pub links_cols: Vec<String>,
    pub start_line: usize,
    pub total_lines: usize,
}

/// What `get` answers for a window read as one text.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct GetContent {
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_end_line: Option<usize>,
    pub links: Vec<CompactLinkRow>,
    pub links_cols: Vec<String>,
    pub total_lines: usize,
}

/// What `get` answers for a text file.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum GetAnswer {
    Content(GetContent),
    Lines(GetLines),
}

/// What `identity_create` answers: the fields, and the YAML they render to.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct IdentityCreated {
    #[serde(rename = "type")]
    pub author_type: String,
    /// How long the call took.
    pub elapsed_ms: u64,
    pub identity: String,
    #[model_schema_prop(nullable)]
    pub key: Option<String>,
    pub yaml: String,
}

/// What `ls` answers.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct LsAnswer {
    /// How long the call took.
    pub elapsed_ms: u64,
    pub entries: Vec<ListEntry>,
}

/// What `sandbox_list` answers.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct SandboxListAnswer {
    /// How long the call took.
    pub elapsed_ms: u64,
    pub files: Vec<SandboxListEntry>,
}

/// What `report_spill` answers: the cap before the report, the size it
/// reported, and the cap now.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct ReportSpillAnswer {
    /// How long the call took.
    pub elapsed_ms: u64,
    pub inferred: bool,
    pub previous_cap: usize,
    pub reported_size: usize,
    pub spill_cap: usize,
}

/// A success that carries the time its call took, set once the call has run.
pub trait TimedResponse {
    /// Returns the success with `elapsed_ms` set.
    #[must_use]
    fn with_elapsed_ms(self, elapsed_ms: u64) -> Self;
}

timed_responses!(
    AckAnswer,
    ActivityPage,
    BatchAnswer,
    CheckOutput,
    CommentCreated,
    CommentEdited,
    CommentsDeleted,
    CommentsPage,
    CpOutcome,
    DoctorReport,
    FolderVerifySummaryView,
    IdentityCreated,
    IdentityReport,
    LintAnswer,
    LsAnswer,
    MvAnswer,
    PlanReport,
    PromptDeleteOutcome,
    PromptListAnswer,
    PromptSetOutcome,
    PurgeDirAnswer,
    PurgeFileAnswer,
    QueryPage,
    ReactAnswer,
    ReplaceReport,
    ReportSpillAnswer,
    ResolvedSystemPrompt,
    RmDirAnswer,
    RmFileAnswer,
    SandboxAddReport,
    SandboxListAnswer,
    SandboxRemoveReport,
    SearchPage,
    ShowOutput,
    SignResult,
    VerifyReportView,
    WriteAnswer,
);

impl TimedResponse for PurgeAnswer {
    fn with_elapsed_ms(self, elapsed_ms: u64) -> Self {
        match self {
            Self::Directory(answer) => Self::Directory(answer.with_elapsed_ms(elapsed_ms)),
            Self::File(answer) => Self::File(answer.with_elapsed_ms(elapsed_ms)),
        }
    }
}

impl TimedResponse for RmAnswer {
    fn with_elapsed_ms(self, elapsed_ms: u64) -> Self {
        match self {
            Self::Directory(answer) => Self::Directory(answer.with_elapsed_ms(elapsed_ms)),
            Self::File(answer) => Self::File(answer.with_elapsed_ms(elapsed_ms)),
        }
    }
}

impl TimedResponse for VerifyAnswer {
    fn with_elapsed_ms(self, elapsed_ms: u64) -> Self {
        match self {
            Self::File(answer) => Self::File(answer.with_elapsed_ms(elapsed_ms)),
            Self::Folder(answer) => Self::Folder(answer.with_elapsed_ms(elapsed_ms)),
        }
    }
}
