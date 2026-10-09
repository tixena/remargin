//! Source scan for the overlay-model symbols and for the word `override` in identity code.

use std::fs;
use std::path::{Path, PathBuf};

/// Must not reappear anywhere under `crates/`.
const BANNED_TOKENS_ANYWHERE: &[&str] = &[
    "CliOverrides",
    "OverrideScratch",
    "apply_identity_overrides",
    "build_overrides",
    "with_identity_overrides",
];

/// Relative to the repo root; these must not contain the word `override`, in any case. Only
/// identity-resolution code is listed.
const IDENTITY_FILES: &[&str] = &[
    "crates/remargin-core/src/config.rs",
    "crates/remargin-core/src/config/identity.rs",
    "crates/remargin-core/src/config/tests.rs",
    "crates/remargin-core/src/mcp.rs",
    "crates/remargin-core/src/mcp/tests.rs",
    "crates/remargin/src/main.rs",
];

/// `(file, substring, reason)` exemptions; empty.
const ALLOWLIST: &[(&str, &str, &str)] = &[];

fn repo_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| manifest.clone(), Path::to_path_buf)
}

fn read(path: &Path) -> String {
    let Ok(text) = fs::read_to_string(path) else {
        return String::new();
    };
    text
}

fn walk_rust_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_rust_files_inner(root, &mut out);
    out
}

fn walk_rust_files_inner(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if name == "target" || name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk_rust_files_inner(&path, out);
        } else {
            let is_rs = path.extension().and_then(|s| s.to_str()) == Some("rs");
            // This guard names the banned symbols and `override` by design, so it skips itself.
            let is_self = name == "no_override_in_identity.rs"
                || path.ends_with("no_override_in_identity/tests.rs");
            if is_rs && !is_self {
                out.push(path);
            }
        }
    }
}

#[test]
fn banned_system_b_symbols_are_absent_from_every_rust_file() {
    let root = repo_root().join("crates");
    let files = walk_rust_files(&root);
    assert!(
        !files.is_empty(),
        "walker found no .rs files under {}",
        root.display()
    );

    let mut hits: Vec<String> = Vec::new();
    for file in &files {
        let content = read(file);
        for &banned in BANNED_TOKENS_ANYWHERE {
            if content.contains(banned) {
                hits.push(format!(
                    "{} contains banned symbol {banned:?}",
                    file.display()
                ));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "System B symbols reappeared; rewire the offending code \
             through config::identity::resolve_identity instead of \
             reintroducing the deleted overlay model. Offenders:\n{}",
        hits.join("\n")
    );
}

#[test]
fn override_word_absent_from_identity_code() {
    let root = repo_root();
    let mut hits: Vec<String> = Vec::new();

    for rel in IDENTITY_FILES {
        let path = root.join(rel);
        let content = read(&path);
        for (line_no, line) in content.lines().enumerate() {
            if !line.to_lowercase().contains("override") {
                continue;
            }
            if ALLOWLIST
                .iter()
                .any(|(allow_file, needle, _)| allow_file == rel && line.contains(needle))
            {
                continue;
            }
            hits.push(format!("{}:{}: {}", rel, line_no + 1, line.trim_end()));
        }
    }

    assert!(
        hits.is_empty(),
        "the word \"override\" has no place in identity resolution \
             code. Rename / rephrase the offenders, or add \
             them to ALLOWLIST with a reason. Offenders:\n{}",
        hits.join("\n")
    );
}
