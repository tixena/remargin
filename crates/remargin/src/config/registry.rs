//! Registry loader: `.remargin-registry.yaml` handling.
//!
//! The registry maps participant IDs to their public keys, author type,
//! and status. It is used for mode enforcement in `registered` and `strict`
//! modes.

extern crate alloc;

use alloc::collections::BTreeMap;

use serde::Deserialize;

/// Parsed contents of a `.remargin-registry.yaml` file.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Registry {
    /// Map of participant ID to participant details.
    pub participants: BTreeMap<String, RegistryParticipant>,
}

/// A single participant entry in the registry.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct RegistryParticipant {
    /// Date the participant was added (ISO 8601 string).
    pub added: Option<String>,
    /// Author type (e.g. "human", "agent").
    #[serde(rename = "type")]
    pub author_type: String,
    /// Public keys for signature verification (supports key rotation).
    #[serde(default)]
    pub pubkeys: Vec<String>,
    /// Whether the participant is active or revoked.
    #[serde(default = "default_status")]
    pub status: RegistryParticipantStatus,
}

/// Status of a registered participant.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum RegistryParticipantStatus {
    /// Participant is active and can post.
    Active,
    /// Participant has been revoked and cannot post.
    Revoked,
}

/// Default participant status.
const fn default_status() -> RegistryParticipantStatus {
    RegistryParticipantStatus::Active
}
