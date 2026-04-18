//! Extension-based MIME type detection.
//!
//! Source of truth is the file extension — no content-sniffing. Used by
//! `metadata` (rem-lqz) and, once it lands, `get --binary` (rem-cdr) so
//! agents can decide whether a file is worth fetching before pulling bytes.

use std::path::Path;

/// Return the MIME type for a path based on its extension.
///
/// Unknown or missing extensions return `application/octet-stream`. Comparison
/// is case-insensitive on the extension.
#[must_use]
pub fn mime_for_extension(path: &Path) -> &'static str {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return "application/octet-stream";
    };
    let lowered = ext.to_lowercase();
    match lowered.as_str() {
        // Prose / data
        "md" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "xml" => "application/xml",
        "json" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        // Documents
        "pdf" => "application/pdf",
        // Audio
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        // Video
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        // Plaintext + source code default to text/plain. Callers that need
        // finer granularity (e.g. application/javascript) can special-case.
        "txt" | "ini" | "env" | "conf" | "sh" | "bash" | "zsh" | "fish" | "ps1" | "psm1"
        | "psd1" | "sql" | "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "mts" | "cts" | "py"
        | "pyi" | "pyw" | "rs" | "go" | "cs" | "csx" | "fs" | "fsx" | "vb" | "java" | "kt"
        | "kts" | "scala" | "sc" | "groovy" | "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hh"
        | "hxx" | "rb" | "php" | "phtml" | "swift" | "m" | "mm" | "dart" | "lua" | "r" | "pl"
        | "pm" | "jl" | "hs" | "ex" | "exs" | "clj" | "cljs" | "cljc" | "edn" | "ml" | "mli"
        | "erl" | "hrl" | "zig" | "nim" | "scss" | "sass" | "less" | "vue" | "svelte" | "pen" => {
            "text/plain"
        }
        // Office documents
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

/// Return `true` when the MIME type is a binary format (not `text/*`).
#[must_use]
pub fn is_binary_mime(mime: &str) -> bool {
    !mime.starts_with("text/")
}

#[cfg(test)]
mod tests {
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
}
