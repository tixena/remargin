//! Back-sign missing-signature comments authored by the current
//! identity (rem-1ec).
//!
//! Recovery primitive for documents that carry unsigned comments from
//! prior broken code paths (e.g. Obsidian plugin bug rem-ce4 or the
//! fail-fast regression rem-dyz fixed). Signing is a pure additive
//! operation on a comment: the canonical signed payload is computed
//! over fields that do not change post-creation (id, author, type, ts,
//! to, reply-to, thread, attachments, content — see [`crypto`]), so
//! adding a signature to a pre-existing unsigned comment yields a
//! comment that verifies byte-identically against the same registry
//! key.
//!
//! # Forgery guard
//!
//! This op **refuses** to sign any comment whose `author` differs from
//! the resolved identity. The signature is cryptographic proof of
//! authorship; allowing the CLI to sign comments for someone else would
//! be indistinguishable from forgery. The refusal is a hard error
//! before any write — `--ids` entries are validated up front and if any
//! fail the ownership check the whole op bails with a per-id
//! diagnosis.
//!
//! Already-signed comments in the `--ids` selection are reported as
//! skipped (not errored); under `--all-mine` they are simply excluded
//! from the candidate set.
//!
//! # Verify gate
//!
//! The write routes through [`commit_with_verify`] (rem-ef1) so the
//! post-op document must pass the mode-driven severity check — exactly
//! the same gate every other mutating op uses. A `sign` run that would
//! somehow leave the document in a bad state is rejected before any
//! byte hits disk.

#[cfg(test)]
mod tests;

extern crate alloc;

use alloc::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use os_shim::System;

use crate::config::ResolvedConfig;
use crate::crypto::{compute_checksum, compute_signature};
use crate::operations::verify::commit_with_verify;
use crate::parser::{self, Comment, Segment};
use crate::writer;

/// Classified candidate set returned by [`classify_candidates`]: first
/// element is the list of (id, ts) pairs to sign, second is the list of
/// skip entries (already-signed ids listed under `--ids`).
pub(crate) type Classification = (Vec<(String, String)>, Vec<SkippedEntry>);

/// Which comments to consider for signing.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SignSelection {
    /// Sign every comment where `author == resolved_identity` AND
    /// `signature is None`. Comments already signed, or authored by a
    /// different participant, are silently excluded from the candidate
    /// set (not reported as skipped — `--all-mine` is a broad filter,
    /// not an exhaustive list).
    AllMine,
    /// Sign the listed comment ids. Every listed id is validated up
    /// front: ids that do not exist, are not authored by the caller,
    /// or are already signed produce a per-id diagnosis. Non-owned
    /// ids produce a hard error (forgery guard); already-signed ids
    /// produce a skip entry in the result.
    Ids(Vec<String>),
}

/// One entry in [`SignResult::signed`] — a comment the op added a
/// signature to.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SignedEntry {
    /// Comment id.
    pub id: String,
    /// Timestamp carried by the comment (unchanged by this op).
    pub ts: String,
}

/// One entry in [`SignResult::skipped`] — a comment the op left alone.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SkippedEntry {
    /// Comment id.
    pub id: String,
    /// Why the comment was skipped. Canonical values:
    /// `"already_signed"`, `"not_mine"`.
    pub reason: String,
}

/// Result of a [`sign_comments`] call.
///
/// Both lists are empty when the op selected no candidates (for example
/// `--all-mine` on a fully signed document); the caller renders the
/// combined shape without branching.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct SignResult {
    /// Comments whose stored checksum was recomputed from current
    /// content before signing. Populated only when
    /// [`SignOptions::repair_checksum`] is set and the comment's
    /// stored checksum disagreed with the freshly computed value.
    pub repaired: Vec<RepairedChecksumEntry>,
    /// Comments the op signed.
    pub signed: Vec<SignedEntry>,
    /// Comments the op skipped with per-id reason.
    pub skipped: Vec<SkippedEntry>,
}

/// One entry in [`SignResult::repaired`] — a comment whose stored
/// checksum the op recomputed from the current content because the
/// caller passed `--repair-checksum`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RepairedChecksumEntry {
    /// Comment id.
    pub id: String,
    /// The freshly computed value that replaced the stale checksum.
    pub new_checksum: String,
    /// The stale value that was stored on disk before the repair.
    pub old_checksum: String,
}

/// Flags that modify [`sign_comments`] behavior.
///
/// Kept in a struct (instead of loose parameters) because this is the
/// second caller-facing knob beyond [`SignSelection`]; future flags plug
/// in without churning every call site.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct SignOptions {
    /// When `true`, the op recomputes `checksum` from the current
    /// comment content before attaching the signature. The forgery
    /// guard still applies — the caller can only repair a comment they
    /// authored. Intended for the legitimate case where an author
    /// edited a comment's bytes out-of-band and wants to re-vouch for
    /// them; by default the op refuses (the verify gate treats a stale
    /// checksum as tampering).
    pub repair_checksum: bool,
}

/// Back-sign missing-signature comments authored by the current
/// identity.
///
/// The op is idempotent: running it twice back-to-back writes the
/// signatures on the first run and reports zero `signed` / every
/// already-signed id under `skipped` on the second.
///
/// Callers who want to preview the outcome without writing should use
/// `remargin plan sign` (rem-0ry dropped the per-op `--dry-run` flag
/// in favour of the uniform plan projection).
///
/// # Errors
///
/// Returns an error if:
/// - The config has no resolved identity (a signature needs an
///   author).
/// - The config has no resolvable signing key. Note: for strict mode
///   this was already enforced by the resolver (rem-xc8x); `sign`
///   additionally refuses to run in open / registered mode when no
///   key is configured, because its job is to attach one.
/// - The file cannot be read or parsed.
/// - An `--ids` entry does not exist in the document.
/// - An `--ids` entry is authored by someone other than the caller
///   (forgery guard).
/// - The post-op document fails the verify gate.
pub fn sign_comments(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    selection: &SignSelection,
    options: SignOptions,
) -> Result<SignResult> {
    writer::ensure_not_forbidden_target(path)?;

    let identity = config
        .identity
        .as_deref()
        .context("identity is required to sign comments")?;

    // Sign is stricter than create / edit: those ops route through
    // `resolve_signing_key`, which returns `None` in open / registered
    // mode (signing is optional, so a missing key is fine). `sign` has
    // no reason to exist without a key — its job is to attach one. So
    // resolve from `key_path` directly, and bail when unset regardless
    // of mode.
    let key_path = match &config.key_path {
        Some(configured) => configured.clone(),
        None => bail!(
            "sign: no signing key resolved for {identity:?} (mode={:?}). \
             Sign requires a key regardless of mode — pass --key or add \
             a `key:` field to .remargin.yaml.",
            config.mode.as_str(),
        ),
    };

    let mut doc = parser::parse_file(system, path)?;

    // Validate `--ids` up front before touching any comment. Collects
    // the id → kind decision so the write loop is a pure projection
    // of the candidate set.
    //
    // `--repair-checksum` changes the "already signed" rule under
    // `--ids`: the caller is explicitly asking the op to re-vouch for
    // the listed comments, so any stale signature that was already
    // attached is slated for overwrite instead of being reported as
    // skipped. The forgery guard still fires first.
    let (targets, skipped_for_ids) =
        classify_candidates(&doc, identity, selection, options.repair_checksum)?;

    // Sign each target in the parsed document. Because signature_payload
    // excludes ack / reactions / checksum, and `compute_signature` is a
    // pure function of the comment's other fields, the order and
    // grouping of signings is irrelevant.
    //
    // When `options.repair_checksum` is set we recompute the checksum
    // from the current `content` first. The signature payload includes
    // the same (whitespace-normalized) content, so a comment whose
    // bytes were edited out-of-band ends up with a coherent pair:
    // signature attesting to the current content plus a checksum that
    // matches it. The forgery guard above already limited targets to
    // the caller's own comments, so the repair is scoped to comments
    // the caller has authority over.
    let target_ids: HashSet<String> = targets.iter().map(|(id, _)| id.clone()).collect();
    let mut signed = Vec::new();
    let mut repaired = Vec::new();
    for seg in &mut doc.segments {
        if let Segment::Comment(cm) = seg
            && target_ids.contains(&cm.id)
        {
            if options.repair_checksum {
                // Repair uses the comment's current `remargin_kind` so
                // the re-vouch includes the same payload the signature
                // will sign over.
                let fresh = compute_checksum(&cm.content, &cm.remargin_kind);
                if fresh != cm.checksum {
                    repaired.push(RepairedChecksumEntry {
                        id: cm.id.clone(),
                        old_checksum: cm.checksum.clone(),
                        new_checksum: fresh.clone(),
                    });
                    cm.checksum = fresh;
                }
            }
            let sig = compute_signature(cm, &key_path, system)
                .with_context(|| format!("signing comment {:?}", cm.id))?;
            cm.signature = Some(sig);
            signed.push(SignedEntry {
                id: cm.id.clone(),
                ts: cm.ts.to_rfc3339(),
            });
        }
    }

    // Route through the shared verify gate. If for some reason the
    // signed document still reads as `bad` (e.g. a pre-existing bad
    // checksum on an unsigned comment we did NOT sign), the gate
    // trips before any byte reaches disk and the caller can recover.
    let empty: HashSet<String> = HashSet::new();
    commit_with_verify(&doc, config, |verified_doc| {
        writer::write_document(system, path, verified_doc, &empty, &empty)
    })?;

    Ok(SignResult {
        repaired,
        signed,
        skipped: skipped_for_ids,
    })
}

/// Walk the document and split each comment into "to sign" / "skip with
/// reason" / "ignore" based on the selection and ownership. Returns an
/// error when an `--ids` entry does not exist or is authored by someone
/// else — forgery guard refusals fire here.
///
/// `repair_checksum` changes the already-signed rule for
/// [`SignSelection::Ids`]: when `true`, an already-signed target is
/// still slated for processing so the op overwrites both the stale
/// checksum and the now-stale signature. Callers that leave it `false`
/// see the historical behavior — already-signed ids become skip
/// entries. [`SignSelection::AllMine`] is untouched: it is a filter,
/// and a filter that sweeps up every one of the caller's comments
/// would re-sign every existing valid signature on every run.
///
/// Shared between [`sign_comments`] and the `plan sign` projection
/// (rem-7y3) so both surfaces reject and skip under identical rules.
pub(crate) fn classify_candidates(
    doc: &parser::ParsedDocument,
    identity: &str,
    selection: &SignSelection,
    repair_checksum: bool,
) -> Result<Classification> {
    let by_id: BTreeMap<&str, &Comment> = doc
        .comments()
        .into_iter()
        .map(|cm| (cm.id.as_str(), cm))
        .collect();

    match selection {
        SignSelection::AllMine => {
            let mut targets = Vec::new();
            for cm in doc.comments() {
                if cm.author == identity && cm.signature.is_none() {
                    targets.push((cm.id.clone(), cm.ts.to_rfc3339()));
                }
            }
            Ok((targets, Vec::new()))
        }
        SignSelection::Ids(ids) => {
            let mut targets = Vec::new();
            let mut skipped = Vec::new();
            for id in ids {
                let Some(cm) = by_id.get(id.as_str()) else {
                    bail!("sign: comment {id:?} not found");
                };
                if cm.author != identity {
                    bail!(
                        "sign: forgery guard — cannot sign comment {id:?} \
                         authored by {:?}, not {:?}",
                        cm.author,
                        identity,
                    );
                }
                if cm.signature.is_some() && !repair_checksum {
                    skipped.push(SkippedEntry {
                        id: id.clone(),
                        reason: String::from("already_signed"),
                    });
                } else {
                    targets.push((cm.id.clone(), cm.ts.to_rfc3339()));
                }
            }
            Ok((targets, skipped))
        }
    }
}
