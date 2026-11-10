//! The typed answers serialize to the JSON the mutating ops have always written.

use serde_json::json;

use super::{ack, batch, comment_created, comment_edited, comments_deleted, react};
use crate::advice::{Advice, OpAdvice};

fn note() -> Advice {
    Advice {
        line: 3_usize,
        message: String::from("wrapped"),
    }
}

#[test]
fn ack_names_the_key_after_the_direction() {
    let ids = [String::from("a1"), String::from("b2")];

    assert_eq!(
        serde_json::to_value(ack(&ids, false)).unwrap(),
        json!({ "elapsed_ms": 0_u64, "acknowledged": ["a1", "b2"] })
    );
    assert_eq!(
        serde_json::to_value(ack(&ids, true)).unwrap(),
        json!({ "elapsed_ms": 0_u64, "unacknowledged": ["a1", "b2"] })
    );
}

#[test]
fn batch_carries_the_op_of_each_warning() {
    let ids = [String::from("a1")];

    assert_eq!(
        serde_json::to_value(batch(&ids, Vec::new())).unwrap(),
        json!({ "elapsed_ms": 0_u64, "ids": ["a1"] })
    );
    assert_eq!(
        serde_json::to_value(batch(&ids, vec![OpAdvice::new(2_usize, note())])).unwrap(),
        json!({
            "elapsed_ms": 0_u64,
            "ids": ["a1"],
            "warnings": [{ "op": 2_u64, "line": 3_u64, "message": "wrapped" }],
        })
    );
}

#[test]
fn comment_created_omits_warnings_when_clean() {
    assert_eq!(
        serde_json::to_value(comment_created("abc", Vec::new())).unwrap(),
        json!({ "elapsed_ms": 0_u64, "id": "abc" })
    );
    assert_eq!(
        serde_json::to_value(comment_created("abc", vec![note()])).unwrap(),
        json!({ "elapsed_ms": 0_u64, "id": "abc", "warnings": [{ "line": 3_u64, "message": "wrapped" }] })
    );
}

#[test]
fn comment_edited_omits_warnings_when_clean() {
    assert_eq!(
        serde_json::to_value(comment_edited("abc", Vec::new())).unwrap(),
        json!({ "elapsed_ms": 0_u64, "edited": "abc" })
    );
    assert_eq!(
        serde_json::to_value(comment_edited("abc", vec![note()])).unwrap(),
        json!({ "elapsed_ms": 0_u64, "edited": "abc", "warnings": [{ "line": 3_u64, "message": "wrapped" }] })
    );
}

#[test]
fn comments_deleted_lists_the_ids() {
    assert_eq!(
        serde_json::to_value(comments_deleted(&[String::from("a1")])).unwrap(),
        json!({ "elapsed_ms": 0_u64, "deleted": ["a1"] })
    );
}

#[test]
fn react_names_the_action_after_the_direction() {
    assert_eq!(
        serde_json::to_value(react("+1", "c1", false)).unwrap(),
        json!({ "elapsed_ms": 0_u64, "action": "added", "emoji": "+1", "comment_id": "c1" })
    );
    assert_eq!(
        serde_json::to_value(react("+1", "c1", true)).unwrap(),
        json!({ "elapsed_ms": 0_u64, "action": "removed", "emoji": "+1", "comment_id": "c1" })
    );
}
