//! The answers of the mutating ops whose core fn returns `()`, as model
//! types. Both the CLI and the MCP server build them here.

#[cfg(test)]
mod tests;

use serde::Serialize;
use tixschema::model_schema;

use crate::advice::{Advice, OpAdvice, OpWarning, Warning, op_warning_schema, warning_schema};

/// What `ack` answers: the ids it acknowledged, or the ids it took the
/// acknowledgment from.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct AckAnswer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unacknowledged: Option<Vec<String>>,
}

/// What `batch` answers: the ids it created, and the notes each operation
/// earned.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct BatchAnswer {
    pub ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<OpWarning>,
}

/// What `comment` and `reply` answer.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct CommentCreated {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
}

/// What `delete` answers: the ids it removed.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct CommentsDeleted {
    pub deleted: Vec<String>,
}

/// What `edit` answers: the id it edited.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct CommentEdited {
    pub edited: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
}

/// What `react` answers: the reaction it added or removed.
#[model_schema(enable = ["jsonschema"])]
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct ReactAnswer {
    pub action: String,
    pub comment_id: String,
    pub emoji: String,
}

#[must_use]
pub fn ack(ids: &[String], remove: bool) -> AckAnswer {
    let listed = Some(ids.to_vec());
    if remove {
        AckAnswer {
            acknowledged: None,
            unacknowledged: listed,
        }
    } else {
        AckAnswer {
            acknowledged: listed,
            unacknowledged: None,
        }
    }
}

#[must_use]
pub fn batch(ids: &[String], notes: Vec<OpAdvice>) -> BatchAnswer {
    BatchAnswer {
        ids: ids.to_vec(),
        warnings: notes.into_iter().map(OpWarning::from).collect(),
    }
}

#[must_use]
pub fn comment_created(id: &str, notes: Vec<Advice>) -> CommentCreated {
    CommentCreated {
        id: String::from(id),
        warnings: notes.into_iter().map(Warning::from).collect(),
    }
}

#[must_use]
pub fn comments_deleted(ids: &[String]) -> CommentsDeleted {
    CommentsDeleted {
        deleted: ids.to_vec(),
    }
}

#[must_use]
pub fn comment_edited(id: &str, notes: Vec<Advice>) -> CommentEdited {
    CommentEdited {
        edited: String::from(id),
        warnings: notes.into_iter().map(Warning::from).collect(),
    }
}

#[must_use]
pub fn react(emoji: &str, comment_id: &str, remove: bool) -> ReactAnswer {
    let action = if remove { "removed" } else { "added" };
    ReactAnswer {
        action: String::from(action),
        comment_id: String::from(comment_id),
        emoji: String::from(emoji),
    }
}
