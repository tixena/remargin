//! Unit tests for [`crate::permissions::claude_sync::rules_for`]
//! (rem-yj1j.4 / rem-wv71).
//!
//! Pure-data round-trips: every test feeds a hand-rolled
//! [`ResolvedRestrict`] in and asserts the returned rule strings.

use std::path::{Path, PathBuf};

use crate::config::permissions::resolve::{ResolvedRestrict, RestrictPath};
use crate::permissions::claude_sync::{ALLOW_MCP_REMARGIN, RuleSet, rules_for};

fn restrict_subpath(path: &str, also_deny_bash: &[&str], cli_allowed: bool) -> ResolvedRestrict {
    ResolvedRestrict {
        also_deny_bash: also_deny_bash.iter().copied().map(String::from).collect(),
        cli_allowed,
        path: RestrictPath::Absolute(PathBuf::from(path)),
        source_file: PathBuf::from("/r/.remargin.yaml"),
    }
}

fn restrict_wildcard(realm: &str, cli_allowed: bool) -> ResolvedRestrict {
    ResolvedRestrict {
        also_deny_bash: Vec::new(),
        cli_allowed,
        path: RestrictPath::Wildcard {
            realm_root: PathBuf::from(realm),
        },
        source_file: PathBuf::from(format!("{realm}/.remargin.yaml")),
    }
}

/// Scenario 1 — subpath, no extras, `cli_allowed = false`.
#[test]
fn subpath_no_extras_emits_full_default_set() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules = rules_for(&entry, Path::new("/a"), &[]);

    // deny: 4 editor-tool path denies + 4 dot-folder wildcards + 6
    // bash mutators + 1 remargin-cli deny = 15 entries.
    assert_eq!(rules.deny.len(), 15, "{:#?}", rules.deny);

    // Editor-tool denies in spec order.
    assert_eq!(rules.deny[0], "Edit(///a/b/**)");
    assert_eq!(rules.deny[1], "Write(///a/b/**)");
    assert_eq!(rules.deny[2], "Read(///a/b/**)");
    assert_eq!(rules.deny[3], "NotebookEdit(///a/b/**)");

    // Dot-folder wildcard denies.
    assert_eq!(rules.deny[4], "Edit(///a/b/.*/**)");
    assert_eq!(rules.deny[5], "Write(///a/b/.*/**)");
    assert_eq!(rules.deny[6], "Read(///a/b/.*/**)");
    assert_eq!(rules.deny[7], "NotebookEdit(///a/b/.*/**)");

    // Bash mutators.
    assert_eq!(rules.deny[8], "Bash(cp * ///a/b/**)");
    assert_eq!(rules.deny[9], "Bash(mv * ///a/b/**)");
    assert_eq!(rules.deny[10], "Bash(tee ///a/b/**)");
    assert_eq!(rules.deny[11], "Bash(sed -i * ///a/b/**)");
    assert_eq!(rules.deny[12], "Bash(truncate * ///a/b/**)");
    assert_eq!(rules.deny[13], "Bash(touch ///a/b/**)");

    // remargin-cli deny because cli_allowed = false.
    assert_eq!(rules.deny[14], "Bash(remargin * ///a/b/**)");

    // Allow set: MCP allow + .remargin re-allow rules (one per editor tool).
    assert_eq!(rules.allow[0], ALLOW_MCP_REMARGIN);
    assert_eq!(rules.allow.len(), 1 + 4); // mcp + .remargin × 4 tools
    assert!(
        rules.allow.iter().any(|r| r == "Edit(///a/b/.remargin/**)"),
        "expected .remargin re-allow in: {:#?}",
        rules.allow
    );
}

/// Scenario 2 — wildcard restrict expands to the realm root glob.
#[test]
fn wildcard_uses_realm_root_for_glob() {
    let entry = restrict_wildcard("/r", false);
    let rules = rules_for(&entry, Path::new("/r"), &[]);

    assert_eq!(rules.deny[0], "Edit(///r/**)");
    assert_eq!(rules.deny[4], "Edit(///r/.*/**)");
    // The realm root is anchored by the entry, not by the supplied
    // anchor (see rules_for's docstring).
    assert!(rules.deny.iter().all(|rule| rule.contains("///r/")));
}

/// Scenario 3 — `cli_allowed = true` removes the remargin-cli deny.
#[test]
fn cli_allowed_skips_remargin_cli_deny() {
    let entry = restrict_subpath("/a/b", &[], true);
    let rules = rules_for(&entry, Path::new("/a"), &[]);

    assert!(
        !rules
            .deny
            .iter()
            .any(|rule| rule.starts_with("Bash(remargin")),
        "cli_allowed=true must omit Bash(remargin ...) deny, got: {:#?}",
        rules.deny
    );
    // 4 editor + 4 dot-folder + 6 bash mutators = 14 (one fewer).
    assert_eq!(rules.deny.len(), 14);
}

/// Scenario 4 — `also_deny_bash` adds extra Bash denies right after
/// the standard mutators.
#[test]
fn also_deny_bash_extras_appended() {
    let entry = restrict_subpath("/a/b", &["curl", "wget"], false);
    let rules = rules_for(&entry, Path::new("/a"), &[]);

    let curl_idx = rules
        .deny
        .iter()
        .position(|r| r == "Bash(curl * ///a/b/**)")
        .unwrap();
    let wget_idx = rules
        .deny
        .iter()
        .position(|r| r == "Bash(wget * ///a/b/**)")
        .unwrap();
    // Extras appear before the cli deny so the surface stays human-
    // readable: standard mutators, callers' extras, then the
    // remargin-cli last-line defense.
    let cli_idx = rules
        .deny
        .iter()
        .position(|r| r.starts_with("Bash(remargin"))
        .unwrap();
    assert!(
        curl_idx < cli_idx && wget_idx < cli_idx,
        "extras should land before the remargin-cli deny: curl={curl_idx}, wget={wget_idx}, cli={cli_idx}"
    );
}

/// Scenario 5 — `allow_dot_folders` re-allows the named folders on top
/// of the wildcard deny. `.remargin/` is always allowed; explicit
/// listing is a no-op (no double allow).
#[test]
fn allow_dot_folders_emits_re_allows() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules = rules_for(&entry, Path::new("/a"), &[String::from(".github")]);

    let github_allows: Vec<&String> = rules
        .allow
        .iter()
        .filter(|rule| rule.contains(".github"))
        .collect();
    assert_eq!(
        github_allows.len(),
        4,
        "expected one .github re-allow per editor tool, got: {github_allows:#?}"
    );
    let remargin_allow_count = rules
        .allow
        .iter()
        .filter(|rule| rule.contains(".remargin"))
        .count();
    assert_eq!(
        remargin_allow_count, 4,
        ".remargin must always be re-allowed exactly once per editor tool"
    );
}

/// `.remargin/` listed explicitly in `allow_dot_folders` does NOT
/// duplicate the always-on re-allow.
#[test]
fn explicit_remargin_in_allow_list_does_not_duplicate() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules = rules_for(&entry, Path::new("/a"), &[String::from(".remargin")]);

    let count = rules
        .allow
        .iter()
        .filter(|rule| rule.contains(".remargin"))
        .count();
    assert_eq!(count, 4, "{:#?}", rules.allow);
}

/// `RuleSet` round-trips through serde so the sidecar (slice 2) can
/// persist it as JSON without losing fidelity.
#[test]
fn rule_set_round_trips_through_json() {
    let original = RuleSet {
        allow: vec![String::from("alpha"), String::from("beta")],
        deny: vec![String::from("gamma")],
    };
    let serialized = serde_json::to_string(&original).unwrap();
    let parsed: RuleSet = serde_json::from_str(&serialized).unwrap();
    assert_eq!(original, parsed);
}

/// Anchor argument is currently unused; document the invariant by
/// pinning that the same entry yields the same `RuleSet` regardless of
/// anchor input. Useful as a regression guard once the anchor starts
/// influencing wildcard re-anchoring.
#[test]
fn anchor_argument_does_not_affect_output() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules_a = rules_for(&entry, Path::new("/a"), &[]);
    let rules_b = rules_for(&entry, Path::new("/somewhere/else"), &[]);
    assert_eq!(rules_a, rules_b);
}
