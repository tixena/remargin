//! Source-level checks that the comment-body gate has no bypass seam.

use std::fs;
use std::path::{Path, PathBuf};

/// Its two parameters are the whole of its input surface; a third would be a bypass seam.
const GATE_SIGNATURE: &str = "pub fn gate(content: &str, author_type: &AuthorType) -> Result<()> {";

/// The old body, the new body and the author type are the whole of its input surface.
const EDIT_GATE_SIGNATURE: &str = "pub fn gate_edit(old_content: &str, new_content: &str, author_type: &AuthorType) -> Result<()> {";

/// The edit path's entry point; its body must carry the edit-gate call.
const EDIT_ENTRY_POINT: &str = "pub fn edit_comment(";
const EDIT_GATE_CALL: &str = "comment_style::gate_edit(";

/// The create path's entry point, and the call it has to carry.
const CREATE_ENTRY_POINT: &str = "pub fn create_comment(";
const GATE_CALL: &str = "comment_style::gate(";

/// Every hop from a body-authoring projection down to the gate, as (function, call its body
/// must carry). Batch gates through a preflight helper, so both hops are pinned.
const PROJECTION_GATE_CHAIN: &[(&str, &str)] = &[
    ("pub fn project_batch(", "preflight_batch_ops("),
    ("fn preflight_batch_ops(", GATE_CALL),
    ("pub fn project_comment(", GATE_CALL),
    ("pub fn project_edit(", EDIT_GATE_CALL),
];

/// Each token is a way for caller-supplied state to reach the gate's decision.
const BANNED_IN_GATE_MODULE: &[&str] = &[
    "ResolvedConfig",
    "env!",
    "option_env!",
    "std::env",
    "var_os",
];

fn gate_module() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/comment_style.rs")
}

fn operations_module() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/operations.rs")
}

fn projections_module() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/operations/projections.rs")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// `source` from the line opening `signature` through the function's closing
/// brace at column 0, or `None` when nothing declares it any more.
fn function_body<'src>(source: &'src str, signature: &str) -> Option<&'src str> {
    let start = source.find(signature)?;
    let rest = &source[start..];
    let end = rest.find("\n}\n").map_or(rest.len(), |idx| idx + 3);
    Some(&rest[..end])
}

#[test]
fn the_gate_takes_the_body_and_the_author_type_and_nothing_else() {
    let path = gate_module();
    let source = read(&path);

    assert!(
        source.contains(GATE_SIGNATURE),
        "{} no longer declares {GATE_SIGNATURE:?}. A new parameter is a new \
         way to turn the checks off; demote the check to the warn tier \
         instead.",
        path.display()
    );
}

#[test]
fn the_edit_gate_takes_both_bodies_and_the_author_type_and_nothing_else() {
    let path = gate_module();
    let source = read(&path);

    assert!(
        source.contains(EDIT_GATE_SIGNATURE),
        "{} no longer declares {EDIT_GATE_SIGNATURE:?}. A new parameter is a \
         new way to turn the checks off; demote the check to the warn tier \
         instead.",
        path.display()
    );
}

#[test]
fn the_edit_path_runs_the_edit_gate() {
    let path = operations_module();
    let source = read(&path);

    assert!(
        function_body(&source, EDIT_ENTRY_POINT).is_some_and(|body| body.contains(EDIT_GATE_CALL)),
        "{} no longer reaches {EDIT_GATE_CALL:?} from {EDIT_ENTRY_POINT:?}, \
         so a body the gate refused on creation can be written through the \
         edit path instead.",
        path.display()
    );
}

#[test]
fn the_create_path_runs_the_gate() {
    let path = operations_module();
    let source = read(&path);

    assert!(
        function_body(&source, CREATE_ENTRY_POINT).is_some_and(|body| body.contains(GATE_CALL)),
        "{} no longer reaches {GATE_CALL:?} from {CREATE_ENTRY_POINT:?}, so a \
         body the gate refuses can be written on creation.",
        path.display()
    );
}

#[test]
fn every_projection_path_runs_the_same_gate_its_write_runs() {
    let path = projections_module();
    let source = read(&path);

    let mut misses: Vec<String> = Vec::new();
    for &(function, required_call) in PROJECTION_GATE_CHAIN {
        if !function_body(&source, function).is_some_and(|body| body.contains(required_call)) {
            misses.push(format!("{function:?} does not reach {required_call:?}"));
        }
    }

    assert!(
        misses.is_empty(),
        "a projection that skips the gate predicts success for a body the \
         write refuses. Offenders in {}:\n{}",
        path.display(),
        misses.join("\n")
    );
}

#[test]
fn the_gate_module_reads_no_config_and_no_environment() {
    let path = gate_module();
    let source = read(&path);

    let mut hits: Vec<String> = Vec::new();
    for (line_no, line) in source.lines().enumerate() {
        for banned in BANNED_IN_GATE_MODULE {
            if line.contains(banned) {
                hits.push(format!("line {}: {}", line_no + 1, line.trim()));
            }
        }
    }

    assert!(
        hits.is_empty(),
        "the comment gate must decide from the body and the author type \
         alone, so that no config key or environment variable can reach it. \
         Offenders in {}:\n{}",
        path.display(),
        hits.join("\n")
    );
}
