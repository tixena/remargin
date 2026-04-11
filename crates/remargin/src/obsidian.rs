//! Obsidian plugin install/uninstall.
//!
//! This module is gated behind the `obsidian` cargo feature. When enabled,
//! it exposes `remargin obsidian install|uninstall` subcommands that install
//! the remargin Obsidian plugin into a vault's `.obsidian/plugins/remargin/`
//! directory.
//!
//! ## Where the plugin bytes come from
//!
//! The install path fetches `main.js` and `manifest.json` at runtime from
//! the GitHub release tagged `obsidian-v{CARGO_PKG_VERSION}`:
//!
//! ```text
//! https://github.com/tixena/remargin/releases/download/obsidian-v{version}/main.js
//! https://github.com/tixena/remargin/releases/download/obsidian-v{version}/manifest.json
//! ```
//!
//! The version is baked in from the workspace `Cargo.toml` at compile time,
//! so a given CLI binary always installs the plugin build that ships with
//! its own release. No TypeScript source tree is required to compile the
//! CLI — `cargo install --git … --features obsidian` succeeds on a clean
//! checkout.
//!
//! The module splits the install flow into two pieces so tests stay pure:
//!
//! - [`fetch_plugin_assets`] performs the network round-trips and returns
//!   raw bytes.
//! - [`install_from_bytes`] takes the bytes plus a vault path and writes
//!   them to disk, preserving any existing `data.json`.
//!
//! The public [`install`] entry point composes the two. Unit tests drive
//! [`install_from_bytes`] directly with stub byte slices so no test touches
//! the network.

use core::fmt::Write as _;
use core::time::Duration;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use serde_json::json;

/// Name of the Obsidian plugin settings file that we preserve across reinstalls.
const DATA_JSON: &str = "data.json";
/// Relative path of the `.obsidian/` directory used as the "is this a vault?"
/// sentinel.
const DOT_OBSIDIAN: &str = ".obsidian";
/// Relative path of the plugin directory inside a vault.
const PLUGIN_REL_PATH: &str = ".obsidian/plugins/remargin";

/// Base URL of the GitHub release assets served by the `tixena/remargin`
/// repository.
const RELEASE_BASE: &str = "https://github.com/tixena/remargin/releases/download";
/// Per-request network timeout. Assets are roughly 100 KB; anything slower
/// than this is a real network problem, not transient flake.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Hard cap on the number of bytes we are willing to read from a single
/// asset. Acts as a defense against a misconfigured release that points at
/// something huge.
const MAX_ASSET_BYTES: u64 = 16 * 1024 * 1024;

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

/// Return the version string baked in at compile time. Exposed so the CLI
/// can print a human-readable "Downloading remargin plugin v…" line before
/// the (potentially slow) network round trip.
#[must_use]
pub const fn plugin_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Fetch `main.js` and `manifest.json` for the current CLI version from
/// GitHub Releases. Returns the two asset bodies as `(main_js, manifest)`.
///
/// Errors surface the fully-formed URL and the HTTP status (on non-200
/// responses) so operators can tell the difference between "network down"
/// and "release missing".
pub fn fetch_plugin_assets() -> Result<(Vec<u8>, Vec<u8>)> {
    let version = plugin_version();
    let main_js_url = format!("{RELEASE_BASE}/obsidian-v{version}/main.js");
    let manifest_url = format!("{RELEASE_BASE}/obsidian-v{version}/manifest.json");

    let agent = ureq::AgentBuilder::new().timeout(FETCH_TIMEOUT).build();

    let main_js = fetch_one(&agent, &main_js_url)?;
    let manifest = fetch_one(&agent, &manifest_url)?;
    Ok((main_js, manifest))
}

/// Perform a single GET against `url`, returning the body bytes. Any HTTP
/// error, non-200 status, or transport failure is mapped to an `anyhow`
/// error with the URL and (if available) status attached.
fn fetch_one(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>> {
    let response = agent
        .get(url)
        .call()
        .with_context(|| format!("failed to request {url}"))?;

    let status = response.status();
    if status != 200 {
        bail!("unexpected HTTP status {status} from {url}");
    }

    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_ASSET_BYTES)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read response body from {url}"))?;
    Ok(bytes)
}

/// Install (or upgrade) the Obsidian plugin into the given vault.
///
/// Fetches the plugin bytes from the matching GitHub release and delegates
/// to [`install_from_bytes`] to write them into the vault.
pub fn install(system: &dyn System, cwd: &Path, vault_path: Option<&Path>) -> Result<Report> {
    let (main_js, manifest) = fetch_plugin_assets()?;
    install_from_bytes(system, cwd, vault_path, &main_js, &manifest)
}

/// Write the provided `main_js` and `manifest` bytes into a vault's plugin
/// directory. This is the test-friendly core of the install flow: callers
/// (production and tests alike) supply the bytes.
///
/// Algorithm:
/// 1. Resolve and validate the vault.
/// 2. If `<plugin>/data.json` exists, read it into memory so we can restore
///    user settings after the reinstall.
/// 3. `remove_dir_all` the plugin directory (ignoring `NotFound`).
/// 4. `create_dir_all` a fresh plugin directory.
/// 5. Write `main.js`, `manifest.json`, and the preserved `data.json`.
pub fn install_from_bytes(
    system: &dyn System,
    cwd: &Path,
    vault_path: Option<&Path>,
    main_js: &[u8],
    manifest: &[u8],
) -> Result<Report> {
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
        .write(&main_js_path, main_js)
        .with_context(|| format!("failed to write {}", main_js_path.display()))?;

    let manifest_path = plugin_dir.join("manifest.json");
    system
        .write(&manifest_path, manifest)
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
        main_js_bytes: main_js.len(),
        manifest_bytes: manifest.len(),
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

    /// Stub `main.js` bytes used by every install test so we never hit the
    /// network. Contents are arbitrary -- only the length and identity
    /// matter for the assertions.
    const STUB_MAIN_JS: &[u8] = b"// stub main.js\nconsole.log('remargin');\n";
    /// Stub `manifest.json` bytes. Valid JSON but also arbitrary.
    const STUB_MANIFEST: &[u8] = br#"{"id":"remargin","name":"Remargin","version":"0.0.0-test"}"#;

    fn seed_vault(fs: &MockSystem, vault: &Path) {
        fs.create_dir_all(&vault.join(".obsidian")).unwrap();
    }

    fn install_stub(
        fs: &MockSystem,
        cwd: &Path,
        vault_path: Option<&Path>,
    ) -> anyhow::Result<Report> {
        install_from_bytes(fs, cwd, vault_path, STUB_MAIN_JS, STUB_MANIFEST)
    }

    #[test]
    fn install_creates_plugin_dir_with_artifacts() {
        let fs = MockSystem::new();
        let vault = PathBuf::from("/home/user/vault");
        seed_vault(&fs, &vault);

        let report = install_stub(&fs, &vault, None).unwrap();
        assert_eq!(report.plugin_dir, vault.join(PLUGIN_REL_PATH));
        assert!(report.preserved_data_bytes.is_none());
        assert_eq!(report.main_js_bytes, STUB_MAIN_JS.len());
        assert_eq!(report.manifest_bytes, STUB_MANIFEST.len());
        assert!(fs.exists(&report.plugin_dir.join("main.js")).unwrap());
        assert!(fs.exists(&report.plugin_dir.join("manifest.json")).unwrap());
    }

    #[test]
    fn install_writes_exact_bytes() {
        let fs = MockSystem::new();
        let vault = PathBuf::from("/home/user/vault");
        seed_vault(&fs, &vault);

        let report = install_stub(&fs, &vault, None).unwrap();
        let main_js = fs
            .read_to_string(&report.plugin_dir.join("main.js"))
            .unwrap();
        let manifest = fs
            .read_to_string(&report.plugin_dir.join("manifest.json"))
            .unwrap();
        assert_eq!(main_js.as_bytes(), STUB_MAIN_JS);
        assert_eq!(manifest.as_bytes(), STUB_MANIFEST);
    }

    #[test]
    fn install_errors_when_not_a_vault() {
        let fs = MockSystem::new();
        let cwd = Path::new("/home/user/docs");
        fs.create_dir_all(cwd).unwrap();

        let err = install_stub(&fs, cwd, None).unwrap_err();
        assert!(err.to_string().contains("not an Obsidian vault"));
    }

    #[test]
    fn install_is_idempotent() {
        let fs = MockSystem::new();
        let vault = PathBuf::from("/home/user/vault");
        seed_vault(&fs, &vault);

        install_stub(&fs, &vault, None).unwrap();
        let second = install_stub(&fs, &vault, None).unwrap();
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

        let report = install_stub(&fs, &vault, None).unwrap();
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

        let report = install_stub(&fs, &cwd, Some(&vault)).unwrap();
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

        install_stub(&fs, &vault, None).unwrap();
        let status = uninstall(&fs, &vault, None).unwrap();
        assert!(matches!(status, UninstallStatus::Removed { .. }));
        if let UninstallStatus::Removed { plugin_dir } = status {
            assert!(!fs.exists(&plugin_dir).unwrap());
        }
    }
}
