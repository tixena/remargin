//! Tests for extension-to-MIME mapping and the binary/text split.

use super::{is_binary_mime, mime_for_extension};
use std::path::Path;

#[test]
fn markdown_is_text_markdown() {
    assert_eq!(mime_for_extension(Path::new("notes.md")), "text/markdown");
    assert!(!is_binary_mime("text/markdown"));
}

#[test]
fn png_is_image_png() {
    assert_eq!(mime_for_extension(Path::new("pic.png")), "image/png");
    assert!(is_binary_mime("image/png"));
}

#[test]
fn jpeg_handles_both_extensions() {
    assert_eq!(mime_for_extension(Path::new("a.jpg")), "image/jpeg");
    assert_eq!(mime_for_extension(Path::new("b.jpeg")), "image/jpeg");
}

#[test]
fn unknown_extension_is_octet_stream() {
    assert_eq!(
        mime_for_extension(Path::new("file.unknown")),
        "application/octet-stream"
    );
    assert!(is_binary_mime("application/octet-stream"));
}

#[test]
fn no_extension_is_octet_stream() {
    assert_eq!(
        mime_for_extension(Path::new("README")),
        "application/octet-stream"
    );
}

#[test]
fn case_insensitive_matching() {
    assert_eq!(mime_for_extension(Path::new("NOTES.MD")), "text/markdown");
    assert_eq!(mime_for_extension(Path::new("PIC.PNG")), "image/png");
}

#[test]
fn pdf_is_application_pdf() {
    assert_eq!(mime_for_extension(Path::new("doc.pdf")), "application/pdf");
    assert!(is_binary_mime("application/pdf"));
}
