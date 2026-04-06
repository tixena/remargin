//! Tests for legacy comment migration.

use std::path::Path;

use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::{Mode, ResolvedConfig};
use crate::operations::migrate::migrate;
use crate::parser::{self, AuthorType};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
    }
}

// ---------------------------------------------------------------------------
// Test 1: Migrate user comment
// ---------------------------------------------------------------------------

#[test]
fn migrate_user_comment() {
    let doc = "\
# Document

```user comments
This is feedback from the user.
```
";
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc.as_bytes())
        .unwrap();
    let config = open_config();

    let results = migrate(&system, Path::new("/docs/test.md"), &config, false, false).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].original_role, "user");

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let parsed = parser::parse(&content).unwrap();
    assert_eq!(parsed.comments().len(), 1);
    assert!(parsed.legacy_comments().is_empty());

    let cm = &parsed.comments()[0];
    assert_eq!(cm.author, "legacy-user");
    assert_eq!(cm.author_type, AuthorType::Human);
}

// ---------------------------------------------------------------------------
// Test 2: Migrate agent comment with done marker
// ---------------------------------------------------------------------------

#[test]
fn migrate_agent_with_done_marker() {
    let doc = "\
# Document

```agent comments [done:2026-04-05]
Agent response.
```
";
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc.as_bytes())
        .unwrap();
    let config = open_config();

    let results = migrate(&system, Path::new("/docs/test.md"), &config, false, false).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].original_role, "agent");

    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let parsed = parser::parse(&content).unwrap();
    let cm = &parsed.comments()[0];
    assert_eq!(cm.author, "legacy-agent");
    assert_eq!(cm.author_type, AuthorType::Agent);
    assert_eq!(cm.ack.len(), 1);
    assert_eq!(cm.ack[0].author, "legacy-user");
}

// ---------------------------------------------------------------------------
// Test 3: Dry run
// ---------------------------------------------------------------------------

#[test]
fn dry_run_no_changes() {
    let doc = "\
```user comments
Feedback.
```
";
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc.as_bytes())
        .unwrap();
    let config = open_config();

    let results = migrate(&system, Path::new("/docs/test.md"), &config, true, false).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].new_id, "(dry-run)");

    // File unchanged.
    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let parsed = parser::parse(&content).unwrap();
    assert_eq!(parsed.legacy_comments().len(), 1);
}

// ---------------------------------------------------------------------------
// Test 4: No legacy comments
// ---------------------------------------------------------------------------

#[test]
fn no_legacy_comments() {
    let doc = "# Just plain markdown\n";
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc.as_bytes())
        .unwrap();
    let config = open_config();

    let results = migrate(&system, Path::new("/docs/test.md"), &config, false, false).unwrap();
    assert!(results.is_empty());
}

// ---------------------------------------------------------------------------
// Test 5: Backup created
// ---------------------------------------------------------------------------

#[test]
fn backup_created() {
    let doc = "\
```user comments
Content.
```
";
    let system = MockSystem::new()
        .with_file(Path::new("/docs/test.md"), doc.as_bytes())
        .unwrap();
    let config = open_config();

    migrate(&system, Path::new("/docs/test.md"), &config, false, true).unwrap();

    let backup_exists = system.exists(Path::new("/docs/test.md.bak")).unwrap();
    assert!(backup_exists);
}
