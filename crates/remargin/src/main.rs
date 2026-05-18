//! Remargin CLI binary.

#[cfg(feature = "obsidian")]
mod obsidian;

use std::env;
use std::io::{self, Read as _, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::OnceLock;
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use clap::Parser;
use os_shim::System;
use os_shim::real::RealSystem;
use serde_json::{Value, json};

use remargin_core::activity;
use remargin_core::config::identity::{IdentityFlags, IdentityReport, resolve_identity_report};
use remargin_core::config::{self, ResolvedConfig};
use remargin_core::display;
use remargin_core::document;
use remargin_core::kind::matches_kind_filter;
use remargin_core::linter;
use remargin_core::mcp;
use remargin_core::operations;
use remargin_core::operations::batch::BatchCommentOp;
use remargin_core::operations::mv as mv_op;
use remargin_core::operations::plan as plan_ops;
use remargin_core::operations::projections;
use remargin_core::operations::purge;
use remargin_core::operations::query;
use remargin_core::operations::sandbox as sandbox_ops;
use remargin_core::operations::search;
use remargin_core::parser;
use remargin_core::path::expand_path;
use remargin_core::permissions::claude_sync::rule_shape::OverlapKind;
use remargin_core::permissions::inspect as permissions_inspect;
use remargin_core::permissions::pretool::{PretoolOutcome, pretool};
use remargin_core::permissions::restrict as permissions_restrict;
use remargin_core::permissions::unprotect as permissions_unprotect;
use remargin_core::responses;
use remargin_core::skill;
use remargin_core::writer::InsertPosition;

const EXIT_ERROR: u8 = 1;
const EXIT_LINT: u8 = 2;
const EXIT_INTEGRITY: u8 = 3;
const EXIT_ATTACHMENT: u8 = 4;
const EXIT_PRESERVATION: u8 = 5;
const EXIT_SKILL: u8 = 6;
const EXIT_NOT_FOUND: u8 = 7;
const EXIT_AMBIGUOUS: u8 = 8;
/// Claude Code's `PreToolUse` hook contract maps exit 2 to "block the
/// tool call and feed stderr back to the model". Use the same value
/// for fail-closed pretool outcomes so the hook signal is intact.
const EXIT_PRETOOL_FAIL: u8 = 2;
/// Marker prefix in the error message so the top-level error mapper
/// can route pretool failures to exit code 2 (Claude Code's blocking
/// signal) without mistaking them for general CLI errors.
const PRETOOL_FAIL_SENTINEL: &str = "__remargin_pretool_fail__:";
/// Gitignore-style "no match" sentinel returned by
/// `permissions check` when the path is unrestricted.
/// Numerically equal to [`EXIT_ERROR`] so existing tooling that branches
/// on `1 vs 0` still works; the `main` harness recognises the sentinel
/// to skip the "error: ..." render that would otherwise prepend the
/// gitignore-style result.
const EXIT_NOT_RESTRICTED: u8 = 1;
/// Internal marker substring used by [`cmd_permissions`] to communicate
/// "not restricted" to [`classify_error`] without leaking through
/// stderr.
const PERMISSIONS_NOT_RESTRICTED_MARKER: &str = "__remargin_permissions_check_not_restricted__";

/// Default user-scope settings file used by `remargin claude restrict`.
/// Resolved through [`expand_path`] so `$HOME` follows the active
/// [`System`] (the `obsidian` feature already exercises this pattern;
/// we follow the same approach so tests stay hermetic via the
/// `--user-settings` flag).
const DEFAULT_USER_SETTINGS: &str = "~/.claude/settings.json";

static START_TIME: OnceLock<Instant> = OnceLock::new();

#[derive(Parser)]
#[command(
    name = "remargin",
    version,
    about = "Enhanced inline review protocol for markdown"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Per-subcommand identity group.
///
/// Flattened only into subcommands that resolve an author identity
/// (comment, edit, ack, react, sign, write, delete, batch, purge,
/// plan, verify, sandbox, mcp). Read-only / utility
/// subcommands do not flatten this group so clap rejects any attempt
/// to pass `--config` / `--identity` / `--type` / `--key` to them.
#[derive(clap::Args, Default)]
struct IdentityArgs {
    /// Path to the config file. Declares a complete identity on its
    /// own — conflicts with --identity, --type, and --key so a caller
    /// cannot mix "config file" and "manual declaration" halves.
    #[arg(long, conflicts_with_all = ["identity", "type", "key"])]
    config: Option<PathBuf>,

    /// Identity (author name) for this operation.
    #[arg(long)]
    identity: Option<String>,

    /// Path to signing key.
    #[arg(long)]
    key: Option<String>,

    /// Author type: human or agent.
    #[arg(long, value_name = "human|agent")]
    r#type: Option<String>,
}

/// Per-subcommand output group.
///
/// Controls how the subcommand renders its result. Flattened into
/// every subcommand that emits a payload. Unlike the old
/// `GlobalFlags`, these flags are scoped to the subcommand — this
/// matches the "per-concern, per-subcommand" structure the rest of
/// the refactor establishes. Invocations that previously placed
/// `--json` before the subcommand must now place it after.
#[derive(clap::Args, Default)]
struct OutputArgs {
    /// Output as JSON.
    #[arg(long)]
    json: bool,

    /// Enable verbose/tracing output.
    #[arg(long)]
    verbose: bool,
}

/// Per-subcommand `--assets-dir` flag.
///
/// Flattened ONLY into subcommands that write attachments: comment,
/// edit, batch. Everything else errors at parse time. Supplied as the
/// `assets_dir_flag` argument to
/// [`remargin_core::config::ResolvedConfig::resolve`] when set.
#[derive(clap::Args, Default)]
struct AssetsArgs {
    /// Path to assets directory.
    #[arg(long)]
    assets_dir: Option<String>,
}

/// Per-subcommand unrestricted escape hatch.
///
/// Compile-gated behind the `unrestricted` feature; flattened into the
/// ops that touch arbitrary filesystem paths (get, ls, metadata, rm,
/// write).
#[cfg(feature = "unrestricted")]
#[derive(clap::Args, Default)]
struct UnrestrictedArgs {
    /// Bypass path sandbox checks (requires compile-time feature).
    #[arg(long)]
    unrestricted: bool,
}

#[cfg(not(feature = "unrestricted"))]
#[derive(clap::Args, Default)]
struct UnrestrictedArgs;

#[cfg(not(feature = "unrestricted"))]
impl UnrestrictedArgs {
    #[expect(
        clippy::unused_self,
        reason = "sibling unrestricted-feature impl reads self.unrestricted; keep the signature uniform"
    )]
    const fn unrestricted(&self) -> bool {
        false
    }
}

#[cfg(feature = "unrestricted")]
impl UnrestrictedArgs {
    const fn unrestricted(&self) -> bool {
        self.unrestricted
    }
}

/// Available subcommands.
#[derive(clap::Subcommand)]
enum Commands {
    /// Acknowledge one or more comments.
    Ack {
        /// Path to the document (use - for stdin). Omit to resolve by ID across the folder tree.
        #[arg(long)]
        file: Option<String>,
        /// Comment IDs to acknowledge.
        #[arg(required = true)]
        ids: Vec<String>,
        /// Base directory to search when resolving by ID (default: .).
        #[arg(long, default_value = ".")]
        path: String,
        /// Remove this identity's ack instead of adding one.
        #[arg(long)]
        remove: bool,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Show "what's new since X" across managed `.md` files.
    ///
    /// Walks `<path>` (file or directory; defaults to cwd) and
    /// returns per-file change records (comments, acks,
    /// sandbox-adds) sorted by ts. When `--since` is omitted, the
    /// per-file cutoff is the caller's last action in that file —
    /// files where the caller has never acted return everything.
    ///
    /// Identity is read-only here (no signature); the quartet is
    /// used only to resolve the caller name that drives the
    /// cutoff.
    Activity {
        /// Path to scan. Defaults to the current directory.
        path: Option<PathBuf>,
        /// Cutoff timestamp (ISO 8601). Omit to derive per-file
        /// from the caller's last action.
        #[arg(long)]
        since: Option<String>,
        /// Render a human-readable timeline instead of JSON.
        #[arg(long)]
        pretty: bool,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Create multiple comments atomically (JSON ops via --ops).
    Batch {
        /// Path to the document.
        file: String,
        /// JSON array of operations.
        #[arg(long)]
        ops: String,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
        #[command(flatten)]
        assets_args: AssetsArgs,
    },
    /// Claude Code integration: manage which paths Claude is allowed to
    /// edit + project the deny rules into both Claude settings files.
    Claude {
        /// Subcommand: `restrict`, `unrestrict`.
        #[command(subcommand)]
        action: ClaudeAction,
    },
    /// Create a comment in a document.
    Comment {
        /// Path to the document (use - for stdin).
        file: String,
        /// Comment body text (mutually exclusive with --comment-file).
        #[arg(allow_hyphen_values = true)]
        content: Option<String>,
        /// Insert after this comment ID.
        #[arg(long, conflicts_with_all = ["after_heading", "after_line"])]
        after_comment: Option<String>,
        /// Insert after the ATX heading addressed by this `>`-separated
        /// path. Setext (underline) headings are NOT
        /// supported in v1.
        #[arg(long, conflicts_with_all = ["after_comment", "after_line"])]
        after_heading: Option<String>,
        /// Insert after this line number (1-indexed).
        #[arg(long, conflicts_with_all = ["after_comment", "after_heading"])]
        after_line: Option<usize>,
        /// Attachments to include.
        #[arg(long)]
        attach: Vec<PathBuf>,
        /// Automatically acknowledge the parent comment when replying.
        #[arg(long)]
        auto_ack: bool,
        /// Read comment body from a file (use - for stdin).
        #[arg(long, short = 'F', conflicts_with = "content")]
        comment_file: Option<PathBuf>,
        /// Classification tag for the new comment. Repeat to attach
        /// multiple (e.g. `--kind question --kind action-item`). Values
        /// must match `[A-Za-z0-9_ \-]{1,15}` — see `remargin_kind`
        /// validation in `remargin-core::kind`.
        #[arg(long = "kind")]
        remargin_kind: Vec<String>,
        /// ID of the comment to reply to.
        #[arg(long)]
        reply_to: Option<String>,
        /// Atomically stage the file in the caller's sandbox in the same write.
        #[arg(long)]
        sandbox: bool,
        /// Addressees of the comment.
        #[arg(long)]
        to: Vec<String>,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
        #[command(flatten)]
        assets_args: AssetsArgs,
    },
    /// List comments in a document.
    Comments {
        /// Path to the document (use - for stdin).
        file: String,
        /// Repeatable `remargin_kind` filter (OR semantics). Omit to
        /// return every comment regardless of tag.
        #[arg(long = "kind")]
        remargin_kind: Vec<String>,
        /// Pretty-print comments as a threaded tree.
        #[arg(long)]
        pretty: bool,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Delete one or more comments.
    Delete {
        /// Path to the document.
        file: String,
        /// Comment IDs to delete.
        #[arg(required = true)]
        ids: Vec<String>,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Edit a comment (cascading ack clear).
    Edit {
        /// Path to the document.
        file: String,
        /// Comment ID to edit.
        id: String,
        /// New comment body.
        content: String,
        /// Replacement classification tag list. Repeat to set multiple
        /// (e.g. `--kind question --kind action-item`). Omit every
        /// `--kind` to leave the stored tag list untouched. Pass
        /// `--kind ""` to clear — validation rejects empty strings so
        /// a single `--kind ''` errors; the right way to clear today
        /// is to run `remargin edit` without any `--kind` flags, then
        /// use the forthcoming tag editor to drop entries.
        #[arg(long = "kind")]
        remargin_kind: Vec<String>,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
        #[command(flatten)]
        assets_args: AssetsArgs,
    },
    /// Read a file's contents. Add `--binary` to fetch non-markdown files as
    /// bytes (base64 in `--json` mode, raw bytes to stdout otherwise, or
    /// written to `--out <path>`). Run `remargin metadata <path>` first to
    /// check `size_bytes` and `mime` before pulling large blobs.
    Get {
        /// Path to the file.
        path: String,
        /// Fetch as bytes. Rejects `.md` (use the text path for markdown).
        /// Mime is derived from the file extension.
        #[arg(long)]
        binary: bool,
        /// End line (1-indexed, inclusive). Text mode only.
        #[arg(long)]
        end: Option<usize>,
        /// Show line numbers in output. Text mode only.
        #[arg(short = 'n', long = "line-numbers")]
        line_numbers: bool,
        /// Write the fetched bytes to this path (binary mode only). Stdout
        /// receives a summary instead of the bytes. The caller names the
        /// target path — no auto-cleanup.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Start line (1-indexed). Text mode only.
        #[arg(long)]
        start: Option<usize>,
        #[command(flatten)]
        output_args: OutputArgs,
        #[command(flatten)]
        unrestricted_args: UnrestrictedArgs,
    },
    /// Resolve, print, or materialize an identity.
    ///
    /// With no subcommand (or `show`), resolves and prints the
    /// effective identity under the supplied [`IdentityArgs`] — the
    /// pre-existing diagnostic surface that tooling (Obsidian plugin,
    /// scripts) polls on startup.
    ///
    /// With `create`, prints a ready-to-use identity YAML block to
    /// stdout so users can redirect into `.remargin.yaml`:
    ///
    /// ```sh
    /// remargin identity create --identity alice --type human > .remargin.yaml
    /// ```
    ///
    /// Resolution for `show` routes through the same three-branch
    /// resolver every mutating subcommand uses: `--config` (branch 1),
    /// manual `--identity/--type/--key` (branch 2), or walk-up
    /// (branch 3).
    Identity {
        /// Subcommand. Omit to invoke `show` (backward-compatible
        /// with the pre-existing surface).
        #[command(subcommand)]
        action: Option<IdentityAction>,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Generate a new Ed25519 signing key pair.
    Keygen {
        /// Output path for the private key (public key gets .pub suffix).
        #[arg(default_value = "remargin_key")]
        output: PathBuf,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Run structural lint checks.
    Lint {
        /// Path to the document (use - for stdin).
        file: String,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// List files and directories.
    Ls {
        /// Directory path to list.
        #[arg(default_value = ".")]
        path: String,
        #[command(flatten)]
        output_args: OutputArgs,
        #[command(flatten)]
        unrestricted_args: UnrestrictedArgs,
    },
    /// MCP server management and execution.
    Mcp {
        /// Subcommand: run, install, uninstall, test.
        #[command(subcommand)]
        action: Option<McpAction>,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Get document metadata.
    Metadata {
        /// Path to the document.
        path: String,
        #[command(flatten)]
        output_args: OutputArgs,
        #[command(flatten)]
        unrestricted_args: UnrestrictedArgs,
    },
    /// Move or rename a single tracked file.
    ///
    /// Same-FS moves use an atomic filesystem rename. Cross-FS moves
    /// fall back to copy + remove (the source is removed only after
    /// the destination write returns Ok). Both endpoints flow through
    /// the same sandbox / forbidden-target / per-op-guard checks every
    /// other mutating op uses, so a `restrict` entry covering either
    /// side refuses the call.
    ///
    /// Idempotent: `remargin mv a a` is a no-op; re-running after a
    /// successful move (`src` missing, `dst` already in place) returns
    /// success with `bytes_moved = 0`.
    Mv {
        /// Source path.
        src: String,
        /// Destination path.
        dst: String,
        /// Overwrite an existing destination.
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
        #[command(flatten)]
        unrestricted_args: UnrestrictedArgs,
    },
    /// Install or uninstall the Obsidian plugin in a vault.
    #[cfg(feature = "obsidian")]
    Obsidian {
        #[command(subcommand)]
        action: ObsidianAction,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Inspect the resolved permissions for the current directory.
    ///
    /// Read-only surface over `permissions::inspect`. `show` prints the
    /// parent-walked `.remargin.yaml` permissions (with `trusted_roots`
    /// recursive expansion); `check <path>` answers gitignore-style
    /// "is this path restricted?" with exit-code semantics
    /// (0 = restricted, 1 = not).
    ///
    /// No identity flags — both subcommands are pure observers.
    Permissions {
        #[command(subcommand)]
        action: PermissionsAction,
    },
    /// Structured pre-commit prediction for a mutating op.
    ///
    /// Per-op subcommand routing wires this to the in-memory projection
    /// of each mutating op. This crate ships the shared shape +
    /// subcommand tree; individual op wiring lands in follow-ups.
    ///
    /// Identity is flattened on the parent so every projection inherits
    /// the same `--identity` / `--type` / `--config` / `--key`. Output
    /// flags, by contrast, belong on each sub-action so `remargin plan
    /// <op> … --json` parses cleanly.
    Plan {
        /// Which mutating op to plan.
        #[command(subcommand)]
        action: PlanAction,
        #[command(flatten)]
        identity_args: IdentityArgs,
    },
    /// Folder-scoped system-prompt resolver.
    ///
    /// Read-only walk-up that mirrors the identity resolver but anchors
    /// on the `system_prompt:` block of a `.remargin.yaml`. Identity
    /// flags are accepted for surface symmetry and never gate the
    /// resolution. Falls through to the locked Default body when the
    /// walk exhausts.
    Prompt {
        /// Subcommand: resolve.
        #[command(subcommand)]
        action: PromptAction,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Strip all comments from a document.
    ///
    /// With `--recursive`, treat `file` as a directory and purge every
    /// visible markdown file under it. Per-file `op_guard`
    /// checks fire individually so a single `deny_ops` or allow-list
    /// refusal does not abort the whole batch.
    Purge {
        /// Path to the document (or directory when `--recursive` is set).
        file: String,
        /// Recursively purge every `.md` file under the directory at `file`.
        #[arg(long)]
        recursive: bool,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Search across documents for comments.
    Query {
        /// Base directory to search.
        #[arg(default_value = ".")]
        path: String,
        /// Only documents with comments by this author.
        #[arg(long)]
        author: Option<String>,
        /// Only documents containing a comment with this structural ID.
        #[arg(long)]
        comment_id: Option<String>,
        /// Regex applied to comment content; composes with metadata filters.
        #[arg(long)]
        content_regex: Option<String>,
        /// Include individual matching comments in each result (default behavior).
        #[arg(long)]
        expanded: bool,
        /// Case-insensitive match for `--content-regex`.
        #[arg(long, short = 'i')]
        ignore_case: bool,
        /// Only documents with pending (unacked) comments. Matches
        /// both directed (unacked recipients) and broadcast (no acks
        /// at all) shapes.
        #[arg(long)]
        pending: bool,
        /// Only surface unacked broadcast (no-`to`) comments the
        /// current identity has not acknowledged. Resolves the
        /// identity the same way every other subcommand does.
        #[arg(long)]
        pending_broadcast: bool,
        /// Only pending for this recipient.
        #[arg(long)]
        pending_for: Option<String>,
        /// Sugar for `--pending-for <current-identity>`. Surfaces
        /// directed comments addressed to the caller that the caller
        /// has not acked yet.
        #[arg(long)]
        pending_for_me: bool,
        /// Pretty-print results grouped by file.
        #[arg(long)]
        pretty: bool,
        /// Repeatable `remargin_kind` filter (OR semantics). Matches any
        /// comment whose tag list contains at least one of the supplied
        /// values.
        #[arg(long = "kind")]
        remargin_kind: Vec<String>,
        /// Only activity after this ISO 8601 timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Return only counts/summary, suppress comment data.
        #[arg(long)]
        summary: bool,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Add or remove an emoji reaction.
    React {
        /// Path to the document.
        file: String,
        /// Comment ID.
        id: String,
        /// Emoji to add/remove.
        emoji: String,
        /// Remove instead of add.
        #[arg(long)]
        remove: bool,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Manage the registry file.
    Registry {
        /// Subcommand: show.
        #[command(subcommand)]
        action: RegistryAction,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Resolve the effective enforcement mode for a directory.
    ///
    /// Walks up from `--cwd` (or the current directory) looking for the
    /// nearest `.remargin.yaml` and returns its `mode:` field. Unlike
    /// `identity`, this does NOT filter by author type — mode is a
    /// directory-tree property. Falls back to `open` when no config is found.
    ///
    /// Prints a JSON object like `{"mode":"strict","source":"/path/to/.remargin.yaml"}`
    /// under `--json`; prints a short human-readable summary otherwise.
    ResolveMode {
        /// Starting directory for the walk-up. Defaults to the process's
        /// current directory.
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Remove a file from the managed document tree.
    Rm {
        /// Path to the file.
        file: String,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
        #[command(flatten)]
        unrestricted_args: UnrestrictedArgs,
    },
    /// Manage per-identity sandbox staging for markdown files.
    Sandbox {
        /// Subcommand: add, list, or remove.
        #[command(subcommand)]
        action: SandboxAction,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Search across documents for text matches.
    Search {
        /// Text or regex pattern to search for.
        pattern: String,
        /// Base directory to search.
        #[arg(long, default_value = ".")]
        path: String,
        /// Treat pattern as a regex.
        #[arg(long)]
        regex: bool,
        /// Search scope: all, body, or comments.
        #[arg(long, default_value = "all")]
        scope: String,
        /// Lines of context around matches.
        #[arg(long, short = 'C', default_value = "0")]
        context: usize,
        /// Case-insensitive matching.
        #[arg(long, short = 'i')]
        ignore_case: bool,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Back-sign missing-signature comments authored by the current
    /// identity.
    ///
    /// Adds an SSH signature to each selected comment. The canonical
    /// signed payload excludes ack / reactions / checksum, so signing
    /// never invalidates an existing comment — it only promotes an
    /// unsigned artifact into one that verifies under the
    /// participant-registry pubkey.
    ///
    /// The op refuses to sign comments authored by anyone other than
    /// the resolved identity (forgery guard). Already-signed comments
    /// listed under `--ids` are reported as skipped, not re-signed;
    /// `--all-mine` silently excludes them.
    Sign {
        /// Path to the document.
        file: String,
        /// Comment ids to sign. Mutually exclusive with `--all-mine`.
        #[arg(long, value_delimiter = ',', conflicts_with = "all_mine")]
        ids: Vec<String>,
        /// Sign every unsigned comment authored by the current
        /// identity. Mutually exclusive with `--ids`.
        #[arg(long)]
        all_mine: bool,
        /// Recompute each target comment's stored checksum from its
        /// current content before signing. The forgery guard still
        /// applies — you can only repair comments you authored.
        /// Without this flag a stale checksum fails the verify gate
        /// and the op refuses to sign.
        #[arg(long)]
        repair_checksum: bool,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Manage the Claude Code skill.
    Skill {
        /// Subcommand: install, uninstall, test.
        #[command(subcommand)]
        action: SkillAction,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Verify comment integrity (checksums and signatures) against the
    /// participant registry.
    ///
    /// No flags: the registry is the single source of truth for pubkeys.
    /// Per-comment resolution runs unconditionally and the aggregate
    /// pass/fail follows the mode-driven severity table (see
    /// `operations::verify`).
    Verify {
        /// Path to the document.
        file: String,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Print version information.
    Version,
    /// Write document contents (comment-preserving).
    Write {
        /// Path to the file.
        path: String,
        /// File content to write (read from stdin if omitted).
        content: Option<String>,
        /// Content is base64-encoded binary data (implies --raw).
        /// Not supported for markdown (.md) files.
        #[arg(long)]
        binary: bool,
        /// Create a new file (parent directory must exist, file must not).
        #[arg(long)]
        create: bool,
        /// Replace only lines `START-END` (1-indexed, inclusive) and leave
        /// every other line byte-identical. Comment blocks inside the
        /// range must be reincluded (by id) in the replacement; writes
        /// that would destroy a comment are rejected. Incompatible with
        /// --create, --raw, and --binary.
        #[arg(long, value_name = "START-END")]
        lines: Option<String>,
        /// Write content exactly as provided, skipping frontmatter and comment
        /// preservation. Not supported for markdown (.md) files.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
        #[command(flatten)]
        unrestricted_args: UnrestrictedArgs,
    },
}

/// `remargin claude` subcommands. Cohesion bucket for ops whose
/// effects are scoped entirely to Claude Code's permission surface
/// (`.claude/settings.local.json`, `~/.claude/settings.json`, and the
/// `.remargin-restrictions.json` sidecar).
#[derive(clap::Subcommand)]
enum ClaudeAction {
    /// Claude Code `PreToolUse` hook dispatcher.
    ///
    /// Reads a `PreToolUse` event JSON envelope from stdin, extracts
    /// the path(s) the tool is about to touch, and emits Claude
    /// Code's `PreToolUse` decision JSON on stdout. Silent allow for
    /// paths outside the realm's `trusted_roots`; deny with a per-tool
    /// contextual message for paths inside it (redirecting Claude at
    /// the right `mcp__remargin__*` op). Fail closed: any internal
    /// error exits 2 so Claude Code blocks the tool call.
    Pretool {
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Restrict an agent-edit subpath.
    ///
    /// Adds a `permissions.trusted_roots` entry to the nearest
    /// `.claude/`-bearing ancestor's `.remargin.yaml` and projects
    /// the equivalent rules into both Claude settings files
    /// (`<anchor>/.claude/settings.local.json` and
    /// `~/.claude/settings.json`). Idempotent.
    ///
    /// No identity flags — `restrict` is a sanctioned config write
    /// that the user is presumed to have authority over.
    Restrict {
        /// Subpath relative to the anchor, OR the literal `*` for
        /// realm-wide.
        path: String,
        /// Extra Bash commands to deny on the restricted path. The
        /// default deny list already covers every common
        /// file-modifying command surface (`rm`, `chmod`, editors,
        /// scriptable interpreters, archivers, shells, VCS, etc. —
        /// see `BASH_MUTATORS` in `claude_sync.rs`); this flag is for
        /// project-specific extras the defaults do not catch.
        /// Comma-separated or repeat the flag:
        /// `--also-deny-bash curl,wget` or
        /// `--also-deny-bash curl --also-deny-bash wget`.
        /// Both forms are equivalent.
        #[arg(long = "also-deny-bash", value_delimiter = ',')]
        also_deny_bash: Vec<String>,
        /// When set, allow `Bash(remargin *)` on the path so the CLI
        /// stays usable. The MCP / agent surfaces are still blocked.
        #[arg(long)]
        cli_allowed: bool,
        /// User-scope settings file. Defaults to
        /// `~/.claude/settings.json`. Pass an explicit path to keep
        /// hermetic test runs out of the user's real home.
        #[arg(long)]
        user_settings: Option<PathBuf>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Reverse a previous `claude restrict`.
    ///
    /// Removes the matching `permissions.trusted_roots` entry from the
    /// nearest `.claude/`-bearing ancestor's `.remargin.yaml` AND
    /// scrubs the sidecar-tracked rules from both Claude settings
    /// files. Idempotent. Surfaces manual-edit divergences as
    /// warnings (never errors).
    ///
    /// No identity flags — symmetric with `restrict`.
    Unrestrict {
        /// Subpath to unrestrict (matches the on-disk `path` field of
        /// the original restrict entry), OR the literal `*` for the
        /// realm-wide wildcard restrict.
        path: String,
        /// Exit non-zero when `<path>` is not currently restricted
        /// instead of the default warn-and-no-op. For scripts that
        /// want hard-fail-on-miss semantics.
        #[arg(long)]
        strict: bool,
        /// User-scope settings file. Defaults to
        /// `~/.claude/settings.json`. Symmetric with `restrict`'s
        /// flag so hermetic test runs can stay out of the user's
        /// real home.
        #[arg(long)]
        user_settings: Option<PathBuf>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
}

/// Registry subcommands.
/// Plan subcommands. One variant per mutating op; per-op
/// wiring is tracked /.
#[derive(clap::Subcommand)]
enum PlanAction {
    /// Project an `ack` op.
    Ack {
        /// Path to the document.
        path: String,
        /// Comment IDs to ack (or un-ack with `--remove`).
        #[arg(required = true)]
        ids: Vec<String>,
        /// Remove the current identity's ack from each comment.
        #[arg(long)]
        remove: bool,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project a `batch` op.
    ///
    /// Reads the sub-op list from a JSON file (same shape as the
    /// `batch` subcommand): an array of objects with `content` (required)
    /// plus optional `reply_to`, `after_comment`, `after_line`,
    /// `attach_names`, `auto_ack`, `to`.
    Batch {
        /// Path to the document.
        path: String,
        /// JSON file containing the `ops` array. Use `-` for stdin.
        ops_file: String,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project a `claude restrict` / `claude unrestrict` op.
    ///
    /// Mirrors `remargin claude <op>` arg-for-arg; routes through the
    /// canonical plan dispatcher. Surfaces the multi-file config diff
    /// (`.remargin.yaml`, project + user settings, sidecar) and any
    /// detectable conflicts. No flags are consumed or written.
    Claude {
        /// Subcommand: `restrict` or `unrestrict`.
        #[command(subcommand)]
        action: PlanClaudeAction,
    },
    /// Project a `comment` creation op.
    Comment {
        /// Path to the document.
        path: String,
        /// Comment body text (read from stdin if omitted).
        content: Option<String>,
        /// Insert after this comment ID.
        #[arg(long, conflicts_with_all = ["after_heading", "after_line"])]
        after_comment: Option<String>,
        /// Project insertion after the ATX heading addressed by this
        /// `>`-separated path. Setext (underline) headings
        /// are NOT supported in v1.
        #[arg(long, conflicts_with_all = ["after_comment", "after_line"])]
        after_heading: Option<String>,
        /// Insert after this line number (1-indexed).
        #[arg(long, conflicts_with_all = ["after_comment", "after_heading"])]
        after_line: Option<usize>,
        /// Attachment basenames to record on the projected comment.
        /// Bytes are *not* copied — `plan` stays side-effect-free. The
        /// caller is responsible for the corresponding files existing
        /// when the mutating `comment` op runs.
        #[arg(long = "attach-name")]
        attach_names: Vec<String>,
        /// Automatically acknowledge the parent comment when replying.
        #[arg(long)]
        auto_ack: bool,
        /// ID of the comment to reply to.
        #[arg(long)]
        reply_to: Option<String>,
        /// Atomically project a sandbox entry in the frontmatter.
        #[arg(long)]
        sandbox: bool,
        /// Addressees of the comment.
        #[arg(long)]
        to: Vec<String>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project a `delete` op.
    Delete {
        /// Path to the document.
        path: String,
        /// Comment IDs to delete.
        #[arg(required = true)]
        ids: Vec<String>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project an `edit` op.
    Edit {
        /// Path to the document.
        path: String,
        /// Comment ID to edit.
        id: String,
        /// New comment body.
        content: String,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project an `mv` op.
    ///
    /// Surfaces the canonical src/dst, whether the destination exists
    /// (and would therefore require `--force`), and whether the live
    /// op would settle as a no-op (same canonical path) or
    /// idempotently as a no-op (src missing, dst already in place).
    /// No bytes are moved, no markdown is rewritten — `mv` does not
    /// change document content.
    Mv {
        /// Source path.
        src: String,
        /// Destination path.
        dst: String,
        /// Project the `--force` semantics.
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project a `purge` op. Pass `--recursive` to project
    /// a directory-level purge.
    Purge {
        /// Path to the document (or directory when `--recursive` is set).
        path: String,
        /// Project a recursive purge over every visible `.md` file
        /// under the directory at `path`.
        #[arg(long)]
        recursive: bool,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project a `react` op.
    React {
        /// Path to the document.
        path: String,
        /// Comment ID to react to.
        id: String,
        /// Emoji to add (or remove with `--remove`).
        emoji: String,
        /// Remove the current identity's reaction with this emoji.
        #[arg(long)]
        remove: bool,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project a `sandbox add` op.
    SandboxAdd {
        /// Path to the document.
        path: String,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project a `sandbox remove` op.
    SandboxRemove {
        /// Path to the document.
        path: String,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project a `sign` op.
    Sign {
        /// Path to the document.
        path: String,
        /// Comment ids to sign. Mutually exclusive with `--all-mine`.
        #[arg(long, value_delimiter = ',', conflicts_with = "all_mine")]
        ids: Vec<String>,
        /// Sign every unsigned comment authored by the current
        /// identity. Mutually exclusive with `--ids`.
        #[arg(long)]
        all_mine: bool,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project a `write` op.
    Write {
        /// Path to the file.
        path: String,
        /// File content to write (read from stdin if omitted).
        content: Option<String>,
        /// Content is base64-encoded binary data (implies --raw). Not
        /// supported for markdown (.md) files and not representable as
        /// a structured plan — the report will carry a `reject_reason`.
        #[arg(long)]
        binary: bool,
        /// Create a new file (parent directory must exist, file must not).
        #[arg(long)]
        create: bool,
        /// Replace only lines `START-END` (1-indexed, inclusive) and
        /// leave every other line byte-identical. See `write --lines`
        /// for the full semantics.
        #[arg(long, value_name = "START-END")]
        lines: Option<String>,
        /// Write content exactly as provided, skipping frontmatter and
        /// comment preservation. Not supported for markdown (.md) files
        /// and not representable as a structured plan — the report will
        /// carry a `reject_reason`.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        output_args: OutputArgs,
    },
}

/// `remargin plan claude` subcommands. Mirror the live `ClaudeAction`
/// shape; route through the canonical plan dispatcher.
#[derive(clap::Subcommand)]
enum PlanClaudeAction {
    /// Project a `claude restrict` op.
    Restrict {
        /// Subpath relative to the anchor, OR the literal `*` for
        /// realm-wide. Same shape as `remargin claude restrict`.
        path: String,
        /// Extra Bash commands to deny on the restricted path,
        /// layered on top of the broad default deny list.
        /// Comma-separated or repeat the flag.
        #[arg(long = "also-deny-bash", value_delimiter = ',')]
        also_deny_bash: Vec<String>,
        /// When set, the projection allows `Bash(remargin *)` on the
        /// path so the CLI stays usable.
        #[arg(long)]
        cli_allowed: bool,
        /// User-scope settings file. Defaults to
        /// `~/.claude/settings.json`. Pin an explicit path for
        /// hermetic test runs.
        #[arg(long)]
        user_settings: Option<PathBuf>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project a `claude unrestrict` op.
    Unrestrict {
        /// Subpath relative to the anchor (matches the on-disk
        /// `path` field of the original restrict entry), OR the
        /// literal `*` for realm-wide. Same shape as `remargin
        /// claude unrestrict`.
        path: String,
        /// User-scope settings file. Defaults to
        /// `~/.claude/settings.json`. Accepted for surface symmetry
        /// but not consulted by the projection (the sidecar's
        /// `added_to_files` list pins the actual targets).
        #[arg(long)]
        user_settings: Option<PathBuf>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
}

#[derive(clap::Subcommand)]
enum RegistryAction {
    /// Show the current registry.
    Show,
}

/// `remargin permissions` subcommands.
#[derive(clap::Subcommand)]
enum PermissionsAction {
    /// Gitignore-style: exit 0 when `path` is restricted, 1 otherwise.
    Check {
        /// Path to test.
        path: PathBuf,
        /// Print the matching rule (kind, source file, rule text) when
        /// the path is restricted. Adds detail to both text and JSON
        /// output.
        #[arg(long)]
        why: bool,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Print the resolved permissions for the current directory.
    Show {
        #[command(flatten)]
        output_args: OutputArgs,
    },
}

/// `remargin identity` subcommands. Default action
/// (no subcommand) is `show` — the pre-existing diagnostic surface.
#[derive(clap::Subcommand)]
enum IdentityAction {
    /// Print a ready-to-use identity YAML block to stdout. Users
    /// redirect to `.remargin.yaml` themselves (no `--write` flag —
    /// bans writes to `.remargin.yaml`).
    ///
    /// `--identity` and `--type` are required; `--key` is optional
    /// (valid in non-strict modes — pairs with `remargin keygen`).
    /// `mode:` is never emitted because mode is a tree property
    /// resolved by walk-up, not an identity-level declaration.
    Create {
        /// Identity (author name) to record.
        #[arg(long)]
        identity: String,
        /// Author type (`human` or `agent`).
        #[arg(long, value_name = "human|agent")]
        r#type: String,
        /// Optional path to the signing key. Emitted verbatim into
        /// the YAML — no existence check (pairs with `remargin
        /// keygen`). Bare names like `mykey` are fine; `.remargin.yaml`
        /// resolves them against `~/.ssh/` at load time.
        #[arg(long)]
        key: Option<String>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Resolve and print the effective identity (pre-existing
    /// behavior). Kept as an explicit alternative to the bare
    /// `remargin identity` form.
    Show {
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
    },
}

/// `remargin prompt` subcommands. Room for `set` / `unset` / `list`
/// later — only `resolve` ships in v1 (the inline editor lives in the
/// Obsidian plugin).
#[derive(clap::Subcommand)]
enum PromptAction {
    /// Strip the `system_prompt:` block from `<folder>/.remargin.yaml`.
    /// Idempotent: a missing block (or missing file) succeeds. The
    /// `.remargin.yaml` file is preserved even if it ends up empty.
    Delete {
        /// Folder containing the `.remargin.yaml`. Defaults to CWD.
        #[arg(default_value = ".")]
        folder: String,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Recursively list every `.remargin.yaml` under the given folder
    /// that declares a `system_prompt:` block.
    List {
        /// Root folder. Defaults to CWD.
        #[arg(default_value = ".")]
        folder: String,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Resolve the nearest folder-scoped system prompt for a file or
    /// directory. Falls through to the locked Default body when the
    /// walk exhausts.
    Resolve {
        /// File or directory to resolve a prompt for. Directories are
        /// treated as the starting directory; files have their parent
        /// walked.
        file: String,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Create or replace the `system_prompt:` block in
    /// `<folder>/.remargin.yaml`. Body comes from `--prompt` when set,
    /// else stdin (when stdin is not a TTY).
    Set {
        /// Folder containing (or to contain) the `.remargin.yaml`.
        /// Defaults to the current working directory when omitted.
        #[arg(default_value = ".")]
        folder: String,
        /// Human-readable display label. Required.
        #[arg(long)]
        name: String,
        /// Prompt body. Required. Pass `-` (or omit) and pipe via
        /// stdin for multi-line content.
        #[arg(long)]
        prompt: Option<String>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
}

/// Sandbox subcommands.
#[derive(clap::Subcommand)]
enum SandboxAction {
    /// Stage one or more markdown files in the caller's sandbox.
    Add {
        /// Markdown files to stage.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// List every markdown file staged for the caller.
    List {
        /// Emit absolute paths instead of paths relative to `--path`.
        #[arg(long)]
        absolute: bool,
        /// Base directory to walk (defaults to current directory).
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Remove the caller's sandbox entry from one or more markdown files.
    Remove {
        /// Markdown files to unstage.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
}

/// Skill subcommands.
#[derive(clap::Subcommand)]
enum SkillAction {
    /// Install the Claude Code skill.
    Install {
        /// Install globally to ~/.claude/skills/remargin/.
        #[arg(long)]
        global: bool,
    },
    /// Check skill installation status.
    Test {
        /// Check global installation.
        #[arg(long)]
        global: bool,
    },
    /// Uninstall the skill.
    Uninstall {
        /// Uninstall from global location.
        #[arg(long)]
        global: bool,
    },
}

/// MCP subcommands.
#[derive(clap::Subcommand)]
enum McpAction {
    /// Register remargin as an MCP server in Claude Code.
    Install {
        /// Install at user scope (default is project scope).
        #[arg(long)]
        user: bool,
    },
    /// Start the MCP server (stdio transport). This is the default.
    Run,
    /// Check MCP registration status.
    Test,
    /// Remove remargin MCP server from Claude Code.
    Uninstall,
}

/// Obsidian plugin install/uninstall actions.
#[cfg(feature = "obsidian")]
#[derive(clap::Subcommand)]
enum ObsidianAction {
    /// Install or upgrade the plugin in the current vault.
    Install {
        /// Vault directory. Defaults to the current working directory.
        #[arg(long)]
        vault_path: Option<PathBuf>,
    },
    /// Remove the plugin from the current vault.
    Uninstall {
        /// Vault directory. Defaults to the current working directory.
        #[arg(long)]
        vault_path: Option<PathBuf>,
    },
}

struct CommentParams<'cmd> {
    after_comment: Option<&'cmd str>,
    after_heading: Option<&'cmd str>,
    after_line: Option<usize>,
    attachments: &'cmd [PathBuf],
    auto_ack: bool,
    content: &'cmd str,
    file: &'cmd str,
    json_mode: bool,
    remargin_kind: &'cmd [String],
    reply_to: Option<&'cmd str>,
    sandbox: bool,
    to: &'cmd [String],
}

struct GetParams<'cmd> {
    binary: bool,
    end: Option<usize>,
    json_mode: bool,
    line_numbers: bool,
    out: Option<&'cmd Path>,
    path: &'cmd str,
    start: Option<usize>,
}

struct EditParams<'cmd> {
    content: &'cmd str,
    file: &'cmd str,
    id: &'cmd str,
    json_mode: bool,
    remargin_kind: Option<&'cmd [String]>,
}

struct ActivityParams<'cmd> {
    explicit_path: Option<&'cmd Path>,
    identity_args: &'cmd IdentityArgs,
    json_mode: bool,
    pretty: bool,
    since: Option<&'cmd str>,
}

struct RestrictParams<'cmd> {
    also_deny_bash: &'cmd [String],
    cli_allowed: bool,
    json_mode: bool,
    path: &'cmd str,
    user_settings_explicit: Option<&'cmd Path>,
}

/// How `query` results are rendered. Mutually-exclusive successor to the
/// previous `json_mode` / `pretty` / `summary` bool triple.
enum QueryOutputMode {
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
struct QueryPendingFilters<'cmd> {
    /// `true` when `--pending` was passed: filter to comments without
    /// any ack.
    any: bool,
    /// `true` when `--pending-broadcast` was passed: include
    /// broadcast-pending comments.
    broadcast: bool,
    /// `true` when `--pending-for-me` was passed: include comments
    /// addressed to the resolved caller identity.
    for_me: bool,
    /// `Some(user)` when `--pending-for <user>` was passed: include
    /// comments whose `to:` list contains `user` and which are still
    /// pending.
    for_user: Option<&'cmd str>,
}

struct PromptSetParams<'params> {
    config: &'params ResolvedConfig,
    cwd: &'params Path,
    folder: &'params str,
    json_mode: bool,
    name: &'params str,
    prompt_flag: Option<&'params str>,
}

struct QueryParams<'cmd> {
    author: Option<&'cmd str>,
    comment_id: Option<&'cmd str>,
    content_regex: Option<&'cmd str>,
    expanded: bool,
    ignore_case: bool,
    output: QueryOutputMode,
    path: &'cmd str,
    pending: QueryPendingFilters<'cmd>,
    remargin_kind: &'cmd [String],
    since: Option<&'cmd str>,
}

struct SearchParams<'cmd> {
    context: usize,
    ignore_case: bool,
    json_mode: bool,
    path: &'cmd str,
    pattern: &'cmd str,
    regex: bool,
    scope: &'cmd str,
}

struct SignParams<'cmd> {
    all_mine: bool,
    file: &'cmd str,
    ids: &'cmd [String],
    json_mode: bool,
    repair_checksum: bool,
}

struct AckParams<'cmd> {
    file: Option<&'cmd str>,
    ids: &'cmd [String],
    json_mode: bool,
    remove: bool,
    search_path: &'cmd str,
}

struct ReactParams<'cmd> {
    emoji: &'cmd str,
    file: &'cmd str,
    id: &'cmd str,
    json_mode: bool,
    remove: bool,
}

struct WriteParams<'cmd> {
    content: Option<&'cmd str>,
    json_mode: bool,
    opts: document::WriteOptions,
    path: &'cmd str,
}

/// Bundled CLI inputs for the [`cmd_mv`] handler.
struct MvParams<'cmd> {
    dst: &'cmd str,
    force: bool,
    json_mode: bool,
    src: &'cmd str,
}

/// Bundle of writers for the CLI's stdout / stderr streams.
///
/// Allows the `cmd_*` functions and `run()` to be exercised in-process by
/// tests with captured `Vec<u8>` buffers instead of writing to the real
/// process streams.
#[non_exhaustive]
pub struct IoSinks<'sinks> {
    pub stderr: &'sinks mut dyn Write,
    pub stdout: &'sinks mut dyn Write,
}

impl<'sinks> IoSinks<'sinks> {
    pub fn new(stdout: &'sinks mut dyn Write, stderr: &'sinks mut dyn Write) -> Self {
        Self { stderr, stdout }
    }
}

fn out(sinks: &mut IoSinks<'_>, msg: &str) -> Result<()> {
    writeln!(sinks.stdout, "{msg}").context("writing to stdout")
}

fn out_raw(sinks: &mut IoSinks<'_>, msg: &str) -> Result<()> {
    write!(sinks.stdout, "{msg}").context("writing to stdout")
}

/// Decorates object payloads with an `elapsed_ms` field so every `--json`
/// response carries timing info.
fn out_json(sinks: &mut IoSinks<'_>, value: &Value) -> Result<()> {
    let decorated = inject_elapsed_ms(value);
    out(
        sinks,
        &serde_json::to_string_pretty(&decorated).unwrap_or_default(),
    )
}

fn elapsed_ms() -> u64 {
    START_TIME.get().map_or(0, |t| {
        u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
    })
}

/// Non-object values pass through unchanged so future non-object top-level
/// outputs are not silently corrupted.
fn inject_elapsed_ms(value: &Value) -> Value {
    if let Value::Object(map) = value {
        let mut new_map = map.clone();
        new_map.insert(String::from("elapsed_ms"), json!(elapsed_ms()));
        return Value::Object(new_map);
    }
    value.clone()
}

fn print_output(sinks: &mut IoSinks<'_>, json_mode: bool, value: &Value) -> Result<()> {
    if json_mode {
        out_json(sinks, value)
    } else {
        print_text_output(sinks, value)
    }
}

fn print_text_output(sinks: &mut IoSinks<'_>, value: &Value) -> Result<()> {
    match value {
        Value::String(s) => out(sinks, s),
        Value::Object(map) => {
            for (key, val) in map {
                if let Value::Array(arr) = val {
                    out(sinks, &format!("{key}:"))?;
                    for item in arr {
                        out(sinks, &format!("  {item}"))?;
                    }
                } else {
                    out(sinks, &format!("{key}: {val}"))?;
                }
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => {
            out(sinks, &value.to_string())
        }
    }
}

fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    io::stdin()
        .read_to_string(&mut buf)
        .context("reading from stdin")?;
    Ok(buf)
}

/// Exactly one of `content` or `comment_file` must be provided. When
/// `comment_file` is `"-"`, the body is read from stdin.
fn resolve_comment_content(
    system: &dyn System,
    cwd: &Path,
    content: Option<&String>,
    comment_file: Option<&PathBuf>,
) -> Result<String> {
    match (content, comment_file) {
        (Some(text), None) => Ok(text.clone()),
        (None, Some(path)) => {
            let path_str = path.to_string_lossy();
            if path_str == "-" {
                read_stdin().context("reading comment body from stdin")
            } else {
                system
                    .read_to_string(&cwd.join(path))
                    .with_context(|| format!("reading comment body from {path_str}"))
            }
        }
        (None, None) => {
            anyhow::bail!("comment body required: provide as argument or via --comment-file")
        }
        (Some(_), Some(_)) => {
            anyhow::bail!("cannot use both positional content and --comment-file")
        }
    }
}

fn resolve_doc_path(system: &dyn System, cwd: &Path, file: &str) -> Result<PathBuf> {
    if file == "-" {
        let input = read_stdin()?;
        let temp_root = system
            .env_var("TMPDIR")
            .unwrap_or_else(|_err| String::from("/tmp"));
        let temp_path = PathBuf::from(temp_root).join("remargin-stdin.md");
        system
            .write(&temp_path, input.as_bytes())
            .context("writing stdin to temp file")?;
        Ok(temp_path)
    } else {
        let expanded = expand_cli_path(system, file)?;
        Ok(cwd.join(expanded))
    }
}

/// Expand a string-typed CLI path argument through [`expand_path`] and
/// surface a clear error naming the offending input. Downstream callers
/// layer their own path semantics (joining against `cwd`, validating that
/// the file exists, etc.) on top of the expanded `PathBuf`.
fn expand_cli_path(system: &dyn System, raw: &str) -> Result<PathBuf> {
    expand_path(system, raw).with_context(|| format!("expanding path argument {raw:?}"))
}

/// Resolve a path argument for the `purge` subcommand. In single-file
/// mode this funnels through [`resolve_doc_path`] (which honours stdin
/// `-`); in `--recursive` mode the path is treated as a directory, so
/// stdin redirection makes no sense and we just expand `~` / `$VAR`
/// before joining onto `cwd`.
fn resolve_purge_path(
    system: &dyn System,
    cwd: &Path,
    raw: &str,
    recursive: bool,
) -> Result<PathBuf> {
    if recursive {
        let expanded = expand_cli_path(system, raw)?;
        Ok(if expanded.is_absolute() {
            expanded
        } else {
            cwd.join(expanded)
        })
    } else {
        resolve_doc_path(system, cwd, raw)
    }
}

/// Same as [`expand_cli_path`] but for a `&Path`. Used by flags that clap
/// already parsed as [`PathBuf`] — we round-trip through `to_string_lossy`
/// so `~`, `$VAR`, etc. in the original arg still get expanded.
fn expand_cli_pathbuf(system: &dyn System, raw: &Path) -> Result<PathBuf> {
    let raw_str = raw.to_string_lossy();
    expand_cli_path(system, raw_str.as_ref())
}

const fn author_type_str(at: &parser::AuthorType) -> &'static str {
    at.as_str()
}

fn truncate_content(content: &str, max_len: usize) -> String {
    let first_line = content.lines().next().unwrap_or("");
    if first_line.len() > max_len {
        format!("{}...", &first_line[..max_len])
    } else {
        String::from(first_line)
    }
}

/// Parse the `--lines START-END` argument used by `remargin write`.
///
/// Accepts `START-END` with 1-indexed inclusive bounds, both required.
/// Returns `(start, end)`; further validation (start <= end, start >= 1)
/// happens in `document::write` so CLI and MCP callers hit the same
/// diagnostics.
fn parse_line_range(raw: &str) -> Result<(usize, usize)> {
    let (start_str, end_str) = raw
        .split_once('-')
        .with_context(|| format!("--lines expects START-END, got {raw:?}"))?;
    let start: usize = start_str
        .parse()
        .with_context(|| format!("--lines: invalid start value {start_str:?}"))?;
    let end: usize = end_str
        .parse()
        .with_context(|| format!("--lines: invalid end value {end_str:?}"))?;
    Ok((start, end))
}

/// Pull the subcommand's [`OutputArgs`] reference for the top-level
/// harness (main + error rendering).
///
/// Returns `None` for subcommands that do not flatten [`OutputArgs`] —
/// currently only `Version`. Callers treat `None` as "no `--json`, no
/// `--verbose`" (the all-defaults case).
const fn subcommand_output(cmd: &Commands) -> Option<&OutputArgs> {
    match cmd {
        Commands::Ack { output_args, .. }
        | Commands::Activity { output_args, .. }
        | Commands::Batch { output_args, .. }
        | Commands::Comment { output_args, .. }
        | Commands::Comments { output_args, .. }
        | Commands::Delete { output_args, .. }
        | Commands::Edit { output_args, .. }
        | Commands::Get { output_args, .. }
        | Commands::Identity { output_args, .. }
        | Commands::Keygen { output_args, .. }
        | Commands::Lint { output_args, .. }
        | Commands::Ls { output_args, .. }
        | Commands::Mcp { output_args, .. }
        | Commands::Metadata { output_args, .. }
        | Commands::Mv { output_args, .. }
        | Commands::Prompt { output_args, .. }
        | Commands::Purge { output_args, .. }
        | Commands::Query { output_args, .. }
        | Commands::React { output_args, .. }
        | Commands::Registry { output_args, .. }
        | Commands::ResolveMode { output_args, .. }
        | Commands::Rm { output_args, .. }
        | Commands::Sandbox { output_args, .. }
        | Commands::Search { output_args, .. }
        | Commands::Sign { output_args, .. }
        | Commands::Skill { output_args, .. }
        | Commands::Verify { output_args, .. }
        | Commands::Write { output_args, .. } => Some(output_args),
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { output_args, .. } => Some(output_args),
        Commands::Claude { action } => Some(claude_action_output(action)),
        Commands::Permissions { action } => Some(permissions_action_output(action)),
        Commands::Plan { action, .. } => Some(plan_action_output(action)),
        Commands::Version => None,
    }
}

/// Pull the per-action [`OutputArgs`] from a [`ClaudeAction`] variant.
const fn claude_action_output(action: &ClaudeAction) -> &OutputArgs {
    match action {
        ClaudeAction::Pretool { output_args }
        | ClaudeAction::Restrict { output_args, .. }
        | ClaudeAction::Unrestrict { output_args, .. } => output_args,
    }
}

/// Pull the per-action [`OutputArgs`] from a [`PlanClaudeAction`] variant.
const fn plan_claude_action_output(action: &PlanClaudeAction) -> &OutputArgs {
    match action {
        PlanClaudeAction::Restrict { output_args, .. }
        | PlanClaudeAction::Unrestrict { output_args, .. } => output_args,
    }
}

/// Pull the per-action [`OutputArgs`] from a [`PermissionsAction`]
/// variant. Both `show` and `check` flatten an `OutputArgs`.
const fn permissions_action_output(action: &PermissionsAction) -> &OutputArgs {
    match action {
        PermissionsAction::Show { output_args } | PermissionsAction::Check { output_args, .. } => {
            output_args
        }
    }
}

/// Pull the per-action [`OutputArgs`] from a [`PlanAction`] variant.
/// Every plan sub-action flattens an `OutputArgs`.
const fn plan_action_output(action: &PlanAction) -> &OutputArgs {
    match action {
        PlanAction::Ack { output_args, .. }
        | PlanAction::Batch { output_args, .. }
        | PlanAction::Comment { output_args, .. }
        | PlanAction::Delete { output_args, .. }
        | PlanAction::Edit { output_args, .. }
        | PlanAction::Mv { output_args, .. }
        | PlanAction::Purge { output_args, .. }
        | PlanAction::React { output_args, .. }
        | PlanAction::SandboxAdd { output_args, .. }
        | PlanAction::SandboxRemove { output_args, .. }
        | PlanAction::Sign { output_args, .. }
        | PlanAction::Write { output_args, .. } => output_args,
        PlanAction::Claude { action: claude } => plan_claude_action_output(claude),
    }
}

fn main() -> ExitCode {
    // Capture the start time before parsing so `elapsed_ms` includes clap's
    // argument-parsing overhead.
    let _: Result<_, _> = START_TIME.set(Instant::now());

    let cli = Cli::parse();

    let output = subcommand_output(&cli.command);
    let verbose = output.is_some_and(|o| o.verbose);

    let system = RealSystem::new();
    if verbose {
        let env_filter_directives = system.env_var("RUST_LOG").unwrap_or_default();
        let base_filter = tracing_subscriber::EnvFilter::try_new(&env_filter_directives)
            .unwrap_or_else(|_err| tracing_subscriber::EnvFilter::new(""));
        tracing_subscriber::fmt()
            .with_env_filter(base_filter.add_directive(tracing::Level::DEBUG.into()))
            .with_writer(io::stderr)
            .init();
    }

    let cwd = match system.current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("error: could not determine current directory: {err}");
            return ExitCode::from(EXIT_ERROR);
        }
    };

    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    let mut sinks = IoSinks::new(&mut stdout, &mut stderr);

    // Non-JSON mode does not emit a timing footer on any stream:
    // stdout stays pure command output and stderr stays clean. The timing
    // value survives as `elapsed_ms` inside the JSON payload.
    run(&cli, &system, &cwd, &mut sinks)
}

fn classify_error(err: &anyhow::Error) -> u8 {
    let msg = format!("{err:#}");
    if msg.contains(PERMISSIONS_NOT_RESTRICTED_MARKER) {
        EXIT_NOT_RESTRICTED
    } else if msg.contains(PRETOOL_FAIL_SENTINEL) {
        EXIT_PRETOOL_FAIL
    } else if msg.contains("Lint error") {
        EXIT_LINT
    } else if msg.contains("checksum") || msg.contains("signature") || msg.contains("integrity") {
        EXIT_INTEGRITY
    } else if msg.contains("attachment not found") {
        EXIT_ATTACHMENT
    } else if msg.contains("was removed") || msg.contains("preservation") {
        EXIT_PRESERVATION
    } else if msg.contains("skill") && msg.contains("not installed") {
        EXIT_SKILL
    } else if msg.contains("ambiguous: comment") {
        EXIT_AMBIGUOUS
    } else if msg.contains("not found") {
        EXIT_NOT_FOUND
    } else {
        EXIT_ERROR
    }
}

/// Build an [`IdentityFlags`] plus an optional `--assets-dir` value from
/// per-subcommand arg groups. The adapter boundary is where `~` /
/// `$VAR` get expanded, so the core never sees unexpanded path sigils.
///
/// The returned flags are consumed by
/// [`config::ResolvedConfig::resolve`], which picks the appropriate
/// branch of [`config::identity::resolve_identity`] — a single whole
/// identity comes out, never a mixture of fields from different files.
fn build_identity_flags(
    system: &dyn System,
    identity_args: &IdentityArgs,
    assets_args: Option<&AssetsArgs>,
) -> Result<(IdentityFlags, Option<String>)> {
    let assets_dir = match assets_args.and_then(|a| a.assets_dir.as_deref()) {
        Some(raw) => Some(expand_cli_path(system, raw)?.to_string_lossy().into_owned()),
        None => None,
    };

    let config_path = match identity_args.config.as_deref() {
        Some(raw) => Some(expand_cli_path(system, &raw.to_string_lossy())?),
        None => None,
    };

    let key = match identity_args.key.as_deref() {
        Some(raw) => {
            // `--key` accepts a bare name shorthand (e.g. `mykey` →
            // `~/.ssh/mykey`). Expand only when the raw value contains
            // a path sigil — bare names are resolved later by
            // `resolve_key_path`.
            if raw.starts_with('~') || raw.contains('$') {
                Some(expand_cli_path(system, raw)?.to_string_lossy().into_owned())
            } else {
                Some(String::from(raw))
            }
        }
        None => None,
    };

    let author_type = match identity_args.r#type.as_deref() {
        Some(raw) => Some(config::parse_author_type(raw)?),
        None => None,
    };

    let mut flags = IdentityFlags::default();
    flags.author_type = author_type;
    flags.config_path = config_path;
    flags.identity.clone_from(&identity_args.identity);
    flags.key = key;

    Ok((flags, assets_dir))
}

/// A handful of subcommands run entirely without a [`ResolvedConfig`]
/// (`Version`, `Identity`, `ResolveMode`, `Keygen`, `Skill`, `Obsidian`).
/// `Identity` is a read-only diagnostic — it calls
/// [`config::ResolvedConfig::resolve`] inside its own handler so a
/// branch-3 walk miss surfaces as `{ "found": false }` instead of
/// bailing the whole process. Returning `true` here
/// short-circuits the config load in [`run`].
const fn subcommand_is_config_free(cmd: &Commands) -> bool {
    match cmd {
        Commands::Version
        | Commands::Activity { .. }
        | Commands::Claude { .. }
        | Commands::Identity { .. }
        | Commands::Permissions { .. }
        | Commands::ResolveMode { .. }
        | Commands::Keygen { .. }
        | Commands::Skill { .. } => true,
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => true,
        Commands::Ack { .. }
        | Commands::Batch { .. }
        | Commands::Comment { .. }
        | Commands::Comments { .. }
        | Commands::Delete { .. }
        | Commands::Edit { .. }
        | Commands::Get { .. }
        | Commands::Lint { .. }
        | Commands::Ls { .. }
        | Commands::Mcp { .. }
        | Commands::Metadata { .. }
        | Commands::Mv { .. }
        | Commands::Plan { .. }
        | Commands::Prompt { .. }
        | Commands::Purge { .. }
        | Commands::Query { .. }
        | Commands::React { .. }
        | Commands::Registry { .. }
        | Commands::Rm { .. }
        | Commands::Sandbox { .. }
        | Commands::Search { .. }
        | Commands::Sign { .. }
        | Commands::Verify { .. }
        | Commands::Write { .. } => false,
    }
}

/// Fetch the [`IdentityArgs`] flatten for subcommands that declare one.
///
/// Subcommands that do not resolve identity (lint, query, search, ls,
/// get, metadata, registry, comments, version, keygen, resolve-mode,
/// skill, obsidian) return `None`; callers use the
/// [`IdentityArgs::default`] to build an empty [`IdentityFlags`].
const fn subcommand_identity(cmd: &Commands) -> Option<&IdentityArgs> {
    match cmd {
        Commands::Ack { identity_args, .. }
        | Commands::Activity { identity_args, .. }
        | Commands::Batch { identity_args, .. }
        | Commands::Comment { identity_args, .. }
        | Commands::Delete { identity_args, .. }
        | Commands::Edit { identity_args, .. }
        | Commands::Identity { identity_args, .. }
        | Commands::Mcp { identity_args, .. }
        | Commands::Mv { identity_args, .. }
        | Commands::Plan { identity_args, .. }
        | Commands::Prompt { identity_args, .. }
        | Commands::Purge { identity_args, .. }
        | Commands::Query { identity_args, .. }
        | Commands::React { identity_args, .. }
        | Commands::Rm { identity_args, .. }
        | Commands::Sandbox { identity_args, .. }
        | Commands::Sign { identity_args, .. }
        | Commands::Verify { identity_args, .. }
        | Commands::Write { identity_args, .. } => Some(identity_args),
        Commands::Claude { .. }
        | Commands::Comments { .. }
        | Commands::Get { .. }
        | Commands::Keygen { .. }
        | Commands::Lint { .. }
        | Commands::Ls { .. }
        | Commands::Metadata { .. }
        | Commands::Permissions { .. }
        | Commands::Registry { .. }
        | Commands::ResolveMode { .. }
        | Commands::Search { .. }
        | Commands::Skill { .. }
        | Commands::Version => None,
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => None,
    }
}

/// Fetch the [`AssetsArgs`] flatten for subcommands that write
/// attachments.
const fn subcommand_assets(cmd: &Commands) -> Option<&AssetsArgs> {
    match cmd {
        Commands::Batch { assets_args, .. }
        | Commands::Comment { assets_args, .. }
        | Commands::Edit { assets_args, .. } => Some(assets_args),
        Commands::Ack { .. }
        | Commands::Activity { .. }
        | Commands::Claude { .. }
        | Commands::Comments { .. }
        | Commands::Delete { .. }
        | Commands::Get { .. }
        | Commands::Identity { .. }
        | Commands::Keygen { .. }
        | Commands::Lint { .. }
        | Commands::Ls { .. }
        | Commands::Mcp { .. }
        | Commands::Metadata { .. }
        | Commands::Mv { .. }
        | Commands::Permissions { .. }
        | Commands::Plan { .. }
        | Commands::Prompt { .. }
        | Commands::Purge { .. }
        | Commands::Query { .. }
        | Commands::React { .. }
        | Commands::Registry { .. }
        | Commands::ResolveMode { .. }
        | Commands::Rm { .. }
        | Commands::Sandbox { .. }
        | Commands::Search { .. }
        | Commands::Sign { .. }
        | Commands::Skill { .. }
        | Commands::Verify { .. }
        | Commands::Version
        | Commands::Write { .. } => None,
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => None,
    }
}

/// Fetch the [`UnrestrictedArgs`] flatten for subcommands that touch
/// arbitrary filesystem paths.
const fn subcommand_unrestricted(cmd: &Commands) -> Option<&UnrestrictedArgs> {
    match cmd {
        Commands::Get {
            unrestricted_args, ..
        }
        | Commands::Ls {
            unrestricted_args, ..
        }
        | Commands::Metadata {
            unrestricted_args, ..
        }
        | Commands::Rm {
            unrestricted_args, ..
        }
        | Commands::Write {
            unrestricted_args, ..
        } => Some(unrestricted_args),
        Commands::Ack { .. }
        | Commands::Activity { .. }
        | Commands::Batch { .. }
        | Commands::Claude { .. }
        | Commands::Comment { .. }
        | Commands::Comments { .. }
        | Commands::Delete { .. }
        | Commands::Edit { .. }
        | Commands::Identity { .. }
        | Commands::Keygen { .. }
        | Commands::Lint { .. }
        | Commands::Mcp { .. }
        | Commands::Mv { .. }
        | Commands::Permissions { .. }
        | Commands::Plan { .. }
        | Commands::Prompt { .. }
        | Commands::Purge { .. }
        | Commands::Query { .. }
        | Commands::React { .. }
        | Commands::Registry { .. }
        | Commands::ResolveMode { .. }
        | Commands::Sandbox { .. }
        | Commands::Search { .. }
        | Commands::Sign { .. }
        | Commands::Skill { .. }
        | Commands::Verify { .. }
        | Commands::Version => None,
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => None,
    }
}

fn run(cli: &Cli, system: &dyn System, cwd: &Path, sinks: &mut IoSinks<'_>) -> ExitCode {
    let json_mode = subcommand_output(&cli.command).is_some_and(|o| o.json);

    match dispatch(cli, system, cwd, sinks) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let err_msg = format!("{err:#}");
            let is_silent_sentinel = err_msg.contains(PERMISSIONS_NOT_RESTRICTED_MARKER);
            let exit_code = classify_error(&err);
            let verify_failure = err.downcast_ref::<operations::verify::VerifyFailure>();
            let subset_failure = err.downcast_ref::<operations::verify::SubsetGateFailure>();
            if is_silent_sentinel {
                // Sentinel for `permissions check`.
                // Output already emitted on the success path; we only
                // need the gitignore-style exit code, no "error: ..."
                // render.
            } else if let Some(reason) = err_msg.strip_prefix(PRETOOL_FAIL_SENTINEL) {
                // Pretool fail-closed: Claude Code reads stderr and
                // feeds it back to the model. No "error: " prefix —
                // just the bare reason.
                let _ = writeln!(sinks.stderr, "{reason}");
            } else if json_mode {
                let payload = subset_failure
                    .map(operations::verify::SubsetGateFailure::to_json)
                    .or_else(|| verify_failure.map(operations::verify::VerifyFailure::to_json))
                    .unwrap_or_else(|| json!({ "error": err_msg }));
                let error_json = inject_elapsed_ms(&payload);
                let _ = writeln!(
                    sinks.stderr,
                    "{}",
                    serde_json::to_string_pretty(&error_json).unwrap_or_default()
                );
            } else if let Some(sg) = subset_failure {
                let _ = writeln!(sinks.stderr, "error: {}\n\n{}", sg.headline(), sg.hint());
            } else if let Some(vf) = verify_failure {
                let _ = writeln!(sinks.stderr, "error: {}", vf.human_text());
            } else {
                let _ = writeln!(sinks.stderr, "error: {err_msg}");
            }
            ExitCode::from(exit_code)
        }
    }
}

fn dispatch(cli: &Cli, system: &dyn System, cwd: &Path, sinks: &mut IoSinks<'_>) -> Result<()> {
    let output = subcommand_output(&cli.command);
    let json_mode = output.is_some_and(|o| o.json);

    if try_dispatch_config_free(cli, system, cwd, sinks)?.is_some() {
        return Ok(());
    }

    let default_identity = IdentityArgs::default();
    // Feature-gated: with `unrestricted`, this is a derived `Default` on a
    // regular struct; without it, a unit struct. Both spell as `UnrestrictedArgs::default()`
    // but clippy flags the unit-struct case as `default_constructed_unit_structs`.
    #[cfg(feature = "unrestricted")]
    let default_unrestricted = UnrestrictedArgs::default();
    #[cfg(not(feature = "unrestricted"))]
    let default_unrestricted = UnrestrictedArgs;
    let identity_args = subcommand_identity(&cli.command).unwrap_or(&default_identity);
    let assets_args = subcommand_assets(&cli.command);
    let unrestricted_args = subcommand_unrestricted(&cli.command).unwrap_or(&default_unrestricted);

    let (flags, assets_dir) = build_identity_flags(system, identity_args, assets_args)?;

    // The Mcp subcommand forwards its flags directly to `mcp::run` so
    // per-tool identity fields can still be declared on each request.
    // Branch out early.
    if let Commands::Mcp { action, .. } = &cli.command {
        return cmd_mcp(
            sinks,
            system,
            cwd,
            &flags,
            assets_dir.as_deref(),
            action.as_ref(),
            json_mode,
        );
    }

    let mut final_config = ResolvedConfig::resolve(system, cwd, &flags, assets_dir.as_deref())?;
    final_config.unrestricted = unrestricted_args.unrestricted();

    dispatch_with_config(sinks, cli, system, cwd, &final_config)
}

/// Handle every config-free subcommand. Returns `Ok(Some(()))` when a
/// matching arm ran, `Ok(None)` when the subcommand needs the
/// config-aware dispatch path.
fn try_dispatch_config_free(
    cli: &Cli,
    system: &dyn System,
    cwd: &Path,
    sinks: &mut IoSinks<'_>,
) -> Result<Option<()>> {
    match &cli.command {
        Commands::Version => handle_version(sinks).map(Some),
        Commands::Identity { .. } => handle_identity(&cli.command, sinks, system, cwd).map(Some),
        Commands::ResolveMode { .. } => {
            handle_resolve_mode(&cli.command, sinks, system, cwd).map(Some)
        }
        Commands::Keygen { .. } => handle_keygen(&cli.command, sinks, system).map(Some),
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => handle_obsidian(&cli.command, sinks, system, cwd).map(Some),
        Commands::Skill { .. } => handle_skill(&cli.command, sinks, system).map(Some),
        Commands::Activity { .. } => handle_activity(&cli.command, sinks, system, cwd).map(Some),
        Commands::Permissions { action } => cmd_permissions(sinks, system, cwd, action).map(Some),
        Commands::Claude { action } => handle_claude(action, sinks, system, cwd).map(Some),
        _ => {
            debug_assert!(
                !subcommand_is_config_free(&cli.command),
                "config-free subcommand fell through short-circuit"
            );
            Ok(None)
        }
    }
}

fn handle_version(sinks: &mut IoSinks<'_>) -> Result<()> {
    writeln!(sinks.stderr, "remargin {}", env!("CARGO_PKG_VERSION")).context("writing to stderr")
}

fn handle_identity(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::Identity {
        action,
        identity_args,
        output_args,
    } = command
    else {
        bail!("internal: handle_identity called with wrong subcommand");
    };
    cmd_identity(
        sinks,
        system,
        cwd,
        action.as_ref(),
        identity_args,
        output_args.json,
    )
}

fn handle_prompt(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Prompt {
        action,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_prompt called with wrong subcommand");
    };
    let cmd_json = output_args.json;
    match action {
        PromptAction::Resolve {
            file,
            output_args: a,
        } => cmd_prompt_resolve(sinks, system, cwd, file, cmd_json || a.json),
        PromptAction::Set {
            folder,
            name,
            prompt,
            output_args: a,
        } => cmd_prompt_set(
            sinks,
            system,
            &PromptSetParams {
                config,
                cwd,
                folder,
                json_mode: cmd_json || a.json,
                name,
                prompt_flag: prompt.as_deref(),
            },
        ),
        PromptAction::Delete {
            folder,
            output_args: a,
        } => cmd_prompt_delete(sinks, system, cwd, config, folder, cmd_json || a.json),
        PromptAction::List {
            folder,
            output_args: a,
        } => cmd_prompt_list(sinks, system, cwd, folder, cmd_json || a.json),
    }
}

fn handle_resolve_mode(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::ResolveMode {
        cwd: cwd_arg,
        output_args,
    } = command
    else {
        bail!("internal: handle_resolve_mode called with wrong subcommand");
    };
    let cwd_expanded = cwd_arg
        .as_deref()
        .map(|c| expand_cli_pathbuf(system, c))
        .transpose()?;
    let start_dir = cwd_expanded.as_deref().unwrap_or(cwd);
    cmd_resolve_mode(sinks, system, start_dir, output_args.json)
}

fn handle_keygen(command: &Commands, sinks: &mut IoSinks<'_>, system: &dyn System) -> Result<()> {
    let Commands::Keygen {
        output: keygen_output,
        ..
    } = command
    else {
        bail!("internal: handle_keygen called with wrong subcommand");
    };
    let expanded_output = expand_cli_pathbuf(system, keygen_output)?;
    cmd_keygen(sinks, system, &expanded_output)
}

#[cfg(feature = "obsidian")]
fn handle_obsidian(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::Obsidian {
        action,
        output_args,
    } = command
    else {
        bail!("internal: handle_obsidian called with wrong subcommand");
    };
    cmd_obsidian(sinks, system, cwd, action, output_args.json)
}

fn handle_skill(command: &Commands, sinks: &mut IoSinks<'_>, system: &dyn System) -> Result<()> {
    let Commands::Skill {
        action,
        output_args,
    } = command
    else {
        bail!("internal: handle_skill called with wrong subcommand");
    };
    cmd_skill(sinks, system, action, output_args.json)
}

fn handle_activity(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::Activity {
        path,
        since,
        pretty,
        identity_args,
        output_args,
    } = command
    else {
        bail!("internal: handle_activity called with wrong subcommand");
    };
    let p = ActivityParams {
        explicit_path: path.as_deref(),
        identity_args,
        json_mode: output_args.json,
        pretty: *pretty,
        since: since.as_deref(),
    };
    cmd_activity(sinks, system, cwd, &p)
}

fn handle_claude(
    action: &ClaudeAction,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    match action {
        ClaudeAction::Pretool { .. } => handle_claude_pretool(sinks, system),
        ClaudeAction::Restrict {
            path,
            also_deny_bash,
            cli_allowed,
            user_settings,
            output_args,
        } => {
            let p = RestrictParams {
                also_deny_bash,
                cli_allowed: *cli_allowed,
                json_mode: output_args.json,
                path,
                user_settings_explicit: user_settings.as_deref(),
            };
            cmd_restrict(sinks, system, cwd, &p)
        }
        ClaudeAction::Unrestrict {
            path,
            strict,
            user_settings,
            output_args,
        } => cmd_unprotect(
            sinks,
            system,
            cwd,
            path,
            *strict,
            user_settings.as_deref(),
            output_args.json,
        ),
    }
}

/// Reads the `PreToolUse` event JSON from stdin, runs the core
/// [`remargin_core::permissions::pretool::pretool`] function, and
/// emits the outcome. Fail-closed: any failure exits via
/// [`anyhow::bail!`] so the surrounding runner returns a non-zero
/// status (mapped to Claude Code's blocking semantics).
fn handle_claude_pretool(sinks: &mut IoSinks<'_>, system: &dyn System) -> Result<()> {
    let mut buf = Vec::new();
    io::stdin()
        .read_to_end(&mut buf)
        .context("reading stdin for claude pretool")?;
    match pretool(system, &buf) {
        PretoolOutcome::SilentAllow => Ok(()),
        PretoolOutcome::Deny(decision) => {
            let json = serde_json::to_string(&decision).context("serializing pretool decision")?;
            writeln!(sinks.stdout, "{json}").context("writing claude pretool decision")
        }
        PretoolOutcome::Fail(reason) => Err(anyhow::anyhow!("{PRETOOL_FAIL_SENTINEL}{reason}")),
        _ => Err(anyhow::anyhow!(
            "{PRETOOL_FAIL_SENTINEL}unexpected pretool outcome",
        )),
    }
}

fn dispatch_with_config(
    sinks: &mut IoSinks<'_>,
    cli: &Cli,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    match &cli.command {
        Commands::Ack { .. } => handle_ack(&cli.command, sinks, system, cwd, config),
        Commands::Batch { .. } => handle_batch(&cli.command, sinks, system, cwd, config),
        Commands::Comment { .. } => handle_comment(&cli.command, sinks, system, cwd, config),
        Commands::Comments { .. } => handle_comments(&cli.command, sinks, system, cwd),
        Commands::Delete { .. } => handle_delete(&cli.command, sinks, system, cwd, config),
        Commands::Edit { .. } => handle_edit(&cli.command, sinks, system, cwd, config),
        Commands::Get { .. } => handle_get(&cli.command, sinks, system, cwd, config),
        Commands::Lint { .. } => handle_lint(&cli.command, sinks, system, cwd),
        Commands::Ls { .. } => handle_ls(&cli.command, sinks, system, cwd, config),
        Commands::Metadata { .. } => handle_metadata(&cli.command, sinks, system, cwd, config),
        Commands::Mv { .. } => handle_mv(&cli.command, sinks, system, cwd, config),
        Commands::Plan { .. } => handle_plan(&cli.command, sinks, system, cwd, config),
        Commands::Prompt { .. } => handle_prompt(&cli.command, sinks, system, cwd, config),
        Commands::Purge { .. } => handle_purge(&cli.command, sinks, system, cwd, config),
        Commands::Query { .. } => handle_query(&cli.command, sinks, system, cwd, config),
        Commands::React { .. } => handle_react(&cli.command, sinks, system, cwd, config),
        Commands::Registry { .. } => handle_registry(&cli.command, sinks, system, cwd),
        Commands::Rm { .. } => handle_rm(&cli.command, sinks, system, cwd, config),
        Commands::Sandbox { .. } => handle_sandbox(&cli.command, sinks, system, cwd, config),
        Commands::Search { .. } => handle_search(&cli.command, sinks, system, cwd),
        Commands::Sign { .. } => handle_sign(&cli.command, sinks, system, cwd, config),
        Commands::Verify { .. } => handle_verify(&cli.command, sinks, system, cwd, config),
        Commands::Write { .. } => handle_write(&cli.command, sinks, system, cwd, config),
        Commands::Version
        | Commands::Activity { .. }
        | Commands::Claude { .. }
        | Commands::Identity { .. }
        | Commands::Mcp { .. }
        | Commands::Keygen { .. }
        | Commands::Permissions { .. }
        | Commands::ResolveMode { .. }
        | Commands::Skill { .. } => Ok(()),
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => Ok(()),
    }
}

fn handle_ack(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Ack {
        file,
        ids,
        path,
        remove,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_ack called with wrong subcommand");
    };
    let ap = AckParams {
        file: file.as_deref(),
        ids,
        json_mode: output_args.json,
        remove: *remove,
        search_path: path,
    };
    cmd_ack(sinks, system, cwd, config, &ap)
}

fn handle_batch(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Batch {
        file,
        ops,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_batch called with wrong subcommand");
    };
    cmd_batch(sinks, system, cwd, config, file, ops, output_args.json)
}

fn handle_comment(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Comment {
        file,
        content,
        after_comment,
        after_heading,
        after_line,
        attach,
        auto_ack,
        comment_file,
        remargin_kind,
        reply_to,
        sandbox,
        to,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_comment called with wrong subcommand");
    };
    let resolved_content =
        resolve_comment_content(system, cwd, content.as_ref(), comment_file.as_ref())?;
    let cp = CommentParams {
        after_comment: after_comment.as_deref(),
        after_heading: after_heading.as_deref(),
        after_line: *after_line,
        attachments: attach,
        auto_ack: *auto_ack,
        content: &resolved_content,
        file,
        json_mode: output_args.json,
        remargin_kind,
        reply_to: reply_to.as_deref(),
        sandbox: *sandbox,
        to,
    };
    cmd_comment(sinks, system, cwd, config, &cp)
}

fn handle_comments(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::Comments {
        file,
        pretty,
        remargin_kind,
        output_args,
    } = command
    else {
        bail!("internal: handle_comments called with wrong subcommand");
    };
    cmd_comments(
        sinks,
        system,
        cwd,
        file,
        remargin_kind,
        output_args.json,
        *pretty,
    )
}

fn handle_delete(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Delete {
        file,
        ids,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_delete called with wrong subcommand");
    };
    cmd_delete(sinks, system, cwd, config, file, ids, output_args.json)
}

fn handle_edit(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Edit {
        file,
        id,
        content,
        remargin_kind,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_edit called with wrong subcommand");
    };
    // When no --kind flags are provided we preserve the stored list; any
    // occurrence (even `--kind x` once) replaces the full list — consistent
    // with how `--to` works.
    let kind_replacement = (!remargin_kind.is_empty()).then_some(remargin_kind.as_slice());
    let p = EditParams {
        content,
        file,
        id,
        json_mode: output_args.json,
        remargin_kind: kind_replacement,
    };
    cmd_edit(sinks, system, cwd, config, &p)
}

fn handle_get(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Get {
        path,
        binary,
        start,
        end,
        line_numbers,
        out,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_get called with wrong subcommand");
    };
    let gp = GetParams {
        binary: *binary,
        end: *end,
        json_mode: output_args.json,
        line_numbers: *line_numbers,
        out: out.as_deref(),
        path,
        start: *start,
    };
    cmd_get(sinks, system, cwd, config, &gp)
}

fn handle_lint(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::Lint { file, output_args } = command else {
        bail!("internal: handle_lint called with wrong subcommand");
    };
    cmd_lint(sinks, system, cwd, file, output_args.json)
}

fn handle_ls(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Ls {
        path, output_args, ..
    } = command
    else {
        bail!("internal: handle_ls called with wrong subcommand");
    };
    cmd_ls(sinks, system, cwd, config, path, output_args.json)
}

fn handle_metadata(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Metadata {
        path, output_args, ..
    } = command
    else {
        bail!("internal: handle_metadata called with wrong subcommand");
    };
    cmd_metadata(sinks, system, cwd, config, path, output_args.json)
}

fn handle_mv(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Mv {
        src,
        dst,
        force,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_mv called with wrong subcommand");
    };
    let p = MvParams {
        dst: dst.as_str(),
        force: *force,
        json_mode: output_args.json,
        src: src.as_str(),
    };
    cmd_mv(sinks, system, cwd, config, &p)
}

fn handle_plan(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Plan { action, .. } = command else {
        bail!("internal: handle_plan called with wrong subcommand");
    };
    cmd_plan(
        sinks,
        system,
        cwd,
        config,
        action,
        plan_action_output(action).json,
    )
}

fn handle_purge(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Purge {
        file,
        output_args,
        recursive,
        ..
    } = command
    else {
        bail!("internal: handle_purge called with wrong subcommand");
    };
    cmd_purge(
        sinks,
        system,
        cwd,
        config,
        file,
        *recursive,
        output_args.json,
    )
}

fn handle_query(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let q = build_query_params(command)?;
    cmd_query(sinks, system, cwd, config, &q)
}

fn build_query_params(command: &Commands) -> Result<QueryParams<'_>> {
    let Commands::Query {
        path,
        author,
        comment_id,
        content_regex,
        expanded,
        ignore_case,
        pending,
        pending_broadcast,
        pending_for,
        pending_for_me,
        pretty,
        remargin_kind,
        since,
        summary,
        output_args,
        ..
    } = command
    else {
        bail!("internal: build_query_params called with wrong subcommand");
    };
    let output = if output_args.json {
        QueryOutputMode::Json
    } else if *pretty {
        QueryOutputMode::Pretty
    } else if *summary {
        QueryOutputMode::Summary
    } else {
        QueryOutputMode::Plain
    };
    let pending_filter = QueryPendingFilters {
        any: *pending,
        broadcast: *pending_broadcast,
        for_user: pending_for.as_deref(),
        for_me: *pending_for_me,
    };
    Ok(QueryParams {
        author: author.as_deref(),
        comment_id: comment_id.as_deref(),
        content_regex: content_regex.as_deref(),
        expanded: *expanded,
        ignore_case: *ignore_case,
        output,
        path: path.as_str(),
        pending: pending_filter,
        remargin_kind,
        since: since.as_deref(),
    })
}

fn handle_react(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::React {
        file,
        id,
        emoji,
        remove,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_react called with wrong subcommand");
    };
    let r = ReactParams {
        emoji: emoji.as_str(),
        file: file.as_str(),
        id: id.as_str(),
        json_mode: output_args.json,
        remove: *remove,
    };
    cmd_react(sinks, system, cwd, config, &r)
}

fn handle_registry(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::Registry {
        action,
        output_args,
    } = command
    else {
        bail!("internal: handle_registry called with wrong subcommand");
    };
    cmd_registry(sinks, system, cwd, action, output_args.json)
}

fn handle_rm(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Rm {
        file, output_args, ..
    } = command
    else {
        bail!("internal: handle_rm called with wrong subcommand");
    };
    cmd_rm(sinks, system, cwd, config, file, output_args.json)
}

fn handle_sandbox(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Sandbox {
        action,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_sandbox called with wrong subcommand");
    };
    cmd_sandbox(sinks, system, cwd, config, action, output_args.json)
}

fn handle_search(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
) -> Result<()> {
    let Commands::Search {
        pattern,
        path,
        regex,
        scope,
        context,
        ignore_case,
        output_args,
    } = command
    else {
        bail!("internal: handle_search called with wrong subcommand");
    };
    let s = SearchParams {
        context: *context,
        ignore_case: *ignore_case,
        json_mode: output_args.json,
        path: path.as_str(),
        pattern: pattern.as_str(),
        regex: *regex,
        scope: scope.as_str(),
    };
    cmd_search(sinks, system, cwd, &s)
}

fn handle_sign(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Sign {
        file,
        ids,
        all_mine,
        repair_checksum,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_sign called with wrong subcommand");
    };
    let sp = SignParams {
        all_mine: *all_mine,
        file,
        ids,
        json_mode: output_args.json,
        repair_checksum: *repair_checksum,
    };
    cmd_sign(sinks, system, cwd, config, &sp)
}

fn handle_verify(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Verify {
        file, output_args, ..
    } = command
    else {
        bail!("internal: handle_verify called with wrong subcommand");
    };
    cmd_verify(sinks, system, cwd, file, config, output_args.json)
}

fn handle_write(
    command: &Commands,
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    let Commands::Write {
        path,
        content,
        binary,
        create,
        lines,
        raw,
        output_args,
        ..
    } = command
    else {
        bail!("internal: handle_write called with wrong subcommand");
    };
    let line_range = lines.as_deref().map(parse_line_range).transpose()?;
    cmd_write(
        sinks,
        system,
        cwd,
        config,
        &WriteParams {
            content: content.as_deref(),
            json_mode: output_args.json,
            opts: document::WriteOptions::new()
                .binary(*binary)
                .create(*create)
                .lines(line_range)
                .raw(*raw),
            path,
        },
    )
}

fn cmd_ack(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &AckParams<'_>,
) -> Result<()> {
    let AckParams {
        file,
        ids,
        json_mode,
        remove,
        search_path,
    } = *params;
    if let Some(doc_file) = file {
        let path = resolve_doc_path(system, cwd, doc_file)?;
        let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        operations::ack_comments(system, &path, config, &id_refs, remove)?;
    } else {
        let base_dir = cwd.join(search_path);
        for comment_id in ids {
            let matches = query::resolve_comment_id(system, &base_dir, comment_id)?;
            match matches.len() {
                0 => {
                    bail!("comment {comment_id:?} not found");
                }
                1 => {
                    let id_refs: Vec<&str> = vec![comment_id.as_str()];
                    operations::ack_comments(system, &matches[0], config, &id_refs, remove)?;
                }
                n => {
                    let file_list: Vec<String> =
                        matches.iter().map(|p| p.display().to_string()).collect();
                    bail!(
                        "ambiguous: comment {comment_id:?} found in {n} files: {}",
                        file_list.join(", ")
                    );
                }
            }
        }
    }
    print_output(sinks, json_mode, &responses::ack(ids, remove))
}

fn cmd_batch(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    ops_json: &str,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let ops_value: Vec<Value> =
        serde_json::from_str(ops_json).context("parsing batch operations JSON")?;

    let mut batch_ops = Vec::with_capacity(ops_value.len());
    for (idx, op_value) in ops_value.iter().enumerate() {
        let op_obj = op_value
            .as_object()
            .with_context(|| format!("batch op[{idx}]: expected object"))?;
        batch_ops.push(BatchCommentOp::from_json_object(op_obj, idx)?);
    }

    let created_ids = operations::batch::batch_comment(system, &path, config, &batch_ops)?;
    print_output(sinks, json_mode, &responses::batch(&created_ids))
}

fn resolve_comment_position(
    reply_to: Option<&str>,
    after_comment: Option<&str>,
    after_heading: Option<&str>,
    after_line: Option<usize>,
) -> InsertPosition {
    InsertPosition::from_hints(reply_to, after_comment, after_heading, after_line)
}

fn cmd_comment(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    cp: &CommentParams<'_>,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, cp.file)?;

    // Replies always go after their parent — explicit placement is ignored.
    let position = resolve_comment_position(
        cp.reply_to,
        cp.after_comment,
        cp.after_heading,
        cp.after_line,
    );

    let mut params = operations::CreateCommentParams::new(cp.content, &position);
    params.attachments = cp.attachments;
    params.auto_ack = cp.auto_ack;
    params.remargin_kind = cp.remargin_kind;
    params.reply_to = cp.reply_to;
    params.sandbox = cp.sandbox;
    params.to = cp.to;

    let new_id = operations::create_comment(system, &path, config, &params)?;

    // Write to stdout if stdin mode.
    if cp.file == "-" {
        let updated = system.read_to_string(&path)?;
        out_raw(sinks, &updated)?;
    }

    print_output(sinks, cp.json_mode, &responses::comment_created(&new_id))
}

fn cmd_comments(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    file: &str,
    kind_filter: &[String],
    json_mode: bool,
    pretty: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let doc = parser::parse_file(system, &path)?;
    // Apply the shared kind filter from `remargin-core::kind` so this
    // surface stays in lockstep with `remargin query` — the
    // design doc explicitly calls out the previous divergence as a bug.
    let comments: Vec<_> = doc
        .comments()
        .into_iter()
        .filter(|cm| matches_kind_filter(cm.kinds(), kind_filter))
        .collect();

    if pretty {
        let formatted = display::format_comments_pretty(file, &comments);
        out(sinks, &formatted)
    } else if json_mode {
        out_json(sinks, &json!({ "comments": comments }))
    } else {
        for cm in &comments {
            let ack_status = if cm.ack.is_empty() {
                "pending"
            } else {
                "acked"
            };
            out(
                sinks,
                &format!(
                    "{} {} ({}) [{}] {}",
                    cm.id,
                    cm.author,
                    author_type_str(&cm.author_type),
                    ack_status,
                    truncate_content(&cm.content, 60_usize),
                ),
            )?;
        }
        Ok(())
    }
}

fn cmd_delete(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    ids: &[String],
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    operations::delete_comments(system, &path, config, &id_refs)?;
    print_output(sinks, json_mode, &responses::comments_deleted(ids))
}

fn cmd_edit(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    p: &EditParams<'_>,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, p.file)?;
    operations::edit_comment(system, &path, config, p.id, p.content, p.remargin_kind)?;
    print_output(sinks, p.json_mode, &responses::comment_edited(p.id))
}

fn cmd_get(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    gp: &GetParams<'_>,
) -> Result<()> {
    let target_buf = expand_cli_path(system, gp.path)?;
    let target = target_buf.as_path();

    if gp.binary {
        return cmd_get_binary(sinks, system, cwd, config, gp, target);
    }

    if gp.out.is_some() {
        bail!("--out requires --binary");
    }

    let lines = match (gp.start, gp.end) {
        (Some(s), Some(e)) => Some((s, e)),
        _ => None,
    };

    if gp.json_mode && gp.line_numbers {
        let content = document::get(
            system,
            cwd,
            target,
            lines,
            false,
            config.unrestricted,
            &config.trusted_roots,
        )?;
        let start_num = lines.map_or(1, |(s, _)| s);
        let json_lines: Vec<Value> = content
            .split('\n')
            .enumerate()
            .map(|(i, text)| json!({ "line": start_num + i, "text": text }))
            .collect();
        print_output(sinks, true, &json!({ "lines": json_lines }))
    } else {
        let content = document::get(
            system,
            cwd,
            target,
            lines,
            gp.line_numbers,
            config.unrestricted,
            &config.trusted_roots,
        )?;
        if gp.json_mode {
            print_output(sinks, true, &json!({ "content": content }))
        } else {
            out_raw(sinks, &content)
        }
    }
}

/// Binary-mode `get` dispatch. Reads bytes once through the shared
/// core helper, then surfaces them in the caller's chosen shape:
/// - `--out <path>` — write bytes to disk, stdout shows `{path, size_bytes, mime}`.
/// - `--json` — base64-encoded `content` in the payload alongside mime / size.
/// - default — raw bytes to stdout (so `remargin get --binary x.png > out.png` works).
///
/// Incompatible flags (`--start`, `--end`, `-n`) are rejected up front so
/// binary requests never silently drop text-mode options.
fn cmd_get_binary(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    gp: &GetParams<'_>,
    target: &Path,
) -> Result<()> {
    if gp.start.is_some() || gp.end.is_some() {
        bail!("--start / --end are not supported with --binary");
    }
    if gp.line_numbers {
        bail!("--line-numbers is not supported with --binary");
    }

    let payload = document::read_binary(
        system,
        cwd,
        target,
        config.unrestricted,
        &config.trusted_roots,
    )?;

    if let Some(out_path) = gp.out {
        system
            .write(out_path, &payload.bytes)
            .with_context(|| format!("writing {}", out_path.display()))?;
        let summary = json!({
            "mime": payload.mime,
            "out": out_path,
            "path": payload.path,
            "size_bytes": payload.size_bytes,
        });
        return print_output(sinks, gp.json_mode, &summary);
    }

    if gp.json_mode {
        let encoded = BASE64_STANDARD.encode(&payload.bytes);
        return print_output(
            sinks,
            true,
            &json!({
                "binary": true,
                "content": encoded,
                "mime": payload.mime,
                "path": payload.path,
                "size_bytes": payload.size_bytes,
            }),
        );
    }

    // Non-JSON, no --out: raw bytes to stdout so shell redirection works.
    sinks
        .stdout
        .write_all(&payload.bytes)
        .context("writing bytes to stdout")
}

/// Resolve and print the identity the CLI's active flag set produces.
///
/// Routes through the same [`ResolvedConfig::resolve`][config::ResolvedConfig::resolve]
/// every mutating op uses, so `remargin identity --config <path>` (or
/// `--identity` + `--type` manual, or a `--type`-filtered walk) returns
/// the same identity the next write would attribute to.
///
/// A branch-3 walk that cannot match the supplied filters is treated as
/// "nothing found" rather than an error: the JSON output collapses to
/// `{ "found": false }`, preserving the historical read-only-diagnostic
/// contract and letting the Obsidian plugin call this during startup
/// without having to special-case transient "no config yet" states.
/// Other resolver errors (unknown type strings, strict-mode registry
/// misses, etc.) still propagate.
fn cmd_identity(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    action: Option<&IdentityAction>,
    identity_args: &IdentityArgs,
    json_mode: bool,
) -> Result<()> {
    match action {
        Some(IdentityAction::Create {
            identity,
            r#type,
            key,
            output_args,
        }) => cmd_identity_create(sinks, identity, r#type, key.as_deref(), output_args.json),
        Some(IdentityAction::Show {
            identity_args: nested,
            output_args,
        }) => cmd_identity_show(sinks, system, cwd, nested, output_args.json),
        None => cmd_identity_show(sinks, system, cwd, identity_args, json_mode),
    }
}

fn cmd_identity_show(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    identity_args: &IdentityArgs,
    json_mode: bool,
) -> Result<()> {
    let (flags, _assets_dir) = build_identity_flags(system, identity_args, None)?;
    let report = resolve_identity_report(system, cwd, &flags)?;
    render_identity_report(sinks, &report, json_mode)
}

/// Print a ready-to-use identity YAML block to stdout.
///
/// `mode:` is deliberately omitted — mode is a tree property resolved
/// by walk-up, not an identity-level declaration. `key:` is emitted
/// verbatim when supplied; an absent key is valid in non-strict modes.
/// `--json` returns the same fields as a structured payload so tooling
/// (the Obsidian plugin, scripts) can pick them up without re-parsing
/// YAML.
fn cmd_identity_create(
    sinks: &mut IoSinks<'_>,
    identity: &str,
    author_type: &str,
    key: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    // Validate the author type early so an invalid value fails before
    // any output is emitted (stdout stays clean for redirection).
    config::parse_author_type(author_type)
        .with_context(|| format!("invalid --type value: {author_type}"))?;

    if json_mode {
        return print_output(
            sinks,
            true,
            &json!({
                "identity": identity,
                "type": author_type,
                "key": key,
            }),
        );
    }
    let mut out_str = format!("identity: {identity}\ntype: {author_type}\n");
    if let Some(k) = key {
        use core::fmt::Write as _;
        let _ = writeln!(out_str, "key: {k}");
    }
    out_raw(sinks, &out_str)
}

fn render_identity_report(
    sinks: &mut IoSinks<'_>,
    report: &IdentityReport,
    json_mode: bool,
) -> Result<()> {
    if !report.found {
        if json_mode {
            return print_output(sinks, true, &json!({ "found": false }));
        }
        writeln!(sinks.stderr, "No identity config found.").context("writing to stderr")?;
        return Ok(());
    }

    if json_mode {
        return print_output(
            sinks,
            true,
            &serde_json::to_value(report).context("serializing identity report")?,
        );
    }

    if let Some(p) = &report.path {
        writeln!(sinks.stderr, "Found config: {p}").context("writing to stderr")?;
    }
    if let Some(i) = &report.identity {
        writeln!(sinks.stderr, "Identity:     {i}").context("writing to stderr")?;
    }
    if let Some(t) = &report.author_type {
        writeln!(sinks.stderr, "Type:         {t}").context("writing to stderr")?;
    }
    if let Some(k) = &report.key {
        writeln!(sinks.stderr, "Key:          {k}").context("writing to stderr")?;
    }
    if let Some(m) = &report.mode {
        writeln!(sinks.stderr, "Mode:         {m}").context("writing to stderr")?;
    }
    Ok(())
}

/// Dispatch `remargin permissions <show|check>`.
///
/// `show` prints the resolved permissions tree at `cwd`. `check`
/// canonicalises its target path, asks the inspector whether any
/// `restrict` or `deny_ops` rule covers it, and exits gitignore-style:
/// 0 when restricted, 1 when not. Both paths support `--json`.
/// Wire the CLI `activity` subcommand to the
/// [`activity::gather_activity`] core.
///
/// Output mode follows the workspace `--json` convention:
/// `--json` (default) emits the structured `ActivityResult`;
/// `--pretty` switches to a human-readable timeline. Both flags
/// at once is rejected (clap-level via the surrounding
/// [`OutputArgs::json`] flag plus the local `pretty` boolean).
///
/// Identity is read-only here — the quartet resolves only the
/// caller name driving the per-file cutoff. No signing, no key
/// requirement.
fn cmd_activity(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    p: &ActivityParams<'_>,
) -> Result<()> {
    if p.pretty && p.json_mode {
        bail!("--pretty and --json are mutually exclusive");
    }

    let resolved_path = match p.explicit_path {
        Some(path) => {
            let expanded = expand_cli_pathbuf(system, path)?;
            if expanded.is_absolute() {
                expanded
            } else {
                cwd.join(expanded)
            }
        }
        None => cwd.to_path_buf(),
    };

    let cutoff = match p.since {
        Some(raw) => Some(
            chrono::DateTime::parse_from_rfc3339(raw)
                .with_context(|| format!("--since: invalid ISO 8601 timestamp {raw:?}"))?,
        ),
        None => None,
    };

    let (flags, _assets_dir) = build_identity_flags(system, p.identity_args, None)?;
    let resolved = ResolvedConfig::resolve(system, cwd, &flags, None)?;
    let caller = resolved
        .identity
        .as_deref()
        .context("activity: caller identity required (declare via --identity / --config)")?;

    let result = activity::gather_activity(system, &resolved_path, cutoff, caller)?;

    if p.pretty {
        emit_activity_pretty(sinks, &result)?;
    } else {
        let value = serde_json::to_value(&result).context("serializing activity result")?;
        print_output(sinks, true, &value)?;
    }
    Ok(())
}

fn emit_activity_pretty(sinks: &mut IoSinks<'_>, result: &activity::ActivityResult) -> Result<()> {
    if result.files.is_empty() {
        writeln!(sinks.stderr, "(no activity)").context("writing to stderr")?;
        return Ok(());
    }
    for file in &result.files {
        writeln!(sinks.stderr, "{}:", file.path.display()).context("writing to stderr")?;
        writeln!(
            sinks.stderr,
            "  {}",
            format_activity_cutoff_header(result.cutoff_explicit, file.cutoff_applied)
        )
        .context("writing to stderr")?;
        for change in &file.changes {
            match change {
                activity::Change::Comment {
                    ts,
                    comment_id,
                    author,
                    line_start,
                    line_end,
                    reply_to,
                    ..
                } => {
                    let arrow = reply_to
                        .as_deref()
                        .map_or_else(String::new, |p| format!(" \u{2934} {p}"));
                    writeln!(
                        sinks.stderr,
                        "  {} \u{00b7} comment \u{00b7} {comment_id} by {author}{arrow} (lines {line_start}-{line_end})",
                        ts.format("%Y-%m-%d %H:%M")
                    )
                    .context("writing to stderr")?;
                }
                activity::Change::Ack {
                    ts,
                    comment_id,
                    author,
                    ..
                } => {
                    writeln!(
                        sinks.stderr,
                        "  {} \u{00b7} ack \u{00b7} {comment_id} acked by {author}",
                        ts.format("%Y-%m-%d %H:%M")
                    )
                    .context("writing to stderr")?;
                }
                activity::Change::Sandbox { ts, author, .. } => {
                    writeln!(
                        sinks.stderr,
                        "  {} \u{00b7} sandbox \u{00b7} {author}",
                        ts.format("%Y-%m-%d %H:%M")
                    )
                    .context("writing to stderr")?;
                }
                // The Change enum is `#[non_exhaustive]`; future
                // variants surface as a generic line until the
                // pretty-printer is taught about them.
                _ => {}
            }
        }
        writeln!(sinks.stderr).context("writing to stderr")?;
    }
    if let Some(ts) = result.newest_ts_overall {
        writeln!(sinks.stderr, "(newest_ts_overall: {})", ts.to_rfc3339())
            .context("writing to stderr")?;
    }
    Ok(())
}

/// Render the per-file cutoff header line for `activity --pretty`.
/// The wording reflects which path produced the cutoff:
///
/// - explicit `--since`: `(since 2026-04-27 02:09)` so the header
///   echoes the user's input.
/// - implicit, with a caller-last-action timestamp: `(since you
///   last touched this file: 2026-04-27 02:09)` to make it clear
///   the cutoff came from the caller's own prior activity.
/// - implicit, no prior activity (initial-touch fallback): `(since
///   the beginning — no prior activity by you in this file)` so
///   the reader knows the full timeline is being shown rather than
///   silently inferring it from the absence of a header.
fn format_activity_cutoff_header(
    cutoff_explicit: bool,
    cutoff: Option<chrono::DateTime<chrono::FixedOffset>>,
) -> String {
    if cutoff_explicit {
        // Explicit `--since` is always `Some(_)`; the `None` arm is
        // defensive against a future caller path that forgets to
        // pre-validate.
        cutoff.map_or_else(
            || String::from("(since: explicit cutoff missing)"),
            |ts| format!("(since {})", ts.format("%Y-%m-%d %H:%M")),
        )
    } else {
        cutoff.map_or_else(
            || String::from("(since the beginning \u{2014} no prior activity by you in this file)"),
            |ts| {
                format!(
                    "(since you last touched this file: {})",
                    ts.format("%Y-%m-%d %H:%M")
                )
            },
        )
    }
}

fn cmd_permissions(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    action: &PermissionsAction,
) -> Result<()> {
    match action {
        PermissionsAction::Show { output_args } => {
            let report = permissions_inspect::show(system, cwd)?;
            if output_args.json {
                let value =
                    serde_json::to_value(&report).context("serializing permissions show output")?;
                print_output(sinks, true, &value)?;
            } else {
                emit_permissions_show_text(sinks, cwd, &report)?;
            }
            Ok(())
        }
        PermissionsAction::Check {
            path,
            why,
            output_args,
        } => {
            let expanded = expand_cli_pathbuf(system, path)?;
            let target = if expanded.is_absolute() {
                expanded
            } else {
                cwd.join(expanded)
            };
            let report = permissions_inspect::check(system, cwd, &target, *why)?;
            if output_args.json {
                let value = serde_json::to_value(&report)
                    .context("serializing permissions check output")?;
                print_output(sinks, true, &value)?;
            } else {
                emit_permissions_check_text(sinks, &report, *why)?;
            }
            // Gitignore-style exit code: 0 when restricted, 1 otherwise.
            // We have already printed our payload, so signal "miss" with
            // a sentinel error that `main` recognises as
            // [`EXIT_NOT_RESTRICTED`] and renders silently (no
            // "error: ..." prefix).
            if report.restricted {
                Ok(())
            } else {
                bail!("{PERMISSIONS_NOT_RESTRICTED_MARKER}");
            }
        }
    }
}

/// Bracket a list of `String` values using `Display` formatting so the
/// output can be read by humans without leaking `Debug`'s escape rules
/// (clippy denies `use_debug` workspace-wide).
fn format_string_list(items: &[String]) -> String {
    let mut out = String::from("[");
    for (idx, item) in items.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        out.push_str(item);
    }
    out.push(']');
    out
}

fn emit_permissions_show_text(
    sinks: &mut IoSinks<'_>,
    cwd: &Path,
    report: &permissions_inspect::ShowOutput,
) -> Result<()> {
    let stderr = &mut sinks.stderr;
    writeln!(stderr, "Permissions resolved at {}:", cwd.display()).context("writing to stderr")?;
    writeln!(stderr).context("writing to stderr")?;

    writeln!(stderr, "  trusted_roots:").context("writing to stderr")?;
    if report.trusted_roots.is_empty() {
        writeln!(stderr, "    (none)").context("writing to stderr")?;
    } else {
        for entry in &report.trusted_roots {
            writeln!(
                stderr,
                "    {}  (source: {})",
                entry.path_text,
                entry.source_file.display()
            )
            .context("writing to stderr")?;
            if let Some(realm) = entry.realm_root.as_deref() {
                writeln!(stderr, "      realm_root: {}", realm.display())
                    .context("writing to stderr")?;
            }
            if !entry.also_deny_bash.is_empty() {
                writeln!(
                    stderr,
                    "      also_deny_bash: {}",
                    format_string_list(&entry.also_deny_bash)
                )
                .context("writing to stderr")?;
            }
            writeln!(stderr, "      cli_allowed: {}", entry.cli_allowed)
                .context("writing to stderr")?;
        }
    }
    writeln!(stderr).context("writing to stderr")?;

    writeln!(stderr, "  deny_ops:").context("writing to stderr")?;
    if report.deny_ops.is_empty() {
        writeln!(stderr, "    (none)").context("writing to stderr")?;
    } else {
        for entry in &report.deny_ops {
            writeln!(
                stderr,
                "    {}  (source: {})",
                entry.path.display(),
                entry.source_file.display()
            )
            .context("writing to stderr")?;
            for item in &entry.ops {
                if item.exceptions.is_empty() {
                    writeln!(stderr, "      - {}", item.name).context("writing to stderr")?;
                } else {
                    writeln!(
                        stderr,
                        "      - {} (exceptions: {})",
                        item.name,
                        format_string_list(&item.exceptions),
                    )
                    .context("writing to stderr")?;
                }
            }
        }
    }
    writeln!(stderr).context("writing to stderr")?;

    writeln!(stderr, "  allow_dot_folders:").context("writing to stderr")?;
    if report.allow_dot_folders.is_empty() {
        writeln!(stderr, "    (none)").context("writing to stderr")?;
    } else {
        for entry in &report.allow_dot_folders {
            writeln!(stderr, "    {}", format_string_list(&entry.names))
                .context("writing to stderr")?;
        }
    }
    Ok(())
}

fn emit_permissions_check_text(
    sinks: &mut IoSinks<'_>,
    report: &permissions_inspect::CheckOutput,
    why: bool,
) -> Result<()> {
    writeln!(sinks.stderr, "restricted: {}", report.restricted).context("writing to stderr")?;
    if why && let Some(rule) = &report.matching_rule {
        writeln!(sinks.stderr, "  matched: {}", rule.rule_text).context("writing to stderr")?;
        writeln!(sinks.stderr, "  kind:    {}", rule.kind).context("writing to stderr")?;
        writeln!(sinks.stderr, "  source:  {}", rule.source_file.display())
            .context("writing to stderr")?;
    }
    Ok(())
}

/// Wire the CLI `restrict` subcommand to the
/// [`permissions_restrict::restrict`] core.
///
/// `user_settings_explicit` lets tests pin a hermetic location for
/// the user-scope file. When `None`, the function expands
/// [`DEFAULT_USER_SETTINGS`] through the active `System`.
fn cmd_restrict(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    p: &RestrictParams<'_>,
) -> Result<()> {
    let user_scope = match p.user_settings_explicit {
        Some(explicit) => expand_cli_pathbuf(system, explicit)?,
        None => expand_cli_path(system, DEFAULT_USER_SETTINGS)?,
    };
    let anchor = permissions_restrict::find_claude_anchor(system, cwd)?;
    let project_scope = anchor.join(".claude/settings.local.json");
    let settings_files = vec![project_scope, user_scope];

    let args = permissions_restrict::RestrictArgs::new(
        String::from(p.path),
        p.also_deny_bash.to_vec(),
        p.cli_allowed,
    );
    let outcome = permissions_restrict::restrict(system, cwd, &args, &settings_files)?;

    if p.json_mode {
        let value = serde_json::json!({
            "absolute_path": outcome.absolute_path.display().to_string(),
            "anchor": outcome.anchor.display().to_string(),
            "claude_files_touched": outcome
                .claude_files_touched
                .iter()
                .map(|file| file.display().to_string())
                .collect::<Vec<_>>(),
            "rules_applied": outcome.rules_applied,
            "yaml_was_created": outcome.yaml_was_created,
        });
        print_output(sinks, true, &value)?;
    } else {
        emit_restrict_summary(sinks, &outcome)?;
    }
    Ok(())
}

fn emit_restrict_summary(
    sinks: &mut IoSinks<'_>,
    outcome: &permissions_restrict::RestrictOutcome,
) -> Result<()> {
    let stderr = &mut sinks.stderr;
    writeln!(stderr, "Restricted: {}", outcome.absolute_path.display())
        .context("writing to stderr")?;
    writeln!(stderr, "  Anchor: {}", outcome.anchor.display()).context("writing to stderr")?;
    if outcome.yaml_was_created {
        writeln!(
            stderr,
            "  .remargin.yaml created at {}",
            outcome.anchor.join(".remargin.yaml").display()
        )
        .context("writing to stderr")?;
    } else {
        writeln!(
            stderr,
            "  .remargin.yaml updated at {}",
            outcome.anchor.join(".remargin.yaml").display()
        )
        .context("writing to stderr")?;
    }
    writeln!(
        stderr,
        "  Settings updated: {} file(s)",
        outcome.claude_files_touched.len()
    )
    .context("writing to stderr")?;
    for file in &outcome.claude_files_touched {
        writeln!(stderr, "    {}", file.display()).context("writing to stderr")?;
    }
    writeln!(stderr, "  Rules written: {}", outcome.rules_applied.len())
        .context("writing to stderr")?;
    writeln!(
        stderr,
        "  Sidecar updated: {}",
        outcome
            .anchor
            .join(".claude/.remargin-restrictions.json")
            .display()
    )
    .context("writing to stderr")?;
    writeln!(
        stderr,
        "  Note: Claude must reload its settings for Layer 2 (NATIVE tool denials) to take effect."
    )
    .context("writing to stderr")?;
    writeln!(
        stderr,
        "  Layer 1 (remargin's own ops) is enforcing immediately on the next call."
    )
    .context("writing to stderr")?;
    Ok(())
}

/// Wire the CLI `unprotect` subcommand to the
/// [`permissions_unprotect::unprotect`] core.
///
/// `_user_settings_explicit` is accepted on the CLI for symmetry
/// with `restrict` but ignored here: the unprotect path consults
/// the sidecar's `added_to_files` list (the resolved settings paths
/// captured at apply time), so the reversal scrubs exactly the files
/// the corresponding `restrict` touched.
fn cmd_unprotect(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    path: &str,
    strict: bool,
    _user_settings_explicit: Option<&Path>,
    json_mode: bool,
) -> Result<()> {
    let args = permissions_unprotect::UnprotectArgs::new(String::from(path)).with_strict(strict);
    let outcome = permissions_unprotect::unprotect(system, cwd, &args)?;

    if json_mode {
        let value = serde_json::json!({
            "absolute_path": outcome.absolute_path.display().to_string(),
            "anchor": outcome.anchor.display().to_string(),
            "claude_files_touched": outcome
                .claude_files_touched
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
            "rules_removed": outcome.rules_removed,
            "warnings": outcome.warnings,
            "yaml_entry_removed": outcome.yaml_entry_removed,
        });
        print_output(sinks, true, &value)?;
    } else {
        emit_unprotect_summary(sinks, &outcome)?;
    }
    Ok(())
}

fn emit_unprotect_summary(
    sinks: &mut IoSinks<'_>,
    outcome: &permissions_unprotect::UnprotectOutcome,
) -> Result<()> {
    let stderr = &mut sinks.stderr;
    writeln!(stderr, "Unprotected: {}", outcome.absolute_path.display())
        .context("writing to stderr")?;
    writeln!(stderr, "  Anchor: {}", outcome.anchor.display()).context("writing to stderr")?;
    if outcome.yaml_entry_removed {
        writeln!(
            stderr,
            "  .remargin.yaml updated at {}",
            outcome.anchor.join(".remargin.yaml").display()
        )
        .context("writing to stderr")?;
    } else {
        writeln!(stderr, "  .remargin.yaml: no matching entry").context("writing to stderr")?;
    }
    if outcome.claude_files_touched.is_empty() {
        writeln!(stderr, "  Settings: none touched (no sidecar entry)")
            .context("writing to stderr")?;
    } else {
        writeln!(
            stderr,
            "  Settings updated: {} file(s)",
            outcome.claude_files_touched.len()
        )
        .context("writing to stderr")?;
        for file in &outcome.claude_files_touched {
            writeln!(stderr, "    {}", file.display()).context("writing to stderr")?;
        }
    }
    if !outcome.warnings.is_empty() {
        writeln!(stderr, "  Warnings:").context("writing to stderr")?;
        for warning in &outcome.warnings {
            writeln!(stderr, "    - {warning}").context("writing to stderr")?;
        }
    }
    writeln!(
        stderr,
        "  Note: Claude must reload its settings for Layer 2 (NATIVE tool denials) to take effect."
    )
    .context("writing to stderr")?;
    writeln!(
        stderr,
        "  Layer 1 (remargin's own ops) stops enforcing immediately on the next call."
    )
    .context("writing to stderr")?;
    Ok(())
}

fn cmd_prompt_resolve(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    file: &str,
    json_mode: bool,
) -> Result<()> {
    let target_buf = expand_cli_path(system, file)?;
    let absolute = if target_buf.is_absolute() {
        target_buf
    } else {
        cwd.join(&target_buf)
    };
    let resolved = config::system_prompt::resolve_system_prompt(system, &absolute)?;
    if json_mode {
        let value = serde_json::to_value(&resolved).context("serializing prompt_resolve output")?;
        return print_output(sinks, true, &value);
    }
    write_prompt_resolve_text(sinks, &absolute, &resolved)
}

fn write_prompt_resolve_text(
    sinks: &mut IoSinks<'_>,
    target: &Path,
    resolved: &config::system_prompt::ResolvedSystemPrompt,
) -> Result<()> {
    writeln!(sinks.stderr, "Resolved prompt for: {}", target.display())
        .context("writing to stderr")?;
    writeln!(sinks.stderr, "  Name:    {}", resolved.name).context("writing to stderr")?;
    match &resolved.source {
        Some(path) => writeln!(sinks.stderr, "  Source:  {}", path.display()),
        None => writeln!(sinks.stderr, "  Source:  (walk exhausted)"),
    }
    .context("writing to stderr")?;
    writeln!(
        sinks.stderr,
        "  Default: {}",
        if resolved.is_default { "yes" } else { "no" },
    )
    .context("writing to stderr")?;
    writeln!(
        sinks.stderr,
        "  Body ({} chars):",
        resolved.prompt.chars().count(),
    )
    .context("writing to stderr")?;
    if resolved.prompt.is_empty() {
        writeln!(sinks.stderr, "    (empty)").context("writing to stderr")?;
    } else {
        for line in resolved.prompt.lines() {
            writeln!(sinks.stderr, "    {line}").context("writing to stderr")?;
        }
    }
    Ok(())
}

fn cmd_prompt_set(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    params: &PromptSetParams<'_>,
) -> Result<()> {
    let absolute = absolute_folder(system, params.cwd, params.folder)?;
    let body = match params.prompt_flag {
        Some(p) => String::from(p),
        None => read_prompt_from_stdin()?,
    };
    if body.is_empty() {
        bail!("prompt body is required (pass --prompt or pipe via stdin)");
    }
    if params.name.is_empty() {
        bail!("--name is required");
    }
    let outcome =
        operations::prompt::set(system, &absolute, Some(params.name), &body, params.config)
            .with_context(|| format!("setting prompt at {}", absolute.display()))?;
    if params.json_mode {
        let value = serde_json::to_value(&outcome).context("serializing prompt_set output")?;
        return print_output(sinks, true, &value);
    }
    writeln!(
        sinks.stderr,
        "Prompt set at {} (created={}, noop={})",
        outcome.source.display(),
        outcome.created,
        outcome.noop,
    )
    .context("writing to stderr")?;
    Ok(())
}

fn cmd_prompt_delete(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    folder: &str,
    json_mode: bool,
) -> Result<()> {
    let absolute = absolute_folder(system, cwd, folder)?;
    let outcome = operations::prompt::delete(system, &absolute, config)
        .with_context(|| format!("deleting prompt at {}", absolute.display()))?;
    if json_mode {
        let value = serde_json::to_value(&outcome).context("serializing prompt_delete output")?;
        return print_output(sinks, true, &value);
    }
    if outcome.absent {
        writeln!(
            sinks.stderr,
            "No prompt to delete at {}",
            outcome.source.display(),
        )
        .context("writing to stderr")?;
    } else {
        writeln!(
            sinks.stderr,
            "Prompt deleted at {} (left_empty={})",
            outcome.source.display(),
            outcome.left_empty,
        )
        .context("writing to stderr")?;
    }
    Ok(())
}

fn cmd_prompt_list(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    folder: &str,
    json_mode: bool,
) -> Result<()> {
    let absolute = absolute_folder(system, cwd, folder)?;
    let entries = operations::prompt::list(system, &absolute)
        .with_context(|| format!("listing prompts under {}", absolute.display()))?;
    if json_mode {
        let value = serde_json::to_value(&entries).context("serializing prompt_list output")?;
        return print_output(sinks, true, &json!({ "entries": value }));
    }
    if entries.is_empty() {
        writeln!(
            sinks.stderr,
            "No declared prompts under {}",
            absolute.display(),
        )
        .context("writing to stderr")?;
        return Ok(());
    }
    for entry in &entries {
        let label = entry.name.as_deref().unwrap_or("(unnamed)");
        let chars = entry.prompt.chars().count();
        writeln!(
            sinks.stdout,
            "{}\t{}\t{} chars",
            entry.source.display(),
            label,
            chars,
        )
        .context("writing to stdout")?;
    }
    Ok(())
}

fn absolute_folder(system: &dyn System, cwd: &Path, folder: &str) -> Result<PathBuf> {
    let expanded = expand_cli_path(system, folder)?;
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };
    Ok(absolute)
}

fn read_prompt_from_stdin() -> Result<String> {
    use std::io::IsTerminal as _;
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(String::new());
    }
    let mut buf = String::new();
    stdin
        .lock()
        .read_to_string(&mut buf)
        .context("reading prompt body from stdin")?;
    Ok(String::from(buf.trim_end_matches('\n')))
}

fn cmd_resolve_mode(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    json_mode: bool,
) -> Result<()> {
    let resolved = config::resolve_mode(system, cwd)?;
    let source = resolved.source.as_ref().map(|p| p.display().to_string());
    let value = json!({
        "mode": resolved.mode.as_str(),
        "source": source,
    });
    if json_mode {
        print_output(sinks, true, &value)?;
    } else {
        writeln!(sinks.stderr, "Mode:   {}", resolved.mode.as_str())
            .context("writing to stderr")?;
        match &source {
            Some(path) => {
                writeln!(sinks.stderr, "Source: {path}").context("writing to stderr")?;
            }
            None => {
                writeln!(sinks.stderr, "Source: <default>").context("writing to stderr")?;
            }
        }
    }
    Ok(())
}

fn cmd_keygen(sinks: &mut IoSinks<'_>, system: &dyn System, output: &Path) -> Result<()> {
    use ssh_key::{Algorithm, LineEnding, PrivateKey, rand_core::OsRng};

    let private_key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
        .map_err(|err| anyhow::anyhow!("key generation failed: {err}"))?;

    let private_pem = private_key
        .to_openssh(LineEnding::LF)
        .map_err(|err| anyhow::anyhow!("encoding private key: {err}"))?;

    let public_key = private_key.public_key();
    let public_openssh = public_key
        .to_openssh()
        .map_err(|err| anyhow::anyhow!("encoding public key: {err}"))?;

    system
        .write(output, private_pem.as_bytes())
        .with_context(|| format!("writing private key to {}", output.display()))?;

    let pub_path = output.with_extension("pub");
    system
        .write(&pub_path, public_openssh.as_bytes())
        .with_context(|| format!("writing public key to {}", pub_path.display()))?;

    writeln!(sinks.stderr, "Private key: {}", output.display()).context("writing to stderr")?;
    writeln!(sinks.stderr, "Public key:  {}", pub_path.display()).context("writing to stderr")?;

    Ok(())
}

fn cmd_lint(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    file: &str,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let report = linter::lint_doc(system, &path)?;

    if json_mode {
        print_output(sinks, true, &report.to_json())?;
    } else {
        write!(sinks.stderr, "{}", report.format_text()).context("writing to stderr")?;
    }

    if !report.is_clean() {
        anyhow::bail!("Lint errors found");
    }
    Ok(())
}

fn cmd_ls(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    path_str: &str,
    json_mode: bool,
) -> Result<()> {
    let target_buf = expand_cli_path(system, path_str)?;
    let target = target_buf.as_path();
    let entries = document::ls(system, cwd, target, config)?;

    if json_mode {
        print_output(sinks, true, &json!({ "entries": entries }))
    } else {
        for entry in &entries {
            let kind = if entry.is_dir { "d" } else { "-" };
            let size_str = entry
                .size
                .map_or_else(|| String::from("-"), |s| format!("{s}"));
            let pending_str = entry
                .remargin_pending
                .map(|p| format!(" [{p} pending]"))
                .unwrap_or_default();
            out(
                sinks,
                &format!(
                    "{kind} {size_str:>8} {}{}",
                    entry.path.display(),
                    pending_str,
                ),
            )?;
        }
        Ok(())
    }
}

fn cmd_metadata(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    path_str: &str,
    json_mode: bool,
) -> Result<()> {
    let target_buf = expand_cli_path(system, path_str)?;
    let target = target_buf.as_path();
    let meta = document::metadata(
        system,
        cwd,
        target,
        config.unrestricted,
        &config.trusted_roots,
    )?;

    print_output(sinks, json_mode, &meta.to_json(false))
}

/// Route a `plan` subcommand to the correct per-op projection.
///
/// Lightweight ops that have not yet been wired surface a deliberate
/// "not yet landed" error so callers discover the subcommand tree and
/// failures are loud. `plan write` is fully wired.
fn cmd_plan(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    action: &PlanAction,
    json_mode: bool,
) -> Result<()> {
    // `Comment` / `Write` arms need owned buffers that outlive the
    // `PlanRequest` (it borrows `&str` / `ProjectCommentParams<'_>`).
    // Stage them here so the borrows survive through `plan_ops::dispatch`.
    // Initialized to empty defaults; the `Comment` / `Write` helpers
    // overwrite them in place before the borrow flows out.
    let mut comment_body = String::new();
    let mut write_body = String::new();
    let mut attach_refs: Vec<&str> = Vec::new();
    let mut position = InsertPosition::Append;

    let request = match action {
        PlanAction::Ack { .. } => build_plan_ack(action, system, cwd)?,
        PlanAction::Batch { .. } => build_plan_batch(action, system, cwd)?,
        PlanAction::Claude { action: claude, .. } => match claude {
            PlanClaudeAction::Restrict { .. } => build_plan_claude_restrict(claude, system, cwd)?,
            PlanClaudeAction::Unrestrict { .. } => build_plan_claude_unrestrict(claude, cwd)?,
        },
        PlanAction::Comment { .. } => build_plan_comment(
            action,
            system,
            cwd,
            &mut comment_body,
            &mut position,
            &mut attach_refs,
        )?,
        PlanAction::Delete { .. } => build_plan_delete(action, system, cwd)?,
        PlanAction::Edit { .. } => build_plan_edit(action, system, cwd)?,
        PlanAction::Mv { .. } => build_plan_mv(action, system)?,
        PlanAction::Purge { .. } => build_plan_purge(action, system, cwd)?,
        PlanAction::React { .. } => build_plan_react(action, system, cwd)?,
        PlanAction::SandboxAdd { .. } => build_plan_sandbox_add(action, system, cwd)?,
        PlanAction::SandboxRemove { .. } => build_plan_sandbox_remove(action, system, cwd)?,
        PlanAction::Sign { .. } => build_plan_sign(action, system, cwd)?,
        PlanAction::Write { .. } => build_plan_write(action, system, &mut write_body)?,
    };

    let report = plan_ops::dispatch(system, cwd, config, &request)?;
    let value = serde_json::to_value(&report).context("serializing plan report")?;

    // Config-mutation plans get a structured text block in text mode so
    // the multi-file projection is readable. JSON mode still emits the
    // full PlanReport payload.
    if !json_mode {
        if report.config_diff.is_some() {
            return emit_plan_restrict_text(sinks, &report);
        }
        if report.unprotect_diff.is_some() {
            return emit_plan_unprotect_text(sinks, &report);
        }
    }

    print_output(sinks, json_mode, &value)
}

fn build_plan_ack(
    action: &PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::Ack {
        path, ids, remove, ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Ack {
        path: resolve_doc_path(system, cwd, path)?,
        ids: ids.clone(),
        remove: *remove,
    })
}

fn build_plan_batch(
    action: &PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::Batch { path, ops_file, .. } = action else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Batch {
        path: resolve_doc_path(system, cwd, path)?,
        ops: read_plan_batch_ops(system, ops_file)?,
    })
}

fn build_plan_comment<'cmd>(
    action: &'cmd PlanAction,
    system: &dyn System,
    cwd: &Path,
    comment_body: &'cmd mut String,
    position: &'cmd mut InsertPosition,
    attach_refs: &'cmd mut Vec<&'cmd str>,
) -> Result<plan_ops::PlanRequest<'cmd>> {
    let PlanAction::Comment {
        path,
        content,
        after_comment,
        after_heading,
        after_line,
        attach_names,
        auto_ack,
        reply_to,
        sandbox,
        to,
        ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    let doc_path = resolve_doc_path(system, cwd, path)?;
    *comment_body = match content {
        Some(s) => s.clone(),
        None => read_stdin()?,
    };
    *position = resolve_comment_position(
        reply_to.as_deref(),
        after_comment.as_deref(),
        after_heading.as_deref(),
        *after_line,
    );
    *attach_refs = attach_names.iter().map(String::as_str).collect();
    let params = projections::ProjectCommentParams::new(comment_body, position)
        .with_attachment_filenames(attach_refs)
        .with_auto_ack(*auto_ack)
        .with_reply_to(reply_to.as_deref())
        .with_sandbox(*sandbox)
        .with_to(to);
    Ok(plan_ops::PlanRequest::Comment {
        path: doc_path,
        params,
    })
}

fn build_plan_delete(
    action: &PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::Delete { path, ids, .. } = action else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Delete {
        path: resolve_doc_path(system, cwd, path)?,
        ids: ids.clone(),
    })
}

fn build_plan_edit<'cmd>(
    action: &'cmd PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'cmd>> {
    let PlanAction::Edit {
        path, id, content, ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Edit {
        path: resolve_doc_path(system, cwd, path)?,
        id,
        content,
    })
}

fn build_plan_mv(
    action: &PlanAction,
    system: &dyn System,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::Mv {
        src, dst, force, ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Mv {
        src: expand_cli_path(system, src)?,
        dst: expand_cli_path(system, dst)?,
        force: *force,
    })
}

fn build_plan_purge(
    action: &PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::Purge {
        path, recursive, ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Purge {
        path: resolve_purge_path(system, cwd, path, *recursive)?,
        recursive: *recursive,
    })
}

fn build_plan_react<'cmd>(
    action: &'cmd PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'cmd>> {
    let PlanAction::React {
        path,
        id,
        emoji,
        remove,
        ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::React {
        path: resolve_doc_path(system, cwd, path)?,
        id,
        emoji,
        remove: *remove,
    })
}

/// Anchor-walk failure surfaces via the projection's reject path; on
/// that path we still produce a report rather than bail here. The
/// fallback project-scope path is unused on the reject branch.
fn build_plan_claude_restrict(
    action: &PlanClaudeAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanClaudeAction::Restrict {
        path,
        also_deny_bash,
        cli_allowed,
        user_settings,
        ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanClaudeAction variant");
    };
    let user_scope = match user_settings {
        Some(explicit) => expand_cli_pathbuf(system, explicit)?,
        None => expand_cli_path(system, DEFAULT_USER_SETTINGS)?,
    };
    let project_scope = permissions_restrict::find_claude_anchor(system, cwd).map_or_else(
        |_err| cwd.join(".claude/settings.local.json"),
        |anchor| anchor.join(".claude/settings.local.json"),
    );
    Ok(plan_ops::PlanRequest::Restrict {
        args: permissions_restrict::RestrictArgs::new(
            path.clone(),
            also_deny_bash.clone(),
            *cli_allowed,
        ),
        cwd: cwd.to_path_buf(),
        settings_files: vec![project_scope, user_scope],
    })
}

fn build_plan_sandbox_add(
    action: &PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::SandboxAdd { path, .. } = action else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::SandboxAdd {
        path: resolve_doc_path(system, cwd, path)?,
    })
}

fn build_plan_sandbox_remove(
    action: &PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::SandboxRemove { path, .. } = action else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::SandboxRemove {
        path: resolve_doc_path(system, cwd, path)?,
    })
}

fn build_plan_sign(
    action: &PlanAction,
    system: &dyn System,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanAction::Sign {
        path,
        ids,
        all_mine,
        ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    Ok(plan_ops::PlanRequest::Sign {
        path: resolve_doc_path(system, cwd, path)?,
        selection: build_sign_selection(*all_mine, ids)?,
    })
}

fn build_plan_claude_unrestrict(
    action: &PlanClaudeAction,
    cwd: &Path,
) -> Result<plan_ops::PlanRequest<'static>> {
    let PlanClaudeAction::Unrestrict { path, .. } = action else {
        bail!("internal: helper called with wrong PlanClaudeAction variant");
    };
    Ok(plan_ops::PlanRequest::Unprotect {
        args: permissions_unprotect::UnprotectArgs::new(path.clone()),
        cwd: cwd.to_path_buf(),
    })
}

fn build_plan_write<'cmd>(
    action: &PlanAction,
    system: &dyn System,
    write_body: &'cmd mut String,
) -> Result<plan_ops::PlanRequest<'cmd>> {
    let PlanAction::Write {
        path,
        content,
        binary,
        create,
        lines,
        raw,
        ..
    } = action
    else {
        bail!("internal: helper called with wrong PlanAction variant");
    };
    *write_body = match content {
        Some(s) => s.clone(),
        None => read_stdin()?,
    };
    let line_range = lines.as_deref().map(parse_line_range).transpose()?;
    let opts = document::WriteOptions::new()
        .binary(*binary)
        .create(*create)
        .lines(line_range)
        .raw(*raw);
    Ok(plan_ops::PlanRequest::Write {
        path: expand_cli_path(system, path)?,
        content: write_body,
        opts,
    })
}

/// Render a `plan restrict` [`PlanReport`] as a structured text block.
/// Mirrors the JSON shape: anchor + `would_commit`/`noop` header, one
/// section per touched file, then conflicts. Emitted on stdout via
/// the standard `out` helper so existing pipe-friendly behaviour is
/// preserved.
fn emit_plan_restrict_text(sinks: &mut IoSinks<'_>, report: &plan_ops::PlanReport) -> Result<()> {
    let Some(diff) = report.config_diff.as_ref() else {
        return Ok(());
    };
    out(
        sinks,
        &format!("Plan: restrict {}", diff.absolute_path.display()),
    )?;
    out(sinks, &format!("  Anchor: {}", diff.anchor.display()))?;
    out(
        sinks,
        &format!(
            "  noop: {}   would_commit: {}",
            report.noop, report.would_commit,
        ),
    )?;
    if let Some(reason) = &report.reject_reason {
        out(sinks, &format!("  reject_reason: {reason}"))?;
    }
    out(
        sinks,
        &format!("  .remargin.yaml: {}", diff.remargin_yaml.path.display()),
    )?;
    out(
        sinks,
        &format!(
            "    will be created: {}",
            diff.remargin_yaml.will_be_created
        ),
    )?;
    out(
        sinks,
        &format!(
            "    entry: {}",
            entry_action_label(diff.remargin_yaml.entry_action),
        ),
    )?;
    out(
        sinks,
        &format!("  Settings: {} file(s)", diff.settings_files.len()),
    )?;
    for sf in &diff.settings_files {
        out(sinks, &format!("    {}", sf.path.display()))?;
        out(
            sinks,
            &format!("      will be created: {}", sf.will_be_created),
        )?;
        out(
            sinks,
            &format!(
                "      deny rules: +{} to add, {} already present",
                sf.deny_rules_to_add.len(),
                sf.deny_rules_already_present.len(),
            ),
        )?;
        out(
            sinks,
            &format!(
                "      allow rules: +{} to add, {} already present",
                sf.allow_rules_to_add.len(),
                sf.allow_rules_already_present.len(),
            ),
        )?;
    }
    out(
        sinks,
        &format!(
            "  Sidecar: {} ({})",
            diff.sidecar.path.display(),
            entry_action_label(diff.sidecar.entry_action),
        ),
    )?;
    if diff.conflicts.is_empty() {
        out(sinks, "  conflicts: 0")?;
    } else {
        out(sinks, &format!("  conflicts: {}", diff.conflicts.len()))?;
        for conflict in &diff.conflicts {
            emit_conflict_line(sinks, conflict)?;
        }
    }
    Ok(())
}

const fn entry_action_label(action: plan_ops::EntryAction) -> &'static str {
    match action {
        plan_ops::EntryAction::Added => "added",
        plan_ops::EntryAction::Noop => "noop",
        plan_ops::EntryAction::Updated => "updated",
        // EntryAction is `#[non_exhaustive]`; cover future variants
        // gracefully without breaking the build.
        _ => "<unknown>",
    }
}

fn emit_conflict_line(sinks: &mut IoSinks<'_>, conflict: &plan_ops::ConfigConflict) -> Result<()> {
    match conflict {
        plan_ops::ConfigConflict::AllowDenyOverlap {
            allow_rule,
            overlap_kind,
            projected_deny_rule,
            settings_file,
        } => {
            let kind_label = match overlap_kind {
                OverlapKind::AllowShadowedByBroaderDeny => {
                    "existing allow is shadowed by broader projected deny"
                }
                OverlapKind::DenyShadowedByBroaderAllow => {
                    "projected deny is shadowed by broader existing allow"
                }
                OverlapKind::Exact => "exact",
                _ => "unknown overlap kind",
            };
            out(
                sinks,
                &format!(
                    "    allow_deny_overlap in {} ({kind_label}):",
                    settings_file.display()
                ),
            )?;
            out(sinks, &format!("      existing allow:  {allow_rule}"))?;
            out(
                sinks,
                &format!("      projected deny:  {projected_deny_rule}"),
            )?;
        }
        plan_ops::ConfigConflict::AnchorIsAncestor { anchor, cwd } => {
            out(
                sinks,
                &format!(
                    "    anchor_is_ancestor: cwd={} anchor={}",
                    cwd.display(),
                    anchor.display(),
                ),
            )?;
        }
        plan_ops::ConfigConflict::YamlEntryWouldChange {
            path,
            previous,
            projected,
        } => {
            out(sinks, &format!("    yaml_entry_would_change: path={path}"))?;
            out(
                sinks,
                &format!(
                    "      previous: also_deny_bash={:?} cli_allowed={}",
                    previous.also_deny_bash, previous.cli_allowed,
                ),
            )?;
            out(
                sinks,
                &format!(
                    "      projected: also_deny_bash={:?} cli_allowed={}",
                    projected.also_deny_bash, projected.cli_allowed,
                ),
            )?;
        }
        // ConfigConflict is `#[non_exhaustive]`; cover future variants
        // gracefully without breaking the build.
        _ => {
            out(sinks, "    <unknown conflict variant>")?;
        }
    }
    Ok(())
}

/// Render a `plan unprotect` [`PlanReport`] as a structured text block.
/// Symmetric mirror of [`emit_plan_restrict_text`] for the reverse
/// direction: anchor + `would_commit`/`noop` header, one section per
/// touched file, then drift conflicts.
fn emit_plan_unprotect_text(sinks: &mut IoSinks<'_>, report: &plan_ops::PlanReport) -> Result<()> {
    let Some(diff) = report.unprotect_diff.as_ref() else {
        return Ok(());
    };
    out(
        sinks,
        &format!("Plan: unprotect {}", diff.absolute_path.display()),
    )?;
    out(sinks, &format!("  Anchor: {}", diff.anchor.display()))?;
    out(
        sinks,
        &format!(
            "  noop: {}   would_commit: {}",
            report.noop, report.would_commit,
        ),
    )?;
    if let Some(reason) = &report.reject_reason {
        out(sinks, &format!("  reject_reason: {reason}"))?;
    }
    out(
        sinks,
        &format!("  .remargin.yaml: {}", diff.remargin_yaml.path.display()),
    )?;
    out(
        sinks,
        &format!(
            "    entry: {}",
            unprotect_entry_action_label(diff.remargin_yaml.entry_action),
        ),
    )?;
    out(
        sinks,
        &format!("  Settings: {} file(s)", diff.settings_files.len()),
    )?;
    for sf in &diff.settings_files {
        out(sinks, &format!("    {}", sf.path.display()))?;
        out(
            sinks,
            &format!(
                "      rules: -{} to remove, {} already absent",
                sf.rules_to_remove.len(),
                sf.rules_already_absent.len(),
            ),
        )?;
    }
    out(
        sinks,
        &format!(
            "  Sidecar: {} ({})",
            diff.sidecar.path.display(),
            unprotect_entry_action_label(diff.sidecar.entry_action),
        ),
    )?;
    if diff.conflicts.is_empty() {
        out(sinks, "  conflicts: 0")?;
    } else {
        out(sinks, &format!("  conflicts: {}", diff.conflicts.len()))?;
        for conflict in &diff.conflicts {
            emit_unprotect_conflict_line(sinks, conflict)?;
        }
    }
    Ok(())
}

const fn unprotect_entry_action_label(action: plan_ops::UnprotectEntryAction) -> &'static str {
    match action {
        plan_ops::UnprotectEntryAction::Absent => "absent",
        plan_ops::UnprotectEntryAction::WouldBeRemoved => "would_be_removed",
        // UnprotectEntryAction is `#[non_exhaustive]`; cover future
        // variants gracefully without breaking the build.
        _ => "<unknown>",
    }
}

fn emit_unprotect_conflict_line(
    sinks: &mut IoSinks<'_>,
    conflict: &plan_ops::UnprotectConflict,
) -> Result<()> {
    match conflict {
        plan_ops::UnprotectConflict::RuleAlreadyAbsent {
            rule,
            settings_file,
        } => {
            out(
                sinks,
                &format!(
                    "    rule_already_absent in {}: {rule}",
                    settings_file.display()
                ),
            )?;
        }
        plan_ops::UnprotectConflict::SidecarEntryMissing { path } => {
            out(
                sinks,
                &format!("    sidecar_entry_missing: {}", path.display()),
            )?;
        }
        plan_ops::UnprotectConflict::YamlEntryMissing { path } => {
            out(
                sinks,
                &format!("    yaml_entry_missing: {}", path.display()),
            )?;
        }
        // UnprotectConflict is `#[non_exhaustive]`; cover future
        // variants gracefully without breaking the build.
        _ => {
            out(sinks, "    <unknown conflict variant>")?;
        }
    }
    Ok(())
}

/// Read a JSON file (or stdin when `path == "-"`) into a vector of
/// [`projections::ProjectBatchOp`] values for `plan batch`.
fn read_plan_batch_ops(
    system: &dyn System,
    path: &str,
) -> Result<Vec<projections::ProjectBatchOp>> {
    let json_text = if path == "-" {
        read_stdin()?
    } else {
        system
            .read_to_string(Path::new(path))
            .with_context(|| format!("reading plan batch ops file {path}"))?
    };
    let raw: Value =
        serde_json::from_str(&json_text).context("parsing plan batch ops JSON body")?;
    let arr = raw
        .as_array()
        .context("plan batch ops JSON must be an array of objects")?;

    let mut ops: Vec<projections::ProjectBatchOp> = Vec::with_capacity(arr.len());
    for (idx, entry) in arr.iter().enumerate() {
        let obj = entry
            .as_object()
            .with_context(|| format!("plan batch op[{idx}]: expected object"))?;
        ops.push(projections::ProjectBatchOp::from_json_object(obj, idx)?);
    }
    Ok(ops)
}

fn cmd_purge(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    recursive: bool,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_purge_path(system, cwd, file, recursive)?;

    if recursive {
        let result = purge::purge_dir(system, &path, config)?;
        return print_output(sinks, json_mode, &result.to_json(cwd));
    }

    if system.is_dir(&path).unwrap_or(false) {
        anyhow::bail!(
            "target is a directory: {file} (pass --recursive to purge every .md file under it)"
        );
    }
    let result = purge::purge(system, &path, config)?;
    print_output(sinks, json_mode, &result.to_json())
}

fn cmd_query(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &QueryParams<'_>,
) -> Result<()> {
    let target = cwd.join(expand_cli_path(system, params.path)?);
    let filter = build_query_filter(config, params)?;
    let results = query::query(system, &target, &filter)?;
    render_query_output(sinks, &results, params, filter.pending_label())
}

fn build_query_filter(
    config: &ResolvedConfig,
    params: &QueryParams<'_>,
) -> Result<query::QueryFilter> {
    let since_dt = params
        .since
        .map(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .with_context(|| format!("invalid timestamp: {s}"))
        })
        .transpose()?;

    let mut filter = query::QueryFilter::default();
    filter.author = params.author.map(String::from);
    filter.comment_id = params.comment_id.map(String::from);
    filter.expanded = params.expanded;
    filter.pending = params.pending.any;
    filter.pending_for = params.pending.for_user.map(String::from);
    filter.remargin_kind = params.remargin_kind.to_vec();
    filter.since = since_dt;
    filter.summary = matches!(params.output, QueryOutputMode::Summary);
    filter = filter.with_caller_identity(
        params.pending.for_me,
        params.pending.broadcast,
        config.identity.clone(),
    )?;
    if let Some(pattern) = params.content_regex {
        filter = filter.with_content_regex(pattern, params.ignore_case)?;
    }
    Ok(filter)
}

fn render_query_output(
    sinks: &mut IoSinks<'_>,
    results: &[query::QueryResult],
    params: &QueryParams<'_>,
    pending_label: Option<&str>,
) -> Result<()> {
    match params.output {
        QueryOutputMode::Json => {
            return print_output(
                sinks,
                true,
                &json!({
                    "base_path": format!("{}/", params.path.trim_end_matches('/')),
                    "results": results,
                }),
            );
        }
        QueryOutputMode::Pretty => {
            return out_raw(sinks, &display::format_query_pretty(results, pending_label));
        }
        QueryOutputMode::Plain | QueryOutputMode::Summary => {}
    }
    for r in results {
        out(
            sinks,
            &format!(
                "{} ({} comments, {} pending)",
                r.path.display(),
                r.comment_count,
                r.pending_count,
            ),
        )?;
        for cm in r.comments.as_deref().unwrap_or(&[]) {
            let status = if cm.ack.is_empty() {
                "pending"
            } else {
                "acked"
            };
            out(
                sinks,
                &format!(
                    "  {} {} ({}) [{}] {}",
                    cm.id,
                    cm.author,
                    cm.author_type.as_str(),
                    status,
                    cm.content,
                ),
            )?;
        }
    }
    Ok(())
}

fn cmd_search(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    params: &SearchParams<'_>,
) -> Result<()> {
    let target = cwd.join(expand_cli_path(system, params.path)?);

    let scope = match params.scope {
        "body" => search::SearchScope::Body,
        "comments" => search::SearchScope::Comments,
        _ => search::SearchScope::All,
    };

    let options = search::SearchOptions::new(String::from(params.pattern))
        .context_lines(params.context)
        .ignore_case(params.ignore_case)
        .regex(params.regex)
        .scope(scope);

    let results = search::search(system, cwd, &target, &options)?;

    if params.json_mode {
        print_output(sinks, true, &json!({ "matches": results }))
    } else {
        for m in &results {
            let loc = match m.location {
                search::MatchLocation::Body => "body",
                search::MatchLocation::Comment => "comment",
                _ => "unknown",
            };
            for line in &m.before {
                out(sinks, &format!("  {line}"))?;
            }
            out(
                sinks,
                &format!("{}:{}  [{}]  {}", m.path.display(), m.line, loc, m.text),
            )?;
            for line in &m.after {
                out(sinks, &format!("  {line}"))?;
            }
        }
        Ok(())
    }
}

fn cmd_react(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &ReactParams<'_>,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, params.file)?;
    operations::react(
        system,
        &path,
        config,
        params.id,
        params.emoji,
        params.remove,
    )?;
    print_output(
        sinks,
        params.json_mode,
        &responses::react(params.emoji, params.id, params.remove),
    )
}

/// Render a single registry participant as a JSON object for
/// `remargin registry show --json`. `display_name` always appears;
/// when absent in the registry it falls back to the participant id
/// so clients never have to handle a null value.
fn registry_participant_json(
    name: &str,
    participant: &config::registry::RegistryParticipant,
) -> Value {
    let status = match participant.status {
        config::registry::RegistryParticipantStatus::Active => "active",
        config::registry::RegistryParticipantStatus::Revoked => "revoked",
        _ => "unknown",
    };
    let display_name = participant
        .display_name
        .clone()
        .unwrap_or_else(|| String::from(name));
    json!({
        "name": name,
        "display_name": display_name,
        "type": participant.author_type,
        "status": status,
        "pubkeys": participant.pubkeys.len(),
    })
}

/// Render a single registry participant as a one-line string for
/// `remargin registry show`. When a display name is set, the prefix
/// is `"Display Name" (id)`; otherwise it's the bare id.
fn registry_participant_pretty(
    name: &str,
    participant: &config::registry::RegistryParticipant,
) -> String {
    let status = match participant.status {
        config::registry::RegistryParticipantStatus::Active => "active",
        config::registry::RegistryParticipantStatus::Revoked => "revoked",
        _ => "unknown",
    };
    let prefix = participant.display_name.as_ref().map_or_else(
        || String::from(name),
        |display| format!("\"{display}\" ({name})"),
    );
    format!(
        "{prefix} ({}) [{status}] {} key(s)",
        participant.author_type,
        participant.pubkeys.len(),
    )
}

fn cmd_registry(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    action: &RegistryAction,
    json_mode: bool,
) -> Result<()> {
    match action {
        RegistryAction::Show => {
            let registry = config::load_registry(system, cwd)?.context("no registry found")?;

            if json_mode {
                let participants: Vec<Value> = registry
                    .participants
                    .iter()
                    .map(|(name, participant)| registry_participant_json(name, participant))
                    .collect();
                print_output(sinks, true, &json!({ "participants": participants }))
            } else {
                for (name, participant) in &registry.participants {
                    out(sinks, &registry_participant_pretty(name, participant))?;
                }
                Ok(())
            }
        }
    }
}

fn cmd_sandbox(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    action: &SandboxAction,
    json_mode: bool,
) -> Result<()> {
    let identity = config
        .identity
        .as_deref()
        .context("identity is required for sandbox operations")?;

    match action {
        SandboxAction::Add { files } => {
            let absolute: Vec<PathBuf> = files
                .iter()
                .map(|f| {
                    let expanded = expand_cli_pathbuf(system, f)?;
                    Ok::<PathBuf, anyhow::Error>(if expanded.is_absolute() {
                        expanded
                    } else {
                        cwd.join(expanded)
                    })
                })
                .collect::<Result<_>>()?;
            let result = sandbox_ops::add_to_files(system, &absolute, identity, config)?;
            emit_sandbox_bulk_result(sinks, &result, cwd, "added", json_mode)?;
            if result.failed.is_empty() {
                Ok(())
            } else {
                bail!("sandbox add: {} file(s) failed", result.failed.len())
            }
        }
        SandboxAction::Remove { files } => {
            let absolute: Vec<PathBuf> = files
                .iter()
                .map(|f| {
                    let expanded = expand_cli_pathbuf(system, f)?;
                    Ok::<PathBuf, anyhow::Error>(if expanded.is_absolute() {
                        expanded
                    } else {
                        cwd.join(expanded)
                    })
                })
                .collect::<Result<_>>()?;
            let result = sandbox_ops::remove_from_files(system, &absolute, identity, config)?;
            emit_sandbox_bulk_result(sinks, &result, cwd, "removed", json_mode)?;
            if result.failed.is_empty() {
                Ok(())
            } else {
                bail!("sandbox remove: {} file(s) failed", result.failed.len())
            }
        }
        SandboxAction::List { absolute, path } => {
            let root = match path.as_ref() {
                Some(p) => cwd.join(expand_cli_pathbuf(system, p)?),
                None => cwd.to_path_buf(),
            };
            let listings = sandbox_ops::list_for_identity(system, &root, identity)?;

            if json_mode {
                let items: Vec<Value> = listings
                    .iter()
                    .map(|l| {
                        let display_path = if *absolute {
                            l.path.display().to_string()
                        } else {
                            l.path
                                .strip_prefix(&root)
                                .unwrap_or(&l.path)
                                .display()
                                .to_string()
                        };
                        json!({
                            "path": display_path,
                            "since": l.since.to_rfc3339(),
                        })
                    })
                    .collect();
                out_json(sinks, &json!({ "files": items }))
            } else {
                for l in &listings {
                    let display_path = if *absolute {
                        l.path.display().to_string()
                    } else {
                        l.path
                            .strip_prefix(&root)
                            .unwrap_or(&l.path)
                            .display()
                            .to_string()
                    };
                    out(sinks, &display_path)?;
                }
                Ok(())
            }
        }
    }
}

fn emit_sandbox_bulk_result(
    sinks: &mut IoSinks<'_>,
    result: &sandbox_ops::SandboxBulkResult,
    cwd: &Path,
    changed_key: &str,
    json_mode: bool,
) -> Result<()> {
    if json_mode {
        out_json(sinks, &result.to_json(cwd, changed_key))?;
    } else {
        for p in &result.changed {
            out(sinks, &strip_prefix_display(p, cwd))?;
        }
        for failure in &result.failed {
            let _ = writeln!(
                sinks.stderr,
                "{}: {}",
                strip_prefix_display(&failure.path, cwd),
                failure.reason,
            );
        }
    }
    Ok(())
}

fn strip_prefix_display(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn cmd_mv(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &MvParams<'_>,
) -> Result<()> {
    let src = expand_cli_path(system, params.src)?;
    let dst = expand_cli_path(system, params.dst)?;

    let args = mv_op::MvArgs::new(src, dst).with_force(params.force);
    let outcome = mv_op::mv(system, cwd, config, &args)?;

    if params.json_mode {
        out_json(sinks, &outcome.to_json())
    } else {
        out(sinks, &mv_outcome_pretty(params.src, params.dst, &outcome))
    }
}

fn mv_outcome_pretty(src: &str, dst: &str, outcome: &mv_op::MvOutcome) -> String {
    let suffix_overwrite = if outcome.action.overwritten {
        ", overwrote destination"
    } else {
        ""
    };
    let suffix_fallback = if outcome.action.fallback_copy {
        ", cross-filesystem copy"
    } else {
        ""
    };
    if outcome.topology.noop_same_path {
        format!("no-op: {src} (same canonical path)")
    } else if outcome.topology.is_directory {
        format!(
            "renamed directory: {src} -> {dst} ({} nested file{}{suffix_overwrite}{suffix_fallback})",
            outcome.nested_files_moved,
            if outcome.nested_files_moved == 1 {
                ""
            } else {
                "s"
            },
        )
    } else if outcome.bytes_moved == 0 {
        format!(
            "already moved: {src} -> {dst} ({} bytes)",
            outcome.bytes_moved
        )
    } else {
        format!(
            "moved: {src} -> {dst} ({} bytes{suffix_overwrite}{suffix_fallback})",
            outcome.bytes_moved,
        )
    }
}

fn cmd_rm(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    json_mode: bool,
) -> Result<()> {
    let target = expand_cli_path(system, file)?;
    let result = document::rm(system, cwd, &target, config)?;

    if json_mode {
        out_json(sinks, &result.to_json(file))
    } else if result.existed {
        out(sinks, &format!("deleted: {file}"))
    } else {
        out(sinks, &format!("already absent: {file}"))
    }
}

/// Expand an optional `--vault-path` for the obsidian subcommand.
#[cfg(feature = "obsidian")]
fn expand_vault_path(system: &dyn System, vault_path: Option<&Path>) -> Result<Option<PathBuf>> {
    vault_path
        .map(|v| expand_cli_pathbuf(system, v))
        .transpose()
}

#[cfg(feature = "obsidian")]
fn cmd_obsidian(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    action: &ObsidianAction,
    json_mode: bool,
) -> Result<()> {
    match action {
        ObsidianAction::Install { vault_path } => {
            if !json_mode {
                writeln!(
                    sinks.stderr,
                    "Downloading remargin plugin v{} from GitHub Releases...",
                    obsidian::plugin_version()
                )
                .context("writing to stderr")?;
            }
            let expanded = expand_vault_path(system, vault_path.as_deref())?;
            let report = obsidian::install(system, cwd, expanded.as_deref())?;
            if json_mode {
                print_output(sinks, true, &report.to_json())
            } else {
                writeln!(sinks.stderr, "{}", report.to_text()).context("writing to stderr")?;
                Ok(())
            }
        }
        ObsidianAction::Uninstall { vault_path } => {
            let expanded = expand_vault_path(system, vault_path.as_deref())?;
            let status = obsidian::uninstall(system, cwd, expanded.as_deref())?;
            match status {
                obsidian::UninstallStatus::Removed { plugin_dir } => {
                    if json_mode {
                        print_output(
                            sinks,
                            true,
                            &json!({
                                "uninstalled": plugin_dir.display().to_string(),
                            }),
                        )
                    } else {
                        writeln!(
                            sinks.stderr,
                            "Uninstalled remargin plugin from {}",
                            plugin_dir.display()
                        )
                        .context("writing to stderr")?;
                        Ok(())
                    }
                }
                obsidian::UninstallStatus::NotInstalled { plugin_dir } => {
                    if json_mode {
                        print_output(
                            sinks,
                            true,
                            &json!({
                                "not_installed": plugin_dir.display().to_string(),
                            }),
                        )
                    } else {
                        writeln!(
                            sinks.stderr,
                            "remargin plugin not installed at {}",
                            plugin_dir.display()
                        )
                        .context("writing to stderr")?;
                        Ok(())
                    }
                }
            }
        }
    }
}

fn cmd_sign(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &SignParams<'_>,
) -> Result<()> {
    let SignParams {
        all_mine,
        file,
        ids,
        json_mode,
        repair_checksum,
    } = *params;
    let selection = build_sign_selection(all_mine, ids)?;
    let path = resolve_doc_path(system, cwd, file)?;
    let mut options = operations::sign::SignOptions::default();
    options.repair_checksum = repair_checksum;
    let result = operations::sign::sign_comments(system, &path, config, &selection, options)?;
    if json_mode {
        print_output(sinks, true, &result.to_json())
    } else {
        render_sign_result_text(sinks, &result)
    }
}

fn build_sign_selection(all_mine: bool, ids: &[String]) -> Result<operations::sign::SignSelection> {
    if !all_mine && ids.is_empty() {
        bail!("sign: pass --ids <ID[,ID...]> or --all-mine");
    }
    Ok(if all_mine {
        operations::sign::SignSelection::AllMine
    } else {
        operations::sign::SignSelection::Ids(ids.to_vec())
    })
}

fn render_sign_result_text(
    sinks: &mut IoSinks<'_>,
    result: &operations::sign::SignResult,
) -> Result<()> {
    for entry in &result.repaired {
        out(
            sinks,
            &format!(
                "repaired checksum: {} ({} -> {})",
                entry.id, entry.old_checksum, entry.new_checksum
            ),
        )?;
    }
    for entry in &result.signed {
        out(sinks, &format!("signed: {} (ts={})", entry.id, entry.ts))?;
    }
    for entry in &result.skipped {
        out(sinks, &format!("skipped: {} ({})", entry.id, entry.reason))?;
    }
    if result.signed.is_empty() && result.skipped.is_empty() && result.repaired.is_empty() {
        out(sinks, "no candidates")?;
    }
    Ok(())
}

fn cmd_skill(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    action: &SkillAction,
    json_mode: bool,
) -> Result<()> {
    match action {
        SkillAction::Install { global } => {
            let path = skill::install(system, *global)?;
            if json_mode {
                print_output(
                    sinks,
                    true,
                    &json!({ "installed": path.display().to_string() }),
                )
            } else {
                writeln!(sinks.stderr, "Skill installed to {}", path.display())
                    .context("writing to stderr")?;
                Ok(())
            }
        }
        SkillAction::Test { global } => {
            let status = skill::test_status(system, *global)?;
            let status_str = match status {
                skill::SkillStatus::NotInstalled => "not_installed",
                skill::SkillStatus::Outdated => "outdated",
                skill::SkillStatus::UpToDate => "up_to_date",
                _ => "unknown",
            };
            if json_mode {
                print_output(sinks, true, &json!({ "status": status_str }))
            } else {
                writeln!(sinks.stderr, "Skill status: {status_str}")
                    .context("writing to stderr")?;
                Ok(())
            }
        }
        SkillAction::Uninstall { global } => {
            skill::uninstall(system, *global)?;
            if json_mode {
                print_output(sinks, true, &json!({ "uninstalled": true }))
            } else {
                writeln!(sinks.stderr, "Skill uninstalled.").context("writing to stderr")?;
                Ok(())
            }
        }
    }
}

fn cmd_mcp(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    startup_flags: &IdentityFlags,
    startup_assets_dir: Option<&str>,
    mcp_action: Option<&McpAction>,
    json_mode: bool,
) -> Result<()> {
    use std::process::Command;

    // Default to Run when no subcommand given (bare `remargin mcp`).
    match mcp_action {
        None | Some(McpAction::Run) => mcp::run(system, cwd, startup_flags, startup_assets_dir),
        Some(McpAction::Install { user }) => {
            let bin = env::current_exe().context("resolving remargin binary path")?;
            let bin_str = bin.display().to_string();
            let scope = if *user { "user" } else { "project" };

            // Remove first to make the operation idempotent.
            let _: Result<_, _> = Command::new("claude")
                .args(["mcp", "remove", "remargin"])
                .output()
                .map(drop);

            let output = Command::new("claude")
                .args(["mcp", "add", "remargin", "-s", scope, "--", &bin_str, "mcp"])
                .output()
                .context("running 'claude mcp add' -- is Claude Code CLI installed?")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("claude mcp add failed: {stderr}");
            }

            if json_mode {
                print_output(
                    sinks,
                    true,
                    &json!({
                        "installed": true,
                        "scope": scope,
                        "binary": bin_str,
                    }),
                )
            } else {
                writeln!(
                    sinks.stderr,
                    "MCP server registered ({scope} scope): {bin_str}"
                )
                .context("writing to stderr")?;
                Ok(())
            }
        }
        Some(McpAction::Uninstall) => {
            let output = Command::new("claude")
                .args(["mcp", "remove", "remargin"])
                .output()
                .context("running 'claude mcp remove' -- is Claude Code CLI installed?")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("claude mcp remove failed: {stderr}");
            }

            if json_mode {
                print_output(sinks, true, &json!({ "uninstalled": true }))
            } else {
                writeln!(sinks.stderr, "MCP server unregistered.").context("writing to stderr")?;
                Ok(())
            }
        }
        Some(McpAction::Test) => {
            let output = Command::new("claude")
                .args(["mcp", "list"])
                .output()
                .context("running 'claude mcp list' -- is Claude Code CLI installed?")?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let registered = stdout.lines().any(|l| l.contains("remargin"));
            let status_str = if registered {
                "registered"
            } else {
                "not_registered"
            };

            if json_mode {
                print_output(sinks, true, &json!({ "status": status_str }))
            } else {
                writeln!(sinks.stderr, "MCP status: {status_str}").context("writing to stderr")?;
                Ok(())
            }
        }
    }
}

fn cmd_verify(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    file: &str,
    config: &ResolvedConfig,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let report = operations::verify::verify_and_refresh(system, &path, config)?;

    if json_mode {
        print_output(sinks, true, &report.to_json())?;
    } else {
        for row in &report.results {
            let chk = if row.checksum_ok { "ok" } else { "FAIL" };
            out(
                sinks,
                &format!(
                    "{}: checksum={} signature={}",
                    row.id,
                    chk,
                    row.signature.as_str(),
                ),
            )?;
        }
    }

    if report.ok {
        Ok(())
    } else {
        anyhow::bail!("integrity check failed");
    }
}

fn cmd_write(
    sinks: &mut IoSinks<'_>,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    wp: &WriteParams<'_>,
) -> Result<()> {
    let target_buf = expand_cli_path(system, wp.path)?;
    let target = target_buf.as_path();

    let body = match wp.content {
        Some(s) => String::from(s),
        None => read_stdin()?,
    };

    let outcome = document::write(system, cwd, target, &body, config, wp.opts)?;

    // A no-op prints a one-line human message in text mode instead of
    // the usual "written: ... / binary: ... / raw: ..." block; JSON mode
    // still returns a single payload, now with `noop: true` alongside
    // the existing fields so callers can branch on it.
    if outcome.noop && !wp.json_mode {
        return out(
            sinks,
            &format!("{}: no changes (already up to date)", wp.path),
        );
    }

    print_output(
        sinks,
        wp.json_mode,
        &outcome.to_json(wp.path, wp.opts.binary, wp.opts.raw),
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use os_shim::mock::MockSystem;
    use remargin_core::config;
    use remargin_core::config::registry::Registry;
    use serde_json::json;

    use super::{
        format_activity_cutoff_header, parse_line_range, registry_participant_json,
        registry_participant_pretty, resolve_comment_content,
    };

    fn ts(s: &str) -> chrono::DateTime<chrono::FixedOffset> {
        chrono::DateTime::parse_from_rfc3339(s).unwrap()
    }

    /// implicit cutoff with a caller-last-action ts
    /// renders as "(since you last touched this file: …)".
    #[test]
    fn cutoff_header_implicit_with_last_action() {
        let header = format_activity_cutoff_header(false, Some(ts("2026-04-27T02:09:00-04:00")));
        assert_eq!(
            header,
            "(since you last touched this file: 2026-04-27 02:09)"
        );
    }

    /// implicit cutoff with no prior activity renders
    /// the initial-touch fallback message.
    #[test]
    fn cutoff_header_implicit_initial_touch() {
        let header = format_activity_cutoff_header(false, None);
        assert!(
            header.contains("since the beginning"),
            "unexpected header: {header}"
        );
        assert!(
            header.contains("no prior activity"),
            "unexpected header: {header}"
        );
    }

    /// explicit `--since` echoes the cutoff with the
    /// "(since …)" wording, matching the user's input.
    #[test]
    fn cutoff_header_explicit_since() {
        let header = format_activity_cutoff_header(true, Some(ts("2026-04-27T02:09:00-04:00")));
        assert_eq!(header, "(since 2026-04-27 02:09)");
    }

    /// the placeholder string `YOUR-LAST-ACTION` from
    /// the design discussion must never reach user-visible output.
    #[test]
    fn cutoff_header_never_emits_placeholder() {
        for explicit in [true, false] {
            let with_ts =
                format_activity_cutoff_header(explicit, Some(ts("2026-04-27T02:09:00-04:00")));
            let without_ts = format_activity_cutoff_header(explicit, None);
            assert!(!with_ts.contains("YOUR-LAST-ACTION"), "{with_ts}");
            assert!(!without_ts.contains("YOUR-LAST-ACTION"), "{without_ts}");
        }
    }

    #[test]
    fn parse_line_range_accepts_simple_pair() {
        let (s, e) = parse_line_range("10-20").unwrap();
        assert_eq!((s, e), (10, 20));
    }

    #[test]
    fn parse_line_range_accepts_single_line_range() {
        let (s, e) = parse_line_range("7-7").unwrap();
        assert_eq!((s, e), (7, 7));
    }

    #[test]
    fn parse_line_range_rejects_missing_dash() {
        let err = parse_line_range("100").unwrap_err();
        assert!(err.to_string().contains("START-END"));
    }

    #[test]
    fn parse_line_range_rejects_non_numeric() {
        let err = parse_line_range("a-b").unwrap_err();
        assert!(err.to_string().contains("invalid start value"));
    }

    #[test]
    fn parse_line_range_rejects_non_numeric_end() {
        let err = parse_line_range("1-b").unwrap_err();
        assert!(err.to_string().contains("invalid end value"));
    }

    #[test]
    fn content_from_positional_arg() {
        let system = MockSystem::new();
        let cwd = Path::new("/project");
        let content = String::from("Hello from arg");

        let result = resolve_comment_content(&system, cwd, Some(&content), None).unwrap();
        assert_eq!(result, "Hello from arg");
    }

    #[test]
    fn content_from_file() {
        let system = MockSystem::new()
            .with_file(Path::new("/project/comment.txt"), b"Hello from file")
            .unwrap();
        let cwd = Path::new("/project");
        let path = PathBuf::from("comment.txt");

        let result = resolve_comment_content(&system, cwd, None, Some(&path)).unwrap();
        assert_eq!(result, "Hello from file");
    }

    #[test]
    fn content_from_absolute_file_path() {
        let system = MockSystem::new()
            .with_file(Path::new("/elsewhere/note.md"), b"Absolute path content")
            .unwrap();
        let cwd = Path::new("/project");
        let path = PathBuf::from("/elsewhere/note.md");

        let result = resolve_comment_content(&system, cwd, None, Some(&path)).unwrap();
        assert_eq!(result, "Absolute path content");
    }

    #[test]
    fn error_when_neither_content_nor_file() {
        let system = MockSystem::new();
        let cwd = Path::new("/project");

        let err = resolve_comment_content(&system, cwd, None, None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("comment body required"),
            "unexpected error: {msg}",
        );
    }

    #[test]
    fn error_when_file_not_found() {
        let system = MockSystem::new();
        let cwd = Path::new("/project");
        let path = PathBuf::from("missing.txt");

        let err = resolve_comment_content(&system, cwd, None, Some(&path)).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("reading comment body from"),
            "unexpected error: {msg}",
        );
    }

    fn registry_with_yaml(yaml: &str) -> Registry {
        let system = MockSystem::new()
            .with_file(
                Path::new("/project/.remargin-registry.yaml"),
                yaml.as_bytes(),
            )
            .unwrap();
        config::load_registry(&system, Path::new("/project"))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn registry_json_includes_display_name_when_set() {
        let registry = registry_with_yaml(
            "\
participants:
  alice:
    display_name: \"Alice Doe\"
    type: human
    status: active
    pubkeys:
      - \"ssh-ed25519 AAAA...\"
",
        );
        let alice = &registry.participants["alice"];
        let value = registry_participant_json("alice", alice);
        assert_eq!(
            value,
            json!({
                "name": "alice",
                "display_name": "Alice Doe",
                "type": "human",
                "status": "active",
                "pubkeys": 1_u64,
            })
        );
    }

    #[test]
    fn registry_json_falls_back_to_name_when_display_name_absent() {
        let registry = registry_with_yaml(
            "\
participants:
  ci-bot:
    type: agent
    status: active
    pubkeys: []
",
        );
        let bot = &registry.participants["ci-bot"];
        let value = registry_participant_json("ci-bot", bot);
        assert_eq!(
            value,
            json!({
                "name": "ci-bot",
                "display_name": "ci-bot",
                "type": "agent",
                "status": "active",
                "pubkeys": 0_u64,
            })
        );
    }

    #[test]
    fn registry_pretty_with_display_name() {
        let registry = registry_with_yaml(
            "\
participants:
  alice:
    display_name: \"Alice Doe\"
    type: human
    status: active
    pubkeys:
      - \"ssh-ed25519 AAAA...\"
",
        );
        let alice = &registry.participants["alice"];
        assert_eq!(
            registry_participant_pretty("alice", alice),
            "\"Alice Doe\" (alice) (human) [active] 1 key(s)",
        );
    }

    #[test]
    fn registry_pretty_without_display_name() {
        let registry = registry_with_yaml(
            "\
participants:
  alice:
    type: human
    status: active
    pubkeys:
      - \"ssh-ed25519 AAAA...\"
",
        );
        let alice = &registry.participants["alice"];
        assert_eq!(
            registry_participant_pretty("alice", alice),
            "alice (human) [active] 1 key(s)",
        );
    }
}
