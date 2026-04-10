//! Registry loader: `.remargin-registry.yaml` handling.

extern crate alloc;

use alloc::collections::BTreeMap;

use serde::Deserialize;

/// Parsed contents of a `.remargin-registry.yaml` file.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct Registry {
    pub participants: BTreeMap<String, RegistryParticipant>,
}

/// A single participant entry in the registry.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct RegistryParticipant {
    pub added: Option<String>,
    #[serde(rename = "type")]
    pub author_type: String,
    /// Supports key rotation: multiple pubkeys can be listed simultaneously.
    #[serde(default)]
    pub pubkeys: Vec<String>,
    #[serde(default = "default_status")]
    pub status: RegistryParticipantStatus,
}

/// Status of a registered participant.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum RegistryParticipantStatus {
    Active,
    Revoked,
}

const fn default_status() -> RegistryParticipantStatus {
    RegistryParticipantStatus::Active
}
