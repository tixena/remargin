//! Extension-based MIME type detection.
//!
//! Source of truth is the file extension — no content-sniffing.

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
        "md" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "xml" => "application/xml",
        "json" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        // Source code defaults to text/plain.
        "txt" | "ini" | "env" | "conf" | "sh" | "bash" | "zsh" | "fish" | "ps1" | "psm1"
        | "psd1" | "sql" | "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "mts" | "cts" | "py"
        | "pyi" | "pyw" | "rs" | "go" | "cs" | "csx" | "fs" | "fsx" | "vb" | "java" | "kt"
        | "kts" | "scala" | "sc" | "groovy" | "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hh"
        | "hxx" | "rb" | "php" | "phtml" | "swift" | "m" | "mm" | "dart" | "lua" | "r" | "pl"
        | "pm" | "jl" | "hs" | "ex" | "exs" | "clj" | "cljs" | "cljc" | "edn" | "ml" | "mli"
        | "erl" | "hrl" | "zig" | "nim" | "scss" | "sass" | "less" | "vue" | "svelte" | "pen" => {
            "text/plain"
        }
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
mod tests;
