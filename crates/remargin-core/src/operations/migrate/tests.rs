//! Tests for legacy comment migration.
//!
//! Coverage layout:
//!
//! - existing happy paths kept (per-role role mapping, ack from
//!   `[done:DATE]`, no-op when nothing to migrate, backup writing) —
//!   re-stated against the new `MigrateIdentities` parameter using
//!   `MigrateIdentities::default()` so the byte-level fallback shape is
//!   pinned.
//! - exhaustive threading split / no-split matrix: every chain-breaking
//!   trigger and every chain-keeping trigger is its own test so a
//!   regression points at the single rule that broke.
//! - identity / signing / verify-gate integration: open-mode fallback,
//!   per-role configs that sign, strict-mode success with both configs,
//!   strict-mode failure modes that drive the original bug from the
//!   outside.

extern crate alloc;

use alloc::collections::BTreeMap;
use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MockSystem;

use crate::config::registry::Registry;
use crate::config::{Mode, ResolvedConfig};
use crate::operations::migrate::{MigrateIdentities, MigrateRoleIdentity, migrate};
use crate::operations::verify::verify_document;
use crate::parser::{self, AuthorType, Comment};

// ---- Test ed25519 key pair (matches `crypto::tests`) ------------------

const TEST_PRIVATE_KEY: &str = "\
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQAAAJDk27dx5Nu3
cQAAAAtzc2gtZWQyNTUxOQAAACC1X7nyFUdfsMF7x8GI40lTjtT8jK7q/sqImy3eaP4ZlQ
AAAEAk2Tz65AVfgL3ddyz72e8OkjFsl+pyRUGWLQkHBKtYx7VfufIVR1+wwXvHwYjjSVOO
1PyMrur+yoibLd5o/hmVAAAADXRlc3RAcmVtYXJnaW4=
-----END OPENSSH PRIVATE KEY-----
";

const TEST_PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILVfufIVR1+wwXvHwYjjSVOO1PyMrur+yoibLd5o/hmV test@remargin";

// ---- Fixtures ---------------------------------------------------------

fn open_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("eduardo")),
        ignore: Vec::new(),
        key_path: None,
        mode: Mode::Open,
        registry: None,
        unrestricted: false,
    }
}

fn registry_with_alice_and_botty() -> Registry {
    let yaml = format!(
        "\
participants:
  alice:
    type: human
    status: active
    pubkeys:
      - {TEST_PUBLIC_KEY}
  botty:
    type: agent
    status: active
    pubkeys:
      - {TEST_PUBLIC_KEY}
",
    );
    serde_yaml::from_str(&yaml).unwrap()
}

fn strict_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("alice")),
        ignore: Vec::new(),
        key_path: Some(PathBuf::from("/keys/ed25519")),
        mode: Mode::Strict,
        registry: Some(registry_with_alice_and_botty()),
        unrestricted: false,
    }
}

/// Registered-mode config with the test registry. Used by signing
/// tests that want signatures to verify against the registered pubkeys
/// (open mode without a registry treats a present signature as
/// `Invalid` since there is nothing to verify against).
fn registered_config() -> ResolvedConfig {
    ResolvedConfig {
        assets_dir: String::from("assets"),
        author_type: Some(AuthorType::Human),
        identity: Some(String::from("alice")),
        ignore: Vec::new(),
        key_path: Some(PathBuf::from("/keys/ed25519")),
        mode: Mode::Registered,
        registry: Some(registry_with_alice_and_botty()),
        unrestricted: false,
    }
}

fn alice_role_identity() -> MigrateRoleIdentity {
    MigrateRoleIdentity {
        identity: String::from("alice"),
        key_path: Some(PathBuf::from("/keys/ed25519")),
    }
}

fn botty_role_identity() -> MigrateRoleIdentity {
    MigrateRoleIdentity {
        identity: String::from("botty"),
        key_path: Some(PathBuf::from("/keys/ed25519")),
    }
}

/// Assemble a mock filesystem with the doc and (always) the test ssh key
/// — keeping the key on disk in every fixture means individual tests
/// can opt into signing just by passing a `MigrateRoleIdentity` with
/// `key_path: Some(...)`.
fn mock_with_doc(doc: &str) -> MockSystem {
    MockSystem::new()
        .with_file(Path::new("/keys/ed25519"), TEST_PRIVATE_KEY.as_bytes())
        .unwrap()
        .with_file(Path::new("/docs/test.md"), doc.as_bytes())
        .unwrap()
}

fn parse_after_migrate(system: &MockSystem) -> Vec<Comment> {
    let content = system.read_to_string(Path::new("/docs/test.md")).unwrap();
    let parsed = parser::parse(&content).unwrap();
    parsed.comments().into_iter().cloned().collect()
}

// ---- Existing happy paths (kept; re-stated against
// MigrateIdentities::default()) -----------------------------------------

#[test]
fn migrate_user_comment() {
    let doc = "\
# Document

```user comments
This is feedback from the user.
```
";
    let system = mock_with_doc(doc);
    let config = open_config();

    let results = migrate(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &MigrateIdentities::default(),
        false,
    )
    .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].original_role, "user");

    let comments = parse_after_migrate(&system);
    assert_eq!(comments.len(), 1);
    let cm = &comments[0];
    assert_eq!(cm.author, "legacy-user");
    assert_eq!(cm.author_type, AuthorType::Human);
    assert!(cm.signature.is_none());
}

#[test]
fn migrate_agent_with_done_marker() {
    let doc = "\
# Document

```agent comments [done:2026-04-05]
Agent response.
```
";
    let system = mock_with_doc(doc);
    let config = open_config();

    let results = migrate(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &MigrateIdentities::default(),
        false,
    )
    .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].original_role, "agent");

    let comments = parse_after_migrate(&system);
    let cm = &comments[0];
    assert_eq!(cm.author, "legacy-agent");
    assert_eq!(cm.author_type, AuthorType::Agent);
    assert_eq!(cm.ack.len(), 1);
    assert_eq!(cm.ack[0].author, "legacy-user");
}

#[test]
fn no_legacy_comments() {
    let doc = "# Just plain markdown\n";
    let system = mock_with_doc(doc);
    let config = open_config();

    let results = migrate(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &MigrateIdentities::default(),
        false,
    )
    .unwrap();
    assert!(results.is_empty());
}

#[test]
fn backup_created() {
    let doc = "\
```user comments
Content.
```
";
    let system = mock_with_doc(doc);
    let config = open_config();

    migrate(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &MigrateIdentities::default(),
        true,
    )
    .unwrap();

    let backup_exists = system.exists(Path::new("/docs/test.md.bak")).unwrap();
    assert!(backup_exists);
}

// ---- Threading: NO-SPLIT cases (chain links) --------------------------

fn assert_replies_to(child: &Comment, parent: &Comment) {
    assert_eq!(
        child.reply_to.as_deref(),
        Some(parent.id.as_str()),
        "child must reply to parent id"
    );
    let expected_thread = parent.thread.clone().unwrap_or_else(|| parent.id.clone());
    assert_eq!(
        child.thread.as_deref(),
        Some(expected_thread.as_str()),
        "child must inherit (or seed from) parent's thread root",
    );
}

fn assert_root(cm: &Comment) {
    assert!(
        cm.reply_to.is_none(),
        "expected root, but reply_to = {:?}",
        cm.reply_to,
    );
    assert!(
        cm.thread.is_none(),
        "expected root, but thread = {:?}",
        cm.thread,
    );
}

fn migrate_open_default(doc: &str) -> Vec<Comment> {
    let system = mock_with_doc(doc);
    let config = open_config();
    migrate(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &MigrateIdentities::default(),
        false,
    )
    .unwrap();
    parse_after_migrate(&system)
}

#[test]
fn link_user_then_agent_adjacent() {
    let doc = "\
# Topic

```user comments
hi
```
```agent comments
hello
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 2);
    let parent = &comments[0];
    let reply = &comments[1];
    assert_root(parent);
    assert_replies_to(reply, parent);
    // implicit ack from replier appears on the parent
    assert_eq!(parent.ack.len(), 1);
    assert_eq!(parent.ack[0].author, "legacy-agent");
}

#[test]
fn link_agent_then_user_adjacent() {
    let doc = "\
# Topic

```agent comments
hi
```
```user comments
hello
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 2);
    let parent = &comments[0];
    let reply = &comments[1];
    assert_root(parent);
    assert_replies_to(reply, parent);
    assert_eq!(parent.ack.len(), 1);
    assert_eq!(parent.ack[0].author, "legacy-user");
}

#[test]
fn link_through_blank_lines() {
    let doc = "\
# Topic

```user comments
hi
```



```agent comments
hello
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 2);
    assert_replies_to(&comments[1], &comments[0]);
}

#[test]
fn link_through_spaces_and_tabs() {
    // Whitespace-only body between fences (mix of spaces, tabs, blank
    // lines). Whitespace is explicitly NOT prose — the chain should
    // stay alive.
    let doc = "\
# Topic

```user comments
hi
```
   \t
  \t
```agent comments
hello
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 2);
    assert_replies_to(&comments[1], &comments[0]);
}

#[test]
fn link_three_alternating_uau() {
    let doc = "\
# Topic

```user comments
u1
```
```agent comments
a
```
```user comments
u2
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 3);
    let u1 = &comments[0];
    let a = &comments[1];
    let u2 = &comments[2];

    assert_root(u1);
    assert_replies_to(a, u1);
    assert_replies_to(u2, a);
    // u2's thread root must be u1.id, propagated through a.
    assert_eq!(u2.thread.as_deref(), Some(u1.id.as_str()));

    // Acks: u1 acked by agent, a acked by user.
    assert_eq!(u1.ack.len(), 1);
    assert_eq!(u1.ack[0].author, "legacy-agent");
    assert_eq!(a.ack.len(), 1);
    assert_eq!(a.ack[0].author, "legacy-user");
}

#[test]
fn link_four_alternating_uaua() {
    let doc = "\
# Topic

```user comments
u1
```
```agent comments
a1
```
```user comments
u2
```
```agent comments
a2
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 4);
    let u1 = &comments[0];
    let root = u1.id.clone();
    for cm in &comments[1..] {
        assert_eq!(
            cm.thread.as_deref(),
            Some(root.as_str()),
            "every reply in the chain shares u1 as the thread root",
        );
    }
    assert_replies_to(&comments[1], &comments[0]);
    assert_replies_to(&comments[2], &comments[1]);
    assert_replies_to(&comments[3], &comments[2]);
}

#[test]
fn link_resumes_after_heading_split() {
    // U,heading,A,U: the U-A link is broken by the heading, but the
    // A-U pair after the heading is its own valid chain.
    let doc = "\
# Topic

```user comments
u1
```

## Subtopic

```agent comments
a
```
```user comments
u2
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 3);
    let u1 = &comments[0];
    let a = &comments[1];
    let u2 = &comments[2];
    assert_root(u1);
    assert_root(a);
    assert_replies_to(u2, a);
}

// ---- Threading: SPLIT cases (chain breaks) ----------------------------

fn assert_all_roots(comments: &[Comment]) {
    for cm in comments {
        assert_root(cm);
        assert!(cm.ack.is_empty(), "split roots must not have implicit ack");
    }
}

#[test]
fn split_on_heading_h1() {
    let doc = "\
```user comments
u
```

# heading

```agent comments
a
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 2);
    assert_all_roots(&comments);
}

#[test]
fn split_on_heading_h2() {
    let doc = "\
```user comments
u
```

## heading

```agent comments
a
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 2);
    assert_all_roots(&comments);
}

#[test]
fn split_on_heading_h3() {
    let doc = "\
```user comments
u
```

### heading

```agent comments
a
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 2);
    assert_all_roots(&comments);
}

#[test]
fn split_on_heading_h6() {
    let doc = "\
```user comments
u
```

###### heading

```agent comments
a
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 2);
    assert_all_roots(&comments);
}

#[test]
fn split_on_prose_paragraph() {
    let doc = "\
```user comments
u
```

some prose between the comments.

```agent comments
a
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 2);
    assert_all_roots(&comments);
}

#[test]
fn split_on_prose_inline_text() {
    // Single non-whitespace char in the body is enough to break the
    // chain.
    let doc = "\
```user comments
u
```
x
```agent comments
a
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 2);
    assert_all_roots(&comments);
}

#[test]
fn split_on_same_role_uu() {
    let doc = "\
```user comments
u1
```
```user comments
u2
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 2);
    assert_all_roots(&comments);
}

#[test]
fn split_on_same_role_aa() {
    let doc = "\
```agent comments
a1
```
```agent comments
a2
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 2);
    assert_all_roots(&comments);
}

#[test]
fn split_on_same_role_then_link() {
    // U,U,A: the second U is a new root (same-role split). Then
    // U(2)→A is a valid alternation: A replies to the second U.
    let doc = "\
```user comments
u1
```
```user comments
u2
```
```agent comments
a
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 3);
    let u1 = &comments[0];
    let u2 = &comments[1];
    let a = &comments[2];
    assert_root(u1);
    assert!(u1.ack.is_empty(), "u1 has no reply, no implicit ack");
    assert_root(u2);
    assert_replies_to(a, u2);
    assert_eq!(u2.ack.len(), 1);
    assert_eq!(u2.ack[0].author, "legacy-agent");
}

#[test]
fn split_on_intervening_remargin_comment() {
    // A pre-existing Remargin comment between two legacy comments must
    // break the legacy chain — it represents a foreign conversation.
    let content = "\
```user comments
u
```

```remargin
---
id: xyz
author: alice
type: human
ts: 2026-04-06T12:00:00-04:00
checksum: sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
---
hello
```

```agent comments
a
```
";
    let comments = migrate_open_default(content);
    // 3 comments: legacy-user (root), pre-existing alice (untouched
    // root), legacy-agent (root — chain was broken).
    assert_eq!(comments.len(), 3);
    let legacy_u = comments.iter().find(|c| c.author == "legacy-user").unwrap();
    let legacy_a = comments
        .iter()
        .find(|c| c.author == "legacy-agent")
        .unwrap();
    assert_root(legacy_u);
    assert_root(legacy_a);
    assert!(legacy_u.ack.is_empty());
}

// ---- Identity / signing / verify-gate cases ---------------------------

#[test]
fn migrate_open_mode_no_configs_uses_legacy_fallback() {
    let doc = "\
```user comments
u
```
```agent comments
a
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].author, "legacy-user");
    assert_eq!(comments[1].author, "legacy-agent");
    assert!(comments.iter().all(|c| c.signature.is_none()));
}

#[test]
fn migrate_with_human_config_signs_and_attributes() {
    let doc = "\
```user comments
hi from alice
```
";
    let system = mock_with_doc(doc);
    // Registered mode: registry has alice's pubkey, so a present
    // signature can be matched to a known author. Open mode without a
    // registry would mark a signed comment as `signature=invalid` (no
    // pubkey to verify against), tripping the gate.
    let config = registered_config();
    let identities = MigrateIdentities {
        agent: None,
        human: Some(alice_role_identity()),
    };

    migrate(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &identities,
        false,
    )
    .unwrap();

    let comments = parse_after_migrate(&system);
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].author, "alice");
    assert!(
        comments[0].signature.is_some(),
        "human config with key_path must produce a signed comment",
    );
}

#[test]
fn migrate_with_both_configs_signs_both_roles() {
    let doc = "\
```user comments
u
```
```agent comments
a
```
";
    let system = mock_with_doc(doc);
    let config = registered_config();
    let identities = MigrateIdentities {
        agent: Some(botty_role_identity()),
        human: Some(alice_role_identity()),
    };

    migrate(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &identities,
        false,
    )
    .unwrap();

    let comments = parse_after_migrate(&system);
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].author, "alice");
    assert_eq!(comments[1].author, "botty");
    assert!(comments.iter().all(|c| c.signature.is_some()));
    // Threading still works.
    assert_replies_to(&comments[1], &comments[0]);
    assert_eq!(comments[0].ack.len(), 1);
    assert_eq!(comments[0].ack[0].author, "botty");
}

#[test]
fn migrate_strict_mode_succeeds_with_both_configs() {
    let doc = "\
```user comments
u
```
```agent comments
a
```
";
    let system = mock_with_doc(doc);
    let config = strict_config();
    let identities = MigrateIdentities {
        agent: Some(botty_role_identity()),
        human: Some(alice_role_identity()),
    };

    // strict-mode migrate must succeed when both role configs are supplied
    let results = migrate(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &identities,
        false,
    )
    .unwrap();
    assert_eq!(results.len(), 2);

    // Verify: post-write doc must pass the same gate the op enforced.
    let parsed = {
        let raw = system.read_to_string(Path::new("/docs/test.md")).unwrap();
        parser::parse(&raw).unwrap()
    };
    let report = verify_document(&parsed, &config);
    assert!(
        report.ok,
        "post-migrate doc must pass strict verify (rows: {:?})",
        report.results,
    );
}

#[test]
fn migrate_strict_mode_fails_without_configs() {
    let doc = "\
```user comments
u
```
```agent comments
a
```
";
    let system = mock_with_doc(doc);
    let config = strict_config();

    // strict-mode migrate without configs must fail at the verify gate
    let err = migrate(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &MigrateIdentities::default(),
        false,
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("verify failed"),
        "expected verify-gate diagnostic, got: {msg}",
    );
}

#[test]
fn migrate_strict_mode_fails_with_only_human_config() {
    // The human comment is signed under alice, but the agent comment
    // falls back to the unsigned `legacy-agent` placeholder — strict
    // mode rejects on the agent's UnknownAuthor.
    let doc = "\
```user comments
u
```
```agent comments
a
```
";
    let system = mock_with_doc(doc);
    let config = strict_config();
    let identities = MigrateIdentities {
        agent: None,
        human: Some(alice_role_identity()),
    };

    // strict-mode migrate with only human config must fail on the agent comment
    let err = migrate(
        &system,
        Path::new("/docs/test.md"),
        &config,
        &identities,
        false,
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("verify failed"), "got: {msg}");
}

// ---- Misc ------------------------------------------------------------

#[test]
fn migrate_timestamps_are_strictly_monotonic_in_emit_order() {
    let doc = "\
```user comments
u1
```
```agent comments
a1
```

# break

```user comments
u2
```
```agent comments
a2
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 4);
    for window in comments.windows(2) {
        assert!(window.len() >= 2);
        let earlier = &window[0];
        let later = &window[1];
        assert!(
            earlier.ts < later.ts,
            "ts must increase strictly in document order: {} vs {}",
            earlier.ts,
            later.ts,
        );
    }
}

#[test]
fn migrate_done_marker_and_threading_coexist() {
    // Parent has a `[done:DATE]` AND gets an implicit-from-reply ack.
    // Both acks land on the parent — same author (`legacy-agent` since
    // no agent config), different timestamps.
    let doc = "\
```user comments [done:2026-04-05]
u
```
```agent comments
a
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 2);
    let parent = &comments[0];
    assert_eq!(parent.ack.len(), 2, "two acks expected: [done:] + reply");
    assert!(parent.ack.iter().all(|a| a.author == "legacy-agent"));
    assert!(
        parent.ack[0].ts != parent.ack[1].ts,
        "the two acks must have distinct timestamps",
    );
}

#[test]
fn migrate_single_comment_is_root() {
    let doc = "\
# Topic

```user comments
solo
```
";
    let comments = migrate_open_default(doc);
    assert_eq!(comments.len(), 1);
    assert_root(&comments[0]);
    assert!(comments[0].ack.is_empty());
}

// Suppress unused warning for BTreeMap that older test helpers may have
// pulled in; explicit alias keeps the import group clean if we re-add
// helper fixtures later.
const _: fn() = || {
    let _: BTreeMap<String, Vec<String>> = BTreeMap::new();
};
