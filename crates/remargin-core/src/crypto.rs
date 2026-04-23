//! Checksum (SHA-256) and signature (Ed25519) operations.

mod hex {
    pub fn encode<T: AsRef<[u8]>>(bytes: T) -> String {
        use core::fmt::Write as _;
        let mut out = String::with_capacity(bytes.as_ref().len() * 2);
        for byte in bytes.as_ref() {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }
}

#[cfg(test)]
mod tests;

use core::fmt::Write as _;
use std::path::Path;

use anyhow::{Context as _, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use os_shim::System;
use sha2::{Digest as _, Sha256};
use ssh_key::{HashAlg, PrivateKey, PublicKey};

use crate::kind::canonical_kinds;
use crate::parser::{Comment, Reactions};

/// Namespace used for SSH signature operations (PROTOCOL.sshsig).
const SIGNATURE_NAMESPACE: &str = "remargin";

/// Normalize whitespace for deterministic checksumming.
///
/// 1. Replace `\r\n` with `\n` (CRLF -> LF)
/// 2. Strip trailing whitespace from each line
/// 3. Trim leading and trailing newlines from the whole content
#[must_use]
pub fn normalize_whitespace(content: &str) -> String {
    let lf_only = content.replace("\r\n", "\n");
    let trimmed_lines: Vec<&str> = lf_only.split('\n').map(str::trim_end).collect();
    let joined = trimmed_lines.join("\n");
    let trimmed = joined.trim_matches('\n');
    String::from(trimmed)
}

/// Applies whitespace normalization before hashing; returns `sha256:<hex>`.
///
/// When `kinds` is empty, the hash input is exactly
/// `normalize_whitespace(content)` — byte-for-byte what the
/// pre-`remargin_kind` implementation produced. That equivalence is the
/// back-compat hinge for every comment created before the field
/// existed: they all carry `remargin_kind: []` post-parse, so their
/// stored checksum keeps matching [`verify_checksum`].
///
/// When `kinds` is non-empty, a separator-plus-canonical-list suffix
/// is appended to the normalised content before hashing. The list is
/// [`canonical_kinds`] — sorted + de-duplicated — so `[a, b]` and
/// `[b, a]` produce identical checksums.
#[must_use]
pub fn compute_checksum(content: &str, kinds: &[String]) -> String {
    let mut payload = normalize_whitespace(content);
    if !kinds.is_empty() {
        let canonical = canonical_kinds(kinds);
        // `\x00remargin_kind:` is a structural separator: the NUL byte
        // cannot appear in `content` (it is not a valid markdown
        // character and `normalize_whitespace` leaves no zero bytes
        // either), so a crafted content string cannot forge the same
        // suffix.
        payload.push_str("\x00remargin_kind:");
        payload.push_str(&canonical.join(","));
    }
    let hash = Sha256::digest(payload.as_bytes());
    format!("sha256:{}", hex::encode(hash))
}

/// Returns a string in the format `sha256:<hex>`.
///
/// Reactions are serialized in sorted order (`BTreeMap` guarantees key order,
/// and each author list is sorted before hashing) to produce a deterministic
/// checksum.
#[must_use]
pub fn compute_reaction_checksum(reactions: &Reactions) -> String {
    let mut payload = String::new();
    for (emoji, authors) in reactions {
        let mut sorted_authors = authors.clone();
        sorted_authors.sort();
        let _ = writeln!(payload, "{emoji}:{}", sorted_authors.join(","));
    }
    let hash = Sha256::digest(payload.as_bytes());
    format!("sha256:{}", hex::encode(hash))
}

/// Returns a signature string in the format `ed25519:<base64>`.
///
/// # Errors
///
/// Returns an error if:
/// - The private key file cannot be read
/// - The key is not a valid OpenSSH private key
/// - Signing fails
pub fn compute_signature(
    comment: &Comment,
    private_key_path: &Path,
    system: &dyn System,
) -> Result<String> {
    let key_data = system
        .read_to_string(private_key_path)
        .with_context(|| format!("reading private key from {}", private_key_path.display()))?;
    let private_key = PrivateKey::from_openssh(&key_data)
        .map_err(|err| anyhow::anyhow!("failed to parse private key: {err}"))?;

    let payload = signature_payload(comment);
    let ssh_sig = private_key
        .sign(SIGNATURE_NAMESPACE, HashAlg::Sha256, payload.as_bytes())
        .map_err(|err| anyhow::anyhow!("signing failed: {err}"))?;

    let pem = ssh_sig
        .to_pem(ssh_key::LineEnding::LF)
        .map_err(|err| anyhow::anyhow!("PEM encoding failed: {err}"))?;

    let encoded = BASE64_STANDARD.encode(pem.as_bytes());
    Ok(format!("ed25519:{encoded}"))
}

#[must_use]
pub fn verify_checksum(comment: &Comment) -> bool {
    compute_checksum(&comment.content, &comment.remargin_kind) == comment.checksum
}

/// The `public_key_str` should be an OpenSSH-formatted public key
/// (e.g. `ssh-ed25519 AAAA... comment`).
///
/// # Errors
///
/// Returns an error if:
/// - The public key string cannot be parsed
/// - The signature string is malformed
/// - PEM decoding fails
pub fn verify_signature(comment: &Comment, public_key_str: &str) -> Result<bool> {
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

    let ssh_sig = ssh_key::SshSig::from_pem(&pem_str)
        .map_err(|err| anyhow::anyhow!("failed to parse signature PEM: {err}"))?;

    let public_key = PublicKey::from_openssh(public_key_str)
        .map_err(|err| anyhow::anyhow!("failed to parse public key: {err}"))?;

    let payload = signature_payload(comment);

    match public_key.verify(SIGNATURE_NAMESPACE, payload.as_bytes(), &ssh_sig) {
        Ok(()) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Canonical payload for signing/verification.
///
/// Signed fields (in order): id, author, type, ts, to, reply-to, thread,
/// attachments, `remargin_kind`, content.
///
/// Excluded: reactions, ack, checksum (these are mutable after creation).
///
/// `remargin_kind` contributes zero bytes when the vector is empty, so
/// pre-`remargin_kind` comments sign and verify identically to how they
/// did before the field existed — see the analogous back-compat note on
/// [`compute_checksum`]. Non-empty lists are emitted in
/// [`canonical_kinds`] order so rewrites that reorder the stored list
/// preserve signature validity.
fn signature_payload(comment: &Comment) -> String {
    let mut payload = String::new();
    let _ = writeln!(payload, "id:{}", comment.id);
    let _ = writeln!(payload, "author:{}", comment.author);
    let _ = writeln!(payload, "type:{}", comment.author_type.as_str());
    let _ = writeln!(payload, "ts:{}", comment.ts.to_rfc3339());
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
    for kind in canonical_kinds(&comment.remargin_kind) {
        let _ = writeln!(payload, "remargin_kind:{kind}");
    }
    let _ = write!(
        payload,
        "content:{}",
        normalize_whitespace(&comment.content)
    );
    payload
}
