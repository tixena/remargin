//! Remargin CLI binary.

#[cfg(feature = "obsidian")]
mod obsidian;

use std::env;
use std::fs;
use std::io::{self, Read as _, Write as _};
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
use remargin_core::config::identity::IdentityFlags;
use remargin_core::config::{self, ResolvedConfig};
use remargin_core::display;
use remargin_core::document;
use remargin_core::kind::matches_kind_filter;
use remargin_core::linter;
use remargin_core::mcp;
use remargin_core::operations;
use remargin_core::operations::batch::BatchCommentOp;
use remargin_core::operations::migrate;
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
use remargin_core::permissions::restrict as permissions_restrict;
use remargin_core::permissions::unprotect as permissions_unprotect;
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
/// Gitignore-style "no match" sentinel returned by
/// `permissions check` when the path is unrestricted (rem-yj1j.7 / T28).
/// Numerically equal to [`EXIT_ERROR`] so existing tooling that branches
/// on `1 vs 0` still works; the `main` harness recognises the sentinel
/// to skip the "error: ..." render that would otherwise prepend the
/// gitignore-style result.
const EXIT_NOT_RESTRICTED: u8 = 1;
/// Internal marker substring used by [`cmd_permissions`] to communicate
/// "not restricted" to [`classify_error`] without leaking through
/// stderr.
const PERMISSIONS_NOT_RESTRICTED_MARKER: &str = "__remargin_permissions_check_not_restricted__";

/// Default user-scope settings file used by `remargin restrict`.
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

/// Per-subcommand identity group (rem-zlx3).
///
/// Flattened only into subcommands that resolve an author identity
/// (comment, edit, ack, react, sign, write, delete, batch, purge,
/// migrate, plan, verify, sandbox, mcp). Read-only / utility
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

/// Per-subcommand output group (rem-zlx3).
///
/// Controls how the subcommand renders its result. Flattened into
/// every subcommand that emits a payload. Unlike the old
/// `GlobalFlags`, these flags are scoped to the subcommand — this
/// matches the "per-concern, per-subcommand" structure the rest of
/// the refactor establishes. Invocations that pre-rem-zlx3 placed
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

/// Per-subcommand `--assets-dir` flag (rem-zlx3).
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

/// Per-subcommand unrestricted escape hatch (rem-zlx3).
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
    /// Show "what's new since X" across managed `.md` files
    /// (rem-g3sy.4 / T34).
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
        /// path (rem-5oqx). Setext (underline) headings are NOT
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
        /// use the forthcoming rem-u8br tag editor to drop entries.
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
    /// pre-rem-8cnc diagnostic surface that tooling (Obsidian plugin,
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
    /// resolver every mutating subcommand uses (see rem-58d6):
    /// `--config` (branch 1), manual `--identity/--type/--key`
    /// (branch 2), or walk-up (branch 3).
    Identity {
        /// Subcommand. Omit to invoke `show` (backward-compatible
        /// with the pre-rem-8cnc surface).
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
    /// Convert old-format comments to remargin format.
    ///
    /// Two optional per-role identity flags let strict-mode docs migrate
    /// successfully: each `--*-config` points at a `.remargin.yaml`
    /// declaring a complete identity (author + signing key) used to
    /// attribute and sign migrated comments of the matching legacy
    /// role. Without them, migrated comments fall back to the historical
    /// `legacy-user` / `legacy-agent` placeholder with no signature —
    /// fine in open mode, rejected by the verify gate in strict mode.
    Migrate {
        /// Path to the document.
        file: String,
        /// Create a .bak backup before modifying.
        #[arg(long)]
        backup: bool,
        /// Path to a `.remargin.yaml` whose identity is used for
        /// migrated `user comments` blocks (author + signing key).
        #[arg(long)]
        human_config: Option<PathBuf>,
        /// Path to a `.remargin.yaml` whose identity is used for
        /// migrated `agent comments` blocks (author + signing key).
        #[arg(long)]
        agent_config: Option<PathBuf>,
        #[command(flatten)]
        identity_args: IdentityArgs,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Move or rename a single tracked file (rem-0j2x / T44).
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
    /// Install or uninstall the embedded Obsidian plugin in a vault.
    #[cfg(feature = "obsidian")]
    Obsidian {
        #[command(subcommand)]
        action: ObsidianAction,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Inspect the resolved permissions for the current directory
    /// (rem-yj1j.7 / T28).
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
    /// Structured pre-commit prediction for a mutating op (rem-bhk).
    ///
    /// Per-op subcommand routing wires this to the in-memory projection
    /// of each mutating op. This crate ships the shared shape +
    /// subcommand tree (rem-2qr); individual op wiring lands in
    /// rem-imc, rem-3uo, rem-qll.
    ///
    /// Identity is flattened on the parent so every projection inherits
    /// the same `--identity` / `--type` / `--config` / `--key`. Output
    /// flags, by contrast, belong on each sub-action so `remargin plan
    /// <op> … --json` parses cleanly (rem-zlx3).
    Plan {
        /// Which mutating op to plan.
        #[command(subcommand)]
        action: PlanAction,
        #[command(flatten)]
        identity_args: IdentityArgs,
    },
    /// Strip all comments from a document.
    Purge {
        /// Path to the document.
        file: String,
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
        /// at all) shapes — fixed in rem-4j91.
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
    /// Restrict an agent-edit subpath (rem-yj1j.5 / T26).
    ///
    /// Adds a `permissions.restrict` entry to the nearest
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
        /// Both forms are equivalent (rem-ss9s).
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
    /// identity (rem-1ec).
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
    /// Reverse a previous `restrict` (rem-yj1j.6 / T27).
    ///
    /// Removes the matching `permissions.restrict` entry from the
    /// nearest `.claude/`-bearing ancestor's `.remargin.yaml` AND
    /// scrubs the sidecar-tracked rules from both Claude settings
    /// files. Idempotent. Surfaces manual-edit divergences as
    /// warnings (never errors).
    ///
    /// No identity flags — symmetric with `restrict`.
    Unprotect {
        /// Subpath to unprotect (matches the on-disk `path` field of
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

/// Registry subcommands.
/// Plan subcommands (rem-bhk). One variant per mutating op; per-op
/// wiring is tracked under rem-imc / rem-3uo / rem-qll.
#[derive(clap::Subcommand)]
enum PlanAction {
    /// Project an `ack` op (rem-3uo).
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
    /// Project a `batch` op (rem-qll).
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
    /// Project a `comment` creation op (rem-3fp).
    Comment {
        /// Path to the document.
        path: String,
        /// Comment body text (read from stdin if omitted).
        content: Option<String>,
        /// Insert after this comment ID.
        #[arg(long, conflicts_with_all = ["after_heading", "after_line"])]
        after_comment: Option<String>,
        /// Project insertion after the ATX heading addressed by this
        /// `>`-separated path (rem-5oqx). Setext (underline) headings
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
    /// Project a `delete` op (rem-3uo).
    Delete {
        /// Path to the document.
        path: String,
        /// Comment IDs to delete.
        #[arg(required = true)]
        ids: Vec<String>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project an `edit` op (rem-3fp).
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
    /// Project a `migrate` op (rem-qll).
    Migrate {
        /// Path to the document.
        path: String,
        /// Path to a `.remargin.yaml` whose identity is used for
        /// migrated `user comments` blocks (author + signing key).
        #[arg(long)]
        human_config: Option<PathBuf>,
        /// Path to a `.remargin.yaml` whose identity is used for
        /// migrated `agent comments` blocks (author + signing key).
        #[arg(long)]
        agent_config: Option<PathBuf>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project an `mv` op (rem-0j2x / T44).
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
    /// Project a `purge` op (rem-qll).
    Purge {
        /// Path to the document.
        path: String,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project a `react` op (rem-3uo).
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
    /// Project a `restrict` op (rem-puy5).
    ///
    /// Mirrors `remargin restrict` arg-for-arg: the projection
    /// describes every file the live op would touch
    /// (`.remargin.yaml`, project + user settings, sidecar) plus
    /// any detectable conflicts (allow-vs-deny overlap, anchor
    /// surprise, YAML entry shape change). No flags are consumed
    /// or written.
    Restrict {
        /// Subpath relative to the anchor, OR the literal `*` for
        /// realm-wide. Same shape as `remargin restrict`.
        path: String,
        /// Extra Bash commands to deny on the restricted path,
        /// layered on top of the broad default deny list (rem-p74a).
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
    /// Project a `sandbox add` op (rem-qll).
    SandboxAdd {
        /// Path to the document.
        path: String,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project a `sandbox remove` op (rem-qll).
    SandboxRemove {
        /// Path to the document.
        path: String,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project a `sign` op (rem-7y3).
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
    /// Project an `unprotect` op (rem-6eop / T43).
    ///
    /// Symmetric mirror of `plan restrict` for the reverse direction:
    /// describes every file the live op would touch
    /// (`.remargin.yaml`, project + user settings, sidecar) plus
    /// every detectable drift conflict (manual edits, missing
    /// entries). No flags are consumed or written.
    Unprotect {
        /// Subpath relative to the anchor (matches the on-disk
        /// `path` field of the original restrict entry), OR the
        /// literal `*` for realm-wide. Same shape as `remargin
        /// unprotect`.
        path: String,
        /// User-scope settings file. Defaults to
        /// `~/.claude/settings.json`. Symmetric with `restrict` /
        /// `unprotect` for hermetic test runs. Accepted for
        /// surface symmetry but not consulted by the projection
        /// (the sidecar's `added_to_files` list pins the actual
        /// targets).
        #[arg(long)]
        user_settings: Option<PathBuf>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Project a `write` op (rem-imc).
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

#[derive(clap::Subcommand)]
enum RegistryAction {
    /// Show the current registry.
    Show,
}

/// `remargin permissions` subcommands (rem-yj1j.7 / T28).
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

/// `remargin identity` subcommands (rem-8cnc). Default action
/// (no subcommand) is `show` — the pre-rem-8cnc diagnostic surface.
#[derive(clap::Subcommand)]
enum IdentityAction {
    /// Print a ready-to-use identity YAML block to stdout. Users
    /// redirect to `.remargin.yaml` themselves (no `--write` flag —
    /// rem-is4z bans writes to `.remargin.yaml`).
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
    /// Resolve and print the effective identity (pre-rem-8cnc
    /// behavior). Kept as an explicit alternative to the bare
    /// `remargin identity` form.
    Show {
        #[command(flatten)]
        identity_args: IdentityArgs,
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

#[expect(
    clippy::struct_excessive_bools,
    reason = "CLI flags are naturally boolean"
)]
struct QueryParams<'cmd> {
    author: Option<&'cmd str>,
    comment_id: Option<&'cmd str>,
    content_regex: Option<&'cmd str>,
    expanded: bool,
    ignore_case: bool,
    json_mode: bool,
    path: &'cmd str,
    pending: bool,
    pending_broadcast: bool,
    pending_for: Option<&'cmd str>,
    pending_for_me: bool,
    pretty: bool,
    remargin_kind: &'cmd [String],
    since: Option<&'cmd str>,
    summary: bool,
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

fn out(msg: &str) -> Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{msg}").context("writing to stdout")
}

fn out_raw(msg: &str) -> Result<()> {
    let mut stdout = io::stdout().lock();
    write!(stdout, "{msg}").context("writing to stdout")
}

/// Decorates object payloads with an `elapsed_ms` field so every `--json`
/// response carries timing info.
fn out_json(value: &Value) -> Result<()> {
    let decorated = inject_elapsed_ms(value);
    out(&serde_json::to_string_pretty(&decorated).unwrap_or_default())
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

fn print_output(json_mode: bool, value: &Value) -> Result<()> {
    if json_mode {
        out_json(value)
    } else {
        print_text_output(value)
    }
}

fn print_text_output(value: &Value) -> Result<()> {
    match value {
        Value::String(s) => out(s),
        Value::Object(map) => {
            for (key, val) in map {
                if let Value::Array(arr) = val {
                    out(&format!("{key}:"))?;
                    for item in arr {
                        out(&format!("  {item}"))?;
                    }
                } else {
                    out(&format!("{key}: {val}"))?;
                }
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => {
            out(&value.to_string())
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
        let temp_path = env::temp_dir().join("remargin-stdin.md");
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

/// Parse the `--lines START-END` argument used by `remargin write` (rem-24p).
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
        | Commands::Migrate { output_args, .. }
        | Commands::Mv { output_args, .. }
        | Commands::Purge { output_args, .. }
        | Commands::Query { output_args, .. }
        | Commands::React { output_args, .. }
        | Commands::Registry { output_args, .. }
        | Commands::ResolveMode { output_args, .. }
        | Commands::Restrict { output_args, .. }
        | Commands::Rm { output_args, .. }
        | Commands::Sandbox { output_args, .. }
        | Commands::Search { output_args, .. }
        | Commands::Sign { output_args, .. }
        | Commands::Skill { output_args, .. }
        | Commands::Unprotect { output_args, .. }
        | Commands::Verify { output_args, .. }
        | Commands::Write { output_args, .. } => Some(output_args),
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { output_args, .. } => Some(output_args),
        Commands::Permissions { action } => Some(permissions_action_output(action)),
        Commands::Plan { action, .. } => Some(plan_action_output(action)),
        Commands::Version => None,
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
/// Every plan sub-action flattens an `OutputArgs` (rem-zlx3).
const fn plan_action_output(action: &PlanAction) -> &OutputArgs {
    match action {
        PlanAction::Ack { output_args, .. }
        | PlanAction::Batch { output_args, .. }
        | PlanAction::Comment { output_args, .. }
        | PlanAction::Delete { output_args, .. }
        | PlanAction::Edit { output_args, .. }
        | PlanAction::Migrate { output_args, .. }
        | PlanAction::Mv { output_args, .. }
        | PlanAction::Purge { output_args, .. }
        | PlanAction::React { output_args, .. }
        | PlanAction::Restrict { output_args, .. }
        | PlanAction::SandboxAdd { output_args, .. }
        | PlanAction::SandboxRemove { output_args, .. }
        | PlanAction::Sign { output_args, .. }
        | PlanAction::Unprotect { output_args, .. }
        | PlanAction::Write { output_args, .. } => output_args,
    }
}

fn main() -> ExitCode {
    // Capture the start time before parsing so `elapsed_ms` includes clap's
    // argument-parsing overhead.
    let _: Result<_, _> = START_TIME.set(Instant::now());

    let cli = Cli::parse();

    let output = subcommand_output(&cli.command);
    let verbose = output.is_some_and(|o| o.verbose);
    let json_mode = output.is_some_and(|o| o.json);

    if verbose {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive(tracing::Level::DEBUG.into()),
            )
            .with_writer(io::stderr)
            .init();
    }

    let system = RealSystem::new();
    let cwd = match system.current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("error: could not determine current directory: {err}");
            return ExitCode::from(EXIT_ERROR);
        }
    };

    // Non-JSON mode does not emit a timing footer on any stream (rem-26w):
    // stdout stays pure command output and stderr stays clean. The timing
    // value survives as `elapsed_ms` inside the JSON payload (rem-4ay).
    match run(&cli, &system, &cwd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let err_msg = format!("{err:#}");
            let is_silent_sentinel = err_msg.contains(PERMISSIONS_NOT_RESTRICTED_MARKER);
            let exit_code = classify_error(&err);
            if is_silent_sentinel {
                // Sentinel for `permissions check` (rem-yj1j.7 / T28).
                // Output already emitted on the success path; we only
                // need the gitignore-style exit code, no "error: ..."
                // render.
            } else if json_mode {
                let error_json = inject_elapsed_ms(&json!({ "error": err_msg }));
                eprintln!(
                    "{}",
                    serde_json::to_string_pretty(&error_json).unwrap_or_default()
                );
            } else {
                eprintln!("error: {err_msg}");
            }
            ExitCode::from(exit_code)
        }
    }
}

fn classify_error(err: &anyhow::Error) -> u8 {
    let msg = format!("{err:#}");
    if msg.contains(PERMISSIONS_NOT_RESTRICTED_MARKER) {
        EXIT_NOT_RESTRICTED
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
/// bailing the whole process (rem-3dw0). Returning `true` here
/// short-circuits the config load in [`run`].
const fn subcommand_is_config_free(cmd: &Commands) -> bool {
    match cmd {
        Commands::Version
        | Commands::Activity { .. }
        | Commands::Identity { .. }
        | Commands::Permissions { .. }
        | Commands::ResolveMode { .. }
        | Commands::Restrict { .. }
        | Commands::Keygen { .. }
        | Commands::Skill { .. }
        | Commands::Unprotect { .. } => true,
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
        | Commands::Migrate { .. }
        | Commands::Mv { .. }
        | Commands::Plan { .. }
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
        | Commands::Migrate { identity_args, .. }
        | Commands::Mv { identity_args, .. }
        | Commands::Plan { identity_args, .. }
        | Commands::Purge { identity_args, .. }
        | Commands::Query { identity_args, .. }
        | Commands::React { identity_args, .. }
        | Commands::Rm { identity_args, .. }
        | Commands::Sandbox { identity_args, .. }
        | Commands::Sign { identity_args, .. }
        | Commands::Verify { identity_args, .. }
        | Commands::Write { identity_args, .. } => Some(identity_args),
        Commands::Comments { .. }
        | Commands::Get { .. }
        | Commands::Keygen { .. }
        | Commands::Lint { .. }
        | Commands::Ls { .. }
        | Commands::Metadata { .. }
        | Commands::Permissions { .. }
        | Commands::Registry { .. }
        | Commands::ResolveMode { .. }
        | Commands::Restrict { .. }
        | Commands::Search { .. }
        | Commands::Skill { .. }
        | Commands::Unprotect { .. }
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
        | Commands::Comments { .. }
        | Commands::Delete { .. }
        | Commands::Get { .. }
        | Commands::Identity { .. }
        | Commands::Keygen { .. }
        | Commands::Lint { .. }
        | Commands::Ls { .. }
        | Commands::Mcp { .. }
        | Commands::Metadata { .. }
        | Commands::Migrate { .. }
        | Commands::Mv { .. }
        | Commands::Permissions { .. }
        | Commands::Plan { .. }
        | Commands::Purge { .. }
        | Commands::Query { .. }
        | Commands::React { .. }
        | Commands::Registry { .. }
        | Commands::ResolveMode { .. }
        | Commands::Restrict { .. }
        | Commands::Rm { .. }
        | Commands::Sandbox { .. }
        | Commands::Search { .. }
        | Commands::Sign { .. }
        | Commands::Skill { .. }
        | Commands::Unprotect { .. }
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
        | Commands::Comment { .. }
        | Commands::Comments { .. }
        | Commands::Delete { .. }
        | Commands::Edit { .. }
        | Commands::Identity { .. }
        | Commands::Keygen { .. }
        | Commands::Lint { .. }
        | Commands::Mcp { .. }
        | Commands::Migrate { .. }
        | Commands::Mv { .. }
        | Commands::Permissions { .. }
        | Commands::Plan { .. }
        | Commands::Purge { .. }
        | Commands::Query { .. }
        | Commands::React { .. }
        | Commands::Registry { .. }
        | Commands::ResolveMode { .. }
        | Commands::Restrict { .. }
        | Commands::Sandbox { .. }
        | Commands::Search { .. }
        | Commands::Sign { .. }
        | Commands::Skill { .. }
        | Commands::Unprotect { .. }
        | Commands::Verify { .. }
        | Commands::Version => None,
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => None,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "config-free subcommand short-circuit list grows linearly with commands"
)]
fn run(cli: &Cli, system: &dyn System, cwd: &Path) -> Result<()> {
    let output = subcommand_output(&cli.command);
    let json_mode = output.is_some_and(|o| o.json);

    // Config-free subcommands short-circuit the config resolution path.
    match &cli.command {
        Commands::Version => {
            eprintln!("remargin {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Commands::Identity {
            action,
            identity_args,
            output_args,
        } => {
            return cmd_identity(
                system,
                cwd,
                action.as_ref(),
                identity_args,
                output_args.json,
            );
        }
        Commands::ResolveMode {
            cwd: cwd_arg,
            output_args,
        } => {
            let cwd_expanded = cwd_arg
                .as_deref()
                .map(|c| expand_cli_pathbuf(system, c))
                .transpose()?;
            let start_dir = cwd_expanded.as_deref().unwrap_or(cwd);
            return cmd_resolve_mode(system, start_dir, output_args.json);
        }
        Commands::Keygen {
            output: keygen_output,
            ..
        } => {
            let expanded_output = expand_cli_pathbuf(system, keygen_output)?;
            return cmd_keygen(system, &expanded_output);
        }
        #[cfg(feature = "obsidian")]
        Commands::Obsidian {
            action,
            output_args,
        } => {
            return cmd_obsidian(system, cwd, action, output_args.json);
        }
        Commands::Skill {
            action,
            output_args,
        } => return cmd_skill(system, action, output_args.json),
        Commands::Activity {
            path,
            since,
            pretty,
            identity_args,
            output_args,
        } => {
            return cmd_activity(
                system,
                cwd,
                path.as_deref(),
                since.as_deref(),
                *pretty,
                identity_args,
                output_args.json,
            );
        }
        Commands::Permissions { action } => {
            return cmd_permissions(system, cwd, action);
        }
        Commands::Restrict {
            path,
            also_deny_bash,
            cli_allowed,
            user_settings,
            output_args,
        } => {
            return cmd_restrict(
                system,
                cwd,
                path,
                also_deny_bash,
                *cli_allowed,
                user_settings.as_deref(),
                output_args.json,
            );
        }
        Commands::Unprotect {
            path,
            strict,
            user_settings,
            output_args,
        } => {
            return cmd_unprotect(
                system,
                cwd,
                path,
                *strict,
                user_settings.as_deref(),
                output_args.json,
            );
        }
        _ => {
            debug_assert!(
                !subcommand_is_config_free(&cli.command),
                "config-free subcommand fell through short-circuit"
            );
        }
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

    dispatch_with_config(cli, system, cwd, &final_config)
}

#[expect(
    clippy::too_many_lines,
    reason = "dispatch function maps all CLI subcommands"
)]
fn dispatch_with_config(
    cli: &Cli,
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
) -> Result<()> {
    match &cli.command {
        Commands::Ack {
            file,
            ids,
            path,
            remove,
            output_args,
            ..
        } => {
            let ap = AckParams {
                file: file.as_deref(),
                ids,
                json_mode: output_args.json,
                remove: *remove,
                search_path: path,
            };
            cmd_ack(system, cwd, config, &ap)
        }
        Commands::Batch {
            file,
            ops,
            output_args,
            ..
        } => cmd_batch(system, cwd, config, file, ops, output_args.json),
        Commands::Comment {
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
        } => {
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
            cmd_comment(system, cwd, config, &cp)
        }
        Commands::Comments {
            file,
            pretty,
            remargin_kind,
            output_args,
        } => cmd_comments(system, cwd, file, remargin_kind, output_args.json, *pretty),
        Commands::Delete {
            file,
            ids,
            output_args,
            ..
        } => cmd_delete(system, cwd, config, file, ids, output_args.json),
        Commands::Edit {
            file,
            id,
            content,
            remargin_kind,
            output_args,
            ..
        } => {
            // When no --kind flags are provided we preserve the stored
            // list; any occurrence (even `--kind x` once) replaces the
            // full list — consistent with how `--to` works.
            let kind_replacement = (!remargin_kind.is_empty()).then_some(remargin_kind.as_slice());
            cmd_edit(
                system,
                cwd,
                config,
                file,
                id,
                content,
                kind_replacement,
                output_args.json,
            )
        }
        Commands::Get {
            path,
            binary,
            start,
            end,
            line_numbers,
            out,
            output_args,
            ..
        } => {
            let gp = GetParams {
                binary: *binary,
                end: *end,
                json_mode: output_args.json,
                line_numbers: *line_numbers,
                out: out.as_deref(),
                path,
                start: *start,
            };
            cmd_get(system, cwd, config, &gp)
        }
        Commands::Lint { file, output_args } => cmd_lint(system, cwd, file, output_args.json),
        Commands::Ls {
            path, output_args, ..
        } => cmd_ls(system, cwd, config, path, output_args.json),
        Commands::Metadata {
            path, output_args, ..
        } => cmd_metadata(system, cwd, config, path, output_args.json),
        Commands::Migrate {
            file,
            backup,
            human_config,
            agent_config,
            output_args,
            ..
        } => {
            let identities = resolve_migrate_identities(
                system,
                cwd,
                config,
                human_config.as_deref(),
                agent_config.as_deref(),
            )?;
            cmd_migrate(
                system,
                cwd,
                config,
                file,
                *backup,
                &identities,
                output_args.json,
            )
        }
        Commands::Mv {
            src,
            dst,
            force,
            output_args,
            ..
        } => {
            let p = MvParams {
                dst: dst.as_str(),
                force: *force,
                json_mode: output_args.json,
                src: src.as_str(),
            };
            cmd_mv(system, cwd, config, &p)
        }
        Commands::Plan { action, .. } => {
            cmd_plan(system, cwd, config, action, plan_action_output(action).json)
        }
        Commands::Purge {
            file, output_args, ..
        } => cmd_purge(system, cwd, config, file, output_args.json),
        Commands::Query {
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
        } => {
            let q = QueryParams {
                author: author.as_deref(),
                comment_id: comment_id.as_deref(),
                content_regex: content_regex.as_deref(),
                expanded: *expanded,
                ignore_case: *ignore_case,
                json_mode: output_args.json,
                path: path.as_str(),
                pending: *pending,
                pending_broadcast: *pending_broadcast,
                pending_for: pending_for.as_deref(),
                pending_for_me: *pending_for_me,
                pretty: *pretty,
                remargin_kind,
                since: since.as_deref(),
                summary: *summary,
            };
            cmd_query(system, cwd, config, &q)
        }
        Commands::React {
            file,
            id,
            emoji,
            remove,
            output_args,
            ..
        } => {
            let r = ReactParams {
                emoji: emoji.as_str(),
                file: file.as_str(),
                id: id.as_str(),
                json_mode: output_args.json,
                remove: *remove,
            };
            cmd_react(system, cwd, config, &r)
        }
        Commands::Registry {
            action,
            output_args,
        } => cmd_registry(system, cwd, action, output_args.json),
        Commands::Rm {
            file, output_args, ..
        } => cmd_rm(system, cwd, config, file, output_args.json),
        Commands::Sandbox {
            action,
            output_args,
            ..
        } => cmd_sandbox(system, cwd, config, action, output_args.json),
        Commands::Search {
            pattern,
            path,
            regex,
            scope,
            context,
            ignore_case,
            output_args,
        } => {
            let s = SearchParams {
                context: *context,
                ignore_case: *ignore_case,
                json_mode: output_args.json,
                path: path.as_str(),
                pattern: pattern.as_str(),
                regex: *regex,
                scope: scope.as_str(),
            };
            cmd_search(system, cwd, &s)
        }
        Commands::Sign {
            file,
            ids,
            all_mine,
            repair_checksum,
            output_args,
            ..
        } => {
            let sp = SignParams {
                all_mine: *all_mine,
                file,
                ids,
                json_mode: output_args.json,
                repair_checksum: *repair_checksum,
            };
            cmd_sign(system, cwd, config, &sp)
        }
        Commands::Verify {
            file, output_args, ..
        } => cmd_verify(system, cwd, file, config, output_args.json),
        Commands::Write {
            path,
            content,
            binary,
            create,
            lines,
            raw,
            output_args,
            ..
        } => {
            let line_range = lines.as_deref().map(parse_line_range).transpose()?;
            cmd_write(
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
        Commands::Version
        | Commands::Activity { .. }
        | Commands::Identity { .. }
        | Commands::Mcp { .. }
        | Commands::Keygen { .. }
        | Commands::Permissions { .. }
        | Commands::ResolveMode { .. }
        | Commands::Restrict { .. }
        | Commands::Skill { .. }
        | Commands::Unprotect { .. } => Ok(()),
        #[cfg(feature = "obsidian")]
        Commands::Obsidian { .. } => Ok(()),
    }
}

fn cmd_ack(
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
    let key = if remove {
        "unacknowledged"
    } else {
        "acknowledged"
    };
    print_output(json_mode, &json!({ key: ids }))
}

fn cmd_batch(
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
    print_output(json_mode, &json!({ "ids": created_ids }))
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
        out_raw(&updated)?;
    }

    print_output(cp.json_mode, &json!({ "id": new_id }))
}

fn cmd_comments(
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
    // surface stays in lockstep with `remargin query` — the rem-49w0
    // design doc explicitly calls out the previous divergence as a bug.
    let comments: Vec<_> = doc
        .comments()
        .into_iter()
        .filter(|cm| matches_kind_filter(cm.kinds(), kind_filter))
        .collect();

    if pretty {
        let formatted = display::format_comments_pretty(file, &comments);
        out(&formatted)
    } else if json_mode {
        out_json(&json!({ "comments": comments }))
    } else {
        for cm in &comments {
            let ack_status = if cm.ack.is_empty() {
                "pending"
            } else {
                "acked"
            };
            out(&format!(
                "{} {} ({}) [{}] {}",
                cm.id,
                cm.author,
                author_type_str(&cm.author_type),
                ack_status,
                truncate_content(&cm.content, 60_usize),
            ))?;
        }
        Ok(())
    }
}

fn cmd_delete(
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
    print_output(json_mode, &json!({ "deleted": ids }))
}

#[expect(
    clippy::too_many_arguments,
    reason = "CLI adapter: each arg is a direct clap flag and collapsing them \
              into a struct adds more noise than it removes"
)]
fn cmd_edit(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    id: &str,
    content: &str,
    remargin_kind: Option<&[String]>,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    operations::edit_comment(system, &path, config, id, content, remargin_kind)?;
    print_output(json_mode, &json!({ "edited": id }))
}

fn cmd_get(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    gp: &GetParams<'_>,
) -> Result<()> {
    let target_buf = expand_cli_path(system, gp.path)?;
    let target = target_buf.as_path();

    if gp.binary {
        return cmd_get_binary(system, cwd, config, gp, target);
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
        print_output(true, &json!({ "lines": json_lines }))
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
            print_output(true, &json!({ "content": content }))
        } else {
            out_raw(&content)
        }
    }
}

/// Binary-mode `get` dispatch (rem-cdr). Reads bytes once through the shared
/// core helper, then surfaces them in the caller's chosen shape:
/// - `--out <path>` — write bytes to disk, stdout shows `{path, size_bytes, mime}`.
/// - `--json` — base64-encoded `content` in the payload alongside mime / size.
/// - default — raw bytes to stdout (so `remargin get --binary x.png > out.png` works).
///
/// Incompatible flags (`--start`, `--end`, `-n`) are rejected up front so
/// binary requests never silently drop text-mode options.
fn cmd_get_binary(
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
        fs::write(out_path, &payload.bytes)
            .with_context(|| format!("writing {}", out_path.display()))?;
        let summary = json!({
            "mime": payload.mime,
            "out": out_path,
            "path": payload.path,
            "size_bytes": payload.size_bytes,
        });
        return print_output(gp.json_mode, &summary);
    }

    if gp.json_mode {
        let encoded = BASE64_STANDARD.encode(&payload.bytes);
        return print_output(
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
    io::stdout()
        .write_all(&payload.bytes)
        .context("writing bytes to stdout")
}

/// Resolve and print the identity the CLI's active flag set produces.
///
/// Routes through the same [`ResolvedConfig::resolve`][config::ResolvedConfig::resolve]
/// every mutating op uses, so `remargin identity --config <path>` (or
/// `--identity` + `--type` manual, or a `--type`-filtered walk) returns
/// the same identity the next write would attribute to (rem-3dw0).
///
/// A branch-3 walk that cannot match the supplied filters is treated as
/// "nothing found" rather than an error: the JSON output collapses to
/// `{ "found": false }`, preserving the historical read-only-diagnostic
/// contract and letting the Obsidian plugin call this during startup
/// without having to special-case transient "no config yet" states.
/// Other resolver errors (unknown type strings, strict-mode registry
/// misses, etc.) still propagate.
fn cmd_identity(
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
        }) => cmd_identity_create(identity, r#type, key.as_deref(), output_args.json),
        Some(IdentityAction::Show {
            identity_args: nested,
            output_args,
        }) => cmd_identity_show(system, cwd, nested, output_args.json),
        None => cmd_identity_show(system, cwd, identity_args, json_mode),
    }
}

fn cmd_identity_show(
    system: &dyn System,
    cwd: &Path,
    identity_args: &IdentityArgs,
    json_mode: bool,
) -> Result<()> {
    let (flags, _assets_dir) = build_identity_flags(system, identity_args, None)?;
    match ResolvedConfig::resolve(system, cwd, &flags, None) {
        Ok(cfg) => render_identity(&cfg, json_mode),
        Err(err) if flags.is_empty() || looks_like_walk_miss(&err) => {
            // Walk-based "no matching config" is a soft miss on a
            // read-only diagnostic. Emit `found: false` and exit
            // cleanly so tooling that polls identity during startup
            // (the Obsidian plugin, rem-3dw0) does not see a hard
            // error for the "no config yet" state.
            render_identity_not_found(json_mode)
        }
        Err(err) => Err(err),
    }
}

/// Print a ready-to-use identity YAML block to stdout (rem-8cnc).
///
/// `mode:` is deliberately omitted — mode is a tree property resolved
/// by walk-up, not an identity-level declaration. `key:` is emitted
/// verbatim when supplied; an absent key is valid in non-strict modes.
/// `--json` returns the same fields as a structured payload so tooling
/// (the Obsidian plugin, scripts) can pick them up without re-parsing
/// YAML.
fn cmd_identity_create(
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
    out_raw(&out_str)
}

fn render_identity(config: &ResolvedConfig, json_mode: bool) -> Result<()> {
    let Some(identity) = config.identity.as_deref() else {
        return render_identity_not_found(json_mode);
    };
    let author_type_str = config
        .author_type
        .as_ref()
        .map(|t| String::from(t.as_str()));
    let key_display = config.key_path.as_ref().map(|p| p.display().to_string());
    let path_display = config.source_path.as_ref().map(|p| p.display().to_string());

    if json_mode {
        return print_output(
            true,
            &json!({
                "found": true,
                "path": path_display,
                "identity": identity,
                "author_type": author_type_str,
                "key": key_display,
                "mode": config.mode.as_str(),
            }),
        );
    }

    if let Some(p) = &path_display {
        eprintln!("Found config: {p}");
    }
    eprintln!("Identity:     {identity}");
    if let Some(t) = &author_type_str {
        eprintln!("Type:         {t}");
    }
    if let Some(k) = &key_display {
        eprintln!("Key:          {k}");
    }
    eprintln!("Mode:         {}", config.mode.as_str());
    Ok(())
}

fn render_identity_not_found(json_mode: bool) -> Result<()> {
    if json_mode {
        return print_output(true, &json!({ "found": false }));
    }
    eprintln!("No identity config found.");
    Ok(())
}

/// Cheap heuristic for the branch-3 walk-exhaust error message emitted
/// by `resolve_identity`. Used by `cmd_identity` to distinguish "walk
/// didn't match" (soft — map to `found: false`) from every other
/// resolver error (hard — propagate).
fn looks_like_walk_miss(err: &anyhow::Error) -> bool {
    let msg = format!("{err:#}");
    msg.contains("no identity resolved")
        || msg.contains("no .remargin.yaml matched the supplied filters")
}

/// Dispatch `remargin permissions <show|check>` (rem-yj1j.7 / T28).
///
/// `show` prints the resolved permissions tree at `cwd`. `check`
/// canonicalises its target path, asks the inspector whether any
/// `restrict` or `deny_ops` rule covers it, and exits gitignore-style:
/// 0 when restricted, 1 when not. Both paths support `--json`.
/// Wire the CLI `activity` subcommand to the
/// [`activity::gather_activity`] core (rem-g3sy.4 / T34).
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
    system: &dyn System,
    cwd: &Path,
    explicit_path: Option<&Path>,
    since: Option<&str>,
    pretty: bool,
    identity_args: &IdentityArgs,
    json_mode: bool,
) -> Result<()> {
    if pretty && json_mode {
        bail!("--pretty and --json are mutually exclusive");
    }

    let resolved_path = match explicit_path {
        Some(p) => {
            let expanded = expand_cli_pathbuf(system, p)?;
            if expanded.is_absolute() {
                expanded
            } else {
                cwd.join(expanded)
            }
        }
        None => cwd.to_path_buf(),
    };

    let cutoff = match since {
        Some(raw) => Some(
            chrono::DateTime::parse_from_rfc3339(raw)
                .with_context(|| format!("--since: invalid ISO 8601 timestamp {raw:?}"))?,
        ),
        None => None,
    };

    let (flags, _assets_dir) = build_identity_flags(system, identity_args, None)?;
    let resolved = ResolvedConfig::resolve(system, cwd, &flags, None)?;
    let caller = resolved
        .identity
        .as_deref()
        .context("activity: caller identity required (declare via --identity / --config)")?;

    let result = activity::gather_activity(system, &resolved_path, cutoff, caller)?;

    if pretty {
        emit_activity_pretty(&result);
    } else {
        let value = serde_json::to_value(&result).context("serializing activity result")?;
        print_output(true, &value)?;
    }
    Ok(())
}

fn emit_activity_pretty(result: &activity::ActivityResult) {
    if result.files.is_empty() {
        eprintln!("(no activity)");
        return;
    }
    for file in &result.files {
        eprintln!("{}:", file.path.display());
        eprintln!(
            "  {}",
            format_activity_cutoff_header(result.cutoff_explicit, file.cutoff_applied)
        );
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
                    eprintln!(
                        "  {} \u{00b7} comment \u{00b7} {comment_id} by {author}{arrow} (lines {line_start}-{line_end})",
                        ts.format("%Y-%m-%d %H:%M")
                    );
                }
                activity::Change::Ack {
                    ts,
                    comment_id,
                    author,
                    ..
                } => {
                    eprintln!(
                        "  {} \u{00b7} ack \u{00b7} {comment_id} acked by {author}",
                        ts.format("%Y-%m-%d %H:%M")
                    );
                }
                activity::Change::Sandbox { ts, author, .. } => {
                    eprintln!(
                        "  {} \u{00b7} sandbox \u{00b7} {author}",
                        ts.format("%Y-%m-%d %H:%M")
                    );
                }
                // The Change enum is `#[non_exhaustive]`; future
                // variants surface as a generic line until the
                // pretty-printer is taught about them.
                _ => {}
            }
        }
        eprintln!();
    }
    if let Some(ts) = result.newest_ts_overall {
        eprintln!("(newest_ts_overall: {})", ts.to_rfc3339());
    }
}

/// Render the per-file cutoff header line for `activity --pretty`
/// (rem-gb5j). The wording reflects which path produced the cutoff:
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

fn cmd_permissions(system: &dyn System, cwd: &Path, action: &PermissionsAction) -> Result<()> {
    match action {
        PermissionsAction::Show { output_args } => {
            let report = permissions_inspect::show(system, cwd)?;
            if output_args.json {
                let value =
                    serde_json::to_value(&report).context("serializing permissions show output")?;
                print_output(true, &value)?;
            } else {
                emit_permissions_show_text(cwd, &report);
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
                print_output(true, &value)?;
            } else {
                emit_permissions_check_text(&report, *why);
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

/// Render a [`permissions_inspect::ShowOutput`] as multi-line text. Top
/// level lists are sorted by source-file ordering already; the function
/// only formats.
fn emit_permissions_show_text(cwd: &Path, report: &permissions_inspect::ShowOutput) {
    eprintln!("Permissions resolved at {}:", cwd.display());
    eprintln!();

    eprintln!("  trusted_roots:");
    if report.trusted_roots.is_empty() {
        eprintln!("    (none)");
    } else {
        for entry in &report.trusted_roots {
            eprintln!(
                "    {}  (source: {})",
                entry.path.display(),
                entry.source_file.display()
            );
            if let Some(nested) = entry.recursive.as_deref() {
                eprintln!(
                    "      recursive permissions inside {}:",
                    entry.path.display()
                );
                emit_permissions_show_indented(nested, "        ");
            }
        }
    }
    eprintln!();

    eprintln!("  restrict:");
    if report.restrict.is_empty() {
        eprintln!("    (none)");
    } else {
        for entry in &report.restrict {
            eprintln!(
                "    {}  (source: {})",
                entry.path_text,
                entry.source_file.display()
            );
            if let Some(realm) = entry.realm_root.as_deref() {
                eprintln!("      realm_root: {}", realm.display());
            }
            if !entry.also_deny_bash.is_empty() {
                eprintln!(
                    "      also_deny_bash: {}",
                    format_string_list(&entry.also_deny_bash)
                );
            }
            eprintln!("      cli_allowed: {}", entry.cli_allowed);
        }
    }
    eprintln!();

    eprintln!("  deny_ops:");
    if report.deny_ops.is_empty() {
        eprintln!("    (none)");
    } else {
        for entry in &report.deny_ops {
            eprintln!(
                "    {}  ops={}  (source: {})",
                entry.path.display(),
                format_string_list(&entry.ops),
                entry.source_file.display()
            );
        }
    }
    eprintln!();

    eprintln!("  allow_dot_folders:");
    if report.allow_dot_folders.is_empty() {
        eprintln!("    (none)");
    } else {
        for entry in &report.allow_dot_folders {
            eprintln!("    {}", format_string_list(&entry.names));
        }
    }
}

/// Indented variant used to render the `recursive` block under a
/// trusted-root entry. Keeps the formatting compact since we are
/// already nested.
fn emit_permissions_show_indented(report: &permissions_inspect::ShowOutput, indent: &str) {
    if !report.trusted_roots.is_empty() {
        eprintln!("{indent}trusted_roots:");
        for entry in &report.trusted_roots {
            eprintln!(
                "{indent}  {}  (source: {})",
                entry.path.display(),
                entry.source_file.display()
            );
        }
    }
    if !report.restrict.is_empty() {
        eprintln!("{indent}restrict:");
        for entry in &report.restrict {
            eprintln!(
                "{indent}  {}  (source: {})",
                entry.path_text,
                entry.source_file.display()
            );
        }
    }
    if !report.deny_ops.is_empty() {
        eprintln!("{indent}deny_ops:");
        for entry in &report.deny_ops {
            eprintln!(
                "{indent}  {}  ops={}",
                entry.path.display(),
                format_string_list(&entry.ops)
            );
        }
    }
    if !report.allow_dot_folders.is_empty() {
        eprintln!("{indent}allow_dot_folders:");
        for entry in &report.allow_dot_folders {
            eprintln!("{indent}  {}", format_string_list(&entry.names));
        }
    }
}

fn emit_permissions_check_text(report: &permissions_inspect::CheckOutput, why: bool) {
    eprintln!("restricted: {}", report.restricted);
    if why && let Some(rule) = &report.matching_rule {
        eprintln!("  matched: {}", rule.rule_text);
        eprintln!("  kind:    {}", rule.kind);
        eprintln!("  source:  {}", rule.source_file.display());
    }
}

/// Wire the CLI `restrict` subcommand to the
/// [`permissions_restrict::restrict`] core (rem-yj1j.5 / T26).
///
/// `user_settings_explicit` lets tests pin a hermetic location for
/// the user-scope file. When `None`, the function expands
/// [`DEFAULT_USER_SETTINGS`] through the active `System`.
fn cmd_restrict(
    system: &dyn System,
    cwd: &Path,
    path: &str,
    also_deny_bash: &[String],
    cli_allowed: bool,
    user_settings_explicit: Option<&Path>,
    json_mode: bool,
) -> Result<()> {
    let user_scope = match user_settings_explicit {
        Some(explicit) => expand_cli_pathbuf(system, explicit)?,
        None => expand_cli_path(system, DEFAULT_USER_SETTINGS)?,
    };
    let anchor = permissions_restrict::find_claude_anchor(system, cwd)?;
    let project_scope = anchor.join(".claude/settings.local.json");
    let settings_files = vec![project_scope, user_scope];

    let args = permissions_restrict::RestrictArgs::new(
        String::from(path),
        also_deny_bash.to_vec(),
        cli_allowed,
    );
    let outcome = permissions_restrict::restrict(system, cwd, &args, &settings_files)?;

    if json_mode {
        let value = serde_json::json!({
            "absolute_path": outcome.absolute_path.display().to_string(),
            "anchor": outcome.anchor.display().to_string(),
            "claude_files_touched": outcome
                .claude_files_touched
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
            "rules_applied": outcome.rules_applied,
            "yaml_was_created": outcome.yaml_was_created,
        });
        print_output(true, &value)?;
    } else {
        emit_restrict_summary(&outcome);
    }
    Ok(())
}

fn emit_restrict_summary(outcome: &permissions_restrict::RestrictOutcome) {
    eprintln!("Restricted: {}", outcome.absolute_path.display());
    eprintln!("  Anchor: {}", outcome.anchor.display());
    if outcome.yaml_was_created {
        eprintln!(
            "  .remargin.yaml created at {}",
            outcome.anchor.join(".remargin.yaml").display()
        );
    } else {
        eprintln!(
            "  .remargin.yaml updated at {}",
            outcome.anchor.join(".remargin.yaml").display()
        );
    }
    eprintln!(
        "  Settings updated: {} file(s)",
        outcome.claude_files_touched.len()
    );
    for file in &outcome.claude_files_touched {
        eprintln!("    {}", file.display());
    }
    eprintln!("  Rules written: {}", outcome.rules_applied.len());
    eprintln!(
        "  Sidecar updated: {}",
        outcome
            .anchor
            .join(".claude/.remargin-restrictions.json")
            .display()
    );
    eprintln!(
        "  Note: Claude must reload its settings for Layer 2 (NATIVE tool denials) to take effect."
    );
    eprintln!("  Layer 1 (remargin's own ops) is enforcing immediately on the next call.");
}

/// Wire the CLI `unprotect` subcommand to the
/// [`permissions_unprotect::unprotect`] core (rem-yj1j.6 / T27).
///
/// `_user_settings_explicit` is accepted on the CLI for symmetry
/// with `restrict` but ignored here: the unprotect path consults
/// the sidecar's `added_to_files` list (rem-7m4u captured the
/// resolved settings paths at apply time), so the reversal
/// scrubs exactly the files the corresponding `restrict` touched.
fn cmd_unprotect(
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
        print_output(true, &value)?;
    } else {
        emit_unprotect_summary(&outcome);
    }
    Ok(())
}

fn emit_unprotect_summary(outcome: &permissions_unprotect::UnprotectOutcome) {
    eprintln!("Unprotected: {}", outcome.absolute_path.display());
    eprintln!("  Anchor: {}", outcome.anchor.display());
    if outcome.yaml_entry_removed {
        eprintln!(
            "  .remargin.yaml updated at {}",
            outcome.anchor.join(".remargin.yaml").display()
        );
    } else {
        eprintln!("  .remargin.yaml: no matching entry");
    }
    if outcome.claude_files_touched.is_empty() {
        eprintln!("  Settings: none touched (no sidecar entry)");
    } else {
        eprintln!(
            "  Settings updated: {} file(s)",
            outcome.claude_files_touched.len()
        );
        for file in &outcome.claude_files_touched {
            eprintln!("    {}", file.display());
        }
    }
    if !outcome.warnings.is_empty() {
        eprintln!("  Warnings:");
        for warning in &outcome.warnings {
            eprintln!("    - {warning}");
        }
    }
    eprintln!(
        "  Note: Claude must reload its settings for Layer 2 (NATIVE tool denials) to take effect."
    );
    eprintln!("  Layer 1 (remargin's own ops) stops enforcing immediately on the next call.");
}

fn cmd_resolve_mode(system: &dyn System, cwd: &Path, json_mode: bool) -> Result<()> {
    let resolved = config::resolve_mode(system, cwd)?;
    let source = resolved.source.as_ref().map(|p| p.display().to_string());
    let value = json!({
        "mode": resolved.mode.as_str(),
        "source": source,
    });
    if json_mode {
        print_output(true, &value)?;
    } else {
        eprintln!("Mode:   {}", resolved.mode.as_str());
        match &source {
            Some(path) => eprintln!("Source: {path}"),
            None => eprintln!("Source: <default>"),
        }
    }
    Ok(())
}

fn cmd_keygen(system: &dyn System, output: &Path) -> Result<()> {
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

    eprintln!("Private key: {}", output.display());
    eprintln!("Public key:  {}", pub_path.display());

    Ok(())
}

fn cmd_lint(system: &dyn System, cwd: &Path, file: &str, json_mode: bool) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let report = linter::lint_doc(system, &path)?;

    if json_mode {
        print_output(true, &report.to_json())?;
    } else {
        eprint!("{}", report.format_text());
    }

    if !report.is_clean() {
        anyhow::bail!("Lint errors found");
    }
    Ok(())
}

fn cmd_ls(
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
        print_output(true, &json!({ "entries": entries }))
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
            out(&format!(
                "{kind} {size_str:>8} {}{}",
                entry.path.display(),
                pending_str,
            ))?;
        }
        Ok(())
    }
}

fn cmd_metadata(
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

    // File-level fields are always present. Markdown fields are only emitted
    // when the file was parsed (rem-lqz).
    let mut result = json!({
        "binary": meta.binary,
        "mime": meta.mime,
        "path": meta.path,
        "size_bytes": meta.size_bytes,
    });
    let map = result.as_object_mut().unwrap();
    if let Some(count) = meta.comment_count {
        map.insert("comment_count".into(), json!(count));
    }
    if let Some(count) = meta.line_count {
        map.insert("line_count".into(), json!(count));
    }
    if let Some(count) = meta.pending_count {
        map.insert("pending_count".into(), json!(count));
    }
    if !meta.pending_for.is_empty() {
        map.insert("pending_for".into(), json!(meta.pending_for));
    }
    if let Some(last) = &meta.last_activity {
        map.insert("last_activity".into(), json!(last));
    }

    print_output(json_mode, &result)
}

fn cmd_migrate(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    backup: bool,
    identities: &migrate::MigrateIdentities,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let migrated = migrate::migrate(system, &path, config, identities, backup)?;

    if json_mode {
        let results: Vec<Value> = migrated
            .iter()
            .map(|m| json!({ "new_id": m.new_id, "original_role": m.original_role }))
            .collect();
        print_output(true, &json!({ "migrated": results }))
    } else if migrated.is_empty() {
        eprintln!("No legacy comments found.");
        Ok(())
    } else {
        for m in &migrated {
            eprintln!("{} -> {} (migrated)", m.original_role, m.new_id);
        }
        Ok(())
    }
}

/// Build the `MigrateIdentities` for a `migrate` (or `plan migrate`)
/// invocation.
///
/// Each `--*-config <path>` is interpreted as a complete identity
/// declaration via branch 1 of [`config::identity::resolve_identity`] —
/// the same path the operator's own identity takes when they pass
/// `--config`. The resolved `author_type` must match the role the flag
/// is wired to (a human config for `--human-config`, an agent config
/// for `--agent-config`); a mismatch is an error before any byte hits
/// disk.
fn resolve_migrate_identities(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    human_config: Option<&Path>,
    agent_config: Option<&Path>,
) -> Result<migrate::MigrateIdentities> {
    let human = match human_config {
        None => None,
        Some(path) => Some(resolve_role_identity(
            system,
            cwd,
            config,
            path,
            &parser::AuthorType::Human,
            "--human-config",
        )?),
    };
    let agent = match agent_config {
        None => None,
        Some(path) => Some(resolve_role_identity(
            system,
            cwd,
            config,
            path,
            &parser::AuthorType::Agent,
            "--agent-config",
        )?),
    };
    Ok(migrate::MigrateIdentities::new(human, agent))
}

fn resolve_role_identity(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    config_path: &Path,
    expected_type: &parser::AuthorType,
    flag_name: &str,
) -> Result<migrate::MigrateRoleIdentity> {
    let flags = config::identity::IdentityFlags::for_config_path(config_path.to_path_buf());
    let resolved = config::identity::resolve_identity(
        system,
        cwd,
        &config.mode,
        &flags,
        config.registry.as_ref(),
    )
    .with_context(|| format!("resolving {flag_name} {}", config_path.display()))?;
    if &resolved.author_type != expected_type {
        bail!(
            "{flag_name} resolved {:?} as type {:?}, but the flag requires type {:?}",
            resolved.identity,
            resolved.author_type,
            expected_type,
        );
    }
    Ok(migrate::MigrateRoleIdentity::new(
        resolved.identity,
        resolved.key_path,
    ))
}

/// Route a `plan` subcommand to the correct per-op projection.
///
/// Lightweight ops that have not yet been wired (tracked under rem-3uo /
/// rem-qll) surface a deliberate "not yet landed" error so callers
/// discover the subcommand tree and failures are loud. `plan write` is
/// fully wired per rem-imc.
#[expect(
    clippy::too_many_lines,
    reason = "single dispatch match is clearer than per-op helpers here"
)]
fn cmd_plan(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    action: &PlanAction,
    json_mode: bool,
) -> Result<()> {
    // `Comment` / `Write` arms need owned buffers that outlive the
    // `PlanRequest` (it borrows `&str` / `ProjectCommentParams<'_>`).
    // Stage them here so the borrows survive through `plan_ops::dispatch`.
    let comment_body;
    let write_body;
    let attach_refs: Vec<&str>;
    let position;

    let request = match action {
        PlanAction::Ack {
            path, ids, remove, ..
        } => plan_ops::PlanRequest::Ack {
            path: resolve_doc_path(system, cwd, path)?,
            ids: ids.clone(),
            remove: *remove,
        },
        PlanAction::Batch { path, ops_file, .. } => plan_ops::PlanRequest::Batch {
            path: resolve_doc_path(system, cwd, path)?,
            ops: read_plan_batch_ops(ops_file)?,
        },
        PlanAction::Comment {
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
        } => {
            let doc_path = resolve_doc_path(system, cwd, path)?;
            comment_body = match content {
                Some(s) => s.clone(),
                None => read_stdin()?,
            };
            position = resolve_comment_position(
                reply_to.as_deref(),
                after_comment.as_deref(),
                after_heading.as_deref(),
                *after_line,
            );
            attach_refs = attach_names.iter().map(String::as_str).collect();
            let params = projections::ProjectCommentParams::new(&comment_body, &position)
                .with_attachment_filenames(&attach_refs)
                .with_auto_ack(*auto_ack)
                .with_reply_to(reply_to.as_deref())
                .with_sandbox(*sandbox)
                .with_to(to);
            plan_ops::PlanRequest::Comment {
                path: doc_path,
                params,
            }
        }
        PlanAction::Delete { path, ids, .. } => plan_ops::PlanRequest::Delete {
            path: resolve_doc_path(system, cwd, path)?,
            ids: ids.clone(),
        },
        PlanAction::Edit {
            path, id, content, ..
        } => plan_ops::PlanRequest::Edit {
            path: resolve_doc_path(system, cwd, path)?,
            id,
            content,
        },
        PlanAction::Migrate {
            path,
            human_config,
            agent_config,
            ..
        } => {
            let identities = resolve_migrate_identities(
                system,
                cwd,
                config,
                human_config.as_deref(),
                agent_config.as_deref(),
            )?;
            plan_ops::PlanRequest::Migrate {
                path: resolve_doc_path(system, cwd, path)?,
                identities,
            }
        }
        PlanAction::Mv {
            src, dst, force, ..
        } => plan_ops::PlanRequest::Mv {
            src: expand_cli_path(system, src)?,
            dst: expand_cli_path(system, dst)?,
            force: *force,
        },
        PlanAction::Purge { path, .. } => plan_ops::PlanRequest::Purge {
            path: resolve_doc_path(system, cwd, path)?,
        },
        PlanAction::React {
            path,
            id,
            emoji,
            remove,
            ..
        } => plan_ops::PlanRequest::React {
            path: resolve_doc_path(system, cwd, path)?,
            id,
            emoji,
            remove: *remove,
        },
        PlanAction::Restrict {
            path,
            also_deny_bash,
            cli_allowed,
            user_settings,
            ..
        } => {
            let user_scope = match user_settings {
                Some(explicit) => expand_cli_pathbuf(system, explicit)?,
                None => expand_cli_path(system, DEFAULT_USER_SETTINGS)?,
            };
            // Anchor-walk failure surfaces via the projection's reject
            // path; on that path we still produce a report rather than
            // bail here. The fallback project-scope path is unused on
            // the reject branch.
            let project_scope = permissions_restrict::find_claude_anchor(system, cwd).map_or_else(
                |_err| cwd.join(".claude/settings.local.json"),
                |anchor| anchor.join(".claude/settings.local.json"),
            );
            let restrict_args = permissions_restrict::RestrictArgs::new(
                path.clone(),
                also_deny_bash.clone(),
                *cli_allowed,
            );
            plan_ops::PlanRequest::Restrict {
                args: restrict_args,
                cwd: cwd.to_path_buf(),
                settings_files: vec![project_scope, user_scope],
            }
        }
        PlanAction::SandboxAdd { path, .. } => plan_ops::PlanRequest::SandboxAdd {
            path: resolve_doc_path(system, cwd, path)?,
        },
        PlanAction::SandboxRemove { path, .. } => plan_ops::PlanRequest::SandboxRemove {
            path: resolve_doc_path(system, cwd, path)?,
        },
        PlanAction::Sign {
            path,
            ids,
            all_mine,
            ..
        } => plan_ops::PlanRequest::Sign {
            path: resolve_doc_path(system, cwd, path)?,
            selection: build_sign_selection(*all_mine, ids)?,
        },
        PlanAction::Unprotect { path, .. } => {
            let unprotect_args = permissions_unprotect::UnprotectArgs::new(path.clone());
            plan_ops::PlanRequest::Unprotect {
                args: unprotect_args,
                cwd: cwd.to_path_buf(),
            }
        }
        PlanAction::Write {
            path,
            content,
            binary,
            create,
            lines,
            raw,
            ..
        } => {
            write_body = match content {
                Some(s) => s.clone(),
                None => read_stdin()?,
            };
            let line_range = lines.as_deref().map(parse_line_range).transpose()?;
            let opts = document::WriteOptions::new()
                .binary(*binary)
                .create(*create)
                .lines(line_range)
                .raw(*raw);
            plan_ops::PlanRequest::Write {
                path: expand_cli_path(system, path)?,
                content: &write_body,
                opts,
            }
        }
    };

    let report = plan_ops::dispatch(system, cwd, config, &request)?;
    let value = serde_json::to_value(&report).context("serializing plan report")?;

    // Config-mutation plans (rem-puy5 / rem-6eop) get a structured
    // text block in text mode so the multi-file projection is
    // readable. JSON mode still emits the full PlanReport payload.
    if !json_mode {
        if report.config_diff.is_some() {
            return emit_plan_restrict_text(&report);
        }
        if report.unprotect_diff.is_some() {
            return emit_plan_unprotect_text(&report);
        }
    }

    print_output(json_mode, &value)
}

/// Render a `plan restrict` [`PlanReport`] as a structured text block
/// (rem-puy5). Mirrors the JSON shape: anchor + `would_commit`/`noop`
/// header, one section per touched file, then conflicts. Emitted on
/// stdout via the standard `out` helper so existing pipe-friendly
/// behaviour is preserved.
fn emit_plan_restrict_text(report: &plan_ops::PlanReport) -> Result<()> {
    let Some(diff) = report.config_diff.as_ref() else {
        return Ok(());
    };
    out(&format!("Plan: restrict {}", diff.absolute_path.display()))?;
    out(&format!("  Anchor: {}", diff.anchor.display()))?;
    out(&format!(
        "  noop: {}   would_commit: {}",
        report.noop, report.would_commit,
    ))?;
    if let Some(reason) = &report.reject_reason {
        out(&format!("  reject_reason: {reason}"))?;
    }
    out(&format!(
        "  .remargin.yaml: {}",
        diff.remargin_yaml.path.display()
    ))?;
    out(&format!(
        "    will be created: {}",
        diff.remargin_yaml.will_be_created
    ))?;
    out(&format!(
        "    entry: {}",
        entry_action_label(diff.remargin_yaml.entry_action),
    ))?;
    out(&format!(
        "  Settings: {} file(s)",
        diff.settings_files.len()
    ))?;
    for sf in &diff.settings_files {
        out(&format!("    {}", sf.path.display()))?;
        out(&format!("      will be created: {}", sf.will_be_created))?;
        out(&format!(
            "      deny rules: +{} to add, {} already present",
            sf.deny_rules_to_add.len(),
            sf.deny_rules_already_present.len(),
        ))?;
        out(&format!(
            "      allow rules: +{} to add, {} already present",
            sf.allow_rules_to_add.len(),
            sf.allow_rules_already_present.len(),
        ))?;
    }
    out(&format!(
        "  Sidecar: {} ({})",
        diff.sidecar.path.display(),
        entry_action_label(diff.sidecar.entry_action),
    ))?;
    if diff.conflicts.is_empty() {
        out("  conflicts: 0")?;
    } else {
        out(&format!("  conflicts: {}", diff.conflicts.len()))?;
        for conflict in &diff.conflicts {
            emit_conflict_line(conflict)?;
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

fn emit_conflict_line(conflict: &plan_ops::ConfigConflict) -> Result<()> {
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
            out(&format!(
                "    allow_deny_overlap in {} ({kind_label}):",
                settings_file.display()
            ))?;
            out(&format!("      existing allow:  {allow_rule}"))?;
            out(&format!("      projected deny:  {projected_deny_rule}"))?;
        }
        plan_ops::ConfigConflict::AnchorIsAncestor { anchor, cwd } => {
            out(&format!(
                "    anchor_is_ancestor: cwd={} anchor={}",
                cwd.display(),
                anchor.display(),
            ))?;
        }
        plan_ops::ConfigConflict::YamlEntryWouldChange {
            path,
            previous,
            projected,
        } => {
            out(&format!("    yaml_entry_would_change: path={path}"))?;
            out(&format!(
                "      previous: also_deny_bash={:?} cli_allowed={}",
                previous.also_deny_bash, previous.cli_allowed,
            ))?;
            out(&format!(
                "      projected: also_deny_bash={:?} cli_allowed={}",
                projected.also_deny_bash, projected.cli_allowed,
            ))?;
        }
        // ConfigConflict is `#[non_exhaustive]`; cover future variants
        // gracefully without breaking the build.
        _ => {
            out("    <unknown conflict variant>")?;
        }
    }
    Ok(())
}

/// Render a `plan unprotect` [`PlanReport`] as a structured text block
/// (rem-6eop / T43). Symmetric mirror of [`emit_plan_restrict_text`]
/// for the reverse direction: anchor + `would_commit`/`noop` header,
/// one section per touched file, then drift conflicts.
fn emit_plan_unprotect_text(report: &plan_ops::PlanReport) -> Result<()> {
    let Some(diff) = report.unprotect_diff.as_ref() else {
        return Ok(());
    };
    out(&format!("Plan: unprotect {}", diff.absolute_path.display()))?;
    out(&format!("  Anchor: {}", diff.anchor.display()))?;
    out(&format!(
        "  noop: {}   would_commit: {}",
        report.noop, report.would_commit,
    ))?;
    if let Some(reason) = &report.reject_reason {
        out(&format!("  reject_reason: {reason}"))?;
    }
    out(&format!(
        "  .remargin.yaml: {}",
        diff.remargin_yaml.path.display()
    ))?;
    out(&format!(
        "    entry: {}",
        unprotect_entry_action_label(diff.remargin_yaml.entry_action),
    ))?;
    out(&format!(
        "  Settings: {} file(s)",
        diff.settings_files.len()
    ))?;
    for sf in &diff.settings_files {
        out(&format!("    {}", sf.path.display()))?;
        out(&format!(
            "      rules: -{} to remove, {} already absent",
            sf.rules_to_remove.len(),
            sf.rules_already_absent.len(),
        ))?;
    }
    out(&format!(
        "  Sidecar: {} ({})",
        diff.sidecar.path.display(),
        unprotect_entry_action_label(diff.sidecar.entry_action),
    ))?;
    if diff.conflicts.is_empty() {
        out("  conflicts: 0")?;
    } else {
        out(&format!("  conflicts: {}", diff.conflicts.len()))?;
        for conflict in &diff.conflicts {
            emit_unprotect_conflict_line(conflict)?;
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

fn emit_unprotect_conflict_line(conflict: &plan_ops::UnprotectConflict) -> Result<()> {
    match conflict {
        plan_ops::UnprotectConflict::RuleAlreadyAbsent {
            rule,
            settings_file,
        } => {
            out(&format!(
                "    rule_already_absent in {}: {rule}",
                settings_file.display()
            ))?;
        }
        plan_ops::UnprotectConflict::SidecarEntryMissing { path } => {
            out(&format!("    sidecar_entry_missing: {}", path.display()))?;
        }
        plan_ops::UnprotectConflict::YamlEntryMissing { path } => {
            out(&format!("    yaml_entry_missing: {}", path.display()))?;
        }
        // UnprotectConflict is `#[non_exhaustive]`; cover future
        // variants gracefully without breaking the build.
        _ => {
            out("    <unknown conflict variant>")?;
        }
    }
    Ok(())
}

/// Read a JSON file (or stdin when `path == "-"`) into a vector of
/// [`projections::ProjectBatchOp`] values for `plan batch`.
fn read_plan_batch_ops(path: &str) -> Result<Vec<projections::ProjectBatchOp>> {
    let json_text = if path == "-" {
        read_stdin()?
    } else {
        fs::read_to_string(path).with_context(|| format!("reading plan batch ops file {path}"))?
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
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let result = purge::purge(system, &path, config)?;

    print_output(
        json_mode,
        &json!({
            "comments_removed": result.comments_removed,
            "attachments_cleaned": result.attachments_cleaned,
        }),
    )
}

fn cmd_query(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    params: &QueryParams<'_>,
) -> Result<()> {
    let target = cwd.join(expand_cli_path(system, params.path)?);
    let filter = build_query_filter(config, params)?;
    let results = query::query(system, &target, &filter)?;
    render_query_output(&results, params, filter.pending_label())
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
    filter.pending = params.pending;
    filter.pending_for = params.pending_for.map(String::from);
    filter.remargin_kind = params.remargin_kind.to_vec();
    filter.since = since_dt;
    filter.summary = params.summary;
    filter = filter.with_caller_identity(
        params.pending_for_me,
        params.pending_broadcast,
        config.identity.clone(),
    )?;
    if let Some(pattern) = params.content_regex {
        filter = filter.with_content_regex(pattern, params.ignore_case)?;
    }
    Ok(filter)
}

fn render_query_output(
    results: &[query::QueryResult],
    params: &QueryParams<'_>,
    pending_label: Option<&str>,
) -> Result<()> {
    if params.json_mode {
        return print_output(
            true,
            &json!({
                "base_path": format!("{}/", params.path.trim_end_matches('/')),
                "results": results,
            }),
        );
    }
    if params.pretty {
        return out_raw(&display::format_query_pretty(results, pending_label));
    }
    for r in results {
        out(&format!(
            "{} ({} comments, {} pending)",
            r.path.display(),
            r.comment_count,
            r.pending_count,
        ))?;
        for cm in &r.comments {
            let status = if cm.ack.is_empty() {
                "pending"
            } else {
                "acked"
            };
            out(&format!(
                "  {} {} ({}) [{}] {}",
                cm.id,
                cm.author,
                cm.author_type.as_str(),
                status,
                cm.content,
            ))?;
        }
    }
    Ok(())
}

fn cmd_search(system: &dyn System, cwd: &Path, params: &SearchParams<'_>) -> Result<()> {
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
        print_output(true, &json!({ "matches": results }))
    } else {
        for m in &results {
            let loc = match m.location {
                search::MatchLocation::Body => "body",
                search::MatchLocation::Comment => "comment",
                _ => "unknown",
            };
            for line in &m.before {
                out(&format!("  {line}"))?;
            }
            out(&format!(
                "{}:{}  [{}]  {}",
                m.path.display(),
                m.line,
                loc,
                m.text
            ))?;
            for line in &m.after {
                out(&format!("  {line}"))?;
            }
        }
        Ok(())
    }
}

fn cmd_react(
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
    let action = if params.remove { "removed" } else { "added" };
    print_output(
        params.json_mode,
        &json!({ "action": action, "emoji": params.emoji, "comment_id": params.id }),
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
                print_output(true, &json!({ "participants": participants }))
            } else {
                for (name, participant) in &registry.participants {
                    out(&registry_participant_pretty(name, participant))?;
                }
                Ok(())
            }
        }
    }
}

fn cmd_sandbox(
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
            emit_sandbox_bulk_result(&result, cwd, "added", json_mode)?;
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
            emit_sandbox_bulk_result(&result, cwd, "removed", json_mode)?;
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
                out_json(&json!({ "files": items }))
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
                    out(&display_path)?;
                }
                Ok(())
            }
        }
    }
}

fn emit_sandbox_bulk_result(
    result: &sandbox_ops::SandboxBulkResult,
    cwd: &Path,
    changed_key: &str,
    json_mode: bool,
) -> Result<()> {
    if json_mode {
        out_json(&result.to_json(cwd, changed_key))?;
    } else {
        for p in &result.changed {
            out(&strip_prefix_display(p, cwd))?;
        }
        for failure in &result.failed {
            let mut stderr = io::stderr().lock();
            let _ = writeln!(
                stderr,
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
        out_json(&json!({
            "bytes_moved": outcome.bytes_moved,
            "dst_absolute": outcome.dst_absolute.display().to_string(),
            "fallback_copy": outcome.fallback_copy,
            "noop_same_path": outcome.noop_same_path,
            "overwritten": outcome.overwritten,
            "src_absolute": outcome.src_absolute.display().to_string(),
        }))
    } else if outcome.noop_same_path {
        out(&format!("no-op: {} (same canonical path)", params.src))
    } else if outcome.bytes_moved == 0 {
        out(&format!(
            "already moved: {} -> {} ({} bytes)",
            params.src, params.dst, outcome.bytes_moved
        ))
    } else {
        out(&format!(
            "moved: {} -> {} ({} bytes{}{})",
            params.src,
            params.dst,
            outcome.bytes_moved,
            if outcome.overwritten {
                ", overwrote destination"
            } else {
                ""
            },
            if outcome.fallback_copy {
                ", cross-filesystem copy"
            } else {
                ""
            }
        ))
    }
}

fn cmd_rm(
    system: &dyn System,
    cwd: &Path,
    config: &ResolvedConfig,
    file: &str,
    json_mode: bool,
) -> Result<()> {
    let target = expand_cli_path(system, file)?;
    let result = document::rm(system, cwd, &target, config)?;

    if json_mode {
        out_json(&json!({
            "deleted": file,
            "existed": result.existed,
        }))
    } else if result.existed {
        out(&format!("deleted: {file}"))
    } else {
        out(&format!("already absent: {file}"))
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
    system: &dyn System,
    cwd: &Path,
    action: &ObsidianAction,
    json_mode: bool,
) -> Result<()> {
    match action {
        ObsidianAction::Install { vault_path } => {
            if !json_mode {
                eprintln!(
                    "Downloading remargin plugin v{} from GitHub Releases...",
                    obsidian::plugin_version()
                );
            }
            let expanded = expand_vault_path(system, vault_path.as_deref())?;
            let report = obsidian::install(system, cwd, expanded.as_deref())?;
            if json_mode {
                print_output(true, &report.to_json())
            } else {
                eprintln!("{}", report.to_text());
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
                            true,
                            &json!({
                                "uninstalled": plugin_dir.display().to_string(),
                            }),
                        )
                    } else {
                        eprintln!("Uninstalled remargin plugin from {}", plugin_dir.display());
                        Ok(())
                    }
                }
                obsidian::UninstallStatus::NotInstalled { plugin_dir } => {
                    if json_mode {
                        print_output(
                            true,
                            &json!({
                                "not_installed": plugin_dir.display().to_string(),
                            }),
                        )
                    } else {
                        eprintln!("remargin plugin not installed at {}", plugin_dir.display());
                        Ok(())
                    }
                }
            }
        }
    }
}

fn cmd_sign(
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
        print_output(true, &sign_result_json(&result))
    } else {
        render_sign_result_text(&result)
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

fn sign_result_json(result: &operations::sign::SignResult) -> Value {
    let signed: Vec<Value> = result
        .signed
        .iter()
        .map(|entry| json!({ "id": entry.id, "ts": entry.ts }))
        .collect();
    let skipped: Vec<Value> = result
        .skipped
        .iter()
        .map(|entry| json!({ "id": entry.id, "reason": entry.reason }))
        .collect();
    let repaired: Vec<Value> = result
        .repaired
        .iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "old_checksum": entry.old_checksum,
                "new_checksum": entry.new_checksum,
            })
        })
        .collect();
    json!({ "repaired": repaired, "signed": signed, "skipped": skipped })
}

fn render_sign_result_text(result: &operations::sign::SignResult) -> Result<()> {
    for entry in &result.repaired {
        out(&format!(
            "repaired checksum: {} ({} -> {})",
            entry.id, entry.old_checksum, entry.new_checksum
        ))?;
    }
    for entry in &result.signed {
        out(&format!("signed: {} (ts={})", entry.id, entry.ts))?;
    }
    for entry in &result.skipped {
        out(&format!("skipped: {} ({})", entry.id, entry.reason))?;
    }
    if result.signed.is_empty() && result.skipped.is_empty() && result.repaired.is_empty() {
        out("no candidates")?;
    }
    Ok(())
}

fn cmd_skill(system: &dyn System, action: &SkillAction, json_mode: bool) -> Result<()> {
    match action {
        SkillAction::Install { global } => {
            let path = skill::install(system, *global)?;
            if json_mode {
                print_output(true, &json!({ "installed": path.display().to_string() }))
            } else {
                eprintln!("Skill installed to {}", path.display());
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
                print_output(true, &json!({ "status": status_str }))
            } else {
                eprintln!("Skill status: {status_str}");
                Ok(())
            }
        }
        SkillAction::Uninstall { global } => {
            skill::uninstall(system, *global)?;
            if json_mode {
                print_output(true, &json!({ "uninstalled": true }))
            } else {
                eprintln!("Skill uninstalled.");
                Ok(())
            }
        }
    }
}

fn cmd_mcp(
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
                    true,
                    &json!({
                        "installed": true,
                        "scope": scope,
                        "binary": bin_str,
                    }),
                )
            } else {
                eprintln!("MCP server registered ({scope} scope): {bin_str}");
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
                print_output(true, &json!({ "uninstalled": true }))
            } else {
                eprintln!("MCP server unregistered.");
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
                print_output(true, &json!({ "status": status_str }))
            } else {
                eprintln!("MCP status: {status_str}");
                Ok(())
            }
        }
    }
}

fn cmd_verify(
    system: &dyn System,
    cwd: &Path,
    file: &str,
    config: &ResolvedConfig,
    json_mode: bool,
) -> Result<()> {
    let path = resolve_doc_path(system, cwd, file)?;
    let doc = parser::parse_file(system, &path)?;

    let report = operations::verify::verify_document(&doc, config);
    let results: Vec<Value> = report
        .results
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "checksum_ok": row.checksum_ok,
                "signature": row.signature.as_str(),
            })
        })
        .collect();

    if json_mode {
        print_output(true, &json!({ "results": results, "ok": report.ok }))?;
    } else {
        for row in &report.results {
            let chk = if row.checksum_ok { "ok" } else { "FAIL" };
            out(&format!(
                "{}: checksum={} signature={}",
                row.id,
                chk,
                row.signature.as_str(),
            ))?;
        }
    }

    if report.ok {
        Ok(())
    } else {
        anyhow::bail!("integrity check failed");
    }
}

fn cmd_write(
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
    // the existing fields so callers can branch on it (rem-1f2).
    if outcome.noop && !wp.json_mode {
        return out(&format!("{}: no changes (already up to date)", wp.path));
    }

    print_output(
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

    /// rem-gb5j: implicit cutoff with a caller-last-action ts
    /// renders as "(since you last touched this file: …)".
    #[test]
    fn cutoff_header_implicit_with_last_action() {
        let header = format_activity_cutoff_header(false, Some(ts("2026-04-27T02:09:00-04:00")));
        assert_eq!(
            header,
            "(since you last touched this file: 2026-04-27 02:09)"
        );
    }

    /// rem-gb5j: implicit cutoff with no prior activity renders
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

    /// rem-gb5j: explicit `--since` echoes the cutoff with the
    /// "(since …)" wording, matching the user's input.
    #[test]
    fn cutoff_header_explicit_since() {
        let header = format_activity_cutoff_header(true, Some(ts("2026-04-27T02:09:00-04:00")));
        assert_eq!(header, "(since 2026-04-27 02:09)");
    }

    /// rem-gb5j: the placeholder string `YOUR-LAST-ACTION` from
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
