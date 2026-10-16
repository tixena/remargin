//! Canonical JSON response shapes for mutating ops whose core fn
//! returns `()`. Both CLI and MCP call these.

use std::path::Path;

use os_shim::System;
use serde_json::{Value, json};

use crate::crypto;

/// Add `hash` (see [`crypto::file_hash`]) to a JSON object result, so a caller learns the version
/// of the file the op just read or left on disk.
///
/// Best effort: a directory, a missing file or a non-object result is left as it is, because the
/// op itself has already succeeded.
pub fn attach_hash(result: &mut Value, system: &dyn System, path: &Path) {
    if system.is_dir(path).unwrap_or(false) {
        return;
    }
    if let (Some(map), Ok(hash)) = (result.as_object_mut(), crypto::file_hash(system, path)) {
        map.insert(String::from("hash"), Value::String(hash));
    }
}

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
