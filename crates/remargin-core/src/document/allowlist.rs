//! File visibility allowlist and path sandboxing.
//!
//! Controls which files are visible through the remargin document access layer.
//! Dotfiles are always hidden, and only files with allowlisted extensions are shown.

use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};
use os_shim::System;

const ALLOWED_EXTENSIONS: &[&str] = &[
    "md", "txt", "csv", "xml", "json", "yaml", "yml", "toml", "ini", "env", "conf", "base", "pen",
    "html", "htm", "css", "scss", "sass", "less", "vue", "svelte", "js", "mjs", "cjs", "jsx", "ts",
    "tsx", "mts", "cts", "py", "pyi", "pyw", "rs", "go", "cs", "csx", "fs", "fsx", "vb", "java",
    "kt", "kts", "scala", "sc", "groovy", "c", "h", "cpp", "cc", "cxx", "hpp", "hh", "hxx", "rb",
    "php", "phtml", "swift", "m", "mm", "dart", "lua", "r", "pl", "pm", "jl", "hs", "ex", "exs",
    "clj", "cljs", "cljc", "edn", "ml", "mli", "erl", "hrl", "zig", "nim", "sh", "bash", "zsh",
    "fish", "ps1", "psm1", "psd1", "sql", "tf", "tfvars", "hcl", "png", "jpg", "jpeg", "gif",
    "svg", "webp", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "mp3", "wav", "ogg", "flac",
    "m4a", "mp4", "webm", "mov", "avi",
];

/// Every non-binary entry of `ALLOWED_EXTENSIONS` also appears here.
const TEXT_EXTENSIONS: &[&str] = &[
    "md", "txt", "csv", "xml", "json", "yaml", "yml", "toml", "ini", "env", "conf", "base", "pen",
    "html", "htm", "css", "scss", "sass", "less", "vue", "svelte", "js", "mjs", "cjs", "jsx", "ts",
    "tsx", "mts", "cts", "py", "pyi", "pyw", "rs", "go", "cs", "csx", "fs", "fsx", "vb", "java",
    "kt", "kts", "scala", "sc", "groovy", "c", "h", "cpp", "cc", "cxx", "hpp", "hh", "hxx", "rb",
    "php", "phtml", "swift", "m", "mm", "dart", "lua", "r", "pl", "pm", "jl", "hs", "ex", "exs",
    "clj", "cljs", "cljc", "edn", "ml", "mli", "erl", "hrl", "zig", "nim", "sh", "bash", "zsh",
    "fish", "ps1", "psm1", "psd1", "sql", "tf", "tfvars", "hcl",
];

/// Check if a path is visible (allowed extension, not a dotfile).
/// Directories are always visible (for navigation).
#[must_use]
pub fn is_visible(path: &Path, is_dir: bool) -> bool {
    let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    if filename.starts_with('.') {
        return false;
    }

    if is_dir {
        return true;
    }

    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ALLOWED_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

/// Build the "not visible" error for a path that failed [`is_visible`].
///
/// When the sole reason is an extension outside the allowlist, name the
/// extension so callers don't read a bare "file not visible" as a
/// sandbox or permission failure.
#[must_use]
pub fn not_visible_message(path: &Path) -> String {
    let display = path.display();
    let is_dotfile = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'));
    if is_dotfile {
        return format!("file not visible: {display}");
    }
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if !ALLOWED_EXTENSIONS.contains(&ext.to_lowercase().as_str()) => {
            format!("file not visible: {display} (extension .{ext} is not in the allowlist)")
        }
        _ => format!("file not visible: {display}"),
    }
}

/// Check if a file extension is text-based (supports `--lines`).
#[must_use]
pub fn is_text(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| TEXT_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

/// Resolve and sandbox a path. Returns an error if it escapes both the
/// base directory AND every declared trusted root.
///
/// When `unrestricted` is `true`, the sandbox check is skipped and the
/// path is resolved directly (absolute paths bypass the base join).
///
/// A resolved path outside `base` is accepted when it sits under one of `trusted_roots`.
///
/// # Errors
///
/// Returns an error if the path cannot be canonicalized or, unless `unrestricted`, escapes both
/// `base` and every trusted root.
pub fn resolve_sandboxed(
    system: &dyn System,
    base: &Path,
    requested: &Path,
    unrestricted: bool,
    trusted_roots: &[PathBuf],
) -> Result<PathBuf> {
    if unrestricted {
        let resolved = if requested.is_absolute() {
            system.canonicalize(requested)?
        } else {
            system.canonicalize(&base.join(requested))?
        };
        return Ok(resolved);
    }

    // An absolute request resolves against itself; the sandbox check below still gates it, so a
    // trusted-root caller need not spell a relative path out of the tree.
    let resolved = if requested.is_absolute() {
        system.canonicalize(requested)?
    } else {
        system.canonicalize(&base.join(requested))?
    };
    let canonical_base = system.canonicalize(base)?;

    if path_under(&resolved, &canonical_base) {
        return Ok(resolved);
    }
    if any_trusted_root_covers(system, trusted_roots, &resolved) {
        return Ok(resolved);
    }

    bail!("path escapes sandbox: {}", requested.display());
}

/// Resolve and sandbox a path for a file that does not yet exist.
///
/// Canonicalizes the **parent directory** and appends the filename. If
/// the parent directory does not exist, walks up the path to find the
/// nearest existing ancestor, validates that it is within the sandbox
/// (or any trusted root), and creates all missing intermediate
/// directories.
///
/// When `unrestricted` is `true`, the sandbox check is skipped
/// (absolute paths bypass the base join).
///
/// # Errors
///
/// Returns an error if no existing ancestor is found, the path has no filename, directory
/// creation fails or, unless `unrestricted`, the path escapes both `base` and every trusted root.
pub fn resolve_sandboxed_create(
    system: &dyn System,
    base: &Path,
    requested: &Path,
    unrestricted: bool,
    trusted_roots: &[PathBuf],
) -> Result<PathBuf> {
    let raw_joined = if (unrestricted || !trusted_roots.is_empty()) && requested.is_absolute() {
        requested.to_path_buf()
    } else {
        base.join(requested)
    };
    // Normalize first: a mocked `canonicalize` does not resolve `.` and `..`.
    let joined = normalize_path(&raw_joined);
    let parent = joined
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", requested.display()))?;
    let filename = joined
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("path has no filename: {}", requested.display()))?;

    let parent_exists = system.exists(parent).unwrap_or(false);

    if !parent_exists {
        // Sandbox-check the nearest existing ancestor before creating any directory.
        let nearest = find_existing_ancestor(system, parent)?;
        let canonical_nearest = system.canonicalize(&nearest)?;

        if !unrestricted {
            let canonical_base = system.canonicalize(base)?;
            if !path_under(&canonical_nearest, &canonical_base)
                && !any_trusted_root_covers(system, trusted_roots, &canonical_nearest)
            {
                bail!("path escapes sandbox: {}", requested.display());
            }
        }

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
        if !path_under(&canonical_parent, &canonical_base)
            && !any_trusted_root_covers(system, trusted_roots, &canonical_parent)
        {
            bail!("path escapes sandbox: {}", requested.display());
        }
    }

    Ok(canonical_parent.join(filename))
}

/// `true` when `target` equals `anchor` or starts with it (descendant).
fn path_under(target: &Path, anchor: &Path) -> bool {
    target == anchor || target.starts_with(anchor)
}

/// `true` when `target` is at-or-below any trusted root.
///
/// Each trusted root is canonicalized when possible; the expanded form is the fallback, so a
/// trusted root that does not exist on disk yet still matches.
fn any_trusted_root_covers(system: &dyn System, trusted_roots: &[PathBuf], target: &Path) -> bool {
    trusted_roots.iter().any(|root| {
        let canonical = system.canonicalize(root).unwrap_or_else(|_| root.clone());
        path_under(target, &canonical)
    })
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
                if parts
                    .last()
                    .is_some_and(|c| matches!(c, Component::Normal(_)))
                {
                    parts.pop();
                } else {
                    parts.push(component);
                }
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                parts.push(component);
            }
        }
    }
    parts.iter().collect()
}

#[cfg(test)]
mod tests;
