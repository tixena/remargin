//! Configuration loader: `.remargin.yaml` walk-up resolution.

pub mod identity;
pub mod permissions;
pub mod registry;
pub mod system_prompt;

#[cfg(test)]
mod tests;

#[cfg(feature = "session")]
use core::time::Duration;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use serde::Deserialize;

use crate::config::permissions::Permissions;
use crate::config::permissions::resolve::resolve_trusted_roots_for_cwd;
use crate::config::registry::{Registry, RegistryParticipantStatus};
use crate::parser::AuthorType;
use crate::path::expand_path;
use crate::permissions::op_guard::CallerInfo;

const CONFIG_FILENAME: &str = ".remargin.yaml";
const REGISTRY_FILENAME: &str = ".remargin-registry.yaml";

/// Parsed contents of a `.remargin.yaml` file.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Config {
    #[serde(default = "default_assets_dir")]
    pub assets_dir: String,
    #[serde(rename = "type")]
    pub author_type: Option<String>,
    pub identity: Option<String>,
    #[serde(default)]
    pub ignore: Vec<String>,
    pub key: Option<String>,
    pub mode: Option<Mode>,
    /// Permissions block. Missing in legacy
    /// `.remargin.yaml` files; defaults to an empty
    /// [`Permissions`] so back-compat parsing stays lossless.
    /// Enforcement is added in T23 and beyond — this loader is data-
    /// only.
    #[serde(default)]
    pub permissions: Permissions,
    /// Per-agent `remargin session launch` parameters. Gated behind the
    /// `session` feature and absent from the default build; a
    /// `.remargin.yaml` with no `session:` block parses to `None`, and
    /// with the feature off the key is ignored entirely.
    #[cfg(feature = "session")]
    #[serde(default)]
    pub session: Option<SessionConfig>,
    /// Optional folder-scoped system prompt for AI runs over docs in
    /// this realm. Resolved by walking parents via
    /// [`system_prompt::resolve_system_prompt`]; identity-free.
    #[serde(default)]
    pub system_prompt: Option<SystemPrompt>,
}

/// Folder-scoped system prompt declared on a `.remargin.yaml`.
///
/// Picked up by [`system_prompt::resolve_system_prompt`], which walks
/// the parent chain from a file and returns the nearest match. Identity
/// is not consulted: the prompt is a property of the directory tree,
/// not of the caller.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct SystemPrompt {
    /// Human-readable label. When absent, callers derive a name from
    /// the basename of the owning folder.
    pub name: Option<String>,
    /// The literal prompt body. Written verbatim into the AI payload
    /// by the caller — this loader does no templating.
    pub prompt: String,
}

/// Per-agent session parameters for `remargin session launch`.
///
/// Optional block; `loop_interval` and `goal` are required to *launch*
/// (enforced in the launch-spec builder, task 84), not to parse. Gated
/// behind the `session` feature.
#[cfg(feature = "session")]
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct SessionConfig {
    /// Optional per-session resource caps. Absent = no cap.
    #[serde(default)]
    pub budget: Option<Budget>,
    /// Optional Claude backend parameters (`model`, `effort`).
    #[serde(default)]
    pub claude: Option<ClaudeParams>,
    /// `/goal` stop condition passed to the backend.
    pub goal: Option<String>,
    /// `/loop` cadence as a duration string (`30s`, `5min`, `1h`).
    /// Stored raw; parsed via [`SessionConfig::loop_duration`].
    #[serde(rename = "loop")]
    pub loop_interval: Option<String>,
}

/// Claude backend parameters declared under `session.claude`.
#[cfg(feature = "session")]
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct ClaudeParams {
    pub effort: Option<String>,
    pub model: Option<String>,
}

/// Per-session resource caps declared under `session.budget`. An absent
/// field (or an absent `budget:` block) means "no cap".
#[cfg(feature = "session")]
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Budget {
    pub max_turns: Option<u32>,
    pub tokens: Option<u64>,
}

#[cfg(feature = "session")]
impl SessionConfig {
    /// Parse `loop_interval` into a `Duration`. `Ok(None)` when unset.
    ///
    /// # Errors
    ///
    /// Returns an error naming the offending value when `loop_interval`
    /// is set but not a valid duration string; the caller adds the
    /// identity context (task 84). Never panics.
    pub fn loop_duration(&self) -> Result<Option<Duration>> {
        self.loop_interval
            .as_deref()
            .map(parse_loop_interval)
            .transpose()
    }
}

/// Enforcement mode for the participant registry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Mode {
    /// Default — registry / signing not enforced.
    #[default]
    Open,
    Registered,
    Strict,
}

impl Mode {
    /// Canonical lowercase name for the mode, matching the YAML
    /// representation and the CLI's JSON output.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Registered => "registered",
            Self::Strict => "strict",
        }
    }
}

/// Final resolved configuration.
///
/// Carries mode, identity, registry, assets dir, and ignore list, as
/// determined by one trip through the three-branch resolver
/// ([`identity::resolve_identity`]) plus a walk for mode and registry.
/// Identity fields are never inherited from one `.remargin.yaml` and
/// half-replaced by flags — the resolver picks one branch and the
/// identity comes whole from that branch.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ResolvedConfig {
    pub assets_dir: String,
    pub author_type: Option<AuthorType>,
    pub identity: Option<String>,
    pub ignore: Vec<String>,
    pub key_path: Option<PathBuf>,
    pub mode: Mode,
    pub registry: Option<Registry>,
    /// Path to the `.remargin.yaml` that declared the identity. `Some`
    /// for branch-1 (`--config`) and branch-3 (walk) resolutions;
    /// `None` for branch-2 (manual declaration via
    /// `--identity`/`--type`/`--key`) and for the no-config fallback
    /// used by read-only invocations in directories that lack a config
    /// file entirely. Exposed so the `remargin identity` JSON output
    /// (and any tooling that wants to report provenance) can name the
    /// file without re-walking.
    pub source_path: Option<PathBuf>,
    /// Allow-listed roots derived from `permissions.trusted_roots` in
    /// the parent walk; `[cwd]` when none declared.
    pub trusted_roots: Vec<PathBuf>,
    /// Only settable via CLI when compiled with `--features unrestricted`.
    pub unrestricted: bool,
}

impl ResolvedConfig {
    /// Build the [`CallerInfo`] view of this config so the per-op
    /// guard can evaluate identity-scoped `deny_ops`. Pure
    /// projection — no I/O.
    #[must_use]
    pub fn caller_info(&self) -> CallerInfo {
        CallerInfo {
            author_type: self.author_type.clone(),
            identity_id: self.identity.clone(),
            identity_name: self.identity.clone(),
            mode: self.mode.clone(),
        }
    }

    /// Check if a recipient is allowed to receive comments in the current
    /// mode (registered/strict: must be an active participant; open: always ok).
    ///
    /// Mirrors [`Self::can_post`] on the recipient side. Empty `to:` lists
    /// (broadcast) are never passed here — callers iterate and call per-id.
    ///
    /// # Errors
    ///
    /// Returns an error when the mode is `Registered` or `Strict` and the
    /// recipient is not an active registry participant (absent or revoked).
    pub(crate) fn can_address(&self, recipient: &str) -> Result<()> {
        match self.mode {
            Mode::Open => Ok(()),
            Mode::Registered | Mode::Strict => {
                let Some(reg) = &self.registry else {
                    bail!("mode is {:?} but no registry found", self.mode);
                };
                if !reg.is_active(recipient) {
                    bail!(
                        "recipient {recipient:?} is not an active registry participant (mode: {:?})",
                        self.mode
                    );
                }
                Ok(())
            }
        }
    }

    /// Check if a participant is allowed to post (mode + registry enforcement).
    ///
    /// Kept as a crate-private helper used by [`Self::resolve`] as a
    /// belt-and-braces final gate. Op handlers do NOT call this directly
    ///; they consume a pre-validated [`ResolvedConfig`].
    ///
    /// # Errors
    ///
    /// Returns an error if the participant is not allowed to post in the current
    /// mode (e.g. unregistered in registered/strict mode, or revoked).
    pub(crate) fn can_post(&self, author: &str) -> Result<()> {
        match self.mode {
            Mode::Open => Ok(()),
            Mode::Registered | Mode::Strict => {
                let Some(reg) = &self.registry else {
                    bail!("mode is {:?} but no registry found", self.mode);
                };
                let Some(participant) = reg.participants.get(author) else {
                    bail!(
                        "author {author:?} is not registered (mode: {:?})",
                        self.mode
                    );
                };
                if participant.status == RegistryParticipantStatus::Revoked {
                    bail!("author {author:?} has been revoked");
                }
                Ok(())
            }
        }
    }

    /// Return a config whose mode is the mode declared by the realm
    /// containing `doc_path`. The file's realm is the sole source of
    /// truth; caller context never participates. Re-validates the
    /// resulting config against the realm's mode so missing keys or
    /// unregistered identities surface here, before any write.
    ///
    /// # Errors
    ///
    /// Returns an error when `resolve_mode` fails on the doc's realm
    /// (e.g. unreadable `.remargin.yaml`), or when the realm's mode
    /// fails [`Self::validate_identity`] — for example, the caller's
    /// declared identity is not in the registry the doc's realm sees,
    /// or the strict-mode key resolution falls through.
    pub fn escalate_for_doc(&self, system: &dyn System, doc_path: &Path) -> Result<Self> {
        let realm_cfg = self.escalate_mode_for_doc(system, doc_path)?;
        if realm_cfg.mode != self.mode {
            realm_cfg.validate_identity().with_context(|| {
                format!(
                    "doc {:?} is in a realm whose mode differs from the \
                     caller's mode; identity does not satisfy the realm's gate",
                    doc_path.display(),
                )
            })?;
        }
        Ok(realm_cfg)
    }

    /// Replace `self.mode` with the mode declared by the realm
    /// containing `doc_path`. The file's realm is the sole source of
    /// truth — caller context (cwd walk, --config target) does not
    /// participate. Use from read-only paths (verify, lint,
    /// comments-list) where the caller does not need to be able to
    /// write into the realm but does need the realm's mode to drive
    /// integrity checks.
    ///
    /// # Errors
    ///
    /// Returns an error when [`resolve_mode`] fails on the doc's realm
    /// (e.g. unreadable `.remargin.yaml`).
    pub fn escalate_mode_for_doc(&self, system: &dyn System, doc_path: &Path) -> Result<Self> {
        let realm_anchor = doc_path.parent().unwrap_or(doc_path);
        let ResolvedMode {
            mode: realm_mode, ..
        } = resolve_mode(system, realm_anchor)?;
        let realm_registry = load_registry(system, realm_anchor)?;

        Ok(Self {
            assets_dir: self.assets_dir.clone(),
            author_type: self.author_type.clone(),
            identity: self.identity.clone(),
            ignore: self.ignore.clone(),
            key_path: self.key_path.clone(),
            mode: realm_mode,
            registry: realm_registry,
            source_path: self.source_path.clone(),
            trusted_roots: self.trusted_roots.clone(),
            unrestricted: self.unrestricted,
        })
    }

    /// Check if a comment must be signed (strict mode + registered participant).
    #[must_use]
    pub fn requires_signature(&self, author: &str) -> bool {
        if self.mode != Mode::Strict {
            return false;
        }
        let Some(reg) = &self.registry else {
            return false;
        };
        reg.participants
            .get(author)
            .is_some_and(|participant| participant.status == RegistryParticipantStatus::Active)
    }

    /// Build the effective configuration for `cwd` from a set of CLI
    /// identity flags plus an optional `--assets-dir` value.
    ///
    /// This is the single entry point used by the CLI, MCP, and every
    /// other adapter. Identity resolution goes through
    /// [`identity::resolve_identity`], which picks exactly one branch
    /// (declared via `--config`, declared manually via
    /// `--identity`/`--type`/`--key`, or walk-up with strict-equality
    /// filters) and never mixes fields from different files.
    ///
    /// Registry lookup anchors on the directory that declared the
    /// identity when a `--config` path was supplied (so cross-realm
    /// declarations find the right `.remargin-registry.yaml`); otherwise
    /// it walks from `cwd`. Mode is a property of the directory tree
    /// rooted at `cwd` (see [`resolve_mode`]) and is not caller-chosen.
    ///
    /// When `flags.is_empty()`, a missing or identity-less config file is
    /// tolerated — the resulting `ResolvedConfig` carries `identity:
    /// None`, and op handlers that require one surface their own error.
    ///
    /// # Errors
    ///
    /// Returns an error when:
    /// - Any branch of identity resolution fails (see
    ///   [`identity::resolve_identity`] for the per-branch error list).
    /// - The configured assets dir value is malformed.
    /// - The resolved identity fails the strict-mode or registry gate in
    ///   [`Self::validate_identity`].
    pub fn resolve(
        system: &dyn System,
        cwd: &Path,
        flags: &identity::IdentityFlags,
        assets_dir_flag: Option<&str>,
    ) -> Result<Self> {
        let ResolvedMode { mode, .. } = resolve_mode(system, cwd)?;

        let registry_anchor = flags
            .config_path
            .as_deref()
            .and_then(Path::parent)
            .map_or_else(|| cwd.to_path_buf(), Path::to_path_buf);
        let registry = load_registry(system, &registry_anchor)?;

        let fields = if flags.is_empty() {
            resolve_fields_from_walk(system, cwd)?
        } else {
            let resolved =
                identity::resolve_identity(system, cwd, &mode, flags, registry.as_ref())?;
            let source_path = match &resolved.source {
                identity::IdentitySource::ConfigFlag(p) | identity::IdentitySource::Walk(p) => {
                    Some(p.clone())
                }
                identity::IdentitySource::Manual => None,
            };
            WalkedIdentityFields {
                author_type: Some(resolved.author_type),
                identity: Some(resolved.identity),
                key_path: resolved.key_path,
                source_config: resolved.source_config,
                source_path,
            }
        };

        let assets_dir = assets_dir_flag
            .map(String::from)
            .or_else(|| fields.source_config.as_ref().map(|c| c.assets_dir.clone()))
            .unwrap_or_else(default_assets_dir);

        let ignore = fields
            .source_config
            .as_ref()
            .map(|c| c.ignore.clone())
            .unwrap_or_default();

        let trusted_roots = resolve_trusted_roots_for_cwd(system, cwd)?;

        let resolved = Self {
            assets_dir,
            author_type: fields.author_type,
            identity: fields.identity,
            ignore,
            key_path: fields.key_path,
            mode,
            registry,
            source_path: fields.source_path,
            trusted_roots,
            unrestricted: false,
        };

        resolved.validate_identity()?;

        Ok(resolved)
    }

    /// Return the signing key for `author` when the active mode requires
    /// signing, otherwise `None`.
    ///
    /// This is a trivial accessor: the key-presence fail-fast that used
    /// to live here has moved into [`Self::validate_identity`],
    /// so every caller-visible [`ResolvedConfig`] has already been
    /// verified to carry a key path when strict mode requires one.
    /// Unregistered authors in strict mode return `None` here because
    /// they never reach op handlers — the resolver rejects them up
    /// front.
    #[must_use]
    pub fn resolve_signing_key(&self, author: &str) -> Option<&Path> {
        if !self.requires_signature(author) {
            return None;
        }
        self.key_path.as_deref()
    }

    /// Enforce mode-level invariants on the current identity: registry
    /// membership (registered / strict) and a resolvable signing key
    /// (strict).
    ///
    /// Invoked automatically by [`Self::resolve`] so every surface that
    /// produces a [`ResolvedConfig`] runs the same gate.
    /// Absent identity is not an error here — some read-only commands
    /// intentionally resolve without one; op handlers check for the
    /// identity they need separately.
    ///
    /// # Errors
    ///
    /// Returns an error when the active mode is registered/strict and
    /// the declared identity is not an active registry participant, or
    /// when strict mode is active but no signing key is resolvable for
    /// the registered identity.
    fn validate_identity(&self) -> Result<()> {
        let Some(identity) = self.identity.as_deref() else {
            return Ok(());
        };

        self.can_post(identity)?;

        // Strict + registered active identity but no key path: fail-fast.
        // Use `requires_signature` so unregistered authors in strict
        // (already rejected by can_post above) do not reach this branch.
        if self.mode == Mode::Strict && self.requires_signature(identity) && self.key_path.is_none()
        {
            bail!(
                "strict mode: no signing key resolved for {identity:?} \
                 (checked: --key flag, config `.remargin.yaml` key field). \
                 Fix your config or pass --key explicitly."
            );
        }

        Ok(())
    }
}

/// Resolved mode with provenance, produced by [`resolve_mode`].
///
/// Unlike the identity walk-up, this resolution ignores the `type:` field:
/// it returns whichever `.remargin.yaml` appears first on the walk (closest
/// to `start_dir`). `mode` is a directory-tree property, not an identity
/// property, so it must not be filtered by author type.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ResolvedMode {
    /// Effective mode for `start_dir`. Defaults to [`Mode::Open`] when no
    /// config with a `mode:` field is found on the walk.
    pub mode: Mode,
    /// Path to the `.remargin.yaml` that declared the mode, or `None` when
    /// the resolution fell back to the default.
    pub source: Option<PathBuf>,
}

/// Fields extracted from a walked `.remargin.yaml` when no identity
/// flags were supplied. Populated by [`resolve_fields_from_walk`] and
/// consumed by [`ResolvedConfig::resolve`].
#[derive(Default)]
struct WalkedIdentityFields {
    author_type: Option<AuthorType>,
    identity: Option<String>,
    key_path: Option<PathBuf>,
    source_config: Option<Config>,
    source_path: Option<PathBuf>,
}

fn default_assets_dir() -> String {
    String::from("assets")
}

/// Parse a `loop:` duration string (`30s`, `5min`, `1h`, `500ms`) via
/// `humantime`. The error names the bad value so a launch-time failure
/// (task 84) is attributable to the config that declared it.
#[cfg(feature = "session")]
fn parse_loop_interval(s: &str) -> Result<Duration> {
    humantime::parse_duration(s).with_context(|| format!("invalid loop interval {s:?}"))
}

/// Walk-based fallback used by [`ResolvedConfig::resolve`] when no
/// identity flags were supplied. The three-branch resolver requires an
/// `identity:` field in every file it considers — it is a signing-oriented
/// function. Some read-only CLI invocations run with an empty flag set in
/// directories whose `.remargin.yaml` legitimately lacks identity (pure
/// mode declarations, for example), and we tolerate that: the resulting
/// [`ResolvedConfig`] simply carries `identity: None`, and the final
/// [`ResolvedConfig::validate_identity`] early-returns `Ok(())`.
fn resolve_fields_from_walk(system: &dyn System, cwd: &Path) -> Result<WalkedIdentityFields> {
    let Some((path, config)) = load_config_filtered_with_path(system, cwd, None)? else {
        return Ok(WalkedIdentityFields::default());
    };

    let identity = config.identity.clone();
    let author_type = match config.author_type.as_deref() {
        Some(raw) => Some(parse_author_type(raw)?),
        None => None,
    };
    let key_path = match config.key.as_deref() {
        Some(raw) => {
            let expanded = resolve_key_path(system, raw)?;
            Some(identity::anchor_key_path_to_config_dir(expanded, &path))
        }
        None => None,
    };

    Ok(WalkedIdentityFields {
        author_type,
        identity,
        key_path,
        source_config: Some(config),
        source_path: Some(path),
    })
}

/// Walk up from `start_dir` looking for a file with the given name.
/// Returns the path to the first found file, or `None`.
///
/// # Errors
///
/// Returns an error if checking file existence fails.
fn find_file_upward(
    system: &dyn System,
    start_dir: &Path,
    filename: &str,
) -> Result<Option<PathBuf>> {
    let mut current = start_dir.to_path_buf();
    loop {
        let candidate = current.join(filename);
        if system
            .exists(&candidate)
            .with_context(|| format!("checking existence of {}", candidate.display()))?
        {
            return Ok(Some(candidate));
        }
        if !current.pop() {
            return Ok(None);
        }
    }
}

/// Resolve the effective mode for a directory by walking up from `start_dir`
/// looking for the first `.remargin.yaml` — without any `type:` filtering.
///
/// Mode is a directory-tree property, independent of whose identity lives in
/// the config. This function is the clean way to ask "what mode applies
/// here?" without going through the identity machinery (which filters by
/// author type and can fall through to a different config).
///
/// Falls back silently to [`Mode::Open`] when no config is found, matching
/// the CLI's existing open-by-default posture.
///
/// # Errors
///
/// Returns an error if a `.remargin.yaml` exists on the walk but cannot be
/// read or parsed.
pub fn resolve_mode(system: &dyn System, start_dir: &Path) -> Result<ResolvedMode> {
    let mut current = start_dir.to_path_buf();
    loop {
        let candidate = current.join(CONFIG_FILENAME);
        if system
            .exists(&candidate)
            .with_context(|| format!("checking existence of {}", candidate.display()))?
        {
            let content = system
                .read_to_string(&candidate)
                .with_context(|| format!("reading {}", candidate.display()))?;
            let cfg: Config = serde_yaml::from_str(&content)
                .with_context(|| format!("parsing {}", candidate.display()))?;
            if let Some(mode) = cfg.mode {
                return Ok(ResolvedMode {
                    mode,
                    source: Some(candidate),
                });
            }
        }
        if !current.pop() {
            return Ok(ResolvedMode {
                mode: Mode::Open,
                source: None,
            });
        }
    }
}

/// Load config by walking up from `start_dir`.
///
/// Returns `None` if no `.remargin.yaml` was found (defaults to open mode).
///
/// # Errors
///
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load_config(system: &dyn System, start_dir: &Path) -> Result<Option<Config>> {
    load_config_filtered(system, start_dir, None)
}

/// Load config by walking up from `start_dir`, optionally filtering by type.
///
/// If `type_filter` is `Some("human")`, only `.remargin.yaml` files with
/// `type: human` are considered. Files with a different type (or no type
/// field) are skipped and the walk continues upward.
///
/// If `type_filter` is `None`, the first `.remargin.yaml` found wins
/// (existing behavior).
///
/// Returns `None` if no matching config was found in the entire walk.
///
/// # Errors
///
/// Returns an error if a config file exists but cannot be read or parsed.
pub fn load_config_filtered(
    system: &dyn System,
    start_dir: &Path,
    type_filter: Option<&str>,
) -> Result<Option<Config>> {
    Ok(load_config_filtered_with_path(system, start_dir, type_filter)?.map(|(_, cfg)| cfg))
}

/// Like [`load_config_filtered`] but also returns the path to the matching
/// config file. Useful for tooling that needs to report *where* the config
/// was resolved from.
///
/// # Errors
///
/// Returns an error if a config file exists but cannot be read or parsed.
pub fn load_config_filtered_with_path(
    system: &dyn System,
    start_dir: &Path,
    type_filter: Option<&str>,
) -> Result<Option<(PathBuf, Config)>> {
    let mut current = start_dir.to_path_buf();
    loop {
        let candidate = current.join(CONFIG_FILENAME);
        if system
            .exists(&candidate)
            .with_context(|| format!("checking existence of {}", candidate.display()))?
        {
            let content = system
                .read_to_string(&candidate)
                .with_context(|| format!("reading {}", candidate.display()))?;
            let config: Config = serde_yaml::from_str(&content)
                .with_context(|| format!("parsing {}", candidate.display()))?;

            match type_filter {
                None => return Ok(Some((candidate, config))),
                Some(filter) => {
                    if config.author_type.as_deref() == Some(filter) {
                        return Ok(Some((candidate, config)));
                    }
                }
            }
        }
        if !current.pop() {
            return Ok(None);
        }
    }
}

/// Load registry by walking up from `start_dir` (independent from config).
///
/// Returns `None` if no `.remargin-registry.yaml` was found.
///
/// # Errors
///
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load_registry(system: &dyn System, start_dir: &Path) -> Result<Option<Registry>> {
    let path = find_file_upward(system, start_dir, REGISTRY_FILENAME)?;
    match path {
        Some(found) => {
            let content = system
                .read_to_string(&found)
                .with_context(|| format!("reading {}", found.display()))?;
            let reg: Registry = serde_yaml::from_str(&content)
                .with_context(|| format!("parsing {}", found.display()))?;
            Ok(Some(reg))
        }
        None => Ok(None),
    }
}

/// # Errors
///
/// Returns an error for unknown type strings.
/// Parse the canonical lowercase name of an [`AuthorType`] (`"human"` or
/// `"agent"`). Exposed publicly so per-call adapters (MCP tool handlers,
/// future IPC surfaces) can accept the same strings the config loader does
/// and reject unknown values identically.
///
/// # Errors
///
/// Returns an error when `type_str` is not one of the canonical lowercase
/// names.
pub fn parse_author_type(type_str: &str) -> Result<AuthorType> {
    match type_str {
        "human" => Ok(AuthorType::Human),
        "agent" => Ok(AuthorType::Agent),
        other => bail!("unknown author type: {other:?}"),
    }
}

/// Resolve the key path shorthand:
/// - Plain name (no `/` or `~` or `$`) maps to `~/.ssh/<name>`.
/// - Anything else is treated as a literal path and run through
///   [`expand_path`] so `~`, `$VAR`, and `${VAR}` resolve
///   identically to every other path surface.
///
/// # Errors
///
/// Returns an error if the `HOME` environment variable is not set (when
/// resolving a plain name) or if [`expand_path`] rejects the literal
/// form.
pub fn resolve_key_path(system: &dyn System, key: &str) -> Result<PathBuf> {
    if key.contains('/') || key.starts_with('~') || key.contains('$') {
        Ok(expand_path(system, key)?)
    } else {
        let home = system
            .env_var("HOME")
            .context("HOME environment variable not set")?;
        Ok(PathBuf::from(home).join(".ssh").join(key))
    }
}
