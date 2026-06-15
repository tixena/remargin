//! Per-comment integrity resolution and the post-write verify gate.
//!
//! Two responsibilities live here:
//!
//! 1. [`verify_document`] — the pure, registry-driven function that walks
//!    every comment in a [`ParsedDocument`] and returns a row-per-comment
//!    [`RowStatus`] plus an aggregated `ok` flag. The aggregation uses
//!    a mode-driven severity table.
//!
//! 2. [`commit_with_verify`] — a one-shot helper every mutating op
//!    wraps its write call in. It compares anomaly sets: P (the
//!    on-disk pre-state) vs Q (the staged in-memory post-state). The
//!    write closure runs iff `Q ⊆ P` — i.e. the op did not introduce
//!    any new anomaly. The invariant "no mutation reaches disk that
//!    introduces a new anomaly" is mechanically enforced at this one
//!    site. Repair ops that strictly reduce the anomaly set always
//!    succeed.
//!
//! The severity table (status × mode → bad?):
//!
//! | status | Open | Registered | Strict |
//! |------------------|---------|------------|--------------------------------------|
//! | `Valid` | neutral | neutral | neutral |
//! | `Invalid` | bad | bad | bad |
//! | `Missing` | neutral | neutral | bad (for registered active authors) |
//! | `UnknownAuthor` | neutral | bad | bad |
//! | `BadChecksum` | bad | bad | bad |
//!
//! `Invalid` (crypto mismatch) and `BadChecksum` are always bad: those are
//! forgery / corruption signals regardless of mode.

extern crate alloc;

use alloc::collections::BTreeMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use core::fmt::Write as _;
use os_shim::System;
use serde::Serialize;
use serde_json::{Value, json};
use thiserror::Error;
use tixschema::model_schema;

use crate::config::registry::{Registry, RegistryParticipantStatus};
use crate::config::{Mode, ResolvedConfig};
use crate::crypto;
use crate::frontmatter;
use crate::parser::{self, Comment, ParsedDocument};
use crate::writer;

const SUMMARY_LINE_LIMIT: usize = 5;

/// Per-comment recipient registry status. Produced by [`verify_document`]
/// and carried in [`RowStatus`].
///
/// `Ok` when all `to:` entries are active registry participants (or the
/// `to:` list is empty). `Unknown` carries the names that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecipientStatus {
    Ok,
    /// One or more `to:` recipients are absent from or revoked in the registry.
    Unknown(Vec<String>),
}

impl RecipientStatus {
    /// Canonical lowercase name for JSON / text output.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// Per-comment signature resolution status. Produced by
/// [`verify_document`] and rendered verbatim in the `signature` column of
/// `remargin verify` / the MCP `verify` tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignatureStatus {
    /// Signature was present but did not match any of the author's active
    /// pubkeys in the registry (or any pubkey at all if the author has no
    /// registered keys).
    Invalid,
    /// Comment has no signature block.
    Missing,
    /// Comment author is not present in the registry at all.
    UnknownAuthor,
    /// Signature matched one of the author's active pubkeys in the
    /// registry.
    Valid,
}

impl SignatureStatus {
    /// Canonical lowercase name, matching the JSON / text output of
    /// `remargin verify`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Missing => "missing",
            Self::UnknownAuthor => "unknown_author",
            Self::Valid => "valid",
        }
    }
}

/// One row of a [`VerifyReport`]: the per-comment resolution for a single
/// comment in the document.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RowStatus {
    pub author: String,
    pub checksum_ok: bool,
    pub id: String,
    pub line: usize,
    pub recipients: RecipientStatus,
    pub signature: SignatureStatus,
}

/// The output of [`verify_document`]: a per-comment row list plus a
/// precomputed aggregate `ok` flag that callers can use directly without
/// reimplementing the mode × status severity rule.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct VerifyReport {
    /// `false` when any row contributes to a failure under the active
    /// [`Mode`]. See module docs for the severity table.
    pub ok: bool,
    /// Per-comment rows, one per parsed comment in document order.
    pub results: Vec<RowStatus>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum RecipientsJson {
    Ok,
    Unknown { unresolved: Vec<String> },
}

#[derive(Serialize)]
struct VerifyRowJson {
    author: String,
    checksum_ok: bool,
    id: String,
    line: usize,
    recipients: RecipientsJson,
    signature: &'static str,
}

#[derive(Serialize)]
struct VerifyReportJson {
    ok: bool,
    results: Vec<VerifyRowJson>,
}

impl VerifyReport {
    #[must_use]
    pub fn to_json(&self) -> Value {
        let results = self
            .results
            .iter()
            .map(|row| VerifyRowJson {
                author: row.author.clone(),
                checksum_ok: row.checksum_ok,
                id: row.id.clone(),
                line: row.line,
                recipients: match &row.recipients {
                    RecipientStatus::Ok => RecipientsJson::Ok,
                    RecipientStatus::Unknown(bad) => RecipientsJson::Unknown {
                        unresolved: bad.clone(),
                    },
                },
                signature: row.signature.as_str(),
            })
            .collect();
        serde_json::to_value(VerifyReportJson {
            ok: self.ok,
            results,
        })
        .unwrap_or(Value::Null)
    }
}

/// Typed verify-gate refusal.
///
/// Carries the failing rows, the active mode and the document path so
/// the CLI / MCP layer can render a human headline + structured machine
/// fields without re-parsing the stringified diagnostic.
#[derive(Debug, Clone, Error)]
#[error("{}", self.legacy_text())]
#[non_exhaustive]
pub struct VerifyFailure {
    /// Failing rows only (rows that contributed to `ok == false` under
    /// the active mode). Order matches document order.
    pub failures: Vec<RowStatus>,
    /// Active mode at refusal time.
    pub mode: Mode,
    /// The document the gate was protecting.
    pub path: PathBuf,
}

impl VerifyFailure {
    /// Build a failure from a parsed document and the active config +
    /// path.
    ///
    /// The verify pass is rerun so the failing rows can be classified
    /// against the registry (the `RowStatus` rows in [`VerifyReport`]
    /// alone do not carry registry membership).
    #[must_use]
    pub fn from_document(doc: &ParsedDocument, cfg: &ResolvedConfig, path: &Path) -> Self {
        let report = verify_document(doc, cfg);
        let comments = doc.comments();
        let mut failures: Vec<RowStatus> = Vec::new();
        for (row, cm) in report.results.iter().zip(comments.iter()) {
            let registered_active = is_registered_active(cm, cfg.registry.as_ref());
            if row_is_bad(
                &cfg.mode,
                row.checksum_ok,
                row.signature,
                registered_active,
                &row.recipients,
            ) {
                failures.push(row.clone());
            }
        }
        Self {
            failures,
            mode: cfg.mode.clone(),
            path: path.to_path_buf(),
        }
    }

    /// One-line plain-English summary suitable for the very top of an
    /// error panel. Counts only failing rows.
    #[must_use]
    pub fn headline(&self) -> String {
        let n = self.failures.len();
        let path = self.path.display();
        if n == 1 {
            format!("verify failed: 1 unsigned or invalid comment in {path}")
        } else {
            format!("verify failed: {n} unsigned or invalid comments in {path}")
        }
    }

    /// Actionable next-step suggestion. Plain prose; safe to append to
    /// the headline when rendering to a single string.
    #[must_use]
    pub fn hint(&self) -> String {
        format!(
            "Try `remargin verify {} --json` for the full breakdown, or contact the document's owner to re-sign legacy entries.",
            self.path.display()
        )
    }

    /// Multi-line human rendering: headline, blank line, summary, blank
    /// line, hint. Suitable for CLI stderr (non-JSON mode).
    #[must_use]
    pub fn human_text(&self) -> String {
        let mut out = self.headline();
        let summary = self.summary_lines();
        if !summary.is_empty() {
            out.push_str("\n\n");
            out.push_str(&summary.join("\n"));
        }
        out.push_str("\n\n");
        out.push_str(&self.hint());
        out
    }

    /// Recreate the legacy `verify failed (mode: …): …` blob so callers
    /// that chain on `format!("{err}")` keep matching. Used by the
    /// `Display` impl through the `thiserror` derive.
    fn legacy_text(&self) -> String {
        let mut out = format!("verify failed (mode: {}):\n", self.mode.as_str());
        for row in &self.failures {
            let chk = if row.checksum_ok { "ok" } else { "FAIL" };
            let recipients_str = match &row.recipients {
                RecipientStatus::Ok => "ok".to_owned(),
                RecipientStatus::Unknown(bad) => format!("unknown({})", bad.join(", ")),
            };
            let _ = writeln!(
                out,
                "  {}: checksum={} signature={} recipients={}",
                row.id,
                chk,
                row.signature.as_str(),
                recipients_str,
            );
        }
        out
    }

    /// Per-failure summary, grouped by signature status, capped at
    /// [`SUMMARY_LINE_LIMIT`] lines with an "and N more" tail when
    /// truncated.
    #[must_use]
    pub fn summary_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut by_status: BTreeMap<&'static str, Vec<&str>> = BTreeMap::new();
        for row in &self.failures {
            let key = if row.checksum_ok {
                row.signature.as_str()
            } else {
                "checksum_FAIL"
            };
            by_status.entry(key).or_default().push(&row.id);
        }
        for (status, ids) in &by_status {
            if lines.len() >= SUMMARY_LINE_LIMIT {
                break;
            }
            let preview = ids.iter().take(5).copied().collect::<Vec<_>>().join(", ");
            let extra = if ids.len() > 5 {
                format!(" (and {} more)", ids.len() - 5)
            } else {
                String::new()
            };
            lines.push(format!("- {preview}: {status}{extra}"));
        }
        let total_groups = by_status.len();
        if total_groups > lines.len() {
            lines.push(format!(
                "- and {} more group(s)",
                total_groups - lines.len()
            ));
        }
        lines
    }

    /// Structured JSON shape: `error_kind`, `headline`, `failures`,
    /// `hint`, `mode`, `path`. Suitable for CLI `--json` and MCP tool
    /// errors.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let failures = self
            .failures
            .iter()
            .map(|row| VerifyFailureRow {
                checksum_ok: row.checksum_ok,
                id: row.id.clone(),
                recipients: row.recipients.as_str().to_owned(),
                signature: row.signature.as_str().to_owned(),
            })
            .collect();
        serde_json::to_value(VerifyFailurePayload {
            error_kind: VerifyErrorKind::VerifyFailed,
            failures,
            headline: self.headline(),
            hint: self.hint(),
            mode: self.mode.as_str().to_owned(),
            path: self.path.display().to_string(),
        })
        .unwrap_or(Value::Null)
    }
}

/// `error_kind` discriminant for the verify-gate refusal payload.
#[derive(Serialize)]
#[non_exhaustive]
#[model_schema]
pub enum VerifyErrorKind {
    #[serde(rename = "verify_failed")]
    VerifyFailed,
}

/// One failing row in a [`VerifyFailure`] JSON payload.
#[derive(Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct VerifyFailureRow {
    pub checksum_ok: bool,
    pub id: String,
    pub recipients: String,
    pub signature: String,
}

/// JSON projection of a [`VerifyFailure`] verify-gate refusal.
#[derive(Serialize)]
#[non_exhaustive]
#[model_schema]
pub struct VerifyFailurePayload {
    pub error_kind: VerifyErrorKind,
    pub failures: Vec<VerifyFailureRow>,
    pub headline: String,
    pub hint: String,
    pub mode: String,
    pub path: String,
}

/// Verifier-detected anomaly, identified by `(comment_id, kind)`.
///
/// The subset gate uses this pair as the stable identity: anomalies in
/// the post-mutation state must all be present (under the same pair)
/// in the pre-mutation state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Anomaly {
    /// Comment ID the anomaly is attached to.
    pub id: String,
    /// What kind of anomaly fired on this comment.
    pub kind: AnomalyKind,
}

/// Disjoint anomaly kinds the verify pass emits. Severity is
/// mode-driven via [`row_is_bad`]; the kind itself is mode-independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AnomalyKind {
    /// Recomputed content checksum did not match the stored one.
    ChecksumInvalid,
    /// At least one `to:` recipient is absent from or revoked in the
    /// registry.
    RecipientUnknown,
    /// Signature block present but did not match any active pubkey.
    SignatureInvalid,
    /// No signature block.
    SignatureMissing,
    /// Author is not in the registry.
    SignatureUnknownAuthor,
}

impl AnomalyKind {
    /// Stable string for diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChecksumInvalid => "checksum_invalid",
            Self::RecipientUnknown => "recipient_unknown",
            Self::SignatureInvalid => "signature_invalid",
            Self::SignatureMissing => "signature_missing",
            Self::SignatureUnknownAuthor => "signature_unknown_author",
        }
    }
}

/// Subset-gate refusal.
///
/// Raised by [`commit_with_verify`] when the in-memory post-mutation
/// anomaly set introduces entries not present in the on-disk
/// pre-mutation set (`Q ⊄ P`).
#[derive(Debug, Clone, Error)]
#[error("{}", self.legacy_text())]
#[non_exhaustive]
pub struct SubsetGateFailure {
    /// `Q \ P`: anomalies in the post-state that weren't in the pre-state.
    pub introduced: Vec<Anomaly>,
    /// Active mode at refusal time.
    pub mode: Mode,
    /// Document the gate was protecting.
    pub path: PathBuf,
}

impl SubsetGateFailure {
    /// One-line plain-English summary.
    #[must_use]
    pub fn headline(&self) -> String {
        let n = self.introduced.len();
        let path = self.path.display();
        if n == 1 {
            format!("op would introduce 1 new anomaly in {path}")
        } else {
            format!("op would introduce {n} new anomalies in {path}")
        }
    }

    /// Actionable next-step.
    #[must_use]
    pub fn hint(&self) -> String {
        format!(
            "Try `remargin verify {} --json` for the full breakdown.",
            self.path.display()
        )
    }

    fn legacy_text(&self) -> String {
        let mut out = format!("verify failed (mode: {}):\n", self.mode.as_str());
        for a in &self.introduced {
            let _ = writeln!(out, "  {}: introduced {}", a.id, a.kind.as_str());
        }
        out
    }

    /// JSON shape for `--json` / MCP tool errors.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let introduced: Vec<Value> = self
            .introduced
            .iter()
            .map(|a| json!({ "id": a.id, "kind": a.kind.as_str() }))
            .collect();
        json!({
            "error_kind": "subset_gate_failed",
            "introduced": introduced,
            "headline": self.headline(),
            "hint": self.hint(),
            "mode": self.mode.as_str(),
            "path": self.path.display().to_string(),
        })
    }
}

/// Walk every comment in `doc` and produce a [`VerifyReport`] under the
/// active mode and registry taken from `cfg`.
///
/// This function is pure: it does not read the filesystem and does not
/// mutate anything. The registry is taken from `cfg.registry`; if no
/// registry is present the behaviour is:
///
/// - In [`Mode::Open`] every signature resolves to [`SignatureStatus::Missing`]
///   (when no signature block exists) or [`SignatureStatus::Invalid`] (when
///   a signature block exists but cannot be matched against any key). No
///   row is "bad" in Open mode unless the checksum is bad or the signature
///   was crypto-invalid.
/// - In [`Mode::Registered`] / [`Mode::Strict`] all authors resolve to
///   [`SignatureStatus::UnknownAuthor`] (bad in those modes).
#[must_use]
pub fn verify_document(doc: &ParsedDocument, cfg: &ResolvedConfig) -> VerifyReport {
    let mut results: Vec<RowStatus> = Vec::new();
    let mut ok = true;

    for cm in &doc.comments() {
        let checksum_ok = crypto::verify_checksum(cm);
        let signature = resolve_signature(cm, cfg.registry.as_ref());
        let recipients = resolve_recipients(cm, cfg.registry.as_ref());

        if row_is_bad(
            &cfg.mode,
            checksum_ok,
            signature,
            is_registered_active(cm, cfg.registry.as_ref()),
            &recipients,
        ) {
            ok = false;
        }

        results.push(RowStatus {
            author: cm.author.clone(),
            checksum_ok,
            id: cm.id.clone(),
            line: cm.line,
            recipients,
            signature,
        });
    }

    VerifyReport { ok, results }
}

/// Derive the anomaly set for `doc` under `cfg`.
///
/// Only rows that would flip `report.ok` to `false` under the active
/// mode contribute. Distinct anomalies can co-fire on the same comment
/// (e.g. a bad checksum *and* a missing signature) and are returned as
/// separate entries.
#[must_use]
pub fn anomalies_for_doc(doc: &ParsedDocument, cfg: &ResolvedConfig) -> HashSet<Anomaly> {
    let mut out = HashSet::new();
    let report = verify_document(doc, cfg);
    for (row, cm) in report.results.iter().zip(doc.comments().iter()) {
        let registered_active = is_registered_active(cm, cfg.registry.as_ref());
        if !row_is_bad(
            &cfg.mode,
            row.checksum_ok,
            row.signature,
            registered_active,
            &row.recipients,
        ) {
            continue;
        }
        if !row.checksum_ok {
            out.insert(Anomaly {
                id: row.id.clone(),
                kind: AnomalyKind::ChecksumInvalid,
            });
        }
        match row.signature {
            SignatureStatus::Invalid => {
                out.insert(Anomaly {
                    id: row.id.clone(),
                    kind: AnomalyKind::SignatureInvalid,
                });
            }
            SignatureStatus::Missing => {
                if matches!(cfg.mode, Mode::Strict) && registered_active {
                    out.insert(Anomaly {
                        id: row.id.clone(),
                        kind: AnomalyKind::SignatureMissing,
                    });
                }
            }
            SignatureStatus::UnknownAuthor => {
                if matches!(cfg.mode, Mode::Registered | Mode::Strict) {
                    out.insert(Anomaly {
                        id: row.id.clone(),
                        kind: AnomalyKind::SignatureUnknownAuthor,
                    });
                }
            }
            SignatureStatus::Valid => {}
        }
        if matches!(row.recipients, RecipientStatus::Unknown(_))
            && matches!(cfg.mode, Mode::Registered | Mode::Strict)
        {
            out.insert(Anomaly {
                id: row.id.clone(),
                kind: AnomalyKind::RecipientUnknown,
            });
        }
    }
    out
}

/// Run [`verify_document`] and refresh `remargin_*` frontmatter on
/// disk only when [`frontmatter::ensure_frontmatter`] would change it.
/// Integrity status surfaces in `report.ok`, never as `Err`.
///
/// # Errors
///
/// Parse, mode-escalation, frontmatter, or write errors.
pub fn verify_and_refresh(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
) -> Result<VerifyReport> {
    let mut doc = parser::parse_file(system, path)?;
    let escalated = config.escalate_mode_for_doc(system, path)?;

    let report = verify_document(&doc, &escalated);

    let before_fm = frontmatter::parse_existing_frontmatter(&doc)
        .context("snapshotting current frontmatter")?;
    frontmatter::ensure_frontmatter(&mut doc, &escalated)?;
    let after_fm = frontmatter::parse_existing_frontmatter(&doc)
        .context("snapshotting recomputed frontmatter")?;

    if before_fm != after_fm {
        let empty: HashSet<String> = HashSet::new();
        writer::write_document(system, path, &doc, &empty, &empty)?;
    }

    Ok(report)
}

/// Format a [`VerifyReport`] as a human-readable diagnostic.
///
/// Suitable for error messages when the post-write gate trips. The format
/// mirrors the CLI's text output so operators see the same rendering in
/// failures and in successful runs.
#[must_use]
pub fn format_report_diagnostic(report: &VerifyReport, mode: &Mode) -> String {
    let mut out = format!("verify failed (mode: {}):\n", mode.as_str());
    for row in &report.results {
        let chk = if row.checksum_ok { "ok" } else { "FAIL" };
        let _ = writeln!(
            out,
            "  {}: checksum={} signature={} recipients={}",
            row.id,
            chk,
            row.signature.as_str(),
            row.recipients.as_str(),
        );
    }
    out
}

/// Wrap `write_fn` with the post-mutation subset gate.
///
/// Computes P (anomalies on the on-disk file at `path`) and Q
/// (anomalies in the in-memory candidate `doc`). The closure is
/// invoked iff `Q ⊆ P` — the mutation did not introduce any new
/// anomaly. Repair ops that strictly reduce the anomaly set
/// (`Q ⊂ P`) and no-op identity transformations (`Q == P`) both
/// succeed. Damaging ops (`Q ⊄ P`) are refused before any byte
/// touches disk.
///
/// # Errors
///
/// Returns a downcastable [`SubsetGateFailure`] when `Q ⊄ P`. The
/// CLI / MCP layer pulls the typed shape out for structured
/// presentation. In that case `write_fn` is not invoked.
/// Otherwise the return value is whatever `write_fn` returns.
pub fn commit_with_verify<F>(
    system: &dyn System,
    doc: &ParsedDocument,
    cfg: &ResolvedConfig,
    path: &Path,
    write_fn: F,
) -> Result<()>
where
    F: FnOnce(&ParsedDocument) -> Result<()>,
{
    let realm_cfg = cfg.escalate_mode_for_doc(system, path)?;

    // P: anomalies the on-disk file already has. An absent file (or
    // unparsable bytes) means P = empty — fresh writes start clean.
    let pre = pre_anomalies(system, path, &realm_cfg);

    // Q: anomalies the in-memory candidate would have.
    let post = anomalies_for_doc(doc, &realm_cfg);

    // Subset gate: refuse iff Q ⊄ P (anything in Q that wasn't in P
    // is a NEW anomaly this op would introduce).
    let mut introduced: Vec<Anomaly> = post.difference(&pre).cloned().collect();
    if !introduced.is_empty() {
        introduced.sort_by(|a, b| {
            a.id.cmp(&b.id)
                .then_with(|| a.kind.as_str().cmp(b.kind.as_str()))
        });
        return Err(SubsetGateFailure {
            introduced,
            mode: realm_cfg.mode,
            path: path.to_path_buf(),
        }
        .into());
    }
    write_fn(doc)
}

/// Apply the severity table for a single row.
fn row_is_bad(
    mode: &Mode,
    checksum_ok: bool,
    signature: SignatureStatus,
    registered_active: bool,
    recipients: &RecipientStatus,
) -> bool {
    if !checksum_ok {
        return true;
    }
    if signature == SignatureStatus::Invalid {
        return true;
    }
    // Unknown recipients are bad in registered/strict, neutral in open —
    // same column as UnknownAuthor in the severity table.
    if matches!(recipients, RecipientStatus::Unknown(_))
        && matches!(mode, Mode::Registered | Mode::Strict)
    {
        return true;
    }
    match mode {
        Mode::Open => false,
        Mode::Registered => signature == SignatureStatus::UnknownAuthor,
        Mode::Strict => match signature {
            SignatureStatus::UnknownAuthor => true,
            SignatureStatus::Missing => registered_active,
            SignatureStatus::Invalid | SignatureStatus::Valid => false,
        },
    }
}

/// Resolve the signature status for a single comment against the optional
/// registry.
fn resolve_signature(cm: &Comment, registry: Option<&Registry>) -> SignatureStatus {
    let Some(reg) = registry else {
        // No registry is present. The best we can do is say the comment has
        // no signature; a present-but-unverifiable signature is `Invalid`
        // because we cannot match it against anything. `UnknownAuthor` is
        // reserved for the case where a registry exists but does not list
        // the author.
        return if cm.signature.is_none() {
            SignatureStatus::Missing
        } else {
            // A signature exists but we cannot validate it — this is a
            // crypto mismatch from the verifier's perspective.
            SignatureStatus::Invalid
        };
    };

    let Some(participant) = reg.participants.get(&cm.author) else {
        return SignatureStatus::UnknownAuthor;
    };

    if cm.signature.is_none() {
        return SignatureStatus::Missing;
    }

    // Only active pubkeys count. Revoked participants were rejected at
    // identity-resolve time before the op even ran; but
    // historical signed comments from a now-revoked participant should
    // still resolve as `UnknownAuthor` because none of their keys are
    // active anymore.
    if participant.status != RegistryParticipantStatus::Active {
        return SignatureStatus::UnknownAuthor;
    }

    if participant.pubkeys.is_empty() {
        // Author is registered but has no keys: a present signature
        // cannot match.
        return SignatureStatus::Invalid;
    }

    for pubkey in &participant.pubkeys {
        if matches!(crypto::verify_signature(cm, pubkey), Ok(true)) {
            return SignatureStatus::Valid;
        }
    }
    SignatureStatus::Invalid
}

/// Resolve the recipient registry status for a single comment.
///
/// Returns [`RecipientStatus::Ok`] when `cm.to` is empty (broadcast) or
/// every listed recipient is an active participant in `registry`.
/// Returns [`RecipientStatus::Unknown`] when any recipient is absent
/// from or revoked in the registry, or when `registry` is absent and
/// `cm.to` is non-empty (no registry → cannot validate).
fn resolve_recipients(cm: &Comment, registry: Option<&Registry>) -> RecipientStatus {
    if cm.to.is_empty() {
        return RecipientStatus::Ok;
    }
    let Some(reg) = registry else {
        return RecipientStatus::Unknown(cm.to.clone());
    };
    let bad: Vec<String> = cm
        .to
        .iter()
        .filter(|r| !reg.is_active(r))
        .cloned()
        .collect();
    if bad.is_empty() {
        RecipientStatus::Ok
    } else {
        RecipientStatus::Unknown(bad)
    }
}

/// True when the comment's author is in the registry with an active
/// status. Used to implement Strict's "missing signature is bad for
/// registered active authors only" rule.
fn is_registered_active(cm: &Comment, registry: Option<&Registry>) -> bool {
    let Some(reg) = registry else { return false };
    reg.participants
        .get(&cm.author)
        .is_some_and(|p| p.status == RegistryParticipantStatus::Active)
}

/// Compute the pre-mutation anomaly set from the on-disk file. Missing
/// or unparsable files yield an empty set so fresh writes don't hit a
/// spurious gate.
fn pre_anomalies(system: &dyn System, path: &Path, cfg: &ResolvedConfig) -> HashSet<Anomaly> {
    let Ok(existing) = system.read_to_string(path) else {
        return HashSet::new();
    };
    let Ok(existing_doc) = parser::parse(&existing) else {
        return HashSet::new();
    };
    anomalies_for_doc(&existing_doc, cfg)
}

#[cfg(test)]
mod tests;
