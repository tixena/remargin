//! CLI grammar — `Cli`, argument groups, `Commands`, and all action sub-enums.
//!
//! Everything here is pure clap grammar: no IO, no config resolution,
//! no business logic.  The dispatch and handler modules match on the
//! enum variants defined here.

use std::path::{Path, PathBuf};

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "remargin",
    version,
    about = "Enhanced inline review protocol for markdown"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

impl Cli {
    pub const fn cmd(&self) -> &Commands {
        &self.command
    }
}

/// Per-subcommand identity group.
///
/// Flattened into every subcommand that acts on behalf of a caller,
/// including reads (get, ls, comments, search, query) so callers can
/// forward identity uniformly. Identity-free machine/tree reads
/// (resolve-mode, registry, obsidian) and utility subcommands do not
/// flatten this group, so clap rejects any attempt to pass
/// `--config` / `--identity` / `--type` / `--key` to them.
#[derive(clap::Args, Default)]
pub struct IdentityArgs {
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

impl IdentityArgs {
    pub fn author_type(&self) -> Option<&str> {
        self.r#type.as_deref()
    }

    pub fn config(&self) -> Option<&Path> {
        self.config.as_deref()
    }

    pub fn identity(&self) -> Option<&str> {
        self.identity.as_deref()
    }

    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }
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
pub struct OutputArgs {
    /// Compact columnar JSON (implies minified output). Requires --json.
    /// Supported by get, search, query, and activity.
    #[arg(long, requires = "json")]
    pub compact: bool,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    /// Enable verbose/tracing output.
    #[arg(long)]
    pub verbose: bool,
}

/// Per-subcommand `--assets-dir` flag.
///
/// Flattened ONLY into subcommands that write attachments: comment,
/// edit, batch. Everything else errors at parse time. Supplied as the
/// `assets_dir_flag` argument to
/// [`remargin_core::config::ResolvedConfig::resolve`] when set.
#[derive(clap::Args, Default)]
pub struct AssetsArgs {
    /// Path to assets directory.
    #[arg(long)]
    assets_dir: Option<String>,
}

impl AssetsArgs {
    pub fn assets_dir(&self) -> Option<&str> {
        self.assets_dir.as_deref()
    }
}

/// Per-subcommand unrestricted escape hatch.
///
/// Compile-gated behind the `unrestricted` feature; flattened into the
/// ops that touch arbitrary filesystem paths (get, ls, metadata, rm,
/// write).
#[cfg(feature = "unrestricted")]
#[derive(clap::Args, Default)]
pub struct UnrestrictedArgs {
    /// Bypass path sandbox checks (requires compile-time feature).
    #[arg(long)]
    unrestricted: bool,
}

#[cfg(not(feature = "unrestricted"))]
#[derive(clap::Args, Default)]
pub struct UnrestrictedArgs;

#[cfg(not(feature = "unrestricted"))]
impl UnrestrictedArgs {
    #[expect(
        clippy::unused_self,
        reason = "sibling unrestricted-feature impl reads self.unrestricted; keep the signature uniform"
    )]
    pub const fn unrestricted(&self) -> bool {
        false
    }
}

#[cfg(feature = "unrestricted")]
impl UnrestrictedArgs {
    pub const fn unrestricted(&self) -> bool {
        self.unrestricted
    }
}

/// Available subcommands.
#[derive(clap::Subcommand)]
pub enum Commands {
    /// Acknowledge one or more comments.
    Ack(AckArgs),
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
    Activity(ActivityArgs),
    /// Create multiple comments atomically (JSON ops via --ops).
    Batch(BatchArgs),
    /// Claude Code integration: manage which paths Claude is allowed to
    /// edit + project the deny rules into both Claude settings files.
    Claude {
        /// Subcommand: `restrict`, `unrestrict`.
        #[command(subcommand)]
        action: ClaudeAction,
    },
    /// Create a comment in a document.
    Comment(CommentArgs),
    /// List comments in a document.
    Comments(CommentsArgs),
    /// Copy a single tracked file without touching the source.
    ///
    /// Non-markdown and comment-free markdown copy verbatim. A
    /// comment-bearing markdown file is copied body-only — the duplicate
    /// carries no comment blocks, so no cross-tree ID ambiguity and no
    /// broken signatures. The source is always left byte-for-byte unchanged.
    /// Both endpoints flow through the same `trusted_roots` / `deny_ops` /
    /// sandbox guards every other mutating op uses.
    Cp(CpArgs),
    /// Delete one or more comments.
    Delete(DeleteArgs),
    /// Run health checks on the remargin permission stack.
    ///
    /// Checks (in order):
    ///
    /// 1. **Hook-installed** — verifies the `PreToolUse` hook is wired into
    ///    Claude settings. When absent from both user- and project-scope,
    ///    no enforcement is active and subsequent checks are skipped
    ///    (all would be moot without the hook).
    ///
    /// Exit code: 0 when clean, 1 when findings are present.
    Doctor(DoctorArgs),
    /// Edit a comment (cascading ack clear).
    Edit(EditArgs),
    /// Read a file's contents. Add `--binary` to fetch non-markdown files as
    /// bytes (base64 in `--json` mode, raw bytes to stdout otherwise, or
    /// written to `--out <path>`). Run `remargin metadata <path>` first to
    /// check `size_bytes` and `mime` before pulling large blobs.
    Get(GetArgs),
    /// Return a downscaled / cropped raster image sized to fit a
    /// caller-specified byte budget. Use when `get --binary` would
    /// exceed an inline limit. Accepts PNG / JPEG / GIF / WebP.
    GetImage(GetImageArgs),
    /// goose integration: manage the hook plugin that keeps goose
    /// sessions off remargin-managed paths.
    Goose {
        /// Subcommand: `pretool`.
        #[command(subcommand)]
        action: GooseAction,
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
    Identity(IdentityCmdArgs),
    /// Generate a new Ed25519 signing key pair.
    Keygen(KeygenArgs),
    /// Run structural lint checks.
    Lint(LintArgs),
    /// List files and directories.
    Ls(LsArgs),
    /// MCP server management and execution.
    Mcp(McpArgs),
    /// Get document metadata.
    Metadata(MetadataArgs),
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
    Mv(MvArgs),
    /// Install or uninstall the Obsidian plugin in a vault.
    #[cfg(feature = "obsidian")]
    Obsidian(ObsidianArgs),
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
    Plan(PlanArgs),
    /// Folder-scoped system-prompt resolver.
    ///
    /// Read-only walk-up that mirrors the identity resolver but anchors
    /// on the `system_prompt:` block of a `.remargin.yaml`. Identity
    /// flags are accepted for surface symmetry and never gate the
    /// resolution. Falls through to the locked Default body when the
    /// walk exhausts.
    Prompt(PromptArgs),
    /// Strip all comments from a document.
    ///
    /// With `--recursive`, treat `file` as a directory and purge every
    /// visible markdown file under it. Per-file `op_guard`
    /// checks fire individually so a single `deny_ops` or allow-list
    /// refusal does not abort the whole batch.
    Purge(PurgeArgs),
    /// Search across documents for comments.
    ///
    /// `comment_count`, `pending_count`, `pending_for`, and
    /// `last_activity` describe the whole file regardless of filters;
    /// `comments` and `matched_count` reflect the active filters.
    Query(QueryArgs),
    /// Add or remove an emoji reaction.
    React(ReactArgs),
    /// Manage the registry file.
    Registry(RegistryArgs),
    /// Find/replace across document body text (never inside comments).
    ///
    /// Substitutes `PATTERN` with `REPLACEMENT` in document body text
    /// only, over a single file or a whole directory tree. Comment
    /// blocks are never in scope — a pattern that occurs only inside a
    /// comment is a no-op, and a comment is left byte-identical even
    /// when the body around it changes. Each per-file write flows
    /// through the same comment-preservation and post-verify subset gate
    /// `write` uses, so a replace can never corrupt a comment or
    /// introduce an integrity anomaly. In folder mode, a file the gate
    /// refuses is skipped and recorded; the run finishes the rest.
    Replace(ReplaceArgs),
    /// Resolve the effective enforcement mode for a directory.
    ///
    /// Walks up from `--cwd` (or the current directory) looking for the
    /// nearest `.remargin.yaml` and returns its `mode:` field. Unlike
    /// `identity`, this does NOT filter by author type — mode is a
    /// directory-tree property. Falls back to `open` when no config is found.
    ///
    /// Prints a JSON object like `{"mode":"strict","source":"/path/to/.remargin.yaml"}`
    /// under `--json`; prints a short human-readable summary otherwise.
    ResolveMode(ResolveModeArgs),
    /// Remove a file from the managed document tree.
    Rm(RmArgs),
    /// Manage per-identity sandbox staging for markdown files.
    Sandbox(SandboxArgs),
    /// Search across documents for text matches.
    Search(SearchArgs),
    /// Launch one agent session per discovered identity under cwd.
    #[cfg(feature = "session")]
    Session {
        /// Subcommand: `launch`.
        #[command(subcommand)]
        action: SessionAction,
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
    Sign(SignArgs),
    /// Verify comment integrity (checksums and signatures) against the
    /// participant registry.
    ///
    /// No flags: the registry is the single source of truth for pubkeys.
    /// Per-comment resolution runs unconditionally and the aggregate
    /// pass/fail follows the mode-driven severity table (see
    /// `operations::verify`).
    Verify(VerifyArgs),
    /// Print version information.
    Version,
    /// Write document contents (comment-preserving).
    Write(WriteArgs),
}

/// `remargin claude` subcommands. Cohesion bucket for ops whose
/// effects are scoped entirely to Claude Code's permission surface
/// (`.claude/settings.local.json`, `~/.claude/settings.json`, and the
/// `.remargin-restrictions.json` sidecar).
#[derive(clap::Subcommand)]
pub enum ClaudeAction {
    /// Manage the remargin Claude Code plugin.
    Plugin {
        /// Subcommand: install, uninstall, test.
        #[command(subcommand)]
        action: PluginAction,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// Claude Code `PreToolUse` hook surface.
    ///
    /// With no subcommand (or `dispatch`), reads a `PreToolUse` event
    /// JSON envelope from stdin and emits Claude Code's decision JSON
    /// on stdout. `install` / `uninstall` / `test` manage the hook
    /// entry in the target Claude settings file.
    Pretool {
        #[command(subcommand)]
        action: Option<PretoolAction>,
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
    /// Claude Code `SessionStart` guard hook surface.
    ///
    /// With no subcommand (or `dispatch`), re-verifies enforcement will
    /// be live for the session — the `remargin` binary resolves on PATH
    /// and the realm's `.remargin.yaml` parses — and emits a diagnostic
    /// on stdout when it will not. `install` / `uninstall` / `test`
    /// manage the hook entry in the target Claude settings file.
    SessionGuard {
        #[command(subcommand)]
        action: Option<SessionGuardAction>,
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

/// `remargin session` subcommands. Nested-subcommand bucket mirroring
/// [`ClaudeAction`]; `launch` discovers identities under cwd and (later)
/// launches one session each.
#[cfg(feature = "session")]
#[derive(clap::Subcommand)]
pub enum SessionAction {
    /// Discover identities under cwd and (later) launch one session each.
    Launch {
        /// List what would launch; spawn nothing.
        #[arg(long)]
        dry_run: bool,
        /// Restrict to these identities (comma-separated).
        #[arg(long, value_delimiter = ',')]
        identity: Vec<String>,
        /// Terminal multiplexer to launch into. Unset means auto: herdr when
        /// its server is reachable, else tmux. (Tasks 86, 89.)
        #[arg(long, value_name = "herdr|tmux")]
        multiplexer: Option<String>,
        /// Named session from the `sessions:` manifest. Omitted: the
        /// manifest's default rule, or pure downward discovery when no
        /// manifest governs cwd.
        name: Option<String>,
        #[command(flatten)]
        output_args: OutputArgs,
        /// Emit the per-identity commands; spawn nothing. (Task 85.)
        #[arg(long)]
        print: bool,
    },
}

/// Registry subcommands.
/// Plan subcommands. One variant per mutating op; per-op
/// wiring is tracked /.
#[derive(clap::Subcommand)]
pub enum PlanAction {
    /// Project an `ack` op.
    Ack(PlanAckArgs),
    /// Project a `batch` op.
    ///
    /// Reads the sub-op list from a JSON file (same shape as the
    /// `batch` subcommand): an array of objects with `content` (required)
    /// plus optional `reply_to`, `after_comment`, `after_line`,
    /// `attach_names`, `auto_ack`, `remargin_kind`, `sandbox`, `to`.
    Batch(PlanBatchArgs),
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
    Comment(PlanCommentArgs),
    /// Project a `cp` op.
    ///
    /// Surfaces the canonical src/dst, whether the destination exists
    /// (and would therefore require `--force`), the copy kind
    /// (`verbatim`, `body_only`, or `noop`), and the number of comment
    /// blocks that would be dropped. No bytes are written — dry-run only.
    Cp(PlanCpArgs),
    /// Project a `delete` op.
    Delete(PlanDeleteArgs),
    /// Project an `edit` op.
    Edit(PlanEditArgs),
    /// Project an `mv` op.
    ///
    /// Surfaces the canonical src/dst, whether the destination exists
    /// (and would therefore require `--force`), and whether the live
    /// op would settle as a no-op (same canonical path) or
    /// idempotently as a no-op (src missing, dst already in place).
    /// No bytes are moved, no markdown is rewritten — `mv` does not
    /// change document content.
    Mv(PlanMvArgs),
    /// Project a `purge` op. Pass `--recursive` to project
    /// a directory-level purge.
    Purge(PlanPurgeArgs),
    /// Project a `react` op.
    React(PlanReactArgs),
    /// Project a `sandbox add` op.
    SandboxAdd(PlanSandboxAddArgs),
    /// Project a `sandbox remove` op.
    SandboxRemove(PlanSandboxRemoveArgs),
    /// Project a `sign` op.
    Sign(PlanSignArgs),
    /// Project a `write` op.
    Write(PlanWriteArgs),
}

/// `remargin plan claude` subcommands. Mirror the live `ClaudeAction`
/// shape; route through the canonical plan dispatcher.
#[derive(clap::Subcommand)]
pub enum PlanClaudeAction {
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
pub enum RegistryAction {
    /// Show the current registry.
    Show,
}

/// `remargin permissions` subcommands.
#[derive(clap::Subcommand)]
pub enum PermissionsAction {
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
pub enum IdentityAction {
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
pub enum PromptAction {
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
        /// Single-line command the composed prompt is piped into.
        /// Omitting it clears any existing runner.
        #[arg(long)]
        runner: Option<String>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
}

/// Sandbox subcommands.
#[derive(clap::Subcommand)]
pub enum SandboxAction {
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

/// `remargin claude pretool` subcommands.
#[derive(clap::Subcommand)]
pub enum PretoolAction {
    /// Read a `PreToolUse` event from stdin and emit the decision JSON.
    Dispatch,
    /// Wire the `PreToolUse` hook into `~/.claude/settings.json`
    /// (default) or `.claude/settings.json` with `--local`.
    Install {
        #[arg(long)]
        local: bool,
    },
    /// Report whether the `PreToolUse` hook is wired.
    Test {
        #[arg(long)]
        local: bool,
    },
    /// Remove the `PreToolUse` hook entry. Preserves unrelated entries.
    Uninstall {
        #[arg(long)]
        local: bool,
    },
}

/// `remargin claude session-guard` subcommands.
#[derive(clap::Subcommand)]
pub enum SessionGuardAction {
    /// Re-verify enforcement will be live and emit the `SessionStart` JSON.
    Dispatch,
    /// Wire the `SessionStart` guard hook into `~/.claude/settings.json`
    /// (default) or `.claude/settings.json` with `--local`.
    Install {
        #[arg(long)]
        local: bool,
    },
    /// Report whether the `SessionStart` guard hook is wired.
    Test {
        #[arg(long)]
        local: bool,
    },
    /// Remove the `SessionStart` guard hook entry. Preserves unrelated entries.
    Uninstall {
        #[arg(long)]
        local: bool,
    },
}

/// `remargin goose` subcommands. Cohesion bucket for ops scoped to
/// goose's plugin surface (`~/.agents/plugins/remargin-guard` and its
/// project-scope twin).
#[derive(clap::Subcommand)]
pub enum GooseAction {
    /// goose MCP extension surface — the guard's redirect target.
    ///
    /// The guard blocks native tools on managed paths and names the
    /// remargin ops to use instead; this registers remargin's stdio
    /// server in goose's `config.yaml` so the session actually has them.
    /// `install` / `uninstall` / `test` manage exactly remargin's entry,
    /// leaving every sibling extension and goose's own provider settings
    /// in place.
    Mcp {
        #[command(subcommand)]
        action: GooseMcpAction,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// goose `PreToolUse` hook surface.
    ///
    /// With no subcommand (or `dispatch`), reads a goose `PreToolUse`
    /// event JSON envelope from stdin and emits goose's block decision
    /// on stdout (and its reason on stderr) when the call would touch a
    /// remargin-managed path. `install` / `uninstall` / `test` manage
    /// the guard plugin directory.
    Pretool {
        #[command(subcommand)]
        action: Option<GoosePretoolAction>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
    /// goose `SessionStart` guard surface — the fail-open backstop.
    ///
    /// With no subcommand (or `dispatch`), re-verifies that the
    /// `PreToolUse` guard will actually run for this session and prints a
    /// diagnostic on stdout when it will not. `SessionStart` cannot block
    /// in goose, so the guard reports; it never stops a session.
    /// `install` / `uninstall` / `test` manage the `SessionStart` entry in
    /// the shared guard plugin.
    SessionGuard {
        #[command(subcommand)]
        action: Option<GooseSessionGuardAction>,
        #[command(flatten)]
        output_args: OutputArgs,
    },
}

/// `remargin goose mcp` subcommands.
///
/// `--local` is not a scope goose discovers. goose reads exactly one
/// config file, `$XDG_CONFIG_HOME/goose/config.yaml`; a project file
/// reaches a session only when `GOOSE_ADDITIONAL_CONFIG_FILES` names it,
/// which every `--local` outcome message says.
#[derive(clap::Subcommand)]
pub enum GooseMcpAction {
    /// Register remargin's stdio server in goose's `config.yaml`.
    /// Preserves sibling extensions and goose's own settings.
    Install {
        #[arg(long)]
        local: bool,
    },
    /// Report whether goose would load remargin's tools: `installed`,
    /// `not_installed`, or `broken` with the specific fault.
    Test {
        #[arg(long)]
        local: bool,
    },
    /// Remove exactly remargin's extension entry.
    Uninstall {
        #[arg(long)]
        local: bool,
    },
}

/// `remargin goose pretool` subcommands.
#[derive(clap::Subcommand)]
pub enum GoosePretoolAction {
    /// Read a goose `PreToolUse` event from stdin and emit the verdict.
    Dispatch,
    /// Write the guard plugin into `~/.agents/plugins/remargin-guard`
    /// (default) or `.agents/plugins/remargin-guard` with `--local`.
    Install {
        #[arg(long)]
        local: bool,
    },
    /// Report whether the guard plugin is wired.
    Test {
        #[arg(long)]
        local: bool,
    },
    /// Remove the guard plugin directory. Preserves sibling plugins.
    Uninstall {
        #[arg(long)]
        local: bool,
    },
}

/// `remargin goose session-guard` subcommands.
#[derive(clap::Subcommand)]
pub enum GooseSessionGuardAction {
    /// Re-verify the guard stack and print the diagnostic when it is broken.
    Dispatch,
    /// Add the `SessionStart` entry to `~/.agents/plugins/remargin-guard`
    /// (default) or `.agents/plugins/remargin-guard` with `--local`,
    /// creating the plugin when it is not there yet.
    Install {
        #[arg(long)]
        local: bool,
    },
    /// Report whether the `SessionStart` entry is wired.
    Test {
        #[arg(long)]
        local: bool,
    },
    /// Remove the `SessionStart` entry. Preserves the `PreToolUse` entry.
    Uninstall {
        #[arg(long)]
        local: bool,
    },
}

/// Plugin subcommands.
#[derive(clap::Subcommand)]
pub enum PluginAction {
    /// Register the marketplace and install the remargin plugin.
    Install {
        /// Install at project scope instead of the default user scope.
        #[arg(long)]
        local: bool,
    },
    /// Check plugin installation status.
    Test {
        /// Check at project scope instead of the default user scope.
        #[arg(long)]
        local: bool,
    },
    /// Uninstall the remargin plugin.
    Uninstall {
        /// Uninstall from project scope instead of the default user scope.
        #[arg(long)]
        local: bool,
    },
}

/// MCP subcommands.
#[derive(clap::Subcommand)]
pub enum McpAction {
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
pub enum ObsidianAction {
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

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct AckArgs {
    /// Path to the document (use - for stdin). Omit to resolve by ID across the folder tree.
    #[arg(long)]
    pub file: Option<String>,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    /// Comment IDs to acknowledge.
    #[arg(index = 1, required = true)]
    pub ids: Vec<String>,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Base directory to search when resolving by ID (default: .).
    #[arg(long, default_value = ".")]
    pub path: String,
    /// Remove this identity's ack instead of adding one.
    #[arg(long)]
    pub remove: bool,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct ActivityArgs {
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to scan. Defaults to the current directory.
    #[arg(index = 1)]
    pub path: Option<PathBuf>,
    /// Render a human-readable timeline instead of JSON.
    #[arg(long)]
    pub pretty: bool,
    /// Cutoff timestamp (ISO 8601). Omit to derive per-file
    /// from the caller's last action.
    #[arg(long)]
    pub since: Option<String>,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct BatchArgs {
    #[command(flatten)]
    pub assets_args: AssetsArgs,
    /// Path to the document.
    #[arg(index = 1)]
    pub file: String,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    /// JSON array of operations.
    #[arg(long)]
    pub ops: String,
    #[command(flatten)]
    pub output_args: OutputArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct CommentArgs {
    /// Insert after this comment ID.
    #[arg(long, conflicts_with_all = ["after_heading", "after_line"])]
    pub after_comment: Option<String>,
    /// Insert after the ATX heading addressed by this `>`-separated
    /// path. Setext (underline) headings are NOT
    /// supported in v1.
    #[arg(long, conflicts_with_all = ["after_comment", "after_line"])]
    pub after_heading: Option<String>,
    /// Insert after this line number (1-indexed).
    #[arg(long, conflicts_with_all = ["after_comment", "after_heading"])]
    pub after_line: Option<usize>,
    #[command(flatten)]
    pub assets_args: AssetsArgs,
    /// File paths to upload. Each file is copied beside the document
    /// and listed in the comment's `attachments` header — stored, not
    /// displayed. To show an image inline, also write
    /// `![alt](assets/<filename>)` in the comment body.
    #[arg(long)]
    pub attach: Vec<PathBuf>,
    /// Acknowledge the parent comment when replying. Default (omitted):
    /// auto-ack iff parent.author differs from the caller — replies to
    /// your own comment don't auto-ack. Pass --no-auto-ack to force skip.
    #[arg(long, conflicts_with = "no_auto_ack")]
    pub auto_ack: bool,
    /// Read comment body from a file (use - for stdin).
    #[arg(long, short = 'F', conflicts_with = "content")]
    pub comment_file: Option<PathBuf>,
    /// Comment body text (mutually exclusive with --comment-file).
    #[arg(index = 2, allow_hyphen_values = true)]
    pub content: Option<String>,
    /// Path to the document (use - for stdin).
    #[arg(index = 1)]
    pub file: String,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    /// Force-skip the auto-ack of the parent comment. Mutually exclusive
    /// with --auto-ack.
    #[arg(long = "no-auto-ack", conflicts_with = "auto_ack")]
    pub no_auto_ack: bool,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Classification tag for the new comment. Repeat to attach
    /// multiple (e.g. `--kind question --kind action-item`). Values
    /// must match `[A-Za-z0-9_ \-]{1,15}` — see `remargin_kind`
    /// validation in `remargin-core::kind`.
    #[arg(long = "kind")]
    pub remargin_kind: Vec<String>,
    /// ID of the comment to reply to.
    #[arg(long)]
    pub reply_to: Option<String>,
    /// Atomically stage the file in the caller's sandbox in the same write.
    #[arg(long)]
    pub sandbox: bool,
    /// Addressees of the comment.
    #[arg(long)]
    pub to: Vec<String>,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct CommentsArgs {
    /// Path to the document (use - for stdin).
    #[arg(index = 1)]
    pub file: String,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Pretty-print comments as a threaded tree.
    #[arg(long)]
    pub pretty: bool,
    /// Repeatable `remargin_kind` filter (OR semantics). Omit to
    /// return every comment regardless of tag.
    #[arg(long = "kind")]
    pub remargin_kind: Vec<String>,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct CpArgs {
    /// Destination path.
    #[arg(index = 2)]
    pub dst: String,
    /// Overwrite an existing destination.
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Source path.
    #[arg(index = 1)]
    pub src: String,
    #[command(flatten)]
    pub unrestricted_args: UnrestrictedArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct DeleteArgs {
    /// Path to the document.
    #[arg(index = 1)]
    pub file: String,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    /// Comment IDs to delete.
    #[arg(index = 2, required = true)]
    pub ids: Vec<String>,
    #[command(flatten)]
    pub output_args: OutputArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct DoctorArgs {
    /// Comma-separated checks to run (e.g. `hook,trusted-root-escape`).
    /// Runs every check by default. The hook-installed gate always runs
    /// and still skips every later check when the hook is missing, but a
    /// missing hook is only reported when `hook` is in the set.
    #[arg(long, value_name = "SET")]
    pub check: Option<String>,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Emit an agent-executable repair prompt instead of the
    /// human-readable report — one imperative instruction per
    /// finding, or a "nothing to do" line when the realm is clean.
    /// Pipe it to an agent to auto-repair the drift:
    /// `remargin doctor --prompt-mode | claude --auto-mode -p`.
    #[arg(long, conflicts_with = "json")]
    pub prompt_mode: bool,
    /// User-scope settings file. Defaults to `~/.claude/settings.json`.
    /// Pass an explicit path to keep hermetic test runs out of the
    /// user's real home.
    #[arg(long)]
    pub user_settings: Option<PathBuf>,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct EditArgs {
    #[command(flatten)]
    pub assets_args: AssetsArgs,
    /// New comment body.
    #[arg(index = 3)]
    pub content: String,
    /// Path to the document.
    #[arg(index = 1)]
    pub file: String,
    /// Comment ID to edit.
    #[arg(index = 2)]
    pub id: String,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Replacement classification tag list. Repeat to set multiple
    /// (e.g. `--kind question --kind action-item`). Omit every
    /// `--kind` to leave the stored tag list untouched. Pass
    /// `--kind ""` to clear — validation rejects empty strings so
    /// a single `--kind ''` errors; the right way to clear today
    /// is to run `remargin edit` without any `--kind` flags, then
    /// use the forthcoming tag editor to drop entries.
    #[arg(long = "kind")]
    pub remargin_kind: Vec<String>,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct GetArgs {
    /// Fetch as bytes. Rejects `.md` (use the text path for markdown).
    /// Mime is derived from the file extension.
    #[arg(long)]
    pub binary: bool,
    /// End line (1-indexed, inclusive). Text mode only.
    #[arg(long)]
    pub end: Option<usize>,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    /// Show line numbers in output. Text mode only.
    #[arg(short = 'n', long = "line-numbers")]
    pub line_numbers: bool,
    /// Write the fetched bytes to this path (binary mode only). Stdout
    /// receives a summary instead of the bytes. The caller names the
    /// target path — no auto-cleanup.
    #[arg(long)]
    pub out: Option<PathBuf>,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the file.
    #[arg(index = 1)]
    pub path: String,
    /// Start line (1-indexed). Text mode only.
    #[arg(long)]
    pub start: Option<usize>,
    #[command(flatten)]
    pub unrestricted_args: UnrestrictedArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct GetImageArgs {
    /// Optional pixel crop applied before scaling, formatted
    /// `X,Y,W,H` (origin top-left). Clamped to the image bounds.
    #[arg(long)]
    pub crop: Option<String>,
    /// Output format: `jpeg`, `jpg`, or `png`. Defaults to `jpeg`
    /// for photographic source formats (JPEG / WebP) and `png`
    /// for lossless source formats (PNG / GIF).
    #[arg(long)]
    pub format: Option<String>,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    /// Target ceiling on the encoded output size in bytes. JPEG
    /// quality is stepped down (and then the dimension cap halved)
    /// until this fits. Defaults to 262144 (256 KiB).
    #[arg(long)]
    pub max_bytes: Option<u64>,
    /// Upper bound (in pixels) on the longer edge of the output.
    /// Defaults to 1024.
    #[arg(long)]
    pub max_dimension: Option<u32>,
    /// Write the encoded bytes to this path. Stdout gets a summary
    /// instead of the bytes.
    #[arg(long)]
    pub out: Option<PathBuf>,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the image attachment.
    #[arg(index = 1)]
    pub path: String,
    #[command(flatten)]
    pub unrestricted_args: UnrestrictedArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct IdentityCmdArgs {
    /// Subcommand. Omit to invoke `show` (backward-compatible
    /// with the pre-existing surface).
    #[command(subcommand)]
    pub action: Option<IdentityAction>,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct KeygenArgs {
    /// Output path for the private key (public key gets .pub suffix).
    #[arg(index = 1, default_value = "remargin_key")]
    pub output: PathBuf,
    #[command(flatten)]
    pub output_args: OutputArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct LintArgs {
    /// Path to the document (use - for stdin).
    #[arg(index = 1)]
    pub file: String,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct LsArgs {
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Directory path to list.
    #[arg(index = 1, default_value = ".")]
    pub path: String,
    #[command(flatten)]
    pub unrestricted_args: UnrestrictedArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct McpArgs {
    /// Subcommand: run, install, uninstall, test.
    #[command(subcommand)]
    pub action: Option<McpAction>,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct MetadataArgs {
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the document.
    #[arg(index = 1)]
    pub path: String,
    #[command(flatten)]
    pub unrestricted_args: UnrestrictedArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct MvArgs {
    /// Destination path.
    #[arg(index = 2)]
    pub dst: String,
    /// Overwrite an existing destination.
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Source path.
    #[arg(index = 1)]
    pub src: String,
    #[command(flatten)]
    pub unrestricted_args: UnrestrictedArgs,
}

/// Arguments for the corresponding subcommand variant.
#[cfg(feature = "obsidian")]
#[derive(clap::Args)]
pub struct ObsidianArgs {
    #[command(subcommand)]
    pub action: ObsidianAction,
    #[command(flatten)]
    pub output_args: OutputArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PlanArgs {
    /// Which mutating op to plan.
    #[command(subcommand)]
    pub action: PlanAction,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PromptArgs {
    /// Subcommand: resolve.
    #[command(subcommand)]
    pub action: PromptAction,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PurgeArgs {
    /// Path to the document (or directory when `--recursive` is set).
    #[arg(index = 1)]
    pub file: String,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Recursively purge every `.md` file under the directory at `file`.
    #[arg(long)]
    pub recursive: bool,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct QueryArgs {
    /// Only documents with comments by this author.
    #[arg(long)]
    pub author: Option<String>,
    /// Only documents containing a comment with this structural ID.
    #[arg(long)]
    pub comment_id: Option<String>,
    /// Regex applied to comment content; composes with metadata filters.
    #[arg(long)]
    pub content_regex: Option<String>,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    /// Case-insensitive match for `--content-regex`.
    #[arg(long, short = 'i')]
    pub ignore_case: bool,
    /// Add checksum + signature columns to the compact comment rows.
    /// Requires `--compact`.
    #[arg(long, requires = "compact")]
    pub include_integrity: bool,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Base directory to search.
    #[arg(index = 1, default_value = ".")]
    pub path: String,
    #[command(flatten)]
    pub pending_flags: QueryPendingFlags,
    /// Repeatable `remargin_kind` filter (OR semantics). Matches any
    /// comment whose tag list contains at least one of the supplied
    /// values.
    #[arg(long = "kind")]
    pub remargin_kind: Vec<String>,
    #[command(flatten)]
    pub render_flags: QueryRenderFlags,
    /// Only activity after this ISO 8601 timestamp.
    #[arg(long)]
    pub since: Option<String>,
}

/// Pending-state filters for `remargin query`.
#[derive(clap::Args)]
pub struct QueryPendingFlags {
    /// Only documents with pending (unacked) comments. Matches
    /// both directed (unacked recipients) and broadcast (no acks
    /// at all) shapes.
    #[arg(long)]
    pub pending: bool,
    /// Only surface unacked broadcast (no-`to`) comments the
    /// current identity has not acknowledged. Resolves the
    /// identity the same way every other subcommand does.
    #[arg(long)]
    pub pending_broadcast: bool,
    /// Only pending for this recipient.
    #[arg(long)]
    pub pending_for: Option<String>,
    /// Sugar for `--pending-for <current-identity>`. Surfaces
    /// directed comments addressed to the caller that the caller
    /// has not acked yet.
    #[arg(long)]
    pub pending_for_me: bool,
}

/// Output-shape flags for `remargin query`.
#[derive(clap::Args)]
pub struct QueryRenderFlags {
    /// Include individual matching comments in each result (default behavior).
    #[arg(long)]
    pub expanded: bool,
    /// Pretty-print results grouped by file.
    #[arg(long)]
    pub pretty: bool,
    /// Return only counts/summary, suppress comment data.
    #[arg(long)]
    pub summary: bool,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct ReactArgs {
    /// Emoji to add/remove.
    #[arg(index = 3)]
    pub emoji: String,
    /// Path to the document.
    #[arg(index = 1)]
    pub file: String,
    /// Comment ID.
    #[arg(index = 2)]
    pub id: String,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Remove instead of add.
    #[arg(long)]
    pub remove: bool,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct RegistryArgs {
    /// Subcommand: show.
    #[command(subcommand)]
    pub action: RegistryAction,
    #[command(flatten)]
    pub output_args: OutputArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct ReplaceArgs {
    /// Report per-file replacement counts and the subset-gate
    /// verdict; write nothing.
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    /// Case-insensitive matching.
    #[arg(long, short = 'i')]
    pub ignore_case: bool,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Target file or directory.
    #[arg(long, default_value = ".")]
    pub path: String,
    /// Text or regex to find.
    #[arg(index = 1)]
    pub pattern: String,
    /// Treat pattern as a regex (default: literal).
    #[arg(long)]
    pub regex: bool,
    /// Replacement text. In `--regex` mode, `$1` / `${name}` expand
    /// to capture groups; otherwise the text is inserted verbatim.
    #[arg(index = 2)]
    pub replacement: String,
    #[command(flatten)]
    pub unrestricted_args: UnrestrictedArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct ResolveModeArgs {
    /// Starting directory for the walk-up. Defaults to the process's
    /// current directory.
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    #[command(flatten)]
    pub output_args: OutputArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct RmArgs {
    /// Path to the file.
    #[arg(index = 1)]
    pub file: String,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
    #[command(flatten)]
    pub unrestricted_args: UnrestrictedArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct SandboxArgs {
    /// Subcommand: add, list, or remove.
    #[command(subcommand)]
    pub action: SandboxAction,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct SearchArgs {
    /// Lines of context around matches.
    #[arg(long, short = 'C', default_value = "0")]
    pub context: usize,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    /// Case-insensitive matching.
    #[arg(long, short = 'i')]
    pub ignore_case: bool,
    /// Page size: return at most this many matches. Omit for all.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Number of matches to skip before the returned page.
    #[arg(long, default_value = "0")]
    pub offset: usize,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Base directory to search.
    #[arg(long, default_value = ".")]
    pub path: String,
    /// Text or regex pattern to search for.
    #[arg(index = 1)]
    pub pattern: String,
    /// Treat pattern as a regex.
    #[arg(long)]
    pub regex: bool,
    /// Search scope: all, body, or comments.
    #[arg(long, default_value = "all")]
    pub scope: String,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct SignArgs {
    /// Sign every unsigned comment authored by the current
    /// identity. Mutually exclusive with `--ids`.
    #[arg(long)]
    pub all_mine: bool,
    /// Path to the document.
    #[arg(index = 1)]
    pub file: String,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    /// Comment ids to sign. Mutually exclusive with `--all-mine`.
    #[arg(long, value_delimiter = ',', conflicts_with = "all_mine")]
    pub ids: Vec<String>,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Recompute each target comment's stored checksum from its
    /// current content before signing. The forgery guard still
    /// applies — you can only repair comments you authored.
    /// Without this flag a stale checksum fails the verify gate
    /// and the op refuses to sign.
    #[arg(long)]
    pub repair_checksum: bool,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct VerifyArgs {
    /// Path to the document.
    #[arg(index = 1)]
    pub file: String,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    #[command(flatten)]
    pub output_args: OutputArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct WriteArgs {
    /// Content is base64-encoded binary data (implies --raw).
    /// Not supported for markdown (.md) files.
    #[arg(long)]
    pub binary: bool,
    /// File content to write (read from stdin if omitted).
    #[arg(index = 2)]
    pub content: Option<String>,
    /// Create a new file, creating any missing parent directories; the file itself must not already exist.
    #[arg(long)]
    pub create: bool,
    #[command(flatten)]
    pub identity_args: IdentityArgs,
    /// Replace only lines `START-END` (1-indexed, inclusive) and leave
    /// every other line byte-identical. Comment blocks inside the
    /// range must be reincluded (by id) in the replacement; writes
    /// that would destroy a comment are rejected. Incompatible with
    /// --create, --raw, and --binary.
    #[arg(long, value_name = "START-END")]
    pub lines: Option<String>,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the file.
    #[arg(index = 1)]
    pub path: String,
    /// Write content exactly as provided, skipping frontmatter and comment
    /// preservation. Not supported for markdown (.md) files.
    #[arg(long)]
    pub raw: bool,
    #[command(flatten)]
    pub unrestricted_args: UnrestrictedArgs,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PlanAckArgs {
    /// Comment IDs to ack (or un-ack with `--remove`).
    #[arg(index = 2, required = true)]
    pub ids: Vec<String>,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the document.
    #[arg(index = 1)]
    pub path: String,
    /// Remove the current identity's ack from each comment.
    #[arg(long)]
    pub remove: bool,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PlanBatchArgs {
    /// JSON file containing the `ops` array. Use `-` for stdin.
    #[arg(index = 2)]
    pub ops_file: String,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the document.
    #[arg(index = 1)]
    pub path: String,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PlanCommentArgs {
    /// Insert after this comment ID.
    #[arg(long, conflicts_with_all = ["after_heading", "after_line"])]
    pub after_comment: Option<String>,
    /// Project insertion after the ATX heading addressed by this
    /// `>`-separated path. Setext (underline) headings
    /// are NOT supported in v1.
    #[arg(long, conflicts_with_all = ["after_comment", "after_line"])]
    pub after_heading: Option<String>,
    /// Insert after this line number (1-indexed).
    #[arg(long, conflicts_with_all = ["after_comment", "after_heading"])]
    pub after_line: Option<usize>,
    /// Attachment basenames to record on the projected comment.
    /// Bytes are *not* copied — `plan` stays side-effect-free. The
    /// caller is responsible for the corresponding files existing
    /// when the mutating `comment` op runs.
    #[arg(long = "attach-name")]
    pub attach_names: Vec<String>,
    /// Acknowledge the parent comment when replying. Default (omitted):
    /// auto-ack iff parent.author differs from the caller. Pass
    /// --no-auto-ack to force skip.
    #[arg(long, conflicts_with = "no_auto_ack")]
    pub auto_ack: bool,
    /// Comment body text (read from stdin if omitted).
    #[arg(index = 2)]
    pub content: Option<String>,
    /// Force-skip the auto-ack of the parent comment. Mutually exclusive
    /// with --auto-ack.
    #[arg(long = "no-auto-ack", conflicts_with = "auto_ack")]
    pub no_auto_ack: bool,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the document.
    #[arg(index = 1)]
    pub path: String,
    /// ID of the comment to reply to.
    #[arg(long)]
    pub reply_to: Option<String>,
    /// Atomically project a sandbox entry in the frontmatter.
    #[arg(long)]
    pub sandbox: bool,
    /// Addressees of the comment.
    #[arg(long)]
    pub to: Vec<String>,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PlanCpArgs {
    /// Destination path.
    #[arg(index = 1, index = 2)]
    pub dst: String,
    /// Project the `--force` semantics.
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Source path.
    #[arg(index = 2, index = 1)]
    pub src: String,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PlanDeleteArgs {
    /// Comment IDs to delete.
    #[arg(index = 2, required = true)]
    pub ids: Vec<String>,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the document.
    #[arg(index = 1)]
    pub path: String,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PlanEditArgs {
    /// New comment body.
    #[arg(index = 3)]
    pub content: String,
    /// Comment ID to edit.
    #[arg(index = 2)]
    pub id: String,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the document.
    #[arg(index = 1)]
    pub path: String,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PlanMvArgs {
    /// Destination path.
    #[arg(index = 2)]
    pub dst: String,
    /// Project the `--force` semantics.
    #[arg(long)]
    pub force: bool,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Source path.
    #[arg(index = 1)]
    pub src: String,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PlanPurgeArgs {
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the document (or directory when `--recursive` is set).
    #[arg(index = 1)]
    pub path: String,
    /// Project a recursive purge over every visible `.md` file
    /// under the directory at `path`.
    #[arg(long)]
    pub recursive: bool,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PlanReactArgs {
    /// Emoji to add (or remove with `--remove`).
    #[arg(index = 3)]
    pub emoji: String,
    /// Comment ID to react to.
    #[arg(index = 2)]
    pub id: String,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the document.
    #[arg(index = 1)]
    pub path: String,
    /// Remove the current identity's reaction with this emoji.
    #[arg(long)]
    pub remove: bool,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PlanSandboxAddArgs {
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the document.
    #[arg(index = 1)]
    pub path: String,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PlanSandboxRemoveArgs {
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the document.
    #[arg(index = 1)]
    pub path: String,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PlanSignArgs {
    /// Sign every unsigned comment authored by the current
    /// identity. Mutually exclusive with `--ids`.
    #[arg(long)]
    pub all_mine: bool,
    /// Comment ids to sign. Mutually exclusive with `--all-mine`.
    #[arg(long, value_delimiter = ',', conflicts_with = "all_mine")]
    pub ids: Vec<String>,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the document.
    #[arg(index = 1)]
    pub path: String,
}

/// Arguments for the corresponding subcommand variant.
#[derive(clap::Args)]
pub struct PlanWriteArgs {
    /// Content is base64-encoded binary data (implies --raw). Not
    /// supported for markdown (.md) files and not representable as
    /// a structured plan — the report will carry a `reject_reason`.
    #[arg(long)]
    pub binary: bool,
    /// File content to write (read from stdin if omitted).
    #[arg(index = 2)]
    pub content: Option<String>,
    /// Create a new file, creating any missing parent directories; the file itself must not already exist.
    #[arg(long)]
    pub create: bool,
    /// Replace only lines `START-END` (1-indexed, inclusive) and
    /// leave every other line byte-identical. See `write --lines`
    /// for the full semantics.
    #[arg(long, value_name = "START-END")]
    pub lines: Option<String>,
    #[command(flatten)]
    pub output_args: OutputArgs,
    /// Path to the file.
    #[arg(index = 1)]
    pub path: String,
    /// Write content exactly as provided, skipping frontmatter and
    /// comment preservation. Not supported for markdown (.md) files
    /// and not representable as a structured plan — the report will
    /// carry a `reject_reason`.
    #[arg(long)]
    pub raw: bool,
}
