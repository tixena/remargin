//! File visibility allowlist and path sandboxing.
//!
//! Controls which files are visible through the remargin document access layer.
//! Dotfiles are always hidden, and only files with allowlisted extensions are shown.

use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};
use os_shim::System;

/// File extensions visible through remargin.
const ALLOWED_EXTENSIONS: &[&str] = &[
    // Markdown/text/data
    "md", "txt", "csv", "xml", "json", // Design
    "pen",  // Images
    "png", "jpg", "jpeg", "gif", "svg", "webp", // Documents
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", // Audio
    "mp3", "wav", "ogg", "flac", "m4a", // Video
    "mp4", "webm", "mov", "avi",
];

/// Extensions that are text-based (support `--lines`).
const TEXT_EXTENSIONS: &[&str] = &["md", "txt", "csv", "xml", "json", "pen"];

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

/// Resolve and sandbox a path. Returns an error if it escapes the base directory.
///
/// When `unrestricted` is `true`, the sandbox check is skipped and the path is
/// resolved directly (absolute paths bypass the base join).
///
/// # Errors
///
/// Returns an error if:
/// - The path cannot be canonicalized
/// - The resolved path escapes the base directory (when `unrestricted` is false)
pub fn resolve_sandboxed(
    system: &dyn System,
    base: &Path,
    requested: &Path,
    unrestricted: bool,
) -> Result<PathBuf> {
    if unrestricted {
        let resolved = if requested.is_absolute() {
            system.canonicalize(requested)?
        } else {
            system.canonicalize(&base.join(requested))?
        };
        return Ok(resolved);
    }

    let resolved = system.canonicalize(&base.join(requested))?;
    let canonical_base = system.canonicalize(base)?;

    if !resolved.starts_with(&canonical_base) {
        bail!("path escapes sandbox: {}", requested.display());
    }

    Ok(resolved)
}

/// Resolve and sandbox a path for a file that does not yet exist.
///
/// Canonicalizes the **parent directory** and appends the filename. If the
/// parent directory does not exist, walks up the path to find the nearest
/// existing ancestor, validates that it is within the sandbox, and creates
/// all missing intermediate directories.
///
/// When `unrestricted` is `true`, the sandbox check is skipped (absolute paths
/// bypass the base join).
///
/// # Errors
///
/// Returns an error if:
/// - No existing ancestor directory can be found
/// - The resolved path escapes the base directory (when `unrestricted` is false)
/// - The requested path has no filename component
/// - Directory creation fails
pub fn resolve_sandboxed_create(
    system: &dyn System,
    base: &Path,
    requested: &Path,
    unrestricted: bool,
) -> Result<PathBuf> {
    let raw_joined = if unrestricted && requested.is_absolute() {
        requested.to_path_buf()
    } else {
        base.join(requested)
    };
    // Normalize to resolve `.` and `..` components so that sandbox checks
    // work correctly even when the system's canonicalize does not (e.g. mocks).
    let joined = normalize_path(&raw_joined);
    let parent = joined
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", requested.display()))?;
    let filename = joined
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("path has no filename: {}", requested.display()))?;

    let parent_exists = system.exists(parent).unwrap_or(false);

    if !parent_exists {
        // Parent doesn't exist. Walk up to find the nearest existing ancestor
        // and sandbox-check it before creating any directories.
        let nearest = find_existing_ancestor(system, parent)?;
        let canonical_nearest = system.canonicalize(&nearest)?;

        if !unrestricted {
            let canonical_base = system.canonicalize(base)?;
            if !canonical_nearest.starts_with(&canonical_base) {
                bail!("path escapes sandbox: {}", requested.display());
            }
        }

        // Create the missing directories.
        system.create_dir_all(parent).map_err(|source| {
            anyhow::anyhow!(
                "failed to create parent directories: {}: {source}",
                parent.display()
            )
        })?;
    }

    let canonical_parent = system.canonicalize(parent).map_err(|source| {
        anyhow::anyhow!(
            "parent directory does not exist: {}: {source}",
            parent.display()
        )
    })?;

    if !unrestricted {
        let canonical_base = system.canonicalize(base)?;
        if !canonical_parent.starts_with(&canonical_base) {
            bail!("path escapes sandbox: {}", requested.display());
        }
    }

    Ok(canonical_parent.join(filename))
}

/// Walk up from `path` to find the nearest ancestor directory that exists.
///
/// # Errors
///
/// Returns an error if no existing ancestor can be found (i.e., the entire
/// path chain is non-existent, which should not happen on a valid filesystem).
fn find_existing_ancestor(system: &dyn System, path: &Path) -> Result<PathBuf> {
    let mut current = path;
    loop {
        if system.exists(current).unwrap_or(false) {
            return Ok(current.to_path_buf());
        }
        current = current
            .parent()
            .ok_or_else(|| anyhow::anyhow!("no existing ancestor for: {}", path.display()))?;
    }
}

/// Normalize a path by resolving `.` and `..` components lexically (without
/// touching the filesystem). Preserves the root prefix for absolute paths.
fn normalize_path(path: &Path) -> PathBuf {
    let mut parts: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                // Pop the last Normal component if there is one; otherwise keep the `..`.
                if parts
                    .last()
                    .is_some_and(|c| matches!(c, Component::Normal(_)))
                {
                    parts.pop();
                } else {
                    parts.push(component);
                }
            }
            Component::CurDir => {
                // Skip `.` — it's a no-op.
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                parts.push(component);
            }
        }
    }
    parts.iter().collect()
}
