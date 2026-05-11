//! Canonical JSON response shapes for mutating ops whose core fn
//! returns `()`. Both CLI and MCP call these.

use serde_json::{Value, json};

#[must_use]
pub fn ack(ids: &[String], remove: bool) -> Value {
    let key = if remove {
        "unacknowledged"
    } else {
        "acknowledged"
    };
    json!({ key: ids })
}

#[must_use]
pub fn batch(ids: &[String]) -> Value {
    json!({ "ids": ids })
}

#[must_use]
pub fn comment_created(id: &str) -> Value {
    json!({ "id": id })
}

#[must_use]
pub fn comments_deleted(ids: &[String]) -> Value {
    json!({ "deleted": ids })
}

#[must_use]
pub fn comment_edited(id: &str) -> Value {
    json!({ "edited": id })
}

#[must_use]
pub fn react(emoji: &str, comment_id: &str, remove: bool) -> Value {
    let action = if remove { "removed" } else { "added" };
    json!({
        "action": action,
        "emoji": emoji,
        "comment_id": comment_id,
    })
}
