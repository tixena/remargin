//! Per-comment integrity resolution and the post-write verify gate.
//!
//! Two responsibilities live here:
//!
//! 1. [`verify_document`] — the pure, registry-driven function that walks
//!    every comment in a [`ParsedDocument`] and returns a row-per-comment
//!    [`RowStatus`] plus an aggregated `ok` flag. The aggregation uses the
//!    mode-driven severity table described in rem-ef1.
//!
//! 2. [`commit_with_verify`] — a one-shot helper every mutating op wraps
//!    its write call in. It runs [`verify_document`] against the staged
//!    in-memory document, and only invokes the caller's write closure when
//!    the report is clean under the active mode. The invariant "no
//!    mutation reaches disk without passing verify" is mechanically
//!    enforced at this one site.
//!
//! The severity table (status × mode → bad?):
//!
//! | status           | Open    | Registered | Strict                               |
//! |------------------|---------|------------|--------------------------------------|
//! | `Valid`          | neutral | neutral    | neutral                              |
//! | `Invalid`        | bad     | bad        | bad                                  |
//! | `Missing`        | neutral | neutral    | bad (for registered active authors)  |
//! | `UnknownAuthor`  | neutral | bad        | bad                                  |
//! | `BadChecksum`    | bad     | bad        | bad                                  |
//!
//! `Invalid` (crypto mismatch) and `BadChecksum` are always bad: those are
//! forgery / corruption signals regardless of mode.

use anyhow::{Result, bail};
use core::fmt::Write as _;

use crate::config::registry::{Registry, RegistryParticipantStatus};
use crate::config::{Mode, ResolvedConfig};
use crate::crypto;
use crate::parser::{Comment, ParsedDocument};

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
    /// Whether [`Comment::checksum`] matches the recomputed SHA-256 of
    /// [`Comment::content`].
    pub checksum_ok: bool,
    /// Comment ID.
    pub id: String,
    /// Per-author-identity signature resolution.
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

        if row_is_bad(
            &cfg.mode,
            checksum_ok,
            signature,
            is_registered_active(cm, cfg.registry.as_ref()),
        ) {
            ok = false;
        }

        results.push(RowStatus {
            id: cm.id.clone(),
            checksum_ok,
            signature,
        });
    }

    VerifyReport { ok, results }
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
            "  {}: checksum={} signature={}",
            row.id,
            chk,
            row.signature.as_str()
        );
    }
    out
}

/// Wrap `write_fn` with a post-mutation verify gate.
///
/// The closure is invoked iff the report against `doc` under `cfg` is
/// clean; otherwise a diagnostic error is returned and `write_fn` is never
/// called. Because every remargin op is an in-memory-then-write pipeline,
/// not calling `write_fn` leaves the on-disk file byte-identical to before
/// the call.
///
/// # Errors
///
/// Returns an error whenever the verify report for `doc` would be `ok ==
/// false` under `cfg.mode`. In that case `write_fn` is not invoked.
/// Otherwise the return value is whatever `write_fn` returns.
pub fn commit_with_verify<F>(doc: &ParsedDocument, cfg: &ResolvedConfig, write_fn: F) -> Result<()>
where
    F: FnOnce(&ParsedDocument) -> Result<()>,
{
    let report = verify_document(doc, cfg);
    if !report.ok {
        bail!("{}", format_report_diagnostic(&report, &cfg.mode));
    }
    write_fn(doc)
}

/// Apply the severity table for a single row.
fn row_is_bad(
    mode: &Mode,
    checksum_ok: bool,
    signature: SignatureStatus,
    registered_active: bool,
) -> bool {
    if !checksum_ok {
        return true;
    }
    if signature == SignatureStatus::Invalid {
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
    // identity-resolve time (rem-xc8x) before the op even ran; but
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

/// True when the comment's author is in the registry with an active
/// status. Used to implement Strict's "missing signature is bad for
/// registered active authors only" rule.
fn is_registered_active(cm: &Comment, registry: Option<&Registry>) -> bool {
    let Some(reg) = registry else { return false };
    reg.participants
        .get(&cm.author)
        .is_some_and(|p| p.status == RegistryParticipantStatus::Active)
}

#[cfg(test)]
mod tests;
