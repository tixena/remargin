//! Embedded Obsidian plugin install/uninstall.
//!
//! This module is gated behind the `obsidian` cargo feature. When enabled,
//! the build embeds `packages/remargin-obsidian/main.js` and
//! `packages/remargin-obsidian/manifest.json` via [`include_bytes!`] and
//! exposes `remargin obsidian install|uninstall` subcommands that write
//! those bytes into a vault's `.obsidian/plugins/remargin/` directory.
//!
//! Build ordering is the caller's responsibility: the TypeScript plugin
//! must be built (`pnpm -C packages/remargin-obsidian build` or
//! `just build-ts`) before `cargo build --features obsidian`. The convenience
//! recipe `just build-cli-obsidian` does both in order.

use core::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use serde_json::json;

/// Embedded plugin bundle.
///
/// Resolved relative to `CARGO_MANIFEST_DIR` so the path stays valid if the
/// crate is moved within the workspace.
static MAIN_JS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/remargin-obsidian/main.js"
));
static MANIFEST_JSON: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/remargin-obsidian/manifest.json"
));

/// Name of the Obsidian plugin settings file that we preserve across reinstalls.
const DATA_JSON: &str = "data.json";
/// Relative path of the `.obsidian/` directory used as the "is this a vault?"
/// sentinel.
const DOT_OBSIDIAN: &str = ".obsidian";
/// Relative path of the plugin directory inside a vault.
const PLUGIN_REL_PATH: &str = ".obsidian/plugins/remargin";

/// Successful install report, used for both JSON and text output formatting.
#[derive(Debug)]
pub struct Report {
    pub main_js_bytes: usize,
    pub manifest_bytes: usize,
    pub plugin_dir: PathBuf,
    /// `Some(n)` if `data.json` was preserved across the reinstall, else `None`.
    pub preserved_data_bytes: Option<usize>,
}

/// Outcome of an uninstall call.
#[derive(Debug)]
pub enum UninstallStatus {
    NotInstalled { plugin_dir: PathBuf },
    Removed { plugin_dir: PathBuf },
}

impl Report {
    pub fn to_json(&self) -> serde_json::Value {
        let mut value = json!({
            "installed": self.plugin_dir.display().to_string(),
            "main_js_bytes": self.main_js_bytes,
            "manifest_bytes": self.manifest_bytes,
        });
        if let Some(bytes) = self.preserved_data_bytes
            && let Some(map) = value.as_object_mut()
        {
            map.insert("preserved_data_bytes".to_owned(), json!(bytes));
        }
        value
    }

    pub fn to_text(&self) -> String {
        let mut msg = format!(
            "Installed remargin plugin to {}: main.js ({} bytes), manifest.json ({} bytes)",
            self.plugin_dir.display(),
            self.main_js_bytes,
            self.manifest_bytes
        );
        if let Some(bytes) = self.preserved_data_bytes {
            let _ = write!(msg, ", preserved data.json ({bytes} bytes)");
        }
        msg
    }
}

/// Install (or upgrade) the Obsidian plugin into the given vault.
///
/// Algorithm:
/// 1. Resolve and validate the vault.
/// 2. If `<plugin>/data.json` exists, read it into memory so we can restore
///    user settings after the reinstall.
/// 3. `remove_dir_all` the plugin directory (ignoring `NotFound`).
/// 4. `create_dir_all` a fresh plugin directory.
/// 5. Write `main.js`, `manifest.json`, and the preserved `data.json`.
pub fn install(system: &dyn System, cwd: &Path, vault_path: Option<&Path>) -> Result<Report> {
    let vault = resolve_vault(system, cwd, vault_path)?;
    let plugin_dir = vault.join(PLUGIN_REL_PATH);
    let data_json_path = plugin_dir.join(DATA_JSON);

    // Preserve user settings if they exist. os-shim exposes read_to_string
    // which is fine here because data.json is JSON text.
    let preserved_data = if system.exists(&data_json_path).unwrap_or(false) {
        match system.read_to_string(&data_json_path) {
            Ok(contents) => Some(contents),
            Err(err) => {
                eprintln!(
                    "warning: failed to read {}: {err}. Settings will not be preserved.",
                    data_json_path.display()
                );
                None
            }
        }
    } else {
        None
    };

    if system.exists(&plugin_dir).unwrap_or(false) {
        system
            .remove_dir_all(&plugin_dir)
            .with_context(|| format!("failed to remove {}", plugin_dir.display()))?;
    }
    system
        .create_dir_all(&plugin_dir)
        .with_context(|| format!("failed to create {}", plugin_dir.display()))?;

    let main_js_path = plugin_dir.join("main.js");
    system
        .write(&main_js_path, MAIN_JS)
        .with_context(|| format!("failed to write {}", main_js_path.display()))?;

    let manifest_path = plugin_dir.join("manifest.json");
    system
        .write(&manifest_path, MANIFEST_JSON)
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    let preserved_bytes = if let Some(contents) = preserved_data.as_ref() {
        system
            .write(&data_json_path, contents.as_bytes())
            .with_context(|| format!("failed to write {}", data_json_path.display()))?;
        Some(contents.len())
    } else {
        None
    };

    Ok(Report {
        main_js_bytes: MAIN_JS.len(),
        manifest_bytes: MANIFEST_JSON.len(),
        plugin_dir,
        preserved_data_bytes: preserved_bytes,
    })
}

/// Remove the plugin directory entirely. Idempotent -- running on a vault
/// without the plugin installed is a no-op that returns [`UninstallStatus::NotInstalled`].
pub fn uninstall(
    system: &dyn System,
    cwd: &Path,
    vault_path: Option<&Path>,
) -> Result<UninstallStatus> {
    let vault = resolve_vault(system, cwd, vault_path)?;
    let plugin_dir = vault.join(PLUGIN_REL_PATH);

    if !system.exists(&plugin_dir).unwrap_or(false) {
        return Ok(UninstallStatus::NotInstalled { plugin_dir });
    }

    system
        .remove_dir_all(&plugin_dir)
        .with_context(|| format!("failed to remove {}", plugin_dir.display()))?;

    Ok(UninstallStatus::Removed { plugin_dir })
}

/// Resolve the vault root from an explicit `--vault-path` override or fall
/// back to the current working directory. Verifies that `<vault>/.obsidian/`
/// exists, erroring loudly if the directory is not an Obsidian vault.
fn resolve_vault(system: &dyn System, cwd: &Path, vault_path: Option<&Path>) -> Result<PathBuf> {
    let vault = vault_path.map_or_else(|| cwd.to_path_buf(), Path::to_path_buf);
    let dot_obsidian = vault.join(DOT_OBSIDIAN);
    let is_dir = system
        .is_dir(&dot_obsidian)
        .with_context(|| format!("failed to inspect {}", dot_obsidian.display()))?;
    if !is_dir {
        bail!(
            "not an Obsidian vault -- {} does not exist",
            dot_obsidian.display()
        );
    }
    Ok(vault)
}

#[cfg(test)]
mod tests {
    use os_shim::mock::MockSystem;

    use super::*;

    fn seed_vault(fs: &MockSystem, vault: &Path) {
        fs.create_dir_all(&vault.join(".obsidian")).unwrap();
    }

    #[test]
    fn install_creates_plugin_dir_with_artifacts() {
        let fs = MockSystem::new();
        let vault = PathBuf::from("/home/user/vault");
        seed_vault(&fs, &vault);

        let report = install(&fs, &vault, None).unwrap();
        assert_eq!(report.plugin_dir, vault.join(PLUGIN_REL_PATH));
        assert!(report.preserved_data_bytes.is_none());
        assert!(fs.exists(&report.plugin_dir.join("main.js")).unwrap());
        assert!(fs.exists(&report.plugin_dir.join("manifest.json")).unwrap());
    }

    #[test]
    fn install_errors_when_not_a_vault() {
        let fs = MockSystem::new();
        let cwd = Path::new("/home/user/docs");
        fs.create_dir_all(cwd).unwrap();

        let err = install(&fs, cwd, None).unwrap_err();
        assert!(err.to_string().contains("not an Obsidian vault"));
    }

    #[test]
    fn install_is_idempotent() {
        let fs = MockSystem::new();
        let vault = PathBuf::from("/home/user/vault");
        seed_vault(&fs, &vault);

        install(&fs, &vault, None).unwrap();
        let second = install(&fs, &vault, None).unwrap();
        assert!(fs.exists(&second.plugin_dir.join("main.js")).unwrap());
    }

    #[test]
    fn install_preserves_data_json() {
        let fs = MockSystem::new();
        let vault = PathBuf::from("/home/user/vault");
        seed_vault(&fs, &vault);

        // Seed an existing install with a data.json.
        let plugin_dir = vault.join(PLUGIN_REL_PATH);
        fs.create_dir_all(&plugin_dir).unwrap();
        let data_json = plugin_dir.join("data.json");
        let payload = br#"{"identity":"alice","sidebarSide":"left"}"#;
        fs.write(&data_json, payload).unwrap();

        let report = install(&fs, &vault, None).unwrap();
        assert_eq!(report.preserved_data_bytes, Some(payload.len()));
        let preserved = fs.read_to_string(&data_json).unwrap();
        assert_eq!(preserved.as_bytes(), payload);
    }

    #[test]
    fn install_with_explicit_vault_path() {
        let fs = MockSystem::new();
        let cwd = PathBuf::from("/tmp/anywhere");
        let vault = PathBuf::from("/home/user/other-vault");
        fs.create_dir_all(&cwd).unwrap();
        seed_vault(&fs, &vault);

        let report = install(&fs, &cwd, Some(&vault)).unwrap();
        assert_eq!(report.plugin_dir, vault.join(PLUGIN_REL_PATH));
    }

    #[test]
    fn uninstall_errors_when_not_a_vault() {
        let fs = MockSystem::new();
        let cwd = Path::new("/home/user/docs");
        fs.create_dir_all(cwd).unwrap();

        let err = uninstall(&fs, cwd, None).unwrap_err();
        assert!(err.to_string().contains("not an Obsidian vault"));
    }

    #[test]
    fn uninstall_is_noop_when_not_installed() {
        let fs = MockSystem::new();
        let vault = PathBuf::from("/home/user/vault");
        seed_vault(&fs, &vault);

        let status = uninstall(&fs, &vault, None).unwrap();
        assert!(matches!(status, UninstallStatus::NotInstalled { .. }));
    }

    #[test]
    fn uninstall_removes_plugin_dir() {
        let fs = MockSystem::new();
        let vault = PathBuf::from("/home/user/vault");
        seed_vault(&fs, &vault);

        install(&fs, &vault, None).unwrap();
        let status = uninstall(&fs, &vault, None).unwrap();
        assert!(matches!(status, UninstallStatus::Removed { .. }));
        if let UninstallStatus::Removed { plugin_dir } = status {
            assert!(!fs.exists(&plugin_dir).unwrap());
        }
    }
}
