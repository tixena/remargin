//! Each `RemarginError` variant serializes to the JSON the tools have always written.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{HumanIdentityRejected, RemarginError};
use crate::config::Mode;
use crate::operations::verify::{
    Anomaly, AnomalyKind, RecipientStatus, RowStatus, SignatureStatus, SubsetGateFailure,
    VerifyFailure,
};

fn to_value(error: &RemarginError) -> Value {
    serde_json::to_value(error).unwrap()
}

#[test]
fn human_identity_refusal_keeps_its_json() {
    let refused = RemarginError::HumanIdentity(HumanIdentityRejected::new("reviewer", "comment"));
    assert_eq!(
        to_value(&refused),
        json!({
            "error_kind": "mcp_human_identity_rejected",
            "headline": "MCP tool 'comment' refused: this server resolved to 'reviewer', a human identity, and an agent must not act as a human. Ask the user for permission to create an agent identity for this realm, then call 'identity_create' to render the `.remargin.yaml` block for them.",
            "identity": "reviewer",
            "tool": "comment",
        }),
    );
}

#[test]
fn verify_failure_keeps_its_json() {
    let failure = VerifyFailure {
        failures: vec![RowStatus {
            author: String::from("charlie"),
            checksum_ok: true,
            id: String::from("abc"),
            line: 3,
            recipients: RecipientStatus::Ok,
            signature: SignatureStatus::UnknownAuthor,
        }],
        mode: Mode::Strict,
        path: PathBuf::from("/d/a.md"),
    };
    let expected = json!({
        "error_kind": "verify_failed",
        "failures": [{
            "checksum_ok": true,
            "id": "abc",
            "recipients": "ok",
            "signature": "unknown_author",
        }],
        "headline": failure.headline(),
        "hint": failure.hint(),
        "mode": "strict",
        "path": "/d/a.md",
    });
    let error = RemarginError::from(anyhow::Error::new(failure));
    assert!(matches!(error, RemarginError::Verify(_)));
    assert_eq!(to_value(&error), expected);
}

#[test]
fn subset_gate_failure_keeps_its_json() {
    let failure = SubsetGateFailure {
        introduced: vec![Anomaly {
            id: String::from("abc"),
            kind: AnomalyKind::SignatureUnknownAuthor,
        }],
        mode: Mode::Registered,
        path: PathBuf::from("/d/a.md"),
    };
    let expected = json!({
        "error_kind": "subset_gate_failed",
        "headline": "op would introduce 1 new anomaly in /d/a.md",
        "hint": "Try `remargin verify /d/a.md --json` for the full breakdown.",
        "introduced": [{ "id": "abc", "kind": "signature_unknown_author" }],
        "mode": "registered",
        "path": "/d/a.md",
    });
    let error = RemarginError::from(anyhow::Error::new(failure));
    assert!(matches!(error, RemarginError::SubsetGate(_)));
    assert_eq!(to_value(&error), expected);
}

#[test]
fn plain_failure_is_failed_with_the_chained_message() {
    let failed = RemarginError::from(anyhow::anyhow!("no such file: a.md"));
    assert_eq!(
        to_value(&failed),
        json!({ "error_kind": "failed", "message": "no such file: a.md" }),
    );

    let chained = RemarginError::from(anyhow::anyhow!("disk full").context("writing a.md"));
    assert_eq!(
        to_value(&chained),
        json!({ "error_kind": "failed", "message": "writing a.md: disk full" }),
    );
}
