//! The refusals the MCP tools answer, as model types whose JSON hosts branch on by `error_kind`.

#[cfg(test)]
mod tests;

use serde::Serialize;
use tixschema::model_schema;

use crate::operations::verify::{
    SubsetGateFailure, SubsetGateFailurePayload, VerifyFailure, VerifyFailurePayload,
    subset_gate_failure_payload_schema, verify_failure_payload_schema,
};

/// Why a tool refused, as the JSON hosts branch on by `error_kind`.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum RemarginError {
    Failed(Failure),
    HumanIdentity(HumanIdentityRejected),
    SubsetGate(SubsetGateFailurePayload),
    Verify(VerifyFailurePayload),
}

/// `error_kind` discriminant of [`HumanIdentityRejected`].
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub enum HumanIdentityErrorKind {
    #[serde(rename = "mcp_human_identity_rejected")]
    McpHumanIdentityRejected,
}

/// A tool refused because the server resolved to a human identity.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct HumanIdentityRejected {
    pub error_kind: HumanIdentityErrorKind,
    pub headline: String,
    pub identity: String,
    pub tool: String,
}

/// `error_kind` discriminant of [`Failure`].
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub enum FailureKind {
    #[serde(rename = "failed")]
    Failed,
}

/// Any other failure, with the message the server used to write as plain text.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct Failure {
    pub error_kind: FailureKind,
    pub message: String,
}

impl HumanIdentityRejected {
    #[must_use]
    pub fn new(identity: &str, tool: &str) -> Self {
        Self {
            error_kind: HumanIdentityErrorKind::McpHumanIdentityRejected,
            headline: format!(
                "MCP tool '{tool}' refused: this server resolved to '{identity}', a human \
                 identity, and an agent must not act as a human. Ask the user for permission to \
                 create an agent identity for this realm, then call 'identity_create' to render \
                 the `.remargin.yaml` block for them.",
            ),
            identity: String::from(identity),
            tool: String::from(tool),
        }
    }
}

impl From<anyhow::Error> for RemarginError {
    fn from(err: anyhow::Error) -> Self {
        if let Some(failure) = err.downcast_ref::<SubsetGateFailure>() {
            return Self::SubsetGate(failure.to_json());
        }
        if let Some(failure) = err.downcast_ref::<VerifyFailure>() {
            return Self::Verify(failure.payload());
        }
        Self::Failed(Failure {
            error_kind: FailureKind::Failed,
            message: format!("{err:#}"),
        })
    }
}
