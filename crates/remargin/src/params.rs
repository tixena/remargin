//! CLI parameter bundles: one `*Params` struct per command handler.
//!
//! These lightweight structs carry the parsed / resolved inputs from the
//! `Commands` enum into the `cmd_*` handler functions. Keeping them in a
//! dedicated module separates the "parse shape" (clap grammar in `main.rs`)
//! from the "handler shape" (business logic in the `cmd_*` fns), and makes
//! future handler unit-tests easier to write without constructing the full
//! `Commands` tree.

use std::path::{Path, PathBuf};

use remargin_core::config::ResolvedConfig;
use remargin_core::document;
use remargin_core::operations::replace;

use crate::IdentityArgs;

pub struct CommentParams<'cmd> {
    pub after_comment: Option<&'cmd str>,
    pub after_heading: Option<&'cmd str>,
    pub after_line: Option<usize>,
    pub attachments: &'cmd [PathBuf],
    pub auto_ack: Option<bool>,
    pub content: &'cmd str,
    pub file: &'cmd str,
    pub json_mode: bool,
    pub remargin_kind: &'cmd [String],
    pub reply_to: Option<&'cmd str>,
    pub sandbox: bool,
    pub to: &'cmd [String],
}

pub struct GetParams<'cmd> {
    pub binary: bool,
    pub end: Option<usize>,
    pub json_mode: bool,
    pub line_numbers: bool,
    pub out: Option<&'cmd Path>,
    pub path: &'cmd str,
    pub start: Option<usize>,
}

pub struct EditParams<'cmd> {
    pub content: &'cmd str,
    pub file: &'cmd str,
    pub id: &'cmd str,
    pub json_mode: bool,
    pub remargin_kind: Option<&'cmd [String]>,
}

pub struct ActivityParams<'cmd> {
    pub explicit_path: Option<&'cmd Path>,
    pub identity_args: &'cmd IdentityArgs,
    pub json_mode: bool,
    pub pretty: bool,
    pub since: Option<&'cmd str>,
}

pub struct RestrictParams<'cmd> {
    pub also_deny_bash: &'cmd [String],
    pub cli_allowed: bool,
    pub json_mode: bool,
    pub path: &'cmd str,
    pub user_settings_explicit: Option<&'cmd Path>,
}

/// How `query` results are rendered. Mutually-exclusive successor to the
/// previous `json_mode` / `pretty` / `summary` bool triple.
pub enum QueryOutputMode {
    Json,
    Plain,
    Pretty,
    Summary,
}

/// Pending-filter knobs for `query`. These compose as a UNION at the
/// filter layer (e.g. `--pending-for-me` AND `--pending-broadcast` both
/// apply, returning the union of matching comments). Grouped into one
/// substruct so the parent [`QueryParams`] stays under clippy's
/// bool-density threshold without changing CLI semantics.
pub struct QueryPendingFilters<'cmd> {
    /// `true` when `--pending` was passed: filter to comments without
    /// any ack.
    pub any: bool,
    /// `true` when `--pending-broadcast` was passed: include
    /// broadcast-pending comments.
    pub broadcast: bool,
    /// `true` when `--pending-for-me` was passed: include comments
    /// addressed to the resolved caller identity.
    pub for_me: bool,
    /// `Some(user)` when `--pending-for <user>` was passed: include
    /// comments whose `to:` list contains `user` and which are still
    /// pending.
    pub for_user: Option<&'cmd str>,
}

pub struct PromptSetParams<'params> {
    pub config: &'params ResolvedConfig,
    pub cwd: &'params Path,
    pub folder: &'params str,
    pub json_mode: bool,
    pub name: &'params str,
    pub prompt_flag: Option<&'params str>,
}

pub struct QueryParams<'cmd> {
    pub author: Option<&'cmd str>,
    pub comment_id: Option<&'cmd str>,
    pub content_regex: Option<&'cmd str>,
    pub expanded: bool,
    pub ignore_case: bool,
    pub output: QueryOutputMode,
    pub path: &'cmd str,
    pub pending: QueryPendingFilters<'cmd>,
    pub remargin_kind: &'cmd [String],
    pub since: Option<&'cmd str>,
}

pub struct SearchParams<'cmd> {
    pub context: usize,
    pub ignore_case: bool,
    pub json_mode: bool,
    pub limit: Option<usize>,
    pub offset: usize,
    pub path: &'cmd str,
    pub pattern: &'cmd str,
    pub regex: bool,
    pub scope: &'cmd str,
}

pub struct SignParams<'cmd> {
    pub all_mine: bool,
    pub file: &'cmd str,
    pub ids: &'cmd [String],
    pub json_mode: bool,
    pub repair_checksum: bool,
}

pub struct AckParams<'cmd> {
    pub file: Option<&'cmd str>,
    pub ids: &'cmd [String],
    pub json_mode: bool,
    pub remove: bool,
    pub search_path: &'cmd str,
}

pub struct ReactParams<'cmd> {
    pub emoji: &'cmd str,
    pub file: &'cmd str,
    pub id: &'cmd str,
    pub json_mode: bool,
    pub remove: bool,
}

pub struct ReplaceParams<'cmd> {
    pub json_mode: bool,
    pub options: replace::ReplaceOptions,
    pub path: &'cmd str,
}

pub struct WriteParams<'cmd> {
    pub content: Option<&'cmd str>,
    pub json_mode: bool,
    pub opts: document::WriteOptions,
    pub path: &'cmd str,
}

/// Bundled CLI inputs for the [`crate::cmd_cp`] handler.
pub struct CpParams<'cmd> {
    pub dst: &'cmd str,
    pub force: bool,
    pub json_mode: bool,
    pub src: &'cmd str,
}

/// Bundled CLI inputs for the [`crate::cmd_mv`] handler.
pub struct MvParams<'cmd> {
    pub dst: &'cmd str,
    pub force: bool,
    pub json_mode: bool,
    pub src: &'cmd str,
}

pub struct GetImageParams<'cli> {
    pub crop: Option<&'cli str>,
    pub format: Option<&'cli str>,
    pub json_mode: bool,
    pub max_bytes: Option<u64>,
    pub max_dimension: Option<u32>,
    pub out: Option<&'cli Path>,
    pub path: &'cli str,
}
