//! File visibility allowlist and path sandboxing.
//!
//! Controls which files are visible through the remargin document access layer.
//! Dotfiles are always hidden, and only files with allowlisted extensions are shown.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use os_shim::System;

// ---------------------------------------------------------------------------
// Allowlist
// ---------------------------------------------------------------------------

/// File extensions visible through remargin.
const ALLOWED_EXTENSIONS: &[&str] = &[
    // Markdown/text/data
    "md", "txt", "csv", "xml", "json", // Images
    "png", "jpg", "jpeg", "gif", "svg", "webp", // Documents
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", // Audio
    "mp3", "wav", "ogg", "flac", "m4a", // Video
    "mp4", "webm", "mov", "avi",
];

/// Extensions that are text-based (support `--lines`).
const TEXT_EXTENSIONS: &[&str] = &["md", "txt", "csv", "xml", "json"];

/// Check if a path is visible (allowed extension, not a dotfile).
/// Directories are always visible (for navigation).
#[must_use]
pub fn is_visible(path: &Path, is_dir: bool) -> bool {
    let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    // Dotfiles and dot-directories are always hidden.
    if filename.starts_with('.') {
        return false;
    }

    // Directories are always visible (for navigation).
    if is_dir {
        return true;
    }

    // Check extension against allowlist.
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ALLOWED_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

/// Check if a file extension is text-based (supports `--lines`).
#[must_use]
pub fn is_text(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| TEXT_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

// ---------------------------------------------------------------------------
// Path sandboxing
// ---------------------------------------------------------------------------

/// Resolve and sandbox a path. Returns an error if it escapes the base directory.
///
/// # Errors
///
/// Returns an error if:
/// - The path cannot be canonicalized
/// - The resolved path escapes the base directory
pub fn resolve_sandboxed(system: &dyn System, base: &Path, requested: &Path) -> Result<PathBuf> {
    let resolved = system.canonicalize(&base.join(requested))?;
    let canonical_base = system.canonicalize(base)?;

    if !resolved.starts_with(&canonical_base) {
        bail!("path escapes sandbox: {}", requested.display());
    }

    Ok(resolved)
}

/// Resolve and sandbox a path for a file that does not yet exist.
///
/// Canonicalizes the **parent directory** (which must exist) and appends the
/// filename. This avoids the `canonicalize` failure on non-existent paths.
///
/// # Errors
///
/// Returns an error if:
/// - The parent directory does not exist or cannot be canonicalized
/// - The resolved path escapes the base directory
/// - The requested path has no filename component
pub fn resolve_sandboxed_create(
    system: &dyn System,
    base: &Path,
    requested: &Path,
) -> Result<PathBuf> {
    let joined = base.join(requested);
    let parent = joined
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", requested.display()))?;
    let filename = joined
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("path has no filename: {}", requested.display()))?;

    let canonical_parent = system.canonicalize(parent).map_err(|source| {
        anyhow::anyhow!(
            "parent directory does not exist: {}: {source}",
            parent.display()
        )
    })?;
    let canonical_base = system.canonicalize(base)?;

    if !canonical_parent.starts_with(&canonical_base) {
        bail!("path escapes sandbox: {}", requested.display());
    }

    Ok(canonical_parent.join(filename))
}
