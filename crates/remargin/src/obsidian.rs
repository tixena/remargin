//! Obsidian plugin install/uninstall.
//!
//! Gated behind the `obsidian` feature. Install fetches `main.js` and
//! `manifest.json` from the GitHub release tagged
//! `obsidian-v{CARGO_PKG_VERSION}` so a CLI binary always installs
//! the plugin build shipped with its own release — no TypeScript
//! source tree required to compile. [`fetch_plugin_assets`] does the
//! network; [`install_from_bytes`] writes bytes to disk preserving
//! `data.json`. Tests drive the latter with stub bytes so no test
//! touches the network.

use core::fmt::Write as _;
use core::time::Duration;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use os_shim::System;
use serde_json::json;

/// The plugin settings file, preserved across reinstalls.
const DATA_JSON: &str = "data.json";
/// The directory that marks a folder as a vault.
const DOT_OBSIDIAN: &str = ".obsidian";
const PLUGIN_REL_PATH: &str = ".obsidian/plugins/remargin";

const RELEASE_BASE: &str = "https://github.com/tixena/remargin/releases/download";
/// Assets are roughly 100 KB; anything slower than this is a real network problem.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Guards against a release asset that points at something huge.
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

    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(FETCH_TIMEOUT))
            .build(),
    );

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
    if status.as_u16() != 200 {
        bail!("unexpected HTTP status {status} from {url}");
    }

    let mut bytes = Vec::new();
    response
        .into_body()
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

    // `read_to_string` is enough: `data.json` is JSON text.
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
mod tests;
