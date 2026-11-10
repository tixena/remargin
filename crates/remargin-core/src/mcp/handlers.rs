//! The handlers of the generated server: each tool's body, reading its typed arguments and
//! answering its typed success.

use core::future::{Future, ready};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use anyhow::{Context as _, bail};

use crate::activity::{self, CompactFileChanges};
use crate::comment_style;
use crate::config::identity::{IdentityFlags, IdentityReport, resolve_identity_report};
use crate::config::system_prompt::ResolvedSystemPrompt;
use crate::config::system_prompt::resolve_system_prompt;
use crate::document::get_image as image_ops;
use crate::document::{self, RmAnswer, WriteAnswer};
use crate::linter::{self, LintAnswer};
use crate::mcp::answers::{
    ActivityPage, CommentRow, CommentsPage, IdentityCreated, LsAnswer, QueryFile, QueryPage,
    ReportSpillAnswer, SandboxListAnswer, SearchPage, TimedResponse,
};
use crate::mcp::args::{
    AckArgs, ActivityArgs, BatchArgs, CommentArgs, CommentsArgs, CpArgs, DeleteArgs, DoctorArgs,
    EditArgs, GetArgs, GetImageArgs, IdentityCreateArgs, IdentityKind, ImageFormat, LintArgs,
    LsArgs, MetadataArgs, MvArgs, PermissionsCheckArgs, PlanArgs, PlanOpArgs, PromptDeleteArgs,
    PromptListArgs, PromptResolveArgs, PromptSetArgs, PurgeArgs, QueryArgs, ReactArgs, ReplaceArgs,
    ReplyArgs, ReportSpillArgs, RmArgs, SandboxAddArgs, SandboxListArgs, SandboxRemoveArgs,
    SearchArgs, SignArgs, VerifyArgs, WriteArgs,
};
use crate::mcp::context::RequestContext;
use crate::mcp::errors::RemarginError;
use crate::mcp::service::Remargin;
use crate::mcp::service::remargin_schema::{McpContent, McpResourceContents};
use crate::mcp::{
    SessionState, column_names, comment_row, enforce_ack_skip_reasons, get_envelope, page_grouped,
    page_window, query_file, read_get_window, search_compact_envelope, size_page,
};
use crate::operations::batch::BatchCommentOp;
use crate::operations::cp::{self as cp_op, CpOutcome};
use crate::operations::mv::{self as mv_op, MvAnswer};
use crate::operations::plan::{self as plan_ops, PlanReport};
use crate::operations::projections;
use crate::operations::prompt::{
    self as prompt_ops, PromptDeleteOutcome, PromptListAnswer, PromptSetOutcome,
};
use crate::operations::purge::{self, PurgeAnswer};
use crate::operations::query::{self, QueryFilter};
use crate::operations::replace::{self, ReplaceReport};
use crate::operations::sandbox::{self as sandbox_ops, SandboxAddReport, SandboxRemoveReport};
use crate::operations::search;
use crate::operations::sign::SignResult;
use crate::operations::verify::VerifyAnswer;
use crate::operations::{self, CreateCommentParams};
use crate::parser;
use crate::path::expand_path;
use crate::permissions::doctor::{self as permissions_doctor, DoctorReport};
use crate::permissions::inspect::{self as permissions_inspect, CheckOutput, ShowOutput};
use crate::responses::{
    self, AckAnswer, BatchAnswer, CommentCreated, CommentEdited, CommentsDeleted, ReactAnswer,
};
use crate::wait::Wait as _;
use crate::writer::InsertPosition;

macro_rules! refuse {
    ($($arg:tt)*) => {
        return Err(anyhow::anyhow!($($arg)*).into())
    };
}

struct CommentFields<'req> {
    ack_skip_reason: Option<&'req str>,
    after_comment: Option<&'req str>,
    after_heading: Option<&'req str>,
    after_line: Option<u64>,
    attachments: &'req [String],
    auto_ack: Option<bool>,
    content: &'req str,
    file: &'req Path,
    kind: &'req [String],
    reply_to: Option<&'req str>,
    sandbox: bool,
    to: &'req [String],
}

struct PlanPaths {
    dst: Option<PathBuf>,
    file: Option<PathBuf>,
    src: Option<PathBuf>,
}

/// The server's state across requests: what the listing tools learn about the client's size
/// limit.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Server {
    session: Mutex<SessionState>,
}

impl PlanPaths {
    fn file(&self) -> anyhow::Result<PathBuf> {
        self.file.clone().context("missing required field: file")
    }

    fn src_dst(&self, force: bool) -> anyhow::Result<(PathBuf, PathBuf, bool)> {
        Ok((
            self.src.clone().context("missing required field: src")?,
            self.dst.clone().context("missing required field: dst")?,
            force,
        ))
    }
}

impl Server {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the size of the answer just written, which `report_spill` reads back.
    pub fn note_response_size(&self, size: usize) {
        self.session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .record_response_size(size);
    }

    fn spill_cap(&self) -> usize {
        self.session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .spill_cap()
    }
}

impl Remargin<RequestContext<'_>> for Server {
    fn ack(
        &self,
        ctx: &RequestContext<'_>,
        req: AckArgs,
    ) -> impl Future<Output = Result<AckAnswer, RemarginError>> + Send {
        ready(timed(|| ack_now(ctx, &req)))
    }

    fn activity(
        &self,
        ctx: &RequestContext<'_>,
        req: ActivityArgs,
    ) -> impl Future<Output = Result<ActivityPage, RemarginError>> + Send {
        ready(timed(|| activity_now(ctx, &req, self.spill_cap())))
    }

    fn batch(
        &self,
        ctx: &RequestContext<'_>,
        req: BatchArgs,
    ) -> impl Future<Output = Result<BatchAnswer, RemarginError>> + Send {
        ready(timed(|| batch_now(ctx, &req)))
    }

    fn comment(
        &self,
        ctx: &RequestContext<'_>,
        req: CommentArgs,
    ) -> impl Future<Output = Result<CommentCreated, RemarginError>> + Send {
        ready(timed(|| comment_now(ctx, &req)))
    }

    fn comments(
        &self,
        ctx: &RequestContext<'_>,
        req: CommentsArgs,
    ) -> impl Future<Output = Result<CommentsPage, RemarginError>> + Send {
        ready(timed(|| comments_now(ctx, &req, self.spill_cap())))
    }

    fn cp(
        &self,
        ctx: &RequestContext<'_>,
        req: CpArgs,
    ) -> impl Future<Output = Result<CpOutcome, RemarginError>> + Send {
        ready(timed(|| cp_now(ctx, &req)))
    }

    fn delete(
        &self,
        ctx: &RequestContext<'_>,
        req: DeleteArgs,
    ) -> impl Future<Output = Result<CommentsDeleted, RemarginError>> + Send {
        ready(timed(|| delete_now(ctx, &req)))
    }

    fn doctor(
        &self,
        ctx: &RequestContext<'_>,
        req: DoctorArgs,
    ) -> impl Future<Output = Result<DoctorReport, RemarginError>> + Send {
        ready(timed(|| doctor_now(ctx, &req)))
    }

    fn edit(
        &self,
        ctx: &RequestContext<'_>,
        req: EditArgs,
    ) -> impl Future<Output = Result<CommentEdited, RemarginError>> + Send {
        ready(timed(|| edit_now(ctx, &req)))
    }

    fn get(
        &self,
        ctx: &RequestContext<'_>,
        req: GetArgs,
    ) -> impl Future<Output = Result<Vec<McpContent>, RemarginError>> + Send {
        ready(get_now(ctx, &req, self.spill_cap()))
    }

    fn get_image(
        &self,
        ctx: &RequestContext<'_>,
        req: GetImageArgs,
    ) -> impl Future<Output = Result<Vec<McpContent>, RemarginError>> + Send {
        ready(get_image_now(ctx, &req))
    }

    fn identity_create(
        &self,
        ctx: &RequestContext<'_>,
        req: IdentityCreateArgs,
    ) -> impl Future<Output = Result<IdentityCreated, RemarginError>> + Send {
        ready(timed(|| identity_create_now(ctx, &req)))
    }

    fn lint(
        &self,
        ctx: &RequestContext<'_>,
        req: LintArgs,
    ) -> impl Future<Output = Result<LintAnswer, RemarginError>> + Send {
        ready(timed(|| lint_now(ctx, &req)))
    }

    fn ls(
        &self,
        ctx: &RequestContext<'_>,
        req: LsArgs,
    ) -> impl Future<Output = Result<LsAnswer, RemarginError>> + Send {
        ready(timed(|| ls_now(ctx, &req)))
    }

    fn metadata(
        &self,
        ctx: &RequestContext<'_>,
        req: MetadataArgs,
    ) -> impl Future<Output = Result<Vec<McpContent>, RemarginError>> + Send {
        ready(metadata_now(ctx, &req))
    }

    fn mv(
        &self,
        ctx: &RequestContext<'_>,
        req: MvArgs,
    ) -> impl Future<Output = Result<MvAnswer, RemarginError>> + Send {
        ready(timed(|| mv_now(ctx, &req)))
    }

    fn permissions_check(
        &self,
        ctx: &RequestContext<'_>,
        req: PermissionsCheckArgs,
    ) -> impl Future<Output = Result<CheckOutput, RemarginError>> + Send {
        ready(timed(|| permissions_check_now(ctx, &req)))
    }

    fn permissions_show(
        &self,
        ctx: &RequestContext<'_>,
    ) -> impl Future<Output = Result<ShowOutput, RemarginError>> + Send {
        ready(timed(|| permissions_show_now(ctx)))
    }

    fn plan(
        &self,
        ctx: &RequestContext<'_>,
        req: PlanArgs,
    ) -> impl Future<Output = Result<PlanReport, RemarginError>> + Send {
        ready(timed(|| plan_now(ctx, &req)))
    }

    fn prompt_delete(
        &self,
        ctx: &RequestContext<'_>,
        req: PromptDeleteArgs,
    ) -> impl Future<Output = Result<PromptDeleteOutcome, RemarginError>> + Send {
        ready(timed(|| prompt_delete_now(ctx, &req)))
    }

    fn prompt_list(
        &self,
        ctx: &RequestContext<'_>,
        req: PromptListArgs,
    ) -> impl Future<Output = Result<PromptListAnswer, RemarginError>> + Send {
        ready(timed(|| prompt_list_now(ctx, &req)))
    }

    fn prompt_resolve(
        &self,
        ctx: &RequestContext<'_>,
        req: PromptResolveArgs,
    ) -> impl Future<Output = Result<ResolvedSystemPrompt, RemarginError>> + Send {
        ready(timed(|| prompt_resolve_now(ctx, &req)))
    }

    fn prompt_set(
        &self,
        ctx: &RequestContext<'_>,
        req: PromptSetArgs,
    ) -> impl Future<Output = Result<PromptSetOutcome, RemarginError>> + Send {
        ready(timed(|| prompt_set_now(ctx, &req)))
    }

    fn purge(
        &self,
        ctx: &RequestContext<'_>,
        req: PurgeArgs,
    ) -> impl Future<Output = Result<PurgeAnswer, RemarginError>> + Send {
        ready(timed(|| purge_now(ctx, &req)))
    }

    fn query(
        &self,
        ctx: &RequestContext<'_>,
        req: QueryArgs,
    ) -> impl Future<Output = Result<QueryPage, RemarginError>> + Send {
        ready(timed(|| query_now(ctx, &req, self.spill_cap())))
    }

    fn react(
        &self,
        ctx: &RequestContext<'_>,
        req: ReactArgs,
    ) -> impl Future<Output = Result<ReactAnswer, RemarginError>> + Send {
        ready(timed(|| react_now(ctx, &req)))
    }

    fn replace(
        &self,
        ctx: &RequestContext<'_>,
        req: ReplaceArgs,
    ) -> impl Future<Output = Result<ReplaceReport, RemarginError>> + Send {
        ready(timed(|| replace_now(ctx, &req)))
    }

    fn reply(
        &self,
        ctx: &RequestContext<'_>,
        req: ReplyArgs,
    ) -> impl Future<Output = Result<CommentCreated, RemarginError>> + Send {
        ready(timed(|| reply_now(ctx, &req)))
    }

    fn report_spill(
        &self,
        ctx: &RequestContext<'_>,
        req: ReportSpillArgs,
    ) -> impl Future<Output = Result<ReportSpillAnswer, RemarginError>> + Send {
        ready(timed(|| report_spill_now(ctx, &req, &self.session)))
    }

    fn rm(
        &self,
        ctx: &RequestContext<'_>,
        req: RmArgs,
    ) -> impl Future<Output = Result<RmAnswer, RemarginError>> + Send {
        ready(timed(|| rm_now(ctx, &req)))
    }

    fn sandbox_add(
        &self,
        ctx: &RequestContext<'_>,
        req: SandboxAddArgs,
    ) -> impl Future<Output = Result<SandboxAddReport, RemarginError>> + Send {
        ready(timed(|| sandbox_add_now(ctx, &req)))
    }

    fn sandbox_list(
        &self,
        ctx: &RequestContext<'_>,
        req: SandboxListArgs,
    ) -> impl Future<Output = Result<SandboxListAnswer, RemarginError>> + Send {
        ready(timed(|| sandbox_list_now(ctx, &req)))
    }

    fn sandbox_remove(
        &self,
        ctx: &RequestContext<'_>,
        req: SandboxRemoveArgs,
    ) -> impl Future<Output = Result<SandboxRemoveReport, RemarginError>> + Send {
        ready(timed(|| sandbox_remove_now(ctx, &req)))
    }

    fn search(
        &self,
        ctx: &RequestContext<'_>,
        req: SearchArgs,
    ) -> impl Future<Output = Result<SearchPage, RemarginError>> + Send {
        ready(timed(|| search_now(ctx, &req, self.spill_cap())))
    }

    fn sign(
        &self,
        ctx: &RequestContext<'_>,
        req: SignArgs,
    ) -> impl Future<Output = Result<SignResult, RemarginError>> + Send {
        ready(timed(|| sign_now(ctx, &req)))
    }

    fn verify(
        &self,
        ctx: &RequestContext<'_>,
        req: VerifyArgs,
    ) -> impl Future<Output = Result<VerifyAnswer, RemarginError>> + Send {
        ready(timed(|| verify_now(ctx, &req)))
    }

    fn whoami(
        &self,
        ctx: &RequestContext<'_>,
    ) -> impl Future<Output = Result<IdentityReport, RemarginError>> + Send {
        ready(timed(|| whoami_now(ctx)))
    }

    fn write(
        &self,
        ctx: &RequestContext<'_>,
        req: WriteArgs,
    ) -> impl Future<Output = Result<WriteAnswer, RemarginError>> + Send {
        ready(timed(|| write_now(ctx, &req)))
    }
}

fn timed<T>(work: impl FnOnce() -> Result<T, RemarginError>) -> Result<T, RemarginError>
where
    T: TimedResponse,
{
    let start = Instant::now();
    let value = work()?;
    Ok(value.with_elapsed_ms(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)))
}

fn usize_of(value: Option<u64>) -> Option<usize> {
    let number = value?;
    usize::try_from(number).ok()
}

fn required<'field>(value: Option<&'field str>, field: &str) -> anyhow::Result<&'field str> {
    value.with_context(|| format!("missing required field: {field}"))
}

fn present<'field>(fields: &[Option<&'field str>]) -> Vec<&'field str> {
    fields.iter().flatten().copied().collect()
}

fn insert_position(
    reply_to: Option<&str>,
    after_comment: Option<&str>,
    after_heading: Option<&str>,
    after_line: Option<u64>,
) -> InsertPosition {
    InsertPosition::from_hints(reply_to, after_comment, after_heading, usize_of(after_line))
}

fn ack_now(ctx: &RequestContext<'_>, req: &AckArgs) -> Result<AckAnswer, RemarginError> {
    let mut given = present(&[req.file.as_deref(), req.path.as_deref()]);
    if given.is_empty() {
        given.push(".");
    }
    let mut admitted = ctx.admit("ack", &given)?.into_iter();
    let document = req.file.as_ref().and_then(|_| admitted.next());
    let search = req.path.as_ref().and_then(|_| admitted.next());

    if let Some(file) = document {
        let path = ctx.base_dir.join(file);
        let id_refs: Vec<&str> = req.ids.iter().map(String::as_str).collect();
        operations::ack_comments(ctx.system, &path, &ctx.config, &id_refs, req.remove)?;
    } else {
        let search_dir = ctx
            .base_dir
            .join(search.unwrap_or_else(|| PathBuf::from(".")));
        for comment_id in &req.ids {
            let matches = query::resolve_comment_id(ctx.system, &search_dir, comment_id)?;
            match matches.len() {
                0 => {
                    return Err(anyhow::anyhow!("comment {comment_id:?} not found").into());
                }
                1 => {
                    let id_refs: Vec<&str> = vec![comment_id.as_str()];
                    operations::ack_comments(
                        ctx.system,
                        &matches[0],
                        &ctx.config,
                        &id_refs,
                        req.remove,
                    )?;
                }
                n => {
                    let file_list: Vec<String> =
                        matches.iter().map(|p| p.display().to_string()).collect();
                    return Err(anyhow::anyhow!(
                        "ambiguous: comment {comment_id:?} found in {n} files: {}",
                        file_list.join(", ")
                    )
                    .into());
                }
            }
        }
    }

    Ok(responses::ack(&req.ids, req.remove))
}

fn activity_now(
    ctx: &RequestContext<'_>,
    req: &ActivityArgs,
    spill_cap: usize,
) -> Result<ActivityPage, RemarginError> {
    let admitted = ctx.admit("activity", &[req.path.as_deref().unwrap_or(".")])?;
    let target = req.path.as_ref().map_or_else(
        || ctx.base_dir.to_path_buf(),
        |_| ctx.base_dir.join(&admitted[0]),
    );
    let cutoff = match req.since.as_deref() {
        Some(raw) => Some(
            chrono::DateTime::parse_from_rfc3339(raw)
                .with_context(|| format!("activity: invalid ISO 8601 since={raw:?}"))?,
        ),
        None => None,
    };
    let result = activity::gather_activity(ctx.system, &target, cutoff, &ctx.config)?;
    let files: Vec<CompactFileChanges> = result.files.iter().map(activity::compact_file).collect();
    let page = page_grouped(
        files,
        usize_of(req.offset).unwrap_or(0),
        usize_of(req.limit),
        spill_cap,
    );
    let (effective_limit, groups, total) = page.into_parts();
    Ok(ActivityPage {
        change_cols: column_names(&activity::CHANGE_COLS),
        cutoff_explicit: result.cutoff_explicit,
        effective_limit,
        elapsed_ms: 0,
        files: groups,
        newest_ts_overall: result.newest_ts_overall,
        total,
    })
}

fn batch_now(ctx: &RequestContext<'_>, req: &BatchArgs) -> Result<BatchAnswer, RemarginError> {
    let admitted = ctx.admit("batch", &[req.file.as_str()])?;
    let mut batch_ops = Vec::with_capacity(req.operations.len());
    for (idx, op) in req.operations.iter().enumerate() {
        let value = serde_json::to_value(op).context("reading a batch operation")?;
        let object = value
            .as_object()
            .with_context(|| format!("batch op[{idx}]: expected object"))?;
        batch_ops.push(BatchCommentOp::from_json_object(object, idx)?);
    }

    let ack_gate: Vec<_> = batch_ops
        .iter()
        .zip(&req.operations)
        .map(|(op, source)| {
            (
                op.reply_to.as_deref(),
                op.auto_ack,
                source.ack_skip_reason.as_deref(),
            )
        })
        .collect();
    let file = admitted[0].to_string_lossy();
    enforce_ack_skip_reasons(ctx.system, ctx.base_dir, &ctx.config, &file, &ack_gate)?;

    let path = ctx.base_dir.join(&admitted[0]);
    let outcome = operations::batch::batch_comment(ctx.system, &path, &ctx.config, &batch_ops)?;
    Ok(responses::batch(&outcome.ids, outcome.warnings))
}

fn comment_now(
    ctx: &RequestContext<'_>,
    req: &CommentArgs,
) -> Result<CommentCreated, RemarginError> {
    let admitted = ctx.admit("comment", &[req.file.as_str()])?;
    create_comment_now(
        ctx,
        &CommentFields {
            ack_skip_reason: req.ack_skip_reason.as_deref(),
            after_comment: req.after_comment.as_deref(),
            after_heading: req.after_heading.as_deref(),
            after_line: req.after_line,
            attachments: &req.attachments,
            auto_ack: req.auto_ack,
            content: &req.content,
            file: &admitted[0],
            kind: &req.kind,
            reply_to: req.reply_to.as_deref(),
            sandbox: req.sandbox,
            to: &req.to,
        },
    )
}

fn create_comment_now(
    ctx: &RequestContext<'_>,
    fields: &CommentFields<'_>,
) -> Result<CommentCreated, RemarginError> {
    let mut attachments: Vec<PathBuf> = Vec::with_capacity(fields.attachments.len());
    for raw in fields.attachments {
        attachments.push(ctx.expand(raw)?);
    }
    let position = insert_position(
        fields.reply_to,
        fields.after_comment,
        fields.after_heading,
        fields.after_line,
    );
    enforce_ack_skip_reasons(
        ctx.system,
        ctx.base_dir,
        &ctx.config,
        &fields.file.to_string_lossy(),
        &[(fields.reply_to, fields.auto_ack, fields.ack_skip_reason)],
    )?;
    let create_params = CreateCommentParams {
        attachments: &attachments,
        auto_ack: fields.auto_ack,
        content: fields.content,
        position: &position,
        remargin_kind: fields.kind,
        reply_to: fields.reply_to,
        sandbox: fields.sandbox,
        to: fields.to,
    };
    let path = ctx.base_dir.join(fields.file);
    let new_id = operations::create_comment(ctx.system, &path, &ctx.config, &create_params)?;
    Ok(responses::comment_created(
        &new_id,
        comment_style::notes(fields.content),
    ))
}

fn comments_filter(req: &CommentsArgs, identity: Option<String>) -> anyhow::Result<QueryFilter> {
    QueryFilter {
        pending: req.pending,
        pending_for: req.pending_for.clone(),
        remargin_kind: req.kind.clone(),
        ..QueryFilter::default()
    }
    .with_caller_identity(req.pending_for_me, false, identity)
}

fn comments_now(
    ctx: &RequestContext<'_>,
    req: &CommentsArgs,
    spill_cap: usize,
) -> Result<CommentsPage, RemarginError> {
    let admitted = ctx.admit("comments", &[req.file.as_str()])?;
    let filter = comments_filter(req, ctx.config.identity.clone())?;

    let path = ctx.base_dir.join(&admitted[0]);
    ctx.config.ensure_can_read(ctx.system, &path)?;
    let doc = parser::parse_file(ctx.system, &path)?;
    let matching = query::comments_matching(&doc, &path, &filter);

    let total = matching.len();
    let rows: Vec<CommentRow> = page_window(
        matching,
        usize_of(req.offset).unwrap_or(0),
        usize_of(req.limit),
    )
    .iter()
    .map(|comment| comment_row(comment, req.include_integrity))
    .collect();
    let (page, effective_limit) = size_page(rows, spill_cap);
    Ok(CommentsPage {
        comment_cols: column_names(query::comment_cols(req.include_integrity)),
        comments: page,
        effective_limit,
        elapsed_ms: 0,
        total,
    })
}

fn cp_now(ctx: &RequestContext<'_>, req: &CpArgs) -> Result<CpOutcome, RemarginError> {
    let admitted = ctx.admit("cp", &[req.dst.as_str(), req.src.as_str()])?;
    let args = cp_op::CpArgs::new(admitted[1].clone(), admitted[0].clone()).with_force(req.force);
    Ok(cp_op::cp(ctx.system, ctx.base_dir, &ctx.config, &args)?)
}

fn delete_now(
    ctx: &RequestContext<'_>,
    req: &DeleteArgs,
) -> Result<CommentsDeleted, RemarginError> {
    let admitted = ctx.admit("delete", &[req.file.as_str()])?;
    let id_refs: Vec<&str> = req.ids.iter().map(String::as_str).collect();
    let path = ctx.base_dir.join(&admitted[0]);
    operations::delete_comments(ctx.system, &path, &ctx.config, &id_refs)?;
    Ok(responses::comments_deleted(&req.ids))
}

fn doctor_now(ctx: &RequestContext<'_>, req: &DoctorArgs) -> Result<DoctorReport, RemarginError> {
    ctx.admit("doctor", &[])?;
    let user_settings_file = req.user_settings_file.as_deref().map_or_else(
        || {
            expand_path(ctx.system, "~/.claude/settings.json")
                .unwrap_or_else(|_| PathBuf::from("~/.claude/settings.json"))
        },
        PathBuf::from,
    );
    let checks = match req.check.as_deref() {
        Some(csv) => permissions_doctor::CheckName::parse_set(csv)?,
        None => permissions_doctor::CheckName::all(),
    };
    Ok(permissions_doctor::run_doctor(
        ctx.system,
        ctx.base_dir,
        &user_settings_file,
        &checks,
    )?)
}

fn edit_now(ctx: &RequestContext<'_>, req: &EditArgs) -> Result<CommentEdited, RemarginError> {
    let admitted = ctx.admit("edit", &[req.file.as_str()])?;
    let path = ctx.base_dir.join(&admitted[0]);
    operations::edit_comment(
        ctx.system,
        &path,
        &ctx.config,
        &req.id,
        &req.content,
        req.kind.as_deref(),
    )?;
    Ok(responses::comment_edited(
        &req.id,
        comment_style::notes(&req.content),
    ))
}

fn get_now(
    ctx: &RequestContext<'_>,
    req: &GetArgs,
    spill_cap: usize,
) -> Result<Vec<McpContent>, RemarginError> {
    let admitted = ctx.admit("get", &[req.path.as_str()])?;
    let end_line = usize_of(req.end_line);
    let start_line = usize_of(req.start_line);
    let target = admitted[0].as_path();

    if req.binary {
        if start_line.is_some() || end_line.is_some() {
            refuse!("start_line / end_line are not supported with binary: true");
        }
        if req.line_numbers {
            refuse!("line_numbers is not supported with binary: true");
        }
        let payload = document::read_binary(ctx.system, ctx.base_dir, target, &ctx.config)?;
        let metadata = serde_json::json!({
            "binary": true,
            "mime": payload.mime,
            "path": payload.path,
            "size_bytes": payload.size_bytes,
        });
        return Ok(vec![
            McpContent::Resource {
                uri: format!("file://{}", payload.path.display()),
                mime_type: Some(String::from(payload.mime)),
                contents: McpResourceContents::Blob(payload.bytes),
            },
            McpContent::Text {
                text: pretty(&metadata)?,
            },
        ]);
    }

    let lines = document::resolve_line_window(start_line, end_line);
    let (result, effective_end_line) = read_get_window(
        ctx.system,
        ctx.base_dir,
        target,
        lines,
        spill_cap,
        &ctx.config,
    )?;
    let answer = get_envelope(
        result,
        lines.map_or(1, |(start, _)| start),
        req.line_numbers,
        effective_end_line,
    );
    Ok(vec![McpContent::Text {
        text: serde_json::to_string(&answer).context("serializing the tool answer")?,
    }])
}

fn pretty(value: &serde_json::Value) -> anyhow::Result<String> {
    serde_json::to_string_pretty(value).context("serializing the tool answer")
}

fn get_image_now(
    ctx: &RequestContext<'_>,
    req: &GetImageArgs,
) -> Result<Vec<McpContent>, RemarginError> {
    let admitted = ctx.admit("get_image", &[req.path.as_str()])?;
    let format = req.format.as_ref().map(|format| match format {
        ImageFormat::Jpeg => "jpeg",
        ImageFormat::Jpg => "jpg",
        ImageFormat::Png => "png",
    });
    let max_dimension = req
        .max_dimension
        .map(|value| u32::try_from(value).unwrap_or(u32::MAX));
    let options = image_ops::GetImageOptions::from_optionals(
        req.crop.as_deref(),
        format,
        req.max_bytes,
        max_dimension,
    )?;
    let result = image_ops::get_image(
        ctx.system,
        ctx.base_dir,
        &admitted[0],
        &ctx.config,
        &options,
    )?;
    let metadata = result.to_json_without_content();
    Ok(vec![
        McpContent::Image {
            data: result.bytes,
            mime_type: String::from(result.mime),
        },
        McpContent::Text {
            text: pretty(&metadata)?,
        },
    ])
}

fn identity_create_now(
    ctx: &RequestContext<'_>,
    req: &IdentityCreateArgs,
) -> Result<IdentityCreated, RemarginError> {
    ctx.admit("identity_create", &[])?;
    let key = req
        .key
        .as_deref()
        .map(|raw| ctx.expand(raw))
        .transpose()?
        .map(|path| path.to_string_lossy().into_owned());
    let author_type = match req.r#type {
        IdentityKind::Agent => "agent",
        IdentityKind::Human => "human",
    };
    let mut yaml = format!("identity: {}\ntype: {author_type}\n", req.identity);
    if let Some(rendered) = &key {
        use core::fmt::Write as _;
        let _ = writeln!(yaml, "key: {rendered}");
    }
    Ok(IdentityCreated {
        author_type: String::from(author_type),
        elapsed_ms: 0,
        identity: req.identity.clone(),
        key,
        yaml,
    })
}

fn lint_now(ctx: &RequestContext<'_>, req: &LintArgs) -> Result<LintAnswer, RemarginError> {
    let admitted = ctx.admit("lint", &[req.file.as_str()])?;
    let path = ctx.base_dir.join(&admitted[0]);
    Ok(linter::lint_doc(ctx.system, &path, &ctx.config)?.answer())
}

fn ls_now(ctx: &RequestContext<'_>, req: &LsArgs) -> Result<LsAnswer, RemarginError> {
    let admitted = ctx.admit("ls", &[req.path.as_deref().unwrap_or(".")])?;
    let entries = document::ls(ctx.system, ctx.base_dir, &admitted[0], &ctx.config)?;
    Ok(LsAnswer {
        elapsed_ms: 0,
        entries,
    })
}

fn metadata_now(
    ctx: &RequestContext<'_>,
    req: &MetadataArgs,
) -> Result<Vec<McpContent>, RemarginError> {
    let admitted = ctx.admit("metadata", &[req.path.as_str()])?;
    let meta = document::metadata(ctx.system, ctx.base_dir, &admitted[0], &ctx.config)?;
    Ok(vec![McpContent::Text {
        text: pretty(&meta.to_json(true))?,
    }])
}

fn mv_now(ctx: &RequestContext<'_>, req: &MvArgs) -> Result<MvAnswer, RemarginError> {
    let admitted = ctx.admit("mv", &[req.dst.as_str(), req.src.as_str()])?;
    let args = mv_op::MvArgs::new(admitted[1].clone(), admitted[0].clone()).with_force(req.force);
    let outcome = mv_op::mv(ctx.system, ctx.base_dir, &ctx.config, &args)?;
    Ok(outcome.answer())
}

fn permissions_check_now(
    ctx: &RequestContext<'_>,
    req: &PermissionsCheckArgs,
) -> Result<CheckOutput, RemarginError> {
    ctx.admit("permissions_check", &[])?;
    let candidate = ctx.expand(&req.path)?;
    let absolute = if candidate.is_absolute() {
        candidate
    } else {
        ctx.base_dir.join(candidate)
    };
    Ok(permissions_inspect::check(
        ctx.system,
        ctx.base_dir,
        &absolute,
        req.why,
    )?)
}

fn permissions_show_now(ctx: &RequestContext<'_>) -> Result<ShowOutput, RemarginError> {
    ctx.admit("permissions_show", &[])?;
    Ok(permissions_inspect::show(ctx.system, ctx.base_dir)?)
}

fn plan_comment_request<'plan>(
    ctx: &RequestContext<'_>,
    op: &str,
    req: &'plan PlanArgs,
    position: &'plan InsertPosition,
    attach_refs: &'plan [&'plan str],
    file: &Path,
) -> anyhow::Result<plan_ops::PlanRequest<'plan>> {
    let content = required(req.content.as_deref(), "content")?;
    let parent = req.parent_id.as_deref().or(req.reply_to.as_deref());
    if op == "reply" && parent.is_none() {
        bail!("plan reply: `parent_id` is required");
    }
    enforce_ack_skip_reasons(
        ctx.system,
        ctx.base_dir,
        &ctx.config,
        &file.to_string_lossy(),
        &[(parent, req.auto_ack, req.ack_skip_reason.as_deref())],
    )?;
    let project_params = projections::ProjectCommentParams::new(content, position)
        .with_attachment_filenames(attach_refs)
        .with_auto_ack(req.auto_ack)
        .with_reply_to(parent)
        .with_sandbox(req.sandbox.unwrap_or(false))
        .with_to(req.to.as_deref().unwrap_or(&[]));
    Ok(plan_ops::PlanRequest::Comment {
        path: ctx.base_dir.join(file),
        params: project_params,
    })
}

fn plan_batch_ops(
    ctx: &RequestContext<'_>,
    ops: Option<&[PlanOpArgs]>,
    file: &Path,
) -> anyhow::Result<Vec<projections::ProjectBatchOp>> {
    let args = ops.context("plan batch: `ops` array is required")?;
    let mut parsed: Vec<projections::ProjectBatchOp> = Vec::with_capacity(args.len());
    for (idx, op) in args.iter().enumerate() {
        let value = serde_json::to_value(op).context("reading a plan batch operation")?;
        let object = value
            .as_object()
            .with_context(|| format!("plan batch op[{idx}]: expected object"))?;
        parsed.push(projections::ProjectBatchOp::from_json_object(object, idx)?);
    }
    let ack_gate: Vec<_> = parsed
        .iter()
        .zip(args)
        .map(|(op, source)| {
            (
                op.reply_to.as_deref(),
                op.auto_ack,
                source.ack_skip_reason.as_deref(),
            )
        })
        .collect();
    enforce_ack_skip_reasons(
        ctx.system,
        ctx.base_dir,
        &ctx.config,
        &file.to_string_lossy(),
        &ack_gate,
    )?;
    Ok(parsed)
}

fn plan_line_range(raw: &str) -> anyhow::Result<(usize, usize)> {
    let (start_str, end_str) = raw
        .split_once('-')
        .with_context(|| format!("lines expects START-END, got {raw:?}"))?;
    let start: usize = start_str
        .parse()
        .with_context(|| format!("lines: invalid start value {start_str:?}"))?;
    let end: usize = end_str
        .parse()
        .with_context(|| format!("lines: invalid end value {end_str:?}"))?;
    Ok((start, end))
}

fn plan_sign_selection(
    selected: Option<&[String]>,
    all_mine: bool,
    op: &str,
) -> anyhow::Result<operations::sign::SignSelection> {
    let ids = selected.unwrap_or(&[]);
    if all_mine && !ids.is_empty() {
        bail!("{op}: `ids` and `all_mine` are mutually exclusive");
    }
    if !all_mine && ids.is_empty() {
        bail!("{op}: pass `ids` (array of comment ids) or `all_mine: true`");
    }
    Ok(if all_mine {
        operations::sign::SignSelection::AllMine
    } else {
        operations::sign::SignSelection::Ids(ids.to_vec())
    })
}

fn plan_write_request<'plan>(
    req: &'plan PlanArgs,
    paths: &PlanPaths,
) -> anyhow::Result<plan_ops::PlanRequest<'plan>> {
    let line_range = req.lines.as_deref().map(plan_line_range).transpose()?;
    let opts = document::WriteOptions::new()
        .binary(req.binary.unwrap_or(false))
        .create(req.create.unwrap_or(false))
        .lines(line_range)
        .raw(req.raw.unwrap_or(false));
    Ok(plan_ops::PlanRequest::Write {
        path: paths.file()?,
        content: required(req.content.as_deref(), "content")?,
        opts,
    })
}

fn plan_request<'plan>(
    ctx: &RequestContext<'_>,
    req: &'plan PlanArgs,
    paths: &PlanPaths,
    position: &'plan InsertPosition,
    attach_refs: &'plan [&'plan str],
) -> anyhow::Result<plan_ops::PlanRequest<'plan>> {
    let op = req.op.as_str();
    let ids = req.ids.clone().unwrap_or_default();
    let remove = req.remove.unwrap_or(false);
    let force = req.force.unwrap_or(false);
    let joined = |relative: anyhow::Result<PathBuf>| relative.map(|path| ctx.base_dir.join(path));

    Ok(match op {
        "ack" => plan_ops::PlanRequest::Ack {
            path: joined(paths.file())?,
            ids,
            remove,
        },
        "comment" | "reply" => {
            plan_comment_request(ctx, op, req, position, attach_refs, &paths.file()?)?
        }
        "delete" => plan_ops::PlanRequest::Delete {
            path: joined(paths.file())?,
            ids,
        },
        "edit" => plan_ops::PlanRequest::Edit {
            path: joined(paths.file())?,
            id: required(req.id.as_deref(), "id")?,
            content: required(req.content.as_deref(), "content")?,
        },
        "react" => plan_ops::PlanRequest::React {
            path: joined(paths.file())?,
            id: required(req.id.as_deref(), "id")?,
            emoji: required(req.emoji.as_deref(), "emoji")?,
            remove,
        },
        "batch" => {
            let file = paths.file()?;
            plan_ops::PlanRequest::Batch {
                ops: plan_batch_ops(ctx, req.ops.as_deref(), &file)?,
                path: ctx.base_dir.join(file),
            }
        }
        "cp" => {
            let (from, to, forced) = paths.src_dst(force)?;
            plan_ops::PlanRequest::Cp {
                src: from,
                dst: to,
                force: forced,
            }
        }
        "mv" => {
            let (from, to, forced) = paths.src_dst(force)?;
            plan_ops::PlanRequest::Mv {
                src: from,
                dst: to,
                force: forced,
            }
        }
        "purge" => plan_ops::PlanRequest::Purge {
            path: joined(paths.file())?,
            recursive: req.recursive.unwrap_or(false),
        },
        "claude_restrict" | "claude_unrestrict" => {
            let cli = if op == "claude_restrict" {
                "remargin plan claude restrict"
            } else {
                "remargin plan claude unrestrict"
            };
            bail!("plan op '{op}' is not available via MCP - use the CLI: '{cli}'")
        }
        "sandbox-add" => plan_ops::PlanRequest::SandboxAdd {
            path: joined(paths.file())?,
        },
        "sandbox-remove" => plan_ops::PlanRequest::SandboxRemove {
            path: joined(paths.file())?,
        },
        "sign" => plan_ops::PlanRequest::Sign {
            path: joined(paths.file())?,
            selection: plan_sign_selection(
                req.ids.as_deref(),
                req.all_mine.unwrap_or(false),
                "plan sign",
            )?,
        },
        "write" => plan_write_request(req, paths)?,
        other => bail!("plan: unknown op {other:?}"),
    })
}

fn plan_now(ctx: &RequestContext<'_>, req: &PlanArgs) -> Result<PlanReport, RemarginError> {
    let given = present(&[req.dst.as_deref(), req.file.as_deref(), req.src.as_deref()]);
    let mut admitted = ctx.admit("plan", &given)?.into_iter();
    let paths = PlanPaths {
        dst: req.dst.as_ref().and_then(|_| admitted.next()),
        file: req.file.as_ref().and_then(|_| admitted.next()),
        src: req.src.as_ref().and_then(|_| admitted.next()),
    };
    let position = insert_position(
        req.parent_id.as_deref().or(req.reply_to.as_deref()),
        req.after_comment.as_deref(),
        req.after_heading.as_deref(),
        req.after_line,
    );
    let attach_refs: Vec<&str> = req
        .attach_names
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(String::as_str)
        .collect();
    let request = plan_request(ctx, req, &paths, &position, &attach_refs)?;
    Ok(plan_ops::dispatch(
        ctx.system,
        ctx.base_dir,
        &ctx.config,
        &request,
    )?)
}

fn folder_of(ctx: &RequestContext<'_>, folder: Option<&str>) -> PathBuf {
    let candidate = Path::new(folder.unwrap_or("."));
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        ctx.base_dir.join(candidate)
    }
}

fn prompt_delete_now(
    ctx: &RequestContext<'_>,
    req: &PromptDeleteArgs,
) -> Result<PromptDeleteOutcome, RemarginError> {
    ctx.admit("prompt_delete", &[])?;
    let folder = folder_of(ctx, req.folder.as_deref());
    Ok(prompt_ops::delete(ctx.system, &folder, &ctx.config)?)
}

fn prompt_list_now(
    ctx: &RequestContext<'_>,
    req: &PromptListArgs,
) -> Result<PromptListAnswer, RemarginError> {
    ctx.admit("prompt_list", &[])?;
    let folder = folder_of(ctx, req.folder.as_deref());
    let entries = prompt_ops::list(ctx.system, &folder)?;
    Ok(prompt_ops::PromptListAnswer {
        elapsed_ms: 0,
        entries,
    })
}

fn prompt_resolve_now(
    ctx: &RequestContext<'_>,
    req: &PromptResolveArgs,
) -> Result<ResolvedSystemPrompt, RemarginError> {
    ctx.admit("prompt_resolve", &[])?;
    let candidate = ctx.expand(&req.file)?;
    let absolute = if candidate.is_absolute() {
        candidate
    } else {
        ctx.base_dir.join(candidate)
    };
    Ok(resolve_system_prompt(ctx.system, &absolute)?)
}

fn prompt_set_now(
    ctx: &RequestContext<'_>,
    req: &PromptSetArgs,
) -> Result<PromptSetOutcome, RemarginError> {
    ctx.admit("prompt_set", &[])?;
    let folder = folder_of(ctx, req.folder.as_deref());
    Ok(prompt_ops::set(
        ctx.system,
        &folder,
        Some(&req.name),
        &req.prompt,
        req.runner.as_deref(),
        &ctx.config,
    )?)
}

fn purge_now(ctx: &RequestContext<'_>, req: &PurgeArgs) -> Result<PurgeAnswer, RemarginError> {
    let admitted = ctx.admit("purge", &[req.file.as_str()])?;
    let path = ctx.base_dir.join(&admitted[0]);

    if req.recursive.unwrap_or(false) {
        let result = purge::purge_dir(ctx.system, &path, &ctx.config)?;
        return Ok(PurgeAnswer::Directory(result.answer(ctx.base_dir)));
    }

    if ctx.system.is_dir(&path).wait().unwrap_or(false) {
        refuse!(
            "target is a directory: {} (pass `recursive: true` to purge every .md file under it)",
            admitted[0].display()
        );
    }

    let result = purge::purge(ctx.system, &path, &ctx.config)?;
    Ok(PurgeAnswer::File(result.answer()))
}

fn query_filter(req: &QueryArgs, identity: Option<String>) -> anyhow::Result<QueryFilter> {
    let since = req
        .since
        .as_deref()
        .map(|raw| {
            chrono::DateTime::parse_from_rfc3339(raw)
                .with_context(|| format!("invalid timestamp: {raw}"))
        })
        .transpose()?;
    let mut filter = QueryFilter {
        author: req.author.clone(),
        comment_id: req.comment_id.clone(),
        expanded: req.expanded.unwrap_or(false),
        pending: req.pending.unwrap_or(false),
        pending_for: req.pending_for.clone(),
        remargin_kind: req.kind.clone(),
        since,
        summary: req.summary.unwrap_or(false),
        ..QueryFilter::default()
    };
    filter = filter.with_caller_identity(
        req.pending_for_me.unwrap_or(false),
        req.pending_broadcast.unwrap_or(false),
        identity,
    )?;
    if let Some(pattern) = req.content_regex.as_deref() {
        filter = filter.with_content_regex(pattern, req.ignore_case.unwrap_or(false))?;
    }
    Ok(filter)
}

fn query_now(
    ctx: &RequestContext<'_>,
    req: &QueryArgs,
    spill_cap: usize,
) -> Result<QueryPage, RemarginError> {
    let admitted = ctx.admit("query", &[req.path.as_deref().unwrap_or(".")])?;
    let path_str = admitted[0].to_string_lossy();
    let include_integrity = req.include_integrity.unwrap_or(false);
    let filter = query_filter(req, ctx.config.identity.clone())?;
    let target = ctx.base_dir.join(&admitted[0]);
    let is_file = ctx.system.is_file(&target).wait().unwrap_or(false);
    let results = query::query(ctx.system, &target, &filter, &ctx.config)?;

    let files: Vec<QueryFile> = results
        .iter()
        .map(|result| query_file(result, include_integrity))
        .collect();
    let page = page_grouped(
        files,
        usize_of(req.offset).unwrap_or(0),
        usize_of(req.limit),
        spill_cap,
    );
    let (effective_limit, groups, total) = page.into_parts();
    Ok(QueryPage {
        base_path: query::display_base_path(&path_str, is_file),
        comment_cols: column_names(query::comment_cols(include_integrity)),
        effective_limit,
        elapsed_ms: 0,
        results: groups,
        total,
    })
}

fn react_now(ctx: &RequestContext<'_>, req: &ReactArgs) -> Result<ReactAnswer, RemarginError> {
    let admitted = ctx.admit("react", &[req.file.as_str()])?;
    let path = ctx.base_dir.join(&admitted[0]);
    operations::react(
        ctx.system,
        &path,
        &ctx.config,
        &req.id,
        &req.emoji,
        req.remove,
    )?;
    Ok(responses::react(&req.emoji, &req.id, req.remove))
}

fn replace_now(
    ctx: &RequestContext<'_>,
    req: &ReplaceArgs,
) -> Result<ReplaceReport, RemarginError> {
    let admitted = ctx.admit("replace", &[req.path.as_str()])?;
    let target = ctx.base_dir.join(&admitted[0]);
    let options = replace::ReplaceOptions::new(req.pattern.clone(), req.replacement.clone())
        .regex(req.regex)
        .ignore_case(req.ignore_case);
    Ok(replace::replace(
        ctx.system,
        ctx.base_dir,
        &target,
        &options,
        &ctx.config,
    )?)
}

fn reply_now(ctx: &RequestContext<'_>, req: &ReplyArgs) -> Result<CommentCreated, RemarginError> {
    let admitted = ctx.admit("reply", &[req.file.as_str()])?;
    create_comment_now(
        ctx,
        &CommentFields {
            ack_skip_reason: req.ack_skip_reason.as_deref(),
            after_comment: None,
            after_heading: None,
            after_line: None,
            attachments: &req.attachments,
            auto_ack: req.auto_ack,
            content: &req.content,
            file: &admitted[0],
            kind: &req.kind,
            reply_to: Some(&req.parent_id),
            sandbox: req.sandbox,
            to: &req.to,
        },
    )
}

fn report_spill_now(
    ctx: &RequestContext<'_>,
    req: &ReportSpillArgs,
    session: &Mutex<SessionState>,
) -> Result<ReportSpillAnswer, RemarginError> {
    ctx.admit("report_spill", &[])?;
    let explicit = usize_of(req.size);
    Ok(session
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .report_spill(explicit))
}

fn rm_now(ctx: &RequestContext<'_>, req: &RmArgs) -> Result<RmAnswer, RemarginError> {
    let admitted = ctx.admit("rm", &[req.path.as_str()])?;
    let result = document::rm(ctx.system, ctx.base_dir, &admitted[0], &ctx.config)?;
    Ok(result.answer(&admitted[0].to_string_lossy()))
}

fn sandbox_add_now(
    ctx: &RequestContext<'_>,
    req: &SandboxAddArgs,
) -> Result<SandboxAddReport, RemarginError> {
    let given: Vec<&str> = req.files.iter().map(String::as_str).collect();
    let admitted = ctx.admit("sandbox_add", &given)?;
    let files: Vec<PathBuf> = admitted
        .iter()
        .map(|file| ctx.base_dir.join(file))
        .collect();
    let identity = ctx
        .config
        .identity
        .as_deref()
        .context("identity is required for sandbox_add")?;
    let result = sandbox_ops::add_to_files(ctx.system, &files, identity, &ctx.config)?;
    Ok(result.add_report(ctx.base_dir))
}

fn sandbox_list_now(
    ctx: &RequestContext<'_>,
    req: &SandboxListArgs,
) -> Result<SandboxListAnswer, RemarginError> {
    let admitted = ctx.admit("sandbox_list", &[req.path.as_deref().unwrap_or(".")])?;
    let root = ctx.base_dir.join(&admitted[0]);
    let identity = ctx
        .config
        .identity
        .as_deref()
        .context("identity is required for sandbox_list")?;
    let listings = sandbox_ops::list_for_identity(ctx.system, &root, identity, &ctx.config)?;
    let files: Vec<sandbox_ops::SandboxListEntry> = listings
        .iter()
        .map(|listing| sandbox_ops::SandboxListEntry::from_listing(listing, &root, false))
        .collect();
    Ok(SandboxListAnswer {
        elapsed_ms: 0,
        files,
    })
}

fn sandbox_remove_now(
    ctx: &RequestContext<'_>,
    req: &SandboxRemoveArgs,
) -> Result<SandboxRemoveReport, RemarginError> {
    let given: Vec<&str> = req.files.iter().map(String::as_str).collect();
    let admitted = ctx.admit("sandbox_remove", &given)?;
    let files: Vec<PathBuf> = admitted
        .iter()
        .map(|file| ctx.base_dir.join(file))
        .collect();
    let identity = ctx
        .config
        .identity
        .as_deref()
        .context("identity is required for sandbox_remove")?;
    let result = sandbox_ops::remove_from_files(ctx.system, &files, identity, &ctx.config)?;
    Ok(result.remove_report(ctx.base_dir))
}

fn search_now(
    ctx: &RequestContext<'_>,
    req: &SearchArgs,
    spill_cap: usize,
) -> Result<SearchPage, RemarginError> {
    let admitted = ctx.admit("search", &[req.path.as_deref().unwrap_or(".")])?;
    let target = ctx.base_dir.join(&admitted[0]);
    let scope = match req.scope.as_deref().unwrap_or("all") {
        "body" => search::SearchScope::Body,
        "comments" => search::SearchScope::Comments,
        _ => search::SearchScope::All,
    };
    let options = search::SearchOptions::new(req.pattern.clone())
        .context_lines(usize_of(req.context).unwrap_or(0))
        .ignore_case(req.ignore_case)
        .limit(usize_of(req.limit))
        .offset(usize_of(req.offset).unwrap_or(0))
        .regex(req.regex)
        .scope(scope);

    let results = search::search(ctx.system, ctx.base_dir, &target, &options, &ctx.config)?;
    Ok(search_compact_envelope(
        &results,
        spill_cap,
        options.context_lines > 0,
    ))
}

fn sign_now(ctx: &RequestContext<'_>, req: &SignArgs) -> Result<SignResult, RemarginError> {
    let admitted = ctx.admit("sign", &[req.file.as_str()])?;
    let selection = plan_sign_selection(req.ids.as_deref(), req.all_mine, "sign")?;
    let path = ctx.base_dir.join(&admitted[0]);
    let options = operations::sign::SignOptions {
        repair_checksum: req.repair_checksum,
    };
    Ok(operations::sign::sign_comments(
        ctx.system,
        &path,
        &ctx.config,
        &selection,
        options,
    )?)
}

fn verify_now(ctx: &RequestContext<'_>, req: &VerifyArgs) -> Result<VerifyAnswer, RemarginError> {
    let given = present(&[req.file.as_deref(), Some(req.path.as_str())]);
    let admitted = ctx.admit("verify", &given)?;
    let target = ctx.base_dir.join(&admitted[admitted.len() - 1]);

    if ctx.system.is_dir(&target).wait().unwrap_or(false) {
        let report =
            operations::verify::verify_path(ctx.system, ctx.base_dir, &target, &ctx.config)?;
        return Ok(operations::verify::VerifyAnswer::Folder(report.answer()));
    }
    let report = operations::verify::verify_and_refresh(ctx.system, &target, &ctx.config)?;
    Ok(operations::verify::VerifyAnswer::File(report.answer()))
}

fn whoami_now(ctx: &RequestContext<'_>) -> Result<IdentityReport, RemarginError> {
    ctx.admit("whoami", &[])?;
    Ok(resolve_identity_report(
        ctx.system,
        ctx.base_dir,
        &IdentityFlags::default(),
    )?)
}

fn write_now(ctx: &RequestContext<'_>, req: &WriteArgs) -> Result<WriteAnswer, RemarginError> {
    let admitted = ctx.admit("write", &[req.path.as_str()])?;
    let lines = match (usize_of(req.start_line), usize_of(req.end_line)) {
        (Some(start), Some(end)) => Some((start, end)),
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => {
            refuse!("partial write requires both start_line and end_line")
        }
    };
    let opts = document::WriteOptions::new()
        .binary(req.binary)
        .create(req.create)
        .lines(lines)
        .raw(req.raw);
    let outcome = document::write(
        ctx.system,
        ctx.base_dir,
        &admitted[0],
        &req.content,
        &ctx.config,
        opts,
    )?;
    Ok(outcome.answer(&admitted[0].to_string_lossy(), req.binary, req.raw))
}
