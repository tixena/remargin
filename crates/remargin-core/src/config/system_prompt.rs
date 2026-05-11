//! System-prompt walk-up resolver.
//!
//! Mirrors the identity walk in [`crate::config::identity::resolve_from_walk`]
//! but anchors on the `system_prompt:` block of a `.remargin.yaml` rather than
//! the identity fields. The walk is identity-free on purpose: a folder's
//! prompt is a property of the directory tree, not the caller.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use os_shim::System;
use serde::Serialize;

use crate::config::{CONFIG_FILENAME, Config};

/// Locked Default prompt body per the `y76` decision.
///
/// Public so adapters (CLI / MCP / tests) can compare against it
/// without re-typing the string. The `<files>` token is substituted by
/// the caller (the Submit pipeline); this resolver returns the body
/// verbatim.
pub const DEFAULT_PROMPT_BODY: &str =
    "Please process the comments in <files> using the remargin skill";

/// Resolved `system_prompt:` answer for one walk.
///
/// `source.is_none()` and `is_default = true` both fire when the walk
/// exhausted without finding a `system_prompt:` block. The two are kept
/// distinct so a future "explicit vault default" can declare a body in
/// `vault/.remargin.yaml` and keep `is_default = false` — the y76
/// fallback only fires when the walk produced nothing at all.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct ResolvedSystemPrompt {
    /// True when the walk exhausted with no match and the resolver
    /// returned [`DEFAULT_PROMPT_BODY`].
    pub is_default: bool,
    /// Human-readable name. Derived from the YAML `name:` field when
    /// present; otherwise from the owning folder basename. `"default"`
    /// for the fallback.
    pub name: String,
    /// Body to send to the AI.
    pub prompt: String,
    /// `.remargin.yaml` that declared the prompt. `None` for the
    /// fallback.
    pub source: Option<PathBuf>,
}

/// Walk upward from `file_path`'s parent directory looking for the
/// nearest `.remargin.yaml` that declares a `system_prompt:` block.
/// Falls through to the y76 Default when the walk exhausts.
///
/// `file_path` itself need not exist on disk; only its parent chain is
/// walked. Callers wanting to resolve for a directory can pass the
/// directory path directly — the function treats an extension-less path
/// as the starting directory.
///
/// # Errors
///
/// Returns an error only when a candidate `.remargin.yaml` exists but
/// cannot be read or parsed. A missing file is not an error; an
/// exhausted walk is not an error.
pub fn resolve_system_prompt(
    system: &dyn System,
    file_path: &Path,
) -> Result<ResolvedSystemPrompt> {
    let start_dir = if file_path.extension().is_some() {
        file_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    } else {
        file_path.to_path_buf()
    };

    let mut current = start_dir;
    loop {
        let candidate = current.join(CONFIG_FILENAME);
        if system
            .exists(&candidate)
            .with_context(|| format!("checking existence of {}", candidate.display()))?
        {
            let content = system
                .read_to_string(&candidate)
                .with_context(|| format!("reading {}", candidate.display()))?;
            let config: Config = serde_yaml::from_str(&content)
                .with_context(|| format!("parsing {}", candidate.display()))?;

            if let Some(sp) = config.system_prompt {
                let name = sp
                    .name
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| folder_name_for(&current));
                return Ok(ResolvedSystemPrompt {
                    is_default: false,
                    name,
                    prompt: sp.prompt,
                    source: Some(candidate),
                });
            }
        }
        if !current.pop() {
            break;
        }
    }

    Ok(ResolvedSystemPrompt {
        is_default: true,
        name: "default".to_owned(),
        prompt: DEFAULT_PROMPT_BODY.to_owned(),
        source: None,
    })
}

/// Derive a display name from the owning folder when `name:` is absent.
/// Empty / `.` / root all fall back to `"prompt"`.
fn folder_name_for(dir: &Path) -> String {
    dir.file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && *s != ".")
        .map_or_else(|| "prompt".to_owned(), str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use os_shim::mock::MockSystem;

    use super::{DEFAULT_PROMPT_BODY, resolve_system_prompt};

    #[test]
    fn nearest_ancestor_wins() {
        let system = MockSystem::new()
            .with_dir(Path::new("/vault/a/b/c"))
            .unwrap()
            .with_file(
                Path::new("/vault/a/.remargin.yaml"),
                b"system_prompt:\n  name: outer\n  prompt: outer body\n",
            )
            .unwrap()
            .with_file(
                Path::new("/vault/a/b/.remargin.yaml"),
                b"system_prompt:\n  name: inner\n  prompt: inner body\n",
            )
            .unwrap();

        let resolved = resolve_system_prompt(&system, Path::new("/vault/a/b/c/file.md")).unwrap();
        assert_eq!(resolved.prompt, "inner body");
        assert_eq!(resolved.name, "inner");
        assert_eq!(
            resolved.source.as_deref(),
            Some(Path::new("/vault/a/b/.remargin.yaml"))
        );
        assert!(!resolved.is_default);
    }

    #[test]
    fn walk_skips_configs_without_system_prompt() {
        let system = MockSystem::new()
            .with_dir(Path::new("/vault/a/b/c"))
            .unwrap()
            .with_file(
                Path::new("/vault/a/.remargin.yaml"),
                b"identity: someone\ntype: human\n",
            )
            .unwrap()
            .with_file(
                Path::new("/vault/.remargin.yaml"),
                b"system_prompt:\n  name: vault\n  prompt: vault body\n",
            )
            .unwrap();

        let resolved = resolve_system_prompt(&system, Path::new("/vault/a/b/c/file.md")).unwrap();
        assert_eq!(resolved.prompt, "vault body");
        assert_eq!(resolved.name, "vault");
    }

    #[test]
    fn walk_exhausts_to_default() {
        let system = MockSystem::new()
            .with_dir(Path::new("/vault/a/b/c"))
            .unwrap();

        let resolved = resolve_system_prompt(&system, Path::new("/vault/a/b/c/file.md")).unwrap();
        assert_eq!(resolved.prompt, DEFAULT_PROMPT_BODY);
        assert_eq!(resolved.name, "default");
        assert!(resolved.source.is_none());
        assert!(resolved.is_default);
    }

    #[test]
    fn name_absent_falls_back_to_folder_basename() {
        let system = MockSystem::new()
            .with_dir(Path::new("/vault/remargin"))
            .unwrap()
            .with_file(
                Path::new("/vault/remargin/.remargin.yaml"),
                b"system_prompt:\n  prompt: body\n",
            )
            .unwrap();

        let resolved =
            resolve_system_prompt(&system, Path::new("/vault/remargin/file.md")).unwrap();
        assert_eq!(resolved.name, "remargin");
    }

    #[test]
    fn explicit_name_overrides_folder() {
        let system = MockSystem::new()
            .with_dir(Path::new("/vault/remargin"))
            .unwrap()
            .with_file(
                Path::new("/vault/remargin/.remargin.yaml"),
                b"system_prompt:\n  name: SWE reviewer\n  prompt: body\n",
            )
            .unwrap();

        let resolved =
            resolve_system_prompt(&system, Path::new("/vault/remargin/file.md")).unwrap();
        assert_eq!(resolved.name, "SWE reviewer");
    }

    #[test]
    fn directory_input_starts_walk_at_directory() {
        let system = MockSystem::new()
            .with_dir(Path::new("/vault/a/b"))
            .unwrap()
            .with_file(
                Path::new("/vault/a/b/.remargin.yaml"),
                b"system_prompt:\n  name: here\n  prompt: body\n",
            )
            .unwrap();

        let resolved = resolve_system_prompt(&system, Path::new("/vault/a/b")).unwrap();
        assert_eq!(resolved.name, "here");
    }

    #[test]
    fn empty_prompt_returned_verbatim() {
        let system = MockSystem::new()
            .with_dir(Path::new("/vault/a"))
            .unwrap()
            .with_file(
                Path::new("/vault/a/.remargin.yaml"),
                b"system_prompt:\n  name: empty\n  prompt: \"\"\n",
            )
            .unwrap();

        let resolved = resolve_system_prompt(&system, Path::new("/vault/a/file.md")).unwrap();
        assert_eq!(resolved.prompt, "");
        assert!(!resolved.is_default);
    }

    #[test]
    fn malformed_yaml_errors_with_path() {
        let system = MockSystem::new()
            .with_dir(Path::new("/vault/a"))
            .unwrap()
            .with_file(
                Path::new("/vault/a/.remargin.yaml"),
                b"system_prompt: [oops\n",
            )
            .unwrap();

        let err = resolve_system_prompt(&system, Path::new("/vault/a/file.md")).unwrap_err();
        let chain = format!("{err:#}");
        assert!(chain.contains("/vault/a/.remargin.yaml"), "{chain}");
    }

    #[test]
    fn legacy_config_without_system_prompt_continues_walk() {
        let system = MockSystem::new()
            .with_dir(Path::new("/vault/a/b"))
            .unwrap()
            .with_file(
                Path::new("/vault/a/b/.remargin.yaml"),
                b"identity: foo\ntype: agent\nmode: open\n",
            )
            .unwrap()
            .with_file(
                Path::new("/vault/a/.remargin.yaml"),
                b"system_prompt:\n  name: outer\n  prompt: outer body\n",
            )
            .unwrap();

        let resolved = resolve_system_prompt(&system, Path::new("/vault/a/b/file.md")).unwrap();
        assert_eq!(resolved.name, "outer");
    }

    #[test]
    fn vault_root_explicit_prompt_not_default() {
        let system = MockSystem::new()
            .with_dir(Path::new("/vault/a"))
            .unwrap()
            .with_file(
                Path::new("/vault/.remargin.yaml"),
                b"system_prompt:\n  name: vault-default\n  prompt: vault body\n",
            )
            .unwrap();

        let resolved = resolve_system_prompt(&system, Path::new("/vault/a/file.md")).unwrap();
        assert_eq!(resolved.name, "vault-default");
        assert!(!resolved.is_default);
        assert!(resolved.source.is_some());
    }

    #[test]
    fn missing_prompt_field_errors() {
        let system = MockSystem::new()
            .with_dir(Path::new("/vault/a"))
            .unwrap()
            .with_file(
                Path::new("/vault/a/.remargin.yaml"),
                b"system_prompt:\n  name: incomplete\n",
            )
            .unwrap();

        let err = resolve_system_prompt(&system, Path::new("/vault/a/file.md")).unwrap_err();
        let chain = format!("{err:#}");
        assert!(chain.contains("/vault/a/.remargin.yaml"), "{chain}");
    }
}
