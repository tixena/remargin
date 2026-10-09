//! Invariant tests for the visibility allowlist internals.

use super::{ALLOWED_EXTENSIONS, TEXT_EXTENSIONS};

const BINARY_EXTENSIONS: &[&str] = &[
    "avi", "doc", "docx", "flac", "gif", "jpeg", "jpg", "m4a", "mov", "mp3", "mp4", "ogg", "pdf",
    "png", "ppt", "pptx", "svg", "wav", "webm", "webp", "xls", "xlsx",
];

/// `--lines` must never meet an allowed extension it cannot handle.
#[test]
fn every_non_binary_allowed_extension_is_text() {
    for ext in ALLOWED_EXTENSIONS {
        if BINARY_EXTENSIONS.contains(ext) {
            continue;
        }
        assert!(
            TEXT_EXTENSIONS.contains(ext),
            "allowed non-binary extension {ext} missing from TEXT_EXTENSIONS"
        );
    }
}
