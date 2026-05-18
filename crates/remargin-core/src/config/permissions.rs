//! On-disk schema for `permissions:` in `.remargin.yaml`.

pub mod op_name;
pub mod resolve;

#[cfg(test)]
mod tests;

use serde::de::{Deserializer, Error as _};
use serde::{Deserialize, Serialize};

use self::op_name::OpName;

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DenyOpsEntry {
    pub ops: Vec<DenyOpsItem>,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(untagged)]
#[non_exhaustive]
pub enum DenyOpsItem {
    Bare(OpName),
    Full(DenyOpsItemFull),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DenyOpsItemFull {
    #[serde(default)]
    pub exceptions: Vec<String>,
    pub name: OpName,
}

impl<'de> Deserialize<'de> for DenyOpsItem {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        if value.is_string() {
            let op = OpName::deserialize(value).map_err(D::Error::custom)?;
            return Ok(Self::Bare(op));
        }
        if value.is_mapping() {
            let full = DenyOpsItemFull::deserialize(value).map_err(D::Error::custom)?;
            return Ok(Self::Full(full));
        }
        Err(D::Error::custom(
            "deny_ops item must be either a bare op-name string or a mapping with `name:` and optional `exceptions:`",
        ))
    }

    fn deserialize_in_place<D: Deserializer<'de>>(
        deserializer: D,
        place: &mut Self,
    ) -> Result<(), D::Error> {
        *place = Self::deserialize(deserializer)?;
        Ok(())
    }
}

impl DenyOpsItem {
    #[must_use]
    pub const fn exceptions(&self) -> &[String] {
        match self {
            Self::Bare(_) => &[],
            Self::Full(full) => full.exceptions.as_slice(),
        }
    }

    #[must_use]
    pub const fn name(&self) -> &OpName {
        match self {
            Self::Bare(name) => name,
            Self::Full(full) => &full.name,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Permissions {
    #[serde(default)]
    pub allow_dot_folders: Vec<String>,

    #[serde(default)]
    pub deny_ops: Vec<DenyOpsEntry>,

    /// `None` = falls back to cwd. `Some(vec![])` = locked realm, deny
    /// everything outside inherited parent roots. `Some(non-empty)` =
    /// exactly those paths reachable. `"*"` = entire declaring realm.
    #[serde(default)]
    pub trusted_roots: Option<Vec<TrustedRootEntry>>,
}

/// Bare-string or full-record on-disk form for a `trusted_roots` entry.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
#[non_exhaustive]
pub enum TrustedRootEntry {
    Full(TrustedRootEntryFull),
    Path(String),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct TrustedRootEntryFull {
    #[serde(default)]
    pub also_deny_bash: Vec<String>,

    /// Suppress the projected `Bash(remargin *)` deny so the CLI stays
    /// usable inside this entry.
    #[serde(default)]
    pub cli_allowed: bool,

    pub path: String,
}

impl TrustedRootEntry {
    #[must_use]
    pub const fn also_deny_bash(&self) -> &[String] {
        match self {
            Self::Path(_) => &[],
            Self::Full(full) => full.also_deny_bash.as_slice(),
        }
    }

    #[must_use]
    pub const fn cli_allowed(&self) -> bool {
        match self {
            Self::Path(_) => false,
            Self::Full(full) => full.cli_allowed,
        }
    }

    #[must_use]
    pub const fn path(&self) -> &str {
        match self {
            Self::Full(full) => full.path.as_str(),
            Self::Path(s) => s.as_str(),
        }
    }
}
