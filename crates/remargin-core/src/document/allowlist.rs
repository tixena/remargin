//! File visibility allowlist and path sandboxing.
//!
//! Controls which files are visible through the remargin document access layer.
//! Dotfiles are always hidden, and only files with allowlisted extensions are shown.

use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};
use os_shim::System;

/// File extensions visible through remargin.
const ALLOWED_EXTENSIONS: &[&str] = &[
    // Prose / data
    "md", "txt", "csv", "xml", "json", "yaml", "yml", "toml", "ini", "env", "conf",
    // Design
    "pen", // Web markup / styles
    "html", "htm", "css", "scss", "sass", "less", "vue", "svelte",
    // JavaScript / TypeScript
    "js", "mjs", "cjs", "jsx", "ts", "tsx", "mts", "cts", // Python
    "py", "pyi", "pyw", // Rust
    "rs",  // Go
    "go",  // .NET
    "cs", "csx", "fs", "fsx", "vb", // JVM
    "java", "kt", "kts", "scala", "sc", "groovy", // C / C++
    "c", "h", "cpp", "cc", "cxx", "hpp", "hh", "hxx", // Ruby / PHP
    "rb", "php", "phtml", // Swift / Objective-C
    "swift", "m", "mm", // Other mainstream languages
    "dart", "lua", "r", "pl", "pm", "jl", "hs", "ex", "exs", "clj", "cljs", "cljc", "edn", "ml",
    "mli", "erl", "hrl", "zig", "nim", // Shell / scripting
    "sh", "bash", "zsh", "fish", "ps1", "psm1", "psd1", // SQL
    "sql",  // Images
    "png", "jpg", "jpeg", "gif", "svg", "webp", // Documents
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", // Audio
    "mp3", "wav", "ogg", "flac", "m4a", // Video
    "mp4", "webm", "mov", "avi",
];

/// Extensions that are text-based (support `--lines`).
/// Every non-binary entry in `ALLOWED_EXTENSIONS` also appears here.
const TEXT_EXTENSIONS: &[&str] = &[
    // Prose / data
    "md", "txt", "csv", "xml", "json", "yaml", "yml", "toml", "ini", "env", "conf",
    // Design
    "pen", // Web markup / styles
    "html", "htm", "css", "scss", "sass", "less", "vue", "svelte",
    // JavaScript / TypeScript
    "js", "mjs", "cjs", "jsx", "ts", "tsx", "mts", "cts", // Python
    "py", "pyi", "pyw", // Rust
    "rs",  // Go
    "go",  // .NET
    "cs", "csx", "fs", "fsx", "vb", // JVM
    "java", "kt", "kts", "scala", "sc", "groovy", // C / C++
    "c", "h", "cpp", "cc", "cxx", "hpp", "hh", "hxx", // Ruby / PHP
    "rb", "php", "phtml", // Swift / Objective-C
    "swift", "m", "mm", // Other mainstream languages
    "dart", "lua", "r", "pl", "pm", "jl", "hs", "ex", "exs", "clj", "cljs", "cljc", "edn", "ml",
    "mli", "erl", "hrl", "zig", "nim", // Shell / scripting
    "sh", "bash", "zsh", "fish", "ps1", "psm1", "psd1", // SQL
    "sql",
];

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

/// Resolve and sandbox a path. Returns an error if it escapes both the
/// base directory AND every declared trusted root.
///
/// When `unrestricted` is `true`, the sandbox check is skipped and the
/// path is resolved directly (absolute paths bypass the base join).
///
/// `trusted_roots` (rem-egp9): when the resolved path is not under
/// `base` but IS under one of the declared trusted roots, the call
/// succeeds. This is what makes `mcp__remargin__write` to a path
/// inside a declared trusted root that lives outside the spawn cwd
/// work — the per-op sandbox layer consults the same trusted-root set
/// the boot-time MCP cover already used.
///
/// # Errors
///
/// Returns an error if:
/// - The path cannot be canonicalized
/// - The resolved path escapes both `base` and every trusted root
///   (when `unrestricted` is false)
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

    // Absolute requests resolve against themselves; relative requests
    // join onto base. A trusted_root caller would otherwise be forced
    // to relative-out-of-tree (`../../trusted/foo.md`), which is
    // awkward. Allowing absolute-from-anywhere is safe because the
    // sandbox check below still gates access.
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
/// `trusted_roots` (rem-egp9): when the parent / nearest ancestor is
/// not under `base` but IS under one of the declared trusted roots,
/// the call succeeds and the missing directories are created. This
/// lets `mcp__remargin__write` create new files inside a declared
/// trusted root that lives outside the MCP spawn cwd.
///
/// # Errors
///
/// Returns an error if:
/// - No existing ancestor directory can be found
/// - The resolved path escapes both `base` and every trusted root
///   (when `unrestricted` is false)
/// - The requested path has no filename component
/// - Directory creation fails
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
    // Normalize to resolve `.` and `..` components so that sandbox
    // checks work correctly even when the system's canonicalize does
    // not (e.g. mocks).
    let joined = normalize_path(&raw_joined);
    let parent = joined
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", requested.display()))?;
    let filename = joined
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("path has no filename: {}", requested.display()))?;

    let parent_exists = system.exists(parent).unwrap_or(false);

    if !parent_exists {
        // Parent doesn't exist. Walk up to find the nearest existing
        // ancestor and sandbox-check it before creating any
        // directories.
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
/// Best-effort: each trusted root is canonicalized (when possible)
/// before the comparison. The expanded form is used as a fallback so
/// trusted roots that don't exist on disk yet still match — same
/// best-effort semantics as the resolver in
/// [`crate::config::permissions::resolve`].
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
