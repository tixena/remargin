//! Registry loader: `.remargin-registry.yaml` handling.
//!
//! Example registry file:
//!
//! ```yaml
//! participants:
//!   eduardo-burgos:
//!     display_name: "Eduardo Burgos Minier"
//!     type: human
//!     status: active
//!     pubkeys:
//!       - "ssh-ed25519 AAAA..."
//!   ci-bot:
//!     type: agent
//!     status: active
//! ```
//!
//! `display_name` is optional. When absent, downstream consumers
//! (e.g. `remargin registry show --json`) substitute the participant
//! id so clients never have to handle a null display name.

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
    /// Human-friendly name for UI rendering. When `None`, consumers
    /// fall back to the participant id (the map key).
    #[serde(default)]
    pub display_name: Option<String>,
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

/// Render a single [`RegistryParticipant`] as a one-line human-readable
/// string.
///
/// When a display name is set the prefix is `"Display Name" (id)`;
/// otherwise it is the bare id. Example output:
/// `"Eduardo Burgos" (eduardo-burgos) (human) [active] 2 key(s)`.
#[must_use]
pub fn render_registry_participant(name: &str, participant: &RegistryParticipant) -> String {
    let status = match participant.status {
        RegistryParticipantStatus::Active => "active",
        RegistryParticipantStatus::Revoked => "revoked",
    };
    let prefix = participant.display_name.as_ref().map_or_else(
        || String::from(name),
        |display| format!("\"{display}\" ({name})"),
    );
    format!(
        "{prefix} ({}) [{status}] {} key(s)",
        participant.author_type,
        participant.pubkeys.len(),
    )
}
