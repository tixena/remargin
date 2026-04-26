//! Unit tests for [`crate::permissions::sidecar`] (rem-yj1j.4 /
//! rem-70za).
//!
//! Covers scenarios 14-17 from the rem-yj1j.4 testing plan
//! (.gitignore idempotency, gitignore creation, sidecar version
//! mismatch, malformed JSON) plus the basic add / remove / round-trip
//! invariants the apply / revert callers (slice 3) will rely on.

use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::permissions::sidecar::{
    SIDECAR_GITIGNORE_ENTRY, SIDECAR_RELATIVE_PATH, SIDECAR_VERSION, Sidecar, SidecarEntry,
    add_entry, load, remove_entry, save, sidecar_path,
};

fn empty_anchor() -> (MockSystem, PathBuf) {
    let anchor = PathBuf::from("/r");
    let system = MockSystem::new().with_dir(&anchor).unwrap();
    (system, anchor)
}

fn sample_entry() -> SidecarEntry {
    SidecarEntry {
        added_at: String::from("2026-04-26T10:00:00Z"),
        added_to_files: vec![
            PathBuf::from(".claude/settings.local.json"),
            PathBuf::from("/home/u/.claude/settings.json"),
        ],
        allow: vec![String::from("mcp__remargin__*")],
        deny: vec![
            String::from("Edit(///r/secret/**)"),
            String::from("Write(///r/secret/**)"),
        ],
    }
}

/// `Sidecar::new` returns an empty payload pinned to the current
/// version constant.
#[test]
fn sidecar_new_starts_empty_at_current_version() {
    let sidecar = Sidecar::new();
    assert!(sidecar.entries.is_empty());
    assert_eq!(sidecar.version, SIDECAR_VERSION);
}

/// Loading from a missing file yields an empty sidecar at the current
/// version — no error. This is the bootstrap case for the very first
/// `restrict` invocation.
#[test]
fn load_missing_returns_empty_sidecar() {
    let (system, anchor) = empty_anchor();
    let sidecar = load(&system, &anchor).unwrap();
    assert!(sidecar.entries.is_empty());
    assert_eq!(sidecar.version, SIDECAR_VERSION);
}

/// Round-trip: save an entry, reload, get the same entry back.
#[test]
fn save_and_load_round_trip() {
    let (system, anchor) = empty_anchor();
    let entry = sample_entry();
    add_entry(&system, &anchor, "/r/secret", entry.clone()).unwrap();

    let reloaded = load(&system, &anchor).unwrap();
    assert_eq!(reloaded.entries.len(), 1);
    assert_eq!(reloaded.entries["/r/secret"], entry);
}

/// `add_entry` for the same key twice replaces the prior record so a
/// re-apply tracks the latest deltas.
#[test]
fn add_entry_replaces_existing_record() {
    let (system, anchor) = empty_anchor();
    let first = sample_entry();
    add_entry(&system, &anchor, "/r/secret", first).unwrap();

    let mut second = sample_entry();
    second.added_at = String::from("2026-04-26T11:00:00Z");
    second.deny.push(String::from("Read(///r/secret/.git/**)"));
    add_entry(&system, &anchor, "/r/secret", second.clone()).unwrap();

    let reloaded = load(&system, &anchor).unwrap();
    assert_eq!(reloaded.entries.len(), 1);
    assert_eq!(reloaded.entries["/r/secret"], second);
}

/// `remove_entry` returns the prior record and persists the updated
/// sidecar minus that entry.
#[test]
fn remove_entry_returns_and_persists() {
    let (system, anchor) = empty_anchor();
    let entry = sample_entry();
    add_entry(&system, &anchor, "/r/secret", entry.clone()).unwrap();

    let removed = remove_entry(&system, &anchor, "/r/secret").unwrap();
    assert_eq!(removed, Some(entry));

    let after = load(&system, &anchor).unwrap();
    assert!(after.entries.is_empty());
}

/// `remove_entry` on a key that was never tracked returns `None` and
/// leaves the sidecar unchanged.
#[test]
fn remove_entry_missing_returns_none() {
    let (system, anchor) = empty_anchor();
    let removed = remove_entry(&system, &anchor, "/r/never-tracked").unwrap();
    assert!(removed.is_none());
}

/// Scenario 16: a sidecar with an unknown version is rejected with a
/// message that names both versions.
#[test]
fn load_rejects_unknown_version() {
    let (system, anchor) = empty_anchor();
    let body = "{\"version\":99,\"entries\":{}}\n";
    let path = sidecar_path(&anchor);
    system.create_dir_all(path.parent().unwrap()).unwrap();
    system.write(&path, body.as_bytes()).unwrap();

    let err = load(&system, &anchor).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains("version 99") && chain.contains(&format!("{SIDECAR_VERSION}")),
        "expected version-mismatch message, got: {chain}"
    );
}

/// Scenario 17: corrupt JSON surfaces an error that names the file.
#[test]
fn load_rejects_malformed_json() {
    let (system, anchor) = empty_anchor();
    let path = sidecar_path(&anchor);
    system.create_dir_all(path.parent().unwrap()).unwrap();
    system.write(&path, b"{not json").unwrap();

    let err = load(&system, &anchor).unwrap_err();
    let chain = format!("{err:#}");
    assert!(
        chain.contains(SIDECAR_RELATIVE_PATH),
        "error must name the sidecar file, got: {chain}"
    );
}

/// Scenario 15: missing `.gitignore` is created with the entry on
/// first save.
#[test]
fn save_creates_gitignore_when_absent() {
    let (system, anchor) = empty_anchor();
    save(&system, &anchor, &Sidecar::new()).unwrap();

    let gitignore = system.read_to_string(&anchor.join(".gitignore")).unwrap();
    assert!(
        gitignore.contains(SIDECAR_GITIGNORE_ENTRY),
        "missing entry in gitignore: {gitignore}"
    );
}

/// Scenario 14: re-running `save` with the entry already in
/// `.gitignore` does NOT duplicate it.
#[test]
fn save_does_not_duplicate_gitignore_entry() {
    let (system, anchor) = empty_anchor();
    save(&system, &anchor, &Sidecar::new()).unwrap();
    save(&system, &anchor, &Sidecar::new()).unwrap();

    let gitignore = system.read_to_string(&anchor.join(".gitignore")).unwrap();
    let count = gitignore
        .lines()
        .filter(|line| line.trim() == SIDECAR_GITIGNORE_ENTRY)
        .count();
    assert_eq!(count, 1, "gitignore: {gitignore}");
}

/// `.gitignore` already containing the entry (e.g. user pre-added it)
/// is left byte-identical.
#[test]
fn save_preserves_existing_gitignore_entry() {
    let (system, anchor) = empty_anchor();
    let original = format!("# user notes\n{SIDECAR_GITIGNORE_ENTRY}\n");
    system
        .write(&anchor.join(".gitignore"), original.as_bytes())
        .unwrap();
    save(&system, &anchor, &Sidecar::new()).unwrap();
    let updated = system.read_to_string(&anchor.join(".gitignore")).unwrap();
    assert_eq!(updated, original);
}

/// Save preserves unrelated `.gitignore` content and appends the new
/// entry on its own line.
#[test]
fn save_appends_to_existing_gitignore_without_clobbering() {
    let (system, anchor) = empty_anchor();
    let original = "target/\n*.log\n";
    system
        .write(&anchor.join(".gitignore"), original.as_bytes())
        .unwrap();
    save(&system, &anchor, &Sidecar::new()).unwrap();
    let updated = system.read_to_string(&anchor.join(".gitignore")).unwrap();
    assert!(updated.starts_with(original));
    assert!(updated.contains(SIDECAR_GITIGNORE_ENTRY));
}

/// Sidecar JSON is human-readable (pretty-printed) so diffs are
/// reviewable.
#[test]
fn saved_json_is_pretty_printed() {
    let (system, anchor) = empty_anchor();
    add_entry(&system, &anchor, "/r/secret", sample_entry()).unwrap();
    let body = system.read_to_string(&sidecar_path(&anchor)).unwrap();
    assert!(
        body.contains("\n  \""),
        "sidecar JSON should be pretty-printed, got: {body}"
    );
}

/// `sidecar_path` is the documented absolute resolution. Pin it so a
/// future relocation must update this test deliberately.
#[test]
fn sidecar_path_is_under_dot_claude() {
    assert_eq!(
        sidecar_path(Path::new("/r")),
        PathBuf::from("/r/.claude/.remargin-restrictions.json")
    );
}
