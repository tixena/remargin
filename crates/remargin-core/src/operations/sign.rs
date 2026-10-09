//! Back-sign missing-signature comments authored by the current
//! identity.
//!
//! Signing is a pure additive operation on a comment: the canonical signed payload is computed
//! over fields that do not change after creation (id, author, type, ts, to, reply-to, thread,
//! attachments, content), so adding a signature to an unsigned comment yields one that verifies
//! against the same registry key.
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
//! The write routes through [`commit_with_verify`] so the
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
use serde_json::{Value, json};

use crate::config::ResolvedConfig;
use crate::crypto::{compute_checksum, compute_signature};
use crate::operations::verify::commit_with_verify;
use crate::parser::{self, Comment, Segment};
use crate::permissions::op_guard::pre_mutate_check_for_caller;
use crate::writer;

/// Classified candidate set returned by [`classify_candidates`]: first
/// element is the list of (id, ts) pairs to sign, second is the list of
/// skip entries (already-signed ids listed under `--ids`).
pub(crate) type Classification = (Vec<(String, String)>, Vec<SkippedEntry>);

/// Which comments to consider for signing.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SignSelection {
    /// Every comment the caller authored that has no signature; signed or foreign comments are
    /// left out silently.
    AllMine,
    /// The listed ids, validated up front: a foreign id is a hard error and an already-signed one
    /// becomes a skip entry.
    Ids(Vec<String>),
}

/// One entry in [`SignResult::signed`] — a comment the op added a
/// signature to.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SignedEntry {
    pub id: String,
    /// Unchanged by this op.
    pub ts: String,
}

/// One entry in [`SignResult::skipped`] — a comment the op left alone.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SkippedEntry {
    pub id: String,
    /// `"already_signed"` or `"not_mine"`.
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
    /// Set only under [`SignOptions::repair_checksum`], for comments whose stored checksum was
    /// stale.
    pub repaired: Vec<RepairedChecksumEntry>,
    pub signed: Vec<SignedEntry>,
    pub skipped: Vec<SkippedEntry>,
}

impl SignResult {
    #[must_use]
    pub fn to_json(&self) -> Value {
        let signed: Vec<Value> = self
            .signed
            .iter()
            .map(|e| json!({ "id": e.id, "ts": e.ts }))
            .collect();
        let skipped: Vec<Value> = self
            .skipped
            .iter()
            .map(|e| json!({ "id": e.id, "reason": e.reason }))
            .collect();
        let repaired: Vec<Value> = self
            .repaired
            .iter()
            .map(|e| {
                json!({
                    "id": e.id,
                    "old_checksum": e.old_checksum,
                    "new_checksum": e.new_checksum,
                })
            })
            .collect();
        json!({ "repaired": repaired, "signed": signed, "skipped": skipped })
    }
}

/// One entry in [`SignResult::repaired`] — a comment whose stored
/// checksum the op recomputed from the current content because the
/// caller passed `--repair-checksum`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RepairedChecksumEntry {
    pub id: String,
    pub new_checksum: String,
    pub old_checksum: String,
}

/// Flags that modify [`sign_comments`] behavior.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct SignOptions {
    /// Recompute `checksum` from the current content before signing, for an author re-vouching
    /// for bytes edited out-of-band. The forgery guard still applies.
    pub repair_checksum: bool,
}

/// Back-sign missing-signature comments authored by the current
/// identity.
///
/// The op is idempotent: running it twice back-to-back writes the
/// signatures on the first run and reports zero `signed` / every
/// already-signed id under `skipped` on the second. `remargin plan sign` previews the outcome
/// without writing.
///
/// # Errors
///
/// Returns an error if the config has no identity or no signing key (whatever the mode), the
/// file cannot be read or parsed, an `--ids` entry is missing or authored by someone else, or
/// the post-op document fails the verify gate.
pub fn sign_comments(
    system: &dyn System,
    path: &Path,
    config: &ResolvedConfig,
    selection: &SignSelection,
    options: SignOptions,
) -> Result<SignResult> {
    writer::ensure_not_forbidden_target(path)?;
    pre_mutate_check_for_caller(system, "sign", path, &config.caller_info())?;

    let identity = config
        .identity
        .as_deref()
        .context("identity is required to sign comments")?;

    // Create and edit treat a missing key as fine outside strict mode; sign exists to attach one,
    // so it bails when `key_path` is unset whatever the mode.
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

    // Validate `--ids` before touching any comment. Under `--repair-checksum` an already-signed
    // id is slated for overwrite instead of being skipped; the forgery guard still fires first.
    let (targets, skipped_for_ids) =
        classify_candidates(&doc, identity, selection, options.repair_checksum)?;

    // With `repair_checksum` the checksum is recomputed from the current content first, so the
    // signature and the checksum attest to the same bytes.
    let target_ids: HashSet<String> = targets.iter().map(|(id, _)| id.clone()).collect();
    let mut signed = Vec::new();
    let mut repaired = Vec::new();
    for seg in &mut doc.segments {
        if let Segment::Comment(cm) = seg
            && target_ids.contains(&cm.id)
        {
            if options.repair_checksum {
                let fresh = compute_checksum(&cm.content, cm.kinds());
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
                ts: parser::rfc3339_z(&cm.ts),
            });
        }
    }

    // The shared verify gate trips before any byte reaches disk if the signed document still fails.
    let empty: HashSet<String> = HashSet::new();
    commit_with_verify(system, &doc, config, path, |verified_doc| {
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
/// checksum and the now-stale signature; when `false` it becomes a skip
/// entry. [`SignSelection::AllMine`] is untouched: it is a filter,
/// and a filter that sweeps up every one of the caller's comments
/// would re-sign every existing valid signature on every run.
///
/// Shared between [`sign_comments`] and the `plan sign` projection
/// so both surfaces reject and skip under identical rules.
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
                    targets.push((cm.id.clone(), parser::rfc3339_z(&cm.ts)));
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
                    targets.push((cm.id.clone(), parser::rfc3339_z(&cm.ts)));
                }
            }
            Ok((targets, skipped))
        }
    }
}

/// Render a [`SignResult`] as human-readable text.
///
/// One line per repaired/signed/skipped entry; when all lists are empty
/// the output is a single `"no candidates"` line.
#[must_use]
pub fn render_sign_result_text(result: &SignResult) -> String {
    use core::fmt::Write as _;
    let mut out = String::new();
    for entry in &result.repaired {
        let _ = writeln!(
            out,
            "repaired checksum: {} ({} -> {})",
            entry.id, entry.old_checksum, entry.new_checksum
        );
    }
    for entry in &result.signed {
        let _ = writeln!(out, "signed: {} (ts={})", entry.id, entry.ts);
    }
    for entry in &result.skipped {
        let _ = writeln!(out, "skipped: {} ({})", entry.id, entry.reason);
    }
    if result.signed.is_empty() && result.skipped.is_empty() && result.repaired.is_empty() {
        out.push_str("no candidates\n");
    }
    out
}
