//! Minimal OpenSSH wire-format support for unencrypted Ed25519 keys.
//!
//! Reimplements exactly the slice of OpenSSH key handling that remargin
//! needs — parsing `openssh-key-v1` private keys and `ssh-ed25519`
//! public keys, plus producing and verifying `PROTOCOL.sshsig`
//! signatures — directly on `ed25519-dalek`, so the dependency graph no
//! longer pulls in `ssh-key` (and its optional, advisory-flagged `rsa`).
//!
//! The on-disk encodings here are byte-compatible with `ssh-keygen`:
//! private keys round-trip through `ssh-keygen`, and signatures verify
//! under `ssh-keygen -Y verify`.

use core::str::from_utf8;

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier as _, VerifyingKey};
use rand_core::OsRng;
use sha2::{Digest as _, Sha256};

/// Algorithm identifier for Ed25519 keys and signatures.
const ED25519_ID: &[u8] = b"ssh-ed25519";
/// Magic prefix for the `openssh-key-v1` private-key container.
const OPENSSH_MAGIC: &[u8] = b"openssh-key-v1\0";
/// PEM label for an `SSHSIG` signature.
const SSHSIG_LABEL: &str = "SSH SIGNATURE";
/// Magic preamble for the `PROTOCOL.sshsig` blob (6 literal bytes).
const SSHSIG_MAGIC: &[u8] = b"SSHSIG";
/// `SSHSIG` wire-format version.
const SSHSIG_VERSION: u32 = 1;

/// A parsed Ed25519 signing key.
pub struct PrivateKey {
    inner: SigningKey,
}

/// A parsed Ed25519 verifying key.
pub struct PublicKey {
    inner: VerifyingKey,
}

/// A parsed `SSHSIG` signature.
pub struct SshSig {
    hash_alg: String,
    namespace: String,
    public_key: VerifyingKey,
    signature: Signature,
}

/// Sequential reader over an SSH wire-format byte buffer.
struct Reader<'buf> {
    buf: &'buf [u8],
    pos: usize,
}

impl PrivateKey {
    /// Parses an unencrypted `openssh-key-v1` Ed25519 private key.
    ///
    /// # Errors
    ///
    /// Returns an error if the PEM is malformed, the key is encrypted,
    /// holds anything other than a single Ed25519 key, or fails the
    /// internal consistency checks (check-ints, key length).
    pub fn from_openssh(pem: &str) -> Result<Self> {
        let raw = decode_pem(pem, "OPENSSH PRIVATE KEY")?;
        let mut reader = Reader::new(&raw);

        let magic = reader.take(OPENSSH_MAGIC.len())?;
        if magic != OPENSSH_MAGIC {
            bail!("not an openssh-key-v1 key");
        }
        let cipher = reader.read_string()?;
        let kdf = reader.read_string()?;
        let _kdf_opts = reader.read_string()?;
        if cipher != b"none" || kdf != b"none" {
            bail!("encrypted private keys are not supported");
        }
        if reader.read_u32()? != 1 {
            bail!("expected exactly one key");
        }
        let _public_section = reader.read_string()?;

        let private_section = reader.read_string()?;
        let mut priv_reader = Reader::new(private_section);
        let check1 = priv_reader.read_u32()?;
        let check2 = priv_reader.read_u32()?;
        if check1 != check2 {
            bail!("private key check-ints do not match");
        }
        let key_type = priv_reader.read_string()?;
        if key_type != ED25519_ID {
            bail!("only ssh-ed25519 keys are supported");
        }
        let _public = priv_reader.read_string()?;
        let keypair = priv_reader.read_string()?;
        if keypair.len() != 64 {
            bail!("unexpected Ed25519 private key length");
        }
        let seed: [u8; 32] = keypair[..32].try_into().context("reading key seed")?;
        Ok(Self {
            inner: SigningKey::from_bytes(&seed),
        })
    }

    /// Generates a fresh Ed25519 signing key from the OS RNG.
    #[must_use]
    pub fn generate() -> Self {
        Self {
            inner: SigningKey::generate(&mut OsRng),
        }
    }

    /// Returns the matching public key.
    #[must_use]
    pub fn public_key(&self) -> PublicKey {
        PublicKey {
            inner: self.inner.verifying_key(),
        }
    }

    /// Signs `message` under `namespace`, returning a base64 PEM-armored
    /// `SSHSIG` (`-----BEGIN SSH SIGNATURE-----`).
    #[must_use]
    pub fn sign(&self, namespace: &str, message: &[u8]) -> String {
        let signature = self.inner.sign(&signed_data(namespace, message));
        let public = self.inner.verifying_key();

        let mut sig_blob = Vec::new();
        write_string(&mut sig_blob, ED25519_ID);
        write_string(&mut sig_blob, &signature.to_bytes());

        let mut blob = Vec::new();
        blob.extend_from_slice(SSHSIG_MAGIC);
        write_u32(&mut blob, SSHSIG_VERSION);
        write_string(&mut blob, &public_key_blob(&public));
        write_string(&mut blob, namespace.as_bytes());
        write_string(&mut blob, b"");
        write_string(&mut blob, b"sha256");
        write_string(&mut blob, &sig_blob);

        encode_pem(SSHSIG_LABEL, &blob)
    }

    /// Serializes the key as an unencrypted `openssh-key-v1` PEM
    /// (`-----BEGIN OPENSSH PRIVATE KEY-----`) with the given comment.
    #[must_use]
    pub fn to_openssh(&self, comment: &str) -> String {
        let public = self.inner.verifying_key();
        let pub_blob = public_key_blob(&public);

        let mut private_section = Vec::new();
        write_u32(&mut private_section, 0_u32);
        write_u32(&mut private_section, 0_u32);
        write_string(&mut private_section, ED25519_ID);
        write_string(&mut private_section, public.as_bytes());
        let mut keypair = Vec::with_capacity(64);
        keypair.extend_from_slice(&self.inner.to_bytes());
        keypair.extend_from_slice(public.as_bytes());
        write_string(&mut private_section, &keypair);
        write_string(&mut private_section, comment.as_bytes());
        let mut pad: u8 = 1;
        while private_section.len() % 8 != 0 {
            private_section.push(pad);
            pad += 1;
        }

        let mut blob = Vec::new();
        blob.extend_from_slice(OPENSSH_MAGIC);
        write_string(&mut blob, b"none");
        write_string(&mut blob, b"none");
        write_string(&mut blob, b"");
        write_u32(&mut blob, 1_u32);
        write_string(&mut blob, &pub_blob);
        write_string(&mut blob, &private_section);

        encode_pem("OPENSSH PRIVATE KEY", &blob)
    }
}

impl PublicKey {
    /// Parses an `ssh-ed25519 <base64> [comment]` public key line.
    ///
    /// # Errors
    ///
    /// Returns an error if the line is not a well-formed `ssh-ed25519`
    /// key or carries a malformed key blob.
    pub fn from_openssh(line: &str) -> Result<Self> {
        let mut fields = line.split_whitespace();
        let algo = fields.next().context("public key is empty")?;
        if algo != "ssh-ed25519" {
            bail!("only ssh-ed25519 public keys are supported");
        }
        let b64 = fields.next().context("public key missing key data")?;
        let blob = BASE64_STANDARD
            .decode(b64)
            .context("base64 decoding public key failed")?;
        let mut reader = Reader::new(&blob);
        let key_type = reader.read_string()?;
        if key_type != ED25519_ID {
            bail!("public key blob is not ssh-ed25519");
        }
        let key_bytes = reader.read_string()?;
        Ok(Self {
            inner: parse_verifying_key(key_bytes)?,
        })
    }

    /// Serializes the key as an `ssh-ed25519 <base64>` line (no comment).
    #[must_use]
    pub fn to_openssh(&self) -> String {
        let encoded = BASE64_STANDARD.encode(public_key_blob(&self.inner));
        format!("ssh-ed25519 {encoded}")
    }

    /// Verifies `sig` over `message` under `namespace`.
    ///
    /// Returns `Ok(true)` on a valid signature, `Ok(false)` on a
    /// cryptographically invalid one.
    ///
    /// # Errors
    ///
    /// Returns an error only on a namespace/hash mismatch that makes the
    /// signature inapplicable rather than merely invalid.
    pub fn verify(&self, namespace: &str, message: &[u8], sig: &SshSig) -> Result<bool> {
        if sig.namespace != namespace {
            bail!("signature namespace mismatch");
        }
        if sig.hash_alg != "sha256" {
            bail!("unsupported signature hash algorithm: {}", sig.hash_alg);
        }
        if sig.public_key != self.inner {
            return Ok(false);
        }
        let data = signed_data(namespace, message);
        Ok(self.inner.verify(&data, &sig.signature).is_ok())
    }
}

impl SshSig {
    /// Parses a PEM-armored `SSHSIG` (`-----BEGIN SSH SIGNATURE-----`).
    ///
    /// # Errors
    ///
    /// Returns an error if the PEM is malformed, the magic/version are
    /// wrong, or the embedded key/signature blobs are not Ed25519.
    pub fn from_pem(pem: &str) -> Result<Self> {
        let raw = decode_pem(pem, SSHSIG_LABEL)?;
        let mut reader = Reader::new(&raw);
        let magic = reader.take(SSHSIG_MAGIC.len())?;
        if magic != SSHSIG_MAGIC {
            bail!("not an SSHSIG signature");
        }
        if reader.read_u32()? != SSHSIG_VERSION {
            bail!("unsupported SSHSIG version");
        }
        let public_key_blob = reader.read_string()?;
        let namespace = reader.read_string()?;
        let _reserved = reader.read_string()?;
        let hash_alg = reader.read_string()?;
        let signature_blob = reader.read_string()?;

        let mut pk_reader = Reader::new(public_key_blob);
        if pk_reader.read_string()? != ED25519_ID {
            bail!("embedded public key is not ssh-ed25519");
        }
        let public_key = parse_verifying_key(pk_reader.read_string()?)?;

        let mut sig_reader = Reader::new(signature_blob);
        if sig_reader.read_string()? != ED25519_ID {
            bail!("signature is not ssh-ed25519");
        }
        let sig_bytes: [u8; 64] = sig_reader
            .read_string()?
            .try_into()
            .map_err(|_ignored| anyhow::anyhow!("Ed25519 signature must be 64 bytes"))?;

        Ok(Self {
            hash_alg: String::from_utf8(hash_alg.to_vec())
                .context("hash algorithm is not UTF-8")?,
            namespace: String::from_utf8(namespace.to_vec()).context("namespace is not UTF-8")?,
            public_key,
            signature: Signature::from_bytes(&sig_bytes),
        })
    }
}

impl<'buf> Reader<'buf> {
    const fn new(buf: &'buf [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn read_string(&mut self) -> Result<&'buf [u8]> {
        let len = self.read_u32()? as usize;
        self.take(len)
    }

    fn read_u32(&mut self) -> Result<u32> {
        let [b0, b1, b2, b3]: [u8; 4] = self
            .take(4)?
            .try_into()
            .context("reading u32 from SSH wire data")?;
        Ok((u32::from(b0) << 24) | (u32::from(b1) << 16) | (u32::from(b2) << 8) | u32::from(b3))
    }

    fn take(&mut self, len: usize) -> Result<&'buf [u8]> {
        let end = self.pos.checked_add(len).context("length overflow")?;
        if end > self.buf.len() {
            bail!("unexpected end of SSH wire data");
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
}

/// Decodes a PEM document, checking its label, and returns the body bytes.
fn decode_pem(pem: &str, label: &str) -> Result<Vec<u8>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let body: String = pem
        .lines()
        .skip_while(|line| line.trim() != begin)
        .skip(1)
        .take_while(|line| line.trim() != end)
        .collect();
    if body.is_empty() {
        bail!("missing PEM body for {label}");
    }
    BASE64_STANDARD
        .decode(body)
        .with_context(|| format!("base64 decoding {label} failed"))
}

/// Encodes `data` as a PEM document with 70-column base64 lines (the
/// width `ssh-keygen` emits for `SSHSIG` and `openssh-key-v1`).
fn encode_pem(label: &str, data: &[u8]) -> String {
    let encoded = BASE64_STANDARD.encode(data);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in encoded.as_bytes().chunks(70) {
        out.push_str(from_utf8(chunk).unwrap_or_default());
        out.push('\n');
    }
    out.push_str("-----END ");
    out.push_str(label);
    out.push_str("-----\n");
    out
}

/// Parses raw Ed25519 public key bytes into a `VerifyingKey`.
fn parse_verifying_key(bytes: &[u8]) -> Result<VerifyingKey> {
    let array: [u8; 32] = bytes
        .try_into()
        .map_err(|_ignored| anyhow::anyhow!("Ed25519 public key must be 32 bytes"))?;
    VerifyingKey::from_bytes(&array).context("invalid Ed25519 public key")
}

/// Encodes an Ed25519 public key as the SSH `ssh-ed25519` key blob.
fn public_key_blob(key: &VerifyingKey) -> Vec<u8> {
    let mut blob = Vec::new();
    write_string(&mut blob, ED25519_ID);
    write_string(&mut blob, key.as_bytes());
    blob
}

/// Builds the `PROTOCOL.sshsig` signed-data blob for `message`.
///
/// Layout: `MAGIC || string(namespace) || string(reserved) ||
/// string(hash_alg) || string(sha256(message))`.
fn signed_data(namespace: &str, message: &[u8]) -> Vec<u8> {
    let digest = Sha256::digest(message);
    let mut blob = Vec::new();
    blob.extend_from_slice(SSHSIG_MAGIC);
    write_string(&mut blob, namespace.as_bytes());
    write_string(&mut blob, b"");
    write_string(&mut blob, b"sha256");
    write_string(&mut blob, &digest);
    blob
}

/// Appends a `u32` as four big-endian bytes (SSH wire byte order).
fn write_u32(out: &mut Vec<u8>, value: u32) {
    let mask = u32::from(u8::MAX);
    let zero: u8 = 0;
    out.push(u8::try_from((value >> 24_u32) & mask).unwrap_or(zero));
    out.push(u8::try_from((value >> 16_u32) & mask).unwrap_or(zero));
    out.push(u8::try_from((value >> 8_u32) & mask).unwrap_or(zero));
    out.push(u8::try_from(value & mask).unwrap_or(zero));
}

/// Appends a length-prefixed (`u32` big-endian) string field.
fn write_string(out: &mut Vec<u8>, bytes: &[u8]) {
    write_u32(out, u32::try_from(bytes.len()).unwrap_or(u32::MAX));
    out.extend_from_slice(bytes);
}
