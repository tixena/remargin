//! Checksum (SHA-256) and signature (Ed25519) operations.

mod hex;
mod ssh;

#[cfg(test)]
mod tests;

use core::fmt::Write as _;
use std::path::Path;

use anyhow::{Context as _, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use jiff::Timestamp;
use jiff::tz::Offset;
use os_shim::System;
use sha2::{Digest as _, Sha256};

use crate::crypto::ssh::{PrivateKey, PublicKey, SshSig};
use crate::kind::canonical_kinds;
use crate::parser::CommentDraft;
use crate::reactions::{Reactions, ReactionsExt as _};
use crate::wait::Wait as _;

const SIGNATURE_NAMESPACE: &str = "remargin";

/// Renders `ts` as every stored signature and reaction checksum signed it, in UTC: `+00:00`, never
/// `Z`, and the fraction in 0, 3, 6 or 9 digits, the fewest that hold it. Any other rendering
/// invalidates them, so `Z` and `+00:00` on disk both sign as `+00:00`.
fn signed_rfc3339(ts: Timestamp) -> String {
    let civil = Offset::UTC.to_datetime(ts);
    let nanos = civil.strftime("%9f").to_string();
    let mut digits = nanos.as_str();
    while let Some(shorter) = digits.strip_suffix("000") {
        digits = shorter;
    }
    let fraction = if digits.is_empty() {
        String::new()
    } else {
        format!(".{digits}")
    };
    format!("{}{fraction}+00:00", civil.strftime("%Y-%m-%dT%H:%M:%S"))
}

/// Normalize whitespace for deterministic checksumming.
///
/// 1. Replace `\r\n` with `\n` (CRLF -> LF)
/// 2. Strip trailing whitespace from each line
/// 3. Trim leading and trailing newlines from the whole content
#[must_use = "the return value is the only result of this call"]
pub fn normalize_whitespace(content: &str) -> String {
    let lf_only = content.replace("\r\n", "\n");
    let trimmed_lines: Vec<&str> = lf_only.split('\n').map(str::trim_end).collect();
    let joined = trimmed_lines.join("\n");
    let trimmed = joined.trim_matches('\n');
    String::from(trimmed)
}

/// Applies whitespace normalization before hashing; returns `sha256:<hex>`.
///
/// With no kinds the hash input is exactly `normalize_whitespace(content)`, so a comment that
/// carries no kinds keeps the checksum it would have without the field. With kinds, a separator
/// and the [`canonical_kinds`] list are appended first, so `[a, b]` and `[b, a]` produce
/// identical checksums.
#[must_use = "the return value is the only result of this call"]
pub fn compute_checksum(content: &str, kinds: &[String]) -> String {
    let mut payload = normalize_whitespace(content);
    if !kinds.is_empty() {
        let canonical = canonical_kinds(kinds);
        // The NUL byte cannot appear in normalized content, so no crafted content can forge this
        // suffix.
        payload.push_str("\x00remargin_kind:");
        payload.push_str(&canonical.join(","));
    }
    let hash = Sha256::digest(payload.as_bytes());
    format!("sha256:{}", hex::encode(hash))
}

/// Returns a string in the format `sha256:<hex>`.
///
/// Emojis are walked in `BTreeMap` key order (already sorted). Within
/// each emoji's list, entries are projected to `author@ts` strings,
/// sorted, then joined with commas — so two writers that add the same
/// reactions in different orders produce the same checksum, and the
/// checksum changes when either an author or a timestamp changes.
#[must_use = "the return value is the only result of this call"]
pub fn compute_reaction_checksum(reactions: &Reactions) -> String {
    let mut payload = String::new();
    for (emoji, entries) in reactions.entries_by_emoji() {
        let mut projected: Vec<String> = entries
            .iter()
            .map(|e| format!("{}@{}", e.author, signed_rfc3339(e.ts)))
            .collect();
        projected.sort();
        let _ = writeln!(payload, "{emoji}:{}", projected.join(","));
    }
    let hash = Sha256::digest(payload.as_bytes());
    format!("sha256:{}", hex::encode(hash))
}

/// Returns a signature string in the format `ed25519:<base64>`.
///
/// # Errors
///
/// Returns an error if the private key file cannot be read, is not a valid OpenSSH private
/// key, or signing fails.
pub fn compute_signature(
    comment: &CommentDraft,
    private_key_path: &Path,
    system: &dyn System,
) -> Result<String> {
    let key_data = system
        .read_to_string(private_key_path)
        .wait()
        .with_context(|| format!("reading private key from {}", private_key_path.display()))?;
    let private_key = PrivateKey::from_openssh(&key_data)
        .map_err(|err| anyhow::anyhow!("failed to parse private key: {err}"))?;

    let payload = signature_payload(comment);
    let pem = private_key.sign(SIGNATURE_NAMESPACE, payload.as_bytes());

    let encoded = BASE64_STANDARD.encode(pem.as_bytes());
    Ok(format!("ed25519:{encoded}"))
}

/// Generates a fresh Ed25519 keypair.
///
/// Returns `(private_openssh, public_openssh)` where the private key is
/// an unencrypted `openssh-key-v1` PEM and the public key is an
/// `ssh-ed25519 <base64>` line — both accepted by `ssh-keygen`.
///
/// # Errors
///
/// Returns an error if the OS random source is unavailable.
pub fn generate_keypair(comment: &str) -> Result<(String, String)> {
    let private_key = PrivateKey::generate()?;
    let private_openssh = private_key.to_openssh(comment);
    let public_openssh = private_key.public_key().to_openssh();
    Ok((private_openssh, public_openssh))
}

#[must_use = "a check whose result is ignored checks nothing"]
pub fn verify_checksum(comment: &CommentDraft) -> bool {
    compute_checksum(&comment.content, comment.kinds()) == comment.checksum
}

/// The `public_key_str` should be an OpenSSH-formatted public key
/// (e.g. `ssh-ed25519 AAAA... comment`).
///
/// # Errors
///
/// Returns an error if the public key cannot be parsed, the signature string is malformed, or
/// PEM decoding fails.
pub fn verify_signature(comment: &CommentDraft, public_key_str: &str) -> Result<bool> {
    let signature_str = comment
        .signature
        .as_ref()
        .context("comment has no signature")?;

    let encoded = signature_str
        .strip_prefix("ed25519:")
        .context("signature does not start with 'ed25519:'")?;

    let pem_bytes = BASE64_STANDARD
        .decode(encoded)
        .context("base64 decoding of signature failed")?;
    let pem_str = String::from_utf8(pem_bytes).context("signature PEM is not valid UTF-8")?;

    let ssh_sig = SshSig::from_pem(&pem_str)
        .map_err(|err| anyhow::anyhow!("failed to parse signature PEM: {err}"))?;

    let public_key = PublicKey::from_openssh(public_key_str)
        .map_err(|err| anyhow::anyhow!("failed to parse public key: {err}"))?;

    let payload = signature_payload(comment);

    public_key
        .verify(SIGNATURE_NAMESPACE, payload.as_bytes(), &ssh_sig)
        .or(Ok(false))
}

/// Canonical payload for signing/verification.
///
/// Signed fields (in order): id, author, type, ts, to, reply-to, thread,
/// attachments, `remargin_kind`, content.
///
/// Excluded: reactions, ack and checksum, which change after creation, and `edited_at`: the
/// signature promises content authorship, not edit metadata.
///
/// An empty `remargin_kind` contributes zero bytes, so a comment without kinds signs the same
/// with or without the field. A non-empty list is emitted in [`canonical_kinds`] order, so
/// reordering the stored list keeps the signature valid.
fn signature_payload(comment: &CommentDraft) -> String {
    let mut payload = String::new();
    let _ = writeln!(payload, "id:{}", comment.id);
    let _ = writeln!(payload, "author:{}", comment.author);
    let _ = writeln!(payload, "type:{}", comment.author_type.as_str());
    let _ = writeln!(payload, "ts:{}", signed_rfc3339(comment.ts));
    for recipient in &comment.to {
        let _ = writeln!(payload, "to:{recipient}");
    }
    if let Some(reply_to) = &comment.reply_to {
        let _ = writeln!(payload, "reply-to:{reply_to}");
    }
    if let Some(thread) = &comment.thread {
        let _ = writeln!(payload, "thread:{thread}");
    }
    for attachment in &comment.attachments {
        let _ = writeln!(payload, "attachment:{attachment}");
    }
    for kind in canonical_kinds(comment.kinds()) {
        let _ = writeln!(payload, "remargin_kind:{kind}");
    }
    let _ = write!(
        payload,
        "content:{}",
        normalize_whitespace(&comment.content)
    );
    payload
}
