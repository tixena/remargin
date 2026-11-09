//! The `Remargin` service: one operation per MCP tool, from which tixschema generates the tool list and the dispatcher.

use crate::config::identity::{IdentityReport, identity_report_schema};
use crate::config::system_prompt::{ResolvedSystemPrompt, resolved_system_prompt_schema};
use crate::document::{RmAnswer, WriteAnswer, rm_answer_schema, write_answer_schema};
use crate::linter::{LintAnswer, lint_answer_schema};
use crate::mcp::answers::{
    ActivityPage, CommentsPage, IdentityCreated, LsAnswer, QueryPage, ReportSpillAnswer,
    SandboxListAnswer, SearchPage, Timed, activity_page_schema, comments_page_schema,
    identity_created_schema, ls_answer_schema, query_page_schema, report_spill_answer_schema,
    sandbox_list_answer_schema, search_page_schema, timed_schema,
};
use crate::mcp::args::{
    AckArgs, ActivityArgs, BatchArgs, CommentArgs, CommentsArgs, CpArgs, DeleteArgs, DoctorArgs,
    EditArgs, GetArgs, GetImageArgs, IdentityCreateArgs, LintArgs, LsArgs, MetadataArgs, MvArgs,
    PermissionsCheckArgs, PlanArgs, PromptDeleteArgs, PromptListArgs, PromptResolveArgs,
    PromptSetArgs, PurgeArgs, QueryArgs, ReactArgs, ReplaceArgs, ReplyArgs, ReportSpillArgs,
    RmArgs, SandboxAddArgs, SandboxListArgs, SandboxRemoveArgs, SearchArgs, SignArgs, VerifyArgs,
    WriteArgs, ack_args_schema, activity_args_schema, batch_args_schema, comment_args_schema,
    comments_args_schema, cp_args_schema, delete_args_schema, doctor_args_schema, edit_args_schema,
    get_args_schema, get_image_args_schema, identity_create_args_schema, lint_args_schema,
    ls_args_schema, metadata_args_schema, mv_args_schema, permissions_check_args_schema,
    plan_args_schema, prompt_delete_args_schema, prompt_list_args_schema,
    prompt_resolve_args_schema, prompt_set_args_schema, purge_args_schema, query_args_schema,
    react_args_schema, replace_args_schema, reply_args_schema, report_spill_args_schema,
    rm_args_schema, sandbox_add_args_schema, sandbox_list_args_schema, sandbox_remove_args_schema,
    search_args_schema, sign_args_schema, verify_args_schema, write_args_schema,
};
use crate::mcp::errors::RemarginError;
use crate::operations::cp::{CpOutcome, cp_outcome_schema};
use crate::operations::mv::{MvAnswer, mv_answer_schema};
use crate::operations::plan::{PlanReport, plan_report_schema};
use crate::operations::prompt::{
    PromptDeleteOutcome, PromptListAnswer, PromptSetOutcome, prompt_delete_outcome_schema,
    prompt_list_answer_schema, prompt_set_outcome_schema,
};
use crate::operations::purge::{PurgeAnswer, purge_answer_schema};
use crate::operations::replace::{ReplaceReport, replace_report_schema};
use crate::operations::sandbox::{
    SandboxAddReport, SandboxRemoveReport, sandbox_add_report_schema, sandbox_remove_report_schema,
};
use crate::operations::sign::{SignResult, sign_result_schema};
use crate::operations::verify::{VerifyAnswer, verify_answer_schema};
use crate::permissions::doctor::{DoctorReport, doctor_report_schema};
use crate::permissions::inspect::{
    CheckOutput, ShowOutput, check_output_schema, show_output_schema,
};
use crate::responses::{
    AckAnswer, BatchAnswer, CommentCreated, CommentEdited, CommentsDeleted, ReactAnswer,
    ack_answer_schema, batch_answer_schema, comment_created_schema, comment_edited_schema,
    comments_deleted_schema, react_answer_schema,
};
use tixschema::service_schema;

/// remargin's document access layer: read, write, comment on and query the markdown of a realm.
#[service_schema(non_exhaustive, transports = ["mcp"])]
pub trait Remargin<Ctx> {
    /// Acknowledge one or more comments (or remove this identity's ack with remove=true). Omit file to resolve by ID across the folder tree.
    async fn ack(&self, ctx: &Ctx, req: AckArgs) -> Result<Timed<AckAnswer>, RemarginError>;

    /// Call this BEFORE processing pending comments on a file or workspace you haven't acted on recently: it surfaces new comments, reactions, acks and sandbox adds you would otherwise miss. Walks `path` (a file or directory; defaults to the MCP server's working directory) and returns a compact, minified columnar payload: `{cutoff_explicit, newest_ts_overall, total, change_cols, files, effective_limit?}`. Each file is `{path, newest_ts, cutoff_applied?, changes}`, and every change is a positional row named by `change_cols`, `[ts, kind, author, author_type, comment_id, line_start, line_end, reply_to, to]`. Here `kind` is the kind of change, `ack`, `comment` or `sandbox` (not a comment's classification tags). Columns a change kind lacks are null: acks and sandbox adds null the comment-only columns, sandbox adds also null `comment_id`, and `to` is null for acks and sandbox adds but `[]` for a broadcast comment. Rows sort by `ts`. With `since` omitted, each file's cutoff is the caller's last action in that file, and files where the caller never acted return everything. Paging: `total` counts every change across all files; one call returns at most `limit` changes starting at `offset` (default 0), then cuts the page further so the response stays under the size budget, and says so with `effective_limit` (the number of changes actually returned). To read everything, call again with `offset` increased by the changes received until it reaches `total`. A file split across pages repeats its `path` on each page.
    #[service_schema_op(mcp(read_only))]
    async fn activity(
        &self,
        ctx: &Ctx,
        req: ActivityArgs,
    ) -> Result<Timed<ActivityPage>, RemarginError>;

    /// PREFERRED for any time you'll post more than one comment on a file. Atomic across the set; correctly tracks line shifts between insertions. Each sub-op has the same fields as a single `comment`; an unknown field refuses the whole batch. Comments are rendered as markdown; record observed state, not future-tense announcements.
    async fn batch(&self, ctx: &Ctx, req: BatchArgs) -> Result<Timed<BatchAnswer>, RemarginError>;

    /// Create a single comment in a document. For two or more comments on the same file in one turn, use `batch` - `comment` doesn't track line shifts between insertions, so a loop of `comment` calls will misplace later entries. Use `reply` (not this tool) when responding to an existing comment. Comments record observed/done state and are rendered as markdown; do not post future-tense announcements ("I'll do X").
    async fn comment(
        &self,
        ctx: &Ctx,
        req: CommentArgs,
    ) -> Result<Timed<CommentCreated>, RemarginError>;

    /// List the comments of one file in document order, as a compact, minified columnar payload: `{total, comment_cols, comments, effective_limit?}`. `comment_cols` names the columns once; every entry of `comments` is one comment as a positional row in that order, `[id, line, author, author_type, ts, reply_to, thread, to, ack, reactions, kind, edited_at, attachments, content]`. Columns: `id` the comment id; `line` the first line of its block in the file; `author` the author identity; `author_type` `human` or `agent`; `ts` creation time (RFC 3339); `reply_to` the parent comment id, or null for a root; `thread` the id of the thread's root comment, or null for a root; `to` the recipient identities (empty means a broadcast); `ack` one `author@ts` string per acknowledgement, the identity and the RFC 3339 time joined by `@`; `reactions` an object from each emoji to the list of `{author, ts}` that reacted with it; `kind` the classification tags, or null; `edited_at` the last edit time, or null if never edited; `attachments` stored attachment paths; `content` the markdown body, always last. With `include_integrity: true`, `checksum` and `signature` columns are inserted immediately before `content`. Filters: `kind` keeps comments carrying any of the given tags; `pending`, `pending_for` and `pending_for_me` keep unacknowledged comments with the same meaning as in `query`, and combine as a union. Reads the named file even when it is gitignored. Paging: `total` counts every comment that matched; one call returns at most `limit` comments starting at `offset` (default 0), then cuts the page further so the response stays under the size budget, and says so with `effective_limit` (the number of comments actually returned). To read everything, call again with `offset` increased by the comments received until it reaches `total`.
    #[service_schema_op(mcp(read_only))]
    async fn comments(
        &self,
        ctx: &Ctx,
        req: CommentsArgs,
    ) -> Result<Timed<CommentsPage>, RemarginError>;

    /// Copy a tracked file. The SOURCE is never modified. Non-markdown and comment-free markdown copy verbatim; a comment-bearing markdown file is copied BODY-ONLY (the duplicate carries no comment blocks) so it introduces no duplicate comment IDs and no broken signatures. Reports kind + `comments_dropped`. Use plan op=cp to preview.
    async fn cp(&self, ctx: &Ctx, req: CpArgs) -> Result<Timed<CpOutcome>, RemarginError>;

    /// Delete one or more comments
    async fn delete(
        &self,
        ctx: &Ctx,
        req: DeleteArgs,
    ) -> Result<Timed<CommentsDeleted>, RemarginError>;

    /// Run health checks on the remargin permission stack. Checks (in order): (1) hook-installed - verifies the `PreToolUse` hook is wired into Claude settings; when absent from both scopes, all other checks are skipped. Returns a structured `DoctorReport`.
    #[service_schema_op(mcp(read_only))]
    async fn doctor(
        &self,
        ctx: &Ctx,
        req: DoctorArgs,
    ) -> Result<Timed<DoctorReport>, RemarginError>;

    /// Edit a comment (cascading ack clear)
    async fn edit(&self, ctx: &Ctx, req: EditArgs) -> Result<Timed<CommentEdited>, RemarginError>;

    /// Read a file's contents. Text mode (default) returns a compact, minified payload. With `line_numbers: false` (default): `{content, total_lines, links_cols, links, effective_end_line?}`. With `line_numbers: true`: `{start_line, lines, total_lines, links_cols, links, effective_end_line?}`, where `lines` is an array of bare strings and line i's number is `start_line + i`. `total_lines` is the whole file's line count. `links` lists the local links found in the returned lines (comment blocks excluded), one positional row per target named by `links_cols`, `[alias, lines, target, title]`: `alias` the link text or null, `lines` the line numbers where it appears, `target` the link target, `title` the target document's title or null. A link's file is `target` itself when it has an extension, otherwise `target` plus `.md`. Paging: `start_line` / `end_line` choose the window; when the window does not fit the size budget, it is cut and `effective_end_line` gives the last line returned, so call again with `start_line` set to `effective_end_line + 1`. Pass `binary: true` to fetch a non-markdown file as bytes: the result is an embedded resource block (a file:// uri, mimeType and base64 blob) plus a text block `{binary, mime, path, size_bytes}`; `.md` files are refused in binary mode, and `metadata` gives the size before pulling a large blob.
    #[service_schema_op(mcp(content, read_only))]
    async fn get(
        &self,
        ctx: &Ctx,
        req: GetArgs,
    ) -> Result<Vec<remargin_schema::McpContent>, RemarginError>;

    /// Return a downscaled / cropped raster image sized to fit the MCP token budget. Use this when `get --binary` would exceed the inline limit. Accepts PNG / JPEG / GIF / WebP; rejects SVG, PDF, audio, video. Returns a content array: an image block the harness renders as vision input, plus a text block with `{format, width, height, mime, size_bytes, source}` where `source` echoes the original mime / dimensions / size. Defaults: `max_dimension=1024`, `max_bytes=262144` (256 KiB), format=jpeg for photographic source formats / png for lossless ones.
    #[service_schema_op(mcp(name = "get_image", content, read_only))]
    async fn get_image(
        &self,
        ctx: &Ctx,
        req: GetImageArgs,
    ) -> Result<Vec<remargin_schema::McpContent>, RemarginError>;

    /// Render a ready-to-use identity YAML block for `.remargin.yaml`. Returns the YAML text; the caller writes it themselves. `mode:` is never emitted (mode is a tree property, not identity-scoped).
    #[service_schema_op(mcp(name = "identity_create"))]
    async fn identity_create(
        &self,
        ctx: &Ctx,
        req: IdentityCreateArgs,
    ) -> Result<Timed<IdentityCreated>, RemarginError>;

    /// Run structural lint checks
    #[service_schema_op(mcp(read_only))]
    async fn lint(&self, ctx: &Ctx, req: LintArgs) -> Result<Timed<LintAnswer>, RemarginError>;

    /// List files and directories
    #[service_schema_op(mcp(read_only))]
    async fn ls(&self, ctx: &Ctx, req: LsArgs) -> Result<Timed<LsAnswer>, RemarginError>;

    /// Get file metadata. Always returns `size_bytes`, mime, binary, path. Markdown files additionally return `comment_count`, `line_count`, `pending_count`, `pending_for`, `last_activity`, frontmatter. Use before `get --binary` to avoid pulling large blobs.
    #[service_schema_op(mcp(content, read_only))]
    async fn metadata(
        &self,
        ctx: &Ctx,
        req: MetadataArgs,
    ) -> Result<Vec<remargin_schema::McpContent>, RemarginError>;

    /// Move or rename a tracked file or directory. Auto-detects a directory source and renames the directory + every nested file as an atomic unit; comments / threads / acks survive because the path of every nested file changes consistently. Atomic same-FS rename, copy+remove fallback on cross-filesystem (EXDEV) for files. Both endpoints flow through the same sandbox / forbidden-target / `trusted_roots` checks every other mutating op uses. Idempotent: same-path no-op; src missing AND dst already in place returns success with `bytes_moved=0`.
    async fn mv(&self, ctx: &Ctx, req: MvArgs) -> Result<Timed<MvAnswer>, RemarginError>;

    /// Gitignore-style: returns `restricted=true` when the path is outside the `trusted_roots` allow-list or covered by a `deny_ops` rule from the parent-walked `.remargin.yaml`. With `why=true`, the matching rule's kind, source file, and rule text are included. Read-only; no identity flags.
    #[service_schema_op(mcp(name = "permissions_check", read_only))]
    async fn permissions_check(
        &self,
        ctx: &Ctx,
        req: PermissionsCheckArgs,
    ) -> Result<Timed<CheckOutput>, RemarginError>;

    /// Print the resolved permissions for the MCP server's working directory (parent-walk of `.remargin.yaml`). Includes recursive expansion of `trusted_roots` that are themselves realms (bounded depth, cycle-safe). Read-only; no identity flags.
    #[service_schema_op(mcp(name = "permissions_show", read_only))]
    async fn permissions_show(&self, ctx: &Ctx) -> Result<Timed<ShowOutput>, RemarginError>;

    /// Dry-run projection for mutating ops. Returns a `PlanReport` (`noop/would_commit/reject_reason/subset_gate/checksums/changed_line_ranges/comment` diff) without touching disk. Document ops: ack, batch, comment, reply, delete, edit, purge, react, sandbox-add, sandbox-remove, sign, write. `reply` is a synonym for `comment` with required `parent_id` (translated to `reply_to`). The `subset_gate` field mirrors `SubsetGateFailure` when the projected op would introduce a new anomaly not present in the on-disk pre-state - the same shape `commit_with_verify` would return. File-relocation op: mv - surfaces an `mv_diff` describing canonical src/dst, `dst_exists`, `noop_same_path`, `idempotent_already_settled`. File-copy op: cp - surfaces a `cp_diff` describing canonical src/dst, `dst_exists`, kind (`verbatim/body_only/noop`), and `comments_to_drop`. Config ops (`claude_restrict` / `claude_unrestrict`) are CLI-only - use `remargin plan claude restrict` / `remargin plan claude unrestrict`.
    async fn plan(&self, ctx: &Ctx, req: PlanArgs) -> Result<Timed<PlanReport>, RemarginError>;

    /// Strip the `system_prompt:` block from `<folder>/.remargin.yaml`. Idempotent: a missing block (or missing file) succeeds. The .remargin.yaml is preserved even if it ends up empty. Folder defaults to the MCP root.
    #[service_schema_op(mcp(name = "prompt_delete"))]
    async fn prompt_delete(
        &self,
        ctx: &Ctx,
        req: PromptDeleteArgs,
    ) -> Result<Timed<PromptDeleteOutcome>, RemarginError>;

    /// Recursively list every `.remargin.yaml` under `folder` that declares a `system_prompt:` block. Read-only. Folder defaults to the MCP root.
    #[service_schema_op(mcp(name = "prompt_list", read_only))]
    async fn prompt_list(
        &self,
        ctx: &Ctx,
        req: PromptListArgs,
    ) -> Result<Timed<PromptListAnswer>, RemarginError>;

    /// Resolve the nearest folder-scoped system prompt for a file or directory. Walks `.remargin.yaml` files upward; first one declaring a `system_prompt:` block wins. Falls through to a locked Default body when the walk exhausts. Read-only.
    #[service_schema_op(mcp(name = "prompt_resolve", read_only))]
    async fn prompt_resolve(
        &self,
        ctx: &Ctx,
        req: PromptResolveArgs,
    ) -> Result<Timed<ResolvedSystemPrompt>, RemarginError>;

    /// Create or replace the `system_prompt:` block in `<folder>/.remargin.yaml`. Other fields in the file are preserved byte-for-byte. The post-write diff refuses any change outside the `system_prompt` mapping. Folder defaults to the MCP root when omitted.
    #[service_schema_op(mcp(name = "prompt_set"))]
    async fn prompt_set(
        &self,
        ctx: &Ctx,
        req: PromptSetArgs,
    ) -> Result<Timed<PromptSetOutcome>, RemarginError>;

    /// Strip all comments from a document, or every .md file in a directory when `recursive` is true. To preview without writing, use `plan` with op="purge".
    async fn purge(&self, ctx: &Ctx, req: PurgeArgs) -> Result<Timed<PurgeAnswer>, RemarginError>;

    /// Search comments across documents. Returns a compact, minified columnar payload: `{base_path, total, comment_cols, results, effective_limit?}`. `base_path` is the searched directory (or a file's folder), ending in `/`, and every result `path` is relative to it. Each result is one file: `{path, comment_count, matched_count, pending_count, pending_for?, last_activity?, comments}`, where `comment_count`, `pending_count`, `pending_for` and `last_activity` describe the whole file and `matched_count` counts the comments that passed the filters. `comments` holds this page's matching comments as positional rows named by `comment_cols`, `[id, line, author, author_type, ts, reply_to, thread, to, ack, reactions, kind, edited_at, attachments, content]`. Columns: `id` the comment id; `line` the first line of its block in the file; `author` the author identity; `author_type` `human` or `agent`; `ts` creation time (RFC 3339); `reply_to` the parent comment id, or null for a root; `thread` the id of the thread's root comment, or null for a root; `to` the recipient identities (empty means a broadcast); `ack` one `author@ts` string per acknowledgement, the identity and the RFC 3339 time joined by `@`; `reactions` an object from each emoji to the list of `{author, ts}` that reacted with it; `kind` the classification tags, or null; `edited_at` the last edit time, or null if never edited; `attachments` stored attachment paths; `content` the markdown body, always last. With `include_integrity: true`, `checksum` and `signature` columns are inserted immediately before `content`. With `summary: true` the per-file `comments` arrays are left out. Filters: `kind`, `author`, `since`, `comment_id`, `content_regex`; the pending filters (`pending`, `pending_for`, `pending_for_me`, `pending_broadcast`) combine as a union. `pending` covers directed comments with an unacknowledged recipient and broadcast comments nobody acknowledged; `pending_for_me` and `pending_broadcast` use the MCP server's identity. A directory search skips gitignored files; naming a file reads it regardless. Paging: `total` counts every matching comment across all files (every listed file in summary mode); one call returns at most `limit` comments starting at `offset` (default 0), then cuts the page further so the response stays under the size budget, and says so with `effective_limit` (the number of comments actually returned). To read everything, call again with `offset` increased by the comments received until it reaches `total`. A file split across pages repeats its summary fields on each page.
    #[service_schema_op(mcp(read_only))]
    async fn query(&self, ctx: &Ctx, req: QueryArgs) -> Result<Timed<QueryPage>, RemarginError>;

    /// Add or remove an emoji reaction
    async fn react(&self, ctx: &Ctx, req: ReactArgs) -> Result<Timed<ReactAnswer>, RemarginError>;

    /// Find/replace across document BODY text only (never inside comments), over a file or folder. Integrity-gated: each file flows through comment-preservation plus the post-verify subset gate, so a comment can never be corrupted. A pattern that occurs only inside comments is a no-op. In a folder, every visible `.md` file is rewritten; a file the gate refuses is skipped, recorded, and the run continues. `path` is required (no silent cwd fan-out). To preview without writing, use the CLI `remargin replace --dry-run`.
    async fn replace(
        &self,
        ctx: &Ctx,
        req: ReplaceArgs,
    ) -> Result<Timed<ReplaceReport>, RemarginError>;

    /// PREFERRED way to respond to a comment. Wraps `comment` with required `parent_id`; the parent must live in the named `file`. Smart auto-ack default: if `auto_ack` is omitted, the parent is acked iff its author differs from the caller (replies to your own comments don't ack). Set `auto_ack: true` to force the ack, `auto_ack: false` to skip it. Comments render as markdown; record observed state, not future-tense announcements.
    async fn reply(
        &self,
        ctx: &Ctx,
        req: ReplyArgs,
    ) -> Result<Timed<CommentCreated>, RemarginError>;

    /// Call this the moment your client spills a remargin tool result to a file (because it exceeded the client's output-token limit) - BEFORE you read that file. remargin infers the offending size from the last result it emitted and ratchets its per-session page cap down so future `search` pages stay under your client's limit. The cap only ever falls, never rises, and resets each session. Omit `size` for the inferred value; pass it only to force an explicit one.
    #[service_schema_op(mcp(name = "report_spill"))]
    async fn report_spill(
        &self,
        ctx: &Ctx,
        req: ReportSpillArgs,
    ) -> Result<Timed<ReportSpillAnswer>, RemarginError>;

    /// Remove a file from the managed document tree (idempotent). Pointed at a directory it removes the tree recursively (always recursive, no flag): it deletes everything remargin can see bottom-up, leaves behind only directories holding entries it cannot list (hidden files, a nested .remargin.yaml), and returns a report {`files_deleted`, `folders_removed`, `folders_left_behind`}. If any listed resource is unreadable the call fails and nothing is deleted. A markdown file carrying one or more comments is refused (single file or anywhere in a directory tree): purge the path first to strip its comments, then rm.
    async fn rm(&self, ctx: &Ctx, req: RmArgs) -> Result<Timed<RmAnswer>, RemarginError>;

    /// Stage one or more markdown files in the caller's sandbox. Idempotent per identity.
    #[service_schema_op(mcp(name = "sandbox_add"))]
    async fn sandbox_add(
        &self,
        ctx: &Ctx,
        req: SandboxAddArgs,
    ) -> Result<Timed<SandboxAddReport>, RemarginError>;

    /// Return all markdown files in the given path that are staged for the caller's identity.
    #[service_schema_op(mcp(name = "sandbox_list", read_only))]
    async fn sandbox_list(
        &self,
        ctx: &Ctx,
        req: SandboxListArgs,
    ) -> Result<Timed<SandboxListAnswer>, RemarginError>;

    /// Remove the caller's sandbox entry from one or more markdown files. Idempotent per identity.
    #[service_schema_op(mcp(name = "sandbox_remove"))]
    async fn sandbox_remove(
        &self,
        ctx: &Ctx,
        req: SandboxRemoveArgs,
    ) -> Result<Timed<SandboxRemoveReport>, RemarginError>;

    /// Search across documents for text matches. Returns a compact, minified columnar payload: `{total, match_cols, files}` (plus `effective_limit` when a page was clamped), where each file is `{path, matches}` and every match is a positional row named by `match_cols` (`[line, location, text, comment_id]`). Matches are grouped by file so `path` is stated once and files appear in first-match order. `location` is `body` / `comment`; `comment_id` is `null` for body matches. With `context` > 0 the rows widen to `[line, location, text, comment_id, before, after]` (before / after being string arrays) and `match_cols` reflects it. `total` is the exact corpus match count; a bounded page carries `effective_limit`, so advance `offset` to page for the rest.
    #[service_schema_op(mcp(read_only))]
    async fn search(&self, ctx: &Ctx, req: SearchArgs) -> Result<Timed<SearchPage>, RemarginError>;

    /// Back-sign missing-signature comments authored by the current identity. Refuses to sign comments authored by anyone else (forgery guard). Already-signed comments listed under `ids` are reported as skipped; `all_mine` silently excludes them. Pass exactly one of `ids` or `all_mine`. Pass `repair_checksum: true` to recompute the target comment's checksum from its current bytes before signing (useful when the content was edited out-of-band and the signer wants to re-vouch for the updated content). To preview without writing, use `plan` with op="sign".
    async fn sign(&self, ctx: &Ctx, req: SignArgs) -> Result<Timed<SignResult>, RemarginError>;

    /// Verify comment integrity (checksums and signatures) against the participant registry. Accepts a file or a directory: a directory walks every visible `.md` file recursively (honoring `.gitignore`) and returns a failures-only summary {ok, `files_verified`, `files_passed`, failures}: passing files are counted, never enumerated; each failing file lists only its not-clean rows (or a parse/read `error`). A file returns per-comment status plus an aggregate `ok` flag driven by the active mode.
    async fn verify(
        &self,
        ctx: &Ctx,
        req: VerifyArgs,
    ) -> Result<Timed<VerifyAnswer>, RemarginError>;

    /// Return the MCP server's startup-resolved identity. Returns `{found: false}` when no identity is configured (soft miss; never errors on missing config).
    #[service_schema_op(mcp(read_only))]
    async fn whoami(&self, ctx: &Ctx) -> Result<Timed<IdentityReport>, RemarginError>;

    /// Replace whole file OR a contiguous line range (via `start_line/end_line` - 1-indexed inclusive). For markdown files, existing remargin comment blocks are preserved automatically - do not pre-strip them, and do not regenerate the whole file when only a section changed. Pass `start_line` and `end_line` together for surgical edits.
    async fn write(&self, ctx: &Ctx, req: WriteArgs) -> Result<Timed<WriteAnswer>, RemarginError>;
}
