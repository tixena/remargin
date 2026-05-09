//! Move / rename a tracked file or directory.
//!
//! `remargin mv <src> <dst>` is the remargin-blessed alternative to Bash
//! `mv` on a restricted realm. It performs an atomic rename when src
//! and dst live on the same filesystem and falls back to copy + remove
//! on EXDEV. Both endpoints flow through the same sandbox / forbidden
//! / pre-mutate guards every other mutating op uses, so a `restrict`
//! entry covering either endpoint refuses the op.
//!
//! Bookkeeping that lives **inside** the markdown file (frontmatter,
//! sandbox entries, comment threads, signatures, identity references)
//! moves with the bytes — there is no path-keyed state to rewrite.
//! Attachments live in `<doc_dir>/<assets_dir>/` (a sibling directory
//! per the rules in [`crate::operations::copy_attachments`]); this op
//! does not relocate them. Cross-directory moves leave attachment
//! references resolving against the destination directory's assets
//! folder, mirroring how a hand-edited `mv` would behave.
//!
//! **Directory source.** When `src` resolves to a directory
//! the op renames the directory atomically via [`os_shim::System::rename`]
//! (filesystem-level rename of the dir as a unit). Every nested file
//! moves with the dir; comment threads / acks / signatures keep their
//! continuity because the path of every nested file changes
//! consistently. The same `op_guard` / sandbox / forbidden-target gates
//! fire — a `restrict` entry covering the directory (or the
//! destination parent) refuses the move.

#[cfg(test)]
mod tests;

use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use serde::Serialize;

use crate::config::ResolvedConfig;
use crate::document::allowlist;
use crate::permissions::op_guard::{CallerInfo, pre_mutate_check_for_caller};
use crate::writer::ensure_not_forbidden_target;

/// Inputs to [`mv`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MvArgs {
    /// Destination path (relative to `base_dir` or absolute).
    pub dst: PathBuf,
    /// Allow overwriting `dst` when it exists.
    pub force: bool,
    /// Source path (relative to `base_dir` or absolute).
    pub src: PathBuf,
}

impl MvArgs {
    /// Constructor that takes ownership of both paths and defaults
    /// `force` to `false`.
    #[must_use]
    pub const fn new(src: PathBuf, dst: PathBuf) -> Self {
        Self {
            dst,
            force: false,
            src,
        }
    }

    /// Builder-style mutator: opt the args into `--force` overwrite
    /// semantics. Returns `self` for chained construction in adapter
    /// code.
    #[must_use]
    pub const fn with_force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }
}

/// Outcome of a successful [`mv`] call.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is a documented JSON output field"
)]
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct MvOutcome {
    /// Bytes moved (file size at the source). `0` for a same-path no-op
    /// or for an idempotent `src missing, dst already at destination`
    /// re-run. For the directory case this is the sum of
    /// the sizes of every regular file inside the directory at rename
    /// time.
    pub bytes_moved: u64,
    /// Canonical absolute destination path the bytes now live at.
    pub dst_absolute: PathBuf,
    /// `true` when the rename fell back to `copy + remove` because the
    /// in-process rename returned `EXDEV` (cross-filesystem move).
    pub fallback_copy: bool,
    /// `true` when the source resolved to a directory: the
    /// op renamed the directory + every nested file as a unit.
    pub is_directory: bool,
    /// Number of regular files inside the source directory at rename
    /// time. `0` for the file-mv case AND for the no-op / already-
    /// settled branches. Reported in JSON so the caller knows how many
    /// nested files moved with the directory.
    pub nested_files_moved: usize,
    /// `true` when [`MvArgs::src`] and [`MvArgs::dst`] resolved to the
    /// same canonical path. The op is a no-op in this case.
    pub noop_same_path: bool,
    /// `true` when the destination existed before the call and was
    /// overwritten. Only ever `true` when [`MvArgs::force`] was set.
    pub overwritten: bool,
    /// Canonical absolute source path the bytes lived at before the
    /// op. For the idempotent `src missing, dst present` re-run case
    /// this is the lexical join of `base_dir` + `args.src` (since
    /// canonicalization fails when the source is gone).
    pub src_absolute: PathBuf,
}

/// Move or rename a single file from `args.src` to `args.dst`.
///
/// Same-FS moves use `os_shim::System::rename` for an atomic
/// filesystem-level rename. When the rename fails with `EXDEV`
/// (cross-filesystem), the op falls back to `System::copy` followed by
/// `System::remove_file` on the source — the source is removed only
/// after the destination write returned `Ok`.
///
/// Both endpoints flow through:
///
/// - [`ensure_not_forbidden_target`] — refuses moves that would touch
///   reserved basenames (`.remargin.yaml`, etc.).
/// - [`allowlist::resolve_sandboxed`] (src) and
///   [`allowlist::resolve_sandboxed_create`] (dst) — refuses paths
///   that escape the sandbox.
/// - [`pre_mutate_check`] — refuses paths covered by a `restrict`
///   entry.
///
/// # Idempotence
///
/// The op is idempotent in the two senses an agent retry needs:
///
/// 1. **Same-path no-op**: `mv a a` returns `bytes_moved = 0`,
///    `noop_same_path = true`, no FS change.
/// 2. **Already-at-destination**: when the canonical `src` is missing
///    AND the canonical `dst` exists, the op succeeds with
///    `bytes_moved = 0` and the dst metadata. This lets a retried
///    `mv` after a partial-success-then-network-blip scenario settle
///    cleanly.
///
/// Both endpoints missing is an error (caller asked to move nothing).
///
/// # Errors
///
/// Returns an error when:
///
/// - Either endpoint is a forbidden target (e.g. `.remargin.yaml`).
/// - Either endpoint escapes the sandbox.
/// - Either endpoint is covered by a `restrict` entry the caller is
///   not authorised under (per [`pre_mutate_check`]).
/// - `args.src` is missing AND `args.dst` is also missing.
/// - `args.src` is a file AND `args.dst` is an existing directory
///   (file-into-directory moves require an explicit destination
///   path; only directory-into-empty-or-non-existent is supported
///   without it, ).
/// - `args.dst` already exists and `args.force` is `false`.
/// - The underlying `rename` (and, on `EXDEV` fallback, `copy` /
///   `remove_file`) fails.
pub fn mv(
    system: &dyn System,
    base_dir: &Path,
    config: &ResolvedConfig,
    args: &MvArgs,
) -> Result<MvOutcome> {
    ensure_not_forbidden_target(&args.src)?;
    ensure_not_forbidden_target(&args.dst)?;

    let src_lexical = lexical_join(base_dir, &args.src);
    let src_is_dir = system.is_dir(&src_lexical).unwrap_or(false);
    let src_resolved_opt = resolve_src(system, base_dir, &args.src, &src_lexical, config)?;

    let dst_lexical = lexical_join(base_dir, &args.dst);
    let dst_is_dir = system.is_dir(&dst_lexical).unwrap_or(false);

    // For the file-mv path, we historically rejected the call when
    // the destination was an existing directory. That rejection still
    // applies — but ONLY when the source is a file. A directory-rename
    // wants to move a dir into the dst path; an existing dst dir there
    // is the overwrite-or-conflict case handled below.
    if !src_is_dir && dst_is_dir {
        bail!(
            "destination is a directory: {} (this op moves a single file; pass an explicit destination path)",
            args.dst.display()
        );
    }

    // Resolve dst as a create-target. This canonicalises the parent
    // dir + appends the filename so the sandbox boundary is enforced
    // even when dst doesn't exist yet.
    let dst_resolved = allowlist::resolve_sandboxed_create(
        system,
        base_dir,
        &args.dst,
        config.unrestricted,
        &config.trusted_roots,
    )?;
    ensure_not_forbidden_target(&dst_resolved)?;

    let caller = config.caller_info();
    let Some(src_resolved) = src_resolved_opt else {
        return idempotent_already_settled(system, &dst_resolved, &src_lexical, &args.src, &caller);
    };

    if system.is_dir(&src_resolved).unwrap_or(false) {
        return mv_directory(system, args, &src_resolved, dst_resolved, &caller);
    }

    if !allowlist::is_visible(&src_resolved, false) {
        bail!("source not visible: {}", args.src.display());
    }

    if src_resolved == dst_resolved {
        return same_path_noop(system, &src_resolved, dst_resolved, &caller);
    }

    // Per-op guard on BOTH endpoints. A restrict entry on either side
    // refuses the move — symmetrically with how `mv`'s default deny
    // expansion now blocks both source-side and destination-side
    // shell `mv`.
    pre_mutate_check_for_caller(system, "mv", &src_resolved, &caller)?;
    pre_mutate_check_for_caller(system, "mv", &dst_resolved, &caller)?;

    let dst_pre_existed = system.exists(&dst_resolved).unwrap_or(false);
    if dst_pre_existed && !args.force {
        bail!(
            "destination exists: {} (pass --force to overwrite)",
            args.dst.display()
        );
    }

    let bytes_moved = file_size(system, &src_resolved);
    let fallback_copy = perform_move(system, &src_resolved, &dst_resolved)?;

    Ok(MvOutcome {
        bytes_moved,
        dst_absolute: dst_resolved,
        fallback_copy,
        is_directory: false,
        nested_files_moved: 0,
        noop_same_path: false,
        overwritten: dst_pre_existed,
        src_absolute: src_resolved,
    })
}

/// Lexical join: relative paths are appended to `base`, absolute paths
/// pass through. Used to size the source / destination before any
/// canonicalisation runs.
fn lexical_join(base: &Path, requested: &Path) -> PathBuf {
    if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        base.join(requested)
    }
}

/// Resolve `src` to a canonical absolute path. Returns `Ok(Some(_))`
/// when the file exists, `Ok(None)` when it is missing but the
/// requested path stayed inside the sandbox (so the
/// `idempotent-already-settled` branch can run), and an error when
/// canonicalisation or the sandbox check fails.
fn resolve_src(
    system: &dyn System,
    base_dir: &Path,
    requested: &Path,
    src_lexical: &Path,
    config: &ResolvedConfig,
) -> Result<Option<PathBuf>> {
    if system.exists(src_lexical).unwrap_or(false) {
        Ok(Some(allowlist::resolve_sandboxed(
            system,
            base_dir,
            requested,
            config.unrestricted,
            &config.trusted_roots,
        )?))
    } else {
        // Sandbox-validate the requested source even though the file
        // is missing — escaping the sandbox is the same kind of
        // boundary violation regardless of whether the file exists.
        allowlist::resolve_sandboxed_create(
            system,
            base_dir,
            requested,
            config.unrestricted,
            &config.trusted_roots,
        )?;
        Ok(None)
    }
}

/// Settle the idempotent re-run case: src is gone but dst is in
/// place. Runs the per-op guard so a freshly-restricted destination
/// still refuses the call, then returns the documented `bytes_moved =
/// 0` outcome.
fn idempotent_already_settled(
    system: &dyn System,
    dst_resolved: &Path,
    src_lexical: &Path,
    requested_src: &Path,
    caller: &CallerInfo,
) -> Result<MvOutcome> {
    if system.exists(dst_resolved).unwrap_or(false) {
        pre_mutate_check_for_caller(system, "mv", dst_resolved, caller)?;
        let is_directory = system.is_dir(dst_resolved).unwrap_or(false);
        return Ok(MvOutcome {
            bytes_moved: 0,
            dst_absolute: dst_resolved.to_path_buf(),
            fallback_copy: false,
            is_directory,
            nested_files_moved: 0,
            noop_same_path: false,
            overwritten: false,
            src_absolute: src_lexical.to_path_buf(),
        });
    }
    bail!(
        "source not found: {} (and destination does not exist either)",
        requested_src.display()
    )
}

/// Settle the same-path no-op: `mv a.md a.md` after canonicalisation.
/// Reads the file size for the outcome and runs the guard so a
/// freshly-restricted same-path call refuses cleanly.
fn same_path_noop(
    system: &dyn System,
    src_resolved: &Path,
    dst_resolved: PathBuf,
    caller: &CallerInfo,
) -> Result<MvOutcome> {
    pre_mutate_check_for_caller(system, "mv", src_resolved, caller)?;
    let is_directory = system.is_dir(src_resolved).unwrap_or(false);
    let bytes_moved = if is_directory {
        0
    } else {
        file_size(system, src_resolved)
    };
    Ok(MvOutcome {
        bytes_moved,
        dst_absolute: dst_resolved,
        fallback_copy: false,
        is_directory,
        nested_files_moved: 0,
        noop_same_path: true,
        overwritten: false,
        src_absolute: src_resolved.to_path_buf(),
    })
}

/// Read the file at `path` and return its size in bytes, or `0` when
/// the read fails (the outcome's `bytes_moved` field is informational
/// — a failure here doesn't change whether the move itself succeeded).
fn file_size(system: &dyn System, path: &Path) -> u64 {
    let Ok(content) = system.read_to_string(path) else {
        return 0;
    };
    u64::try_from(content.len()).unwrap_or(0)
}

/// Perform the actual byte move. Tries `rename` first; on `EXDEV`,
/// falls back to `copy` + `remove_file` (source is removed only after
/// the destination write returns `Ok`). Returns `true` when the
/// fallback fired.
fn perform_move(system: &dyn System, src: &Path, dst: &Path) -> Result<bool> {
    match system.rename(src, dst) {
        Ok(()) => Ok(false),
        Err(err) if is_cross_filesystem(&err) => {
            system.copy(src, dst).with_context(|| {
                format!(
                    "cross-filesystem fallback: copying {} -> {}",
                    src.display(),
                    dst.display()
                )
            })?;
            system.remove_file(src).with_context(|| {
                format!(
                    "cross-filesystem fallback: removing source {} after copy",
                    src.display()
                )
            })?;
            Ok(true)
        }
        Err(err) => Err(anyhow::Error::from(err).context(format!(
            "renaming {} -> {}",
            src.display(),
            dst.display()
        ))),
    }
}

/// Move / rename a directory atomically. Mirrors the
/// file-mv path: same `op_guard` / sandbox / forbidden-target gates,
/// same `--force` semantics, but operates on a directory tree as a
/// single unit via [`os_shim::System::rename`].
///
/// The `is_visible(_, true)` check rejects dot-prefixed source dirs
/// (`.git/foo` vs `secret`) so the dot-folder default-deny remains
/// enforced symmetrically with the file path.
fn mv_directory(
    system: &dyn System,
    args: &MvArgs,
    src_resolved: &Path,
    dst_resolved: PathBuf,
    caller: &CallerInfo,
) -> Result<MvOutcome> {
    if !allowlist::is_visible(src_resolved, true) {
        bail!("source not visible: {}", args.src.display());
    }

    if *src_resolved == dst_resolved {
        return same_path_noop(system, src_resolved, dst_resolved, caller);
    }

    pre_mutate_check_for_caller(system, "mv", src_resolved, caller)?;
    pre_mutate_check_for_caller(system, "mv", &dst_resolved, caller)?;

    let dst_pre_existed = system.exists(&dst_resolved).unwrap_or(false);
    if dst_pre_existed && !args.force {
        bail!(
            "destination exists: {} (pass --force to overwrite)",
            args.dst.display()
        );
    }

    // Count nested files + total bytes before the rename so the
    // outcome can report them. After the rename the source path is
    // gone.
    let (nested_files_moved, bytes_moved) = directory_size_summary(system, src_resolved);

    if dst_pre_existed && args.force {
        // Clear the destination first so the rename can land. We
        // remove the whole subtree (matching `mv -f` semantics on a
        // directory destination). If removal fails, surface that
        // error verbatim — the rename has not started.
        if system.is_dir(&dst_resolved).unwrap_or(false) {
            system.remove_dir_all(&dst_resolved).with_context(|| {
                format!("removing existing destination {}", dst_resolved.display())
            })?;
        } else {
            system
                .remove_file(&dst_resolved)
                .with_context(|| format!("removing existing file {}", dst_resolved.display()))?;
        }
    }

    let fallback_copy = perform_move(system, src_resolved, &dst_resolved)?;

    Ok(MvOutcome {
        bytes_moved,
        dst_absolute: dst_resolved,
        fallback_copy,
        is_directory: true,
        nested_files_moved,
        noop_same_path: false,
        overwritten: dst_pre_existed,
        src_absolute: src_resolved.to_path_buf(),
    })
}

/// Walk the tree rooted at `dir` and return `(file_count, total_bytes)`.
/// Best-effort: an unreadable subtree returns `(0, 0)` rather than
/// failing the rename. The numbers are informational — the rename's
/// success doesn't hinge on them.
fn directory_size_summary(system: &dyn System, dir: &Path) -> (usize, u64) {
    let Ok(entries) = system.walk_dir(dir, false, true) else {
        return (0, 0);
    };
    let mut count: usize = 0;
    let mut bytes: u64 = 0;
    for entry in entries {
        if !entry.is_file {
            continue;
        }
        count = count.saturating_add(1);
        if let Ok(meta) = system.metadata(&entry.path) {
            bytes = bytes.saturating_add(meta.len);
        }
    }
    (count, bytes)
}

/// Detect EXDEV (cross-filesystem rename) on both real Linux/macOS
/// (`raw_os_error == 18` on Linux, `18` on macOS) and the synthetic
/// `ErrorKind::CrossesDevices` surface stable Rust started exposing
/// without a stable enum variant. The existing `kind() ==
/// ErrorKind::Other` fallback covers older toolchains and mocks that
/// don't carry a raw OS error.
fn is_cross_filesystem(err: &io::Error) -> bool {
    if err.raw_os_error() == Some(18_i32) {
        return true;
    }
    let msg = err.to_string();
    msg.contains("EXDEV")
        || msg.contains("cross-device")
        || msg.contains("Cross-device")
        || msg.contains("crosses devices")
}
