//! Unit tests for [`crate::permissions::claude_sync::rules_for`]
//! (rem-yj1j.4 / rem-wv71).
//!
//! Pure-data round-trips: every test feeds a hand-rolled
//! [`ResolvedRestrict`] in and asserts the returned rule strings.

use core::slice::from_ref;
use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MockSystem;
use serde_json::{Value, json};

use crate::config::permissions::resolve::{ResolvedRestrict, RestrictPath};
use crate::permissions::claude_sync::rule_shape::{
    OverlapKind, PathGlob, RuleShape, rules_overlap,
};
use crate::permissions::claude_sync::{
    ALLOW_MCP_REMARGIN, RuleSet, apply_rules, revert_rules, rules_for,
};
use crate::permissions::sidecar::{self, sidecar_path};

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

    // Allow set: only the MCP allow (rem-2plr removed the implicit
    // `.remargin/` editor-tool re-allow — `mcp__remargin__*` is the
    // single allow that keeps remargin's runtime working).
    assert_eq!(rules.allow.len(), 1, "{:#?}", rules.allow);
    assert_eq!(rules.allow[0], ALLOW_MCP_REMARGIN);
    assert!(
        !rules.allow.iter().any(|r| r.contains(".remargin")),
        "rem-2plr: no implicit .remargin/ re-allow expected, got: {:#?}",
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
/// of the wildcard deny. `.remargin/` is NOT auto-allowed (rem-2plr).
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
        remargin_allow_count, 0,
        "rem-2plr: .remargin must NOT be auto-allowed unless explicitly listed"
    );
}

/// rem-2plr: `.remargin/` listed explicitly in `allow_dot_folders` IS
/// honoured — the explicit-list path still emits per-tool re-allows.
#[test]
fn explicit_remargin_in_allow_list_emits_re_allows() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules = rules_for(&entry, Path::new("/a"), &[String::from(".remargin")]);

    let count = rules
        .allow
        .iter()
        .filter(|rule| rule.contains(".remargin"))
        .count();
    assert_eq!(count, 4, "{:#?}", rules.allow);
}

/// rem-2plr negative-presence guard: by default, neither settings array
/// (deny/allow) contains the four native-tool `.remargin/**` allows.
#[test]
fn no_implicit_remargin_native_allows_emitted() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules = rules_for(&entry, Path::new("/a"), &[]);

    for tool in ["Edit", "Write", "Read", "NotebookEdit"] {
        let needle = format!("{tool}(///a/b/.remargin/**)");
        assert!(
            !rules.allow.iter().any(|r| r == &needle),
            "rem-2plr: {needle} must not appear in allow, got: {:#?}",
            rules.allow
        );
        assert!(
            !rules.deny.iter().any(|r| r == &needle),
            "rem-2plr: {needle} must not appear in deny either, got: {:#?}",
            rules.deny
        );
    }
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

// ---------------------------------------------------------------------
// apply_rules / revert_rules
// ---------------------------------------------------------------------

fn empty_anchor() -> (MockSystem, PathBuf) {
    let anchor = PathBuf::from("/r");
    let system = MockSystem::new().with_dir(&anchor).unwrap();
    (system, anchor)
}

fn small_rule_set() -> RuleSet {
    RuleSet {
        allow: vec![String::from(ALLOW_MCP_REMARGIN)],
        deny: vec![
            String::from("Edit(///r/secret/**)"),
            String::from("Write(///r/secret/**)"),
        ],
    }
}

fn settings_files(anchor: &Path) -> Vec<PathBuf> {
    vec![
        anchor.join(".claude/settings.local.json"),
        PathBuf::from("/home/u/.claude/settings.json"),
    ]
}

fn read_settings(system: &MockSystem, path: &Path) -> Value {
    let body = system.read_to_string(path).unwrap();
    serde_json::from_str(&body).unwrap()
}

/// Scenario 6: both settings files missing → both created with the
/// rules; sidecar created; gitignore updated.
#[test]
fn apply_creates_missing_settings_files_and_sidecar() {
    let (system, anchor) = empty_anchor();
    let rules = small_rule_set();
    let files = settings_files(&anchor);
    apply_rules(
        &system,
        &anchor,
        "/r/secret",
        &rules,
        &files,
        "2026-04-26T10:00:00Z",
    )
    .unwrap();

    for file in &files {
        let value = read_settings(&system, file);
        let deny = value["permissions"]["deny"].as_array().unwrap();
        assert_eq!(deny.len(), 2, "{file:?} -> {value:#?}");
        let allow = value["permissions"]["allow"].as_array().unwrap();
        assert_eq!(allow.len(), 1);
    }

    let sidecar = sidecar::load(&system, &anchor).unwrap();
    let entry = &sidecar.entries["/r/secret"];
    assert_eq!(entry.deny, rules.deny);
    assert_eq!(entry.allow, rules.allow);
    assert_eq!(entry.added_at, "2026-04-26T10:00:00Z");

    let gitignore = system.read_to_string(&anchor.join(".gitignore")).unwrap();
    assert!(gitignore.contains(".claude/.remargin-restrictions.json"));
}

/// Scenario 7: pre-existing unrelated rules in the deny / allow arrays
/// stay put; new rules append.
#[test]
fn apply_preserves_existing_unrelated_rules() {
    let (system, anchor) = empty_anchor();
    let prior = json!({
        "permissions": {
            "deny": ["Edit(///some/other/path/**)"],
            "allow": ["Bash(ls *)"]
        },
        "env": { "FOO": "bar" }
    });
    let local = anchor.join(".claude/settings.local.json");
    system.create_dir_all(local.parent().unwrap()).unwrap();
    system.write(&local, prior.to_string().as_bytes()).unwrap();

    let rules = small_rule_set();
    apply_rules(
        &system,
        &anchor,
        "/r/secret",
        &rules,
        from_ref(&local),
        "2026-04-26T10:00:00Z",
    )
    .unwrap();

    let value = read_settings(&system, &local);
    let deny = value["permissions"]["deny"].as_array().unwrap();
    assert!(
        deny.iter()
            .any(|v| v.as_str() == Some("Edit(///some/other/path/**)"))
    );
    assert!(
        deny.iter()
            .any(|v| v.as_str() == Some("Edit(///r/secret/**)"))
    );
    assert_eq!(
        value["env"]["FOO"],
        json!("bar"),
        "unrelated keys must be preserved"
    );
}

/// Scenario 8 + 19: re-applying the same entry produces the same
/// state. No duplicates in deny/allow arrays.
#[test]
fn apply_is_idempotent_on_repeat() {
    let (system, anchor) = empty_anchor();
    let rules = small_rule_set();
    let files = settings_files(&anchor);
    apply_rules(
        &system,
        &anchor,
        "/r/secret",
        &rules,
        &files,
        "2026-04-26T10:00:00Z",
    )
    .unwrap();
    let first_local = read_settings(&system, &files[0]);
    apply_rules(
        &system,
        &anchor,
        "/r/secret",
        &rules,
        &files,
        "2026-04-26T11:00:00Z",
    )
    .unwrap();
    let second_local = read_settings(&system, &files[0]);
    assert_eq!(first_local, second_local, "re-apply must not mutate");
}

/// Manually-duplicated rule does not create a third copy on re-apply.
#[test]
fn apply_dedupes_against_manually_duplicated_rules() {
    let (system, anchor) = empty_anchor();
    let local = anchor.join(".claude/settings.local.json");
    system.create_dir_all(local.parent().unwrap()).unwrap();
    let prior = json!({
        "permissions": {
            "deny": [
                "Edit(///r/secret/**)",
                "Edit(///r/secret/**)"
            ],
            "allow": []
        }
    });
    system.write(&local, prior.to_string().as_bytes()).unwrap();

    let rules = small_rule_set();
    apply_rules(
        &system,
        &anchor,
        "/r/secret",
        &rules,
        from_ref(&local),
        "2026-04-26T10:00:00Z",
    )
    .unwrap();

    let value = read_settings(&system, &local);
    let deny = value["permissions"]["deny"].as_array().unwrap();
    let edit_count = deny
        .iter()
        .filter(|v| v.as_str() == Some("Edit(///r/secret/**)"))
        .count();
    // The pre-existing duplicate is preserved (we don't aggressively
    // de-dupe other people's data); apply only adds the missing
    // entries, so the count stays at the pre-existing 2.
    assert_eq!(edit_count, 2, "{value:#?}");
}

/// Scenario 9: applying entries for two different paths leaves both
/// rules in the settings file and both records in the sidecar.
#[test]
fn apply_two_different_entries_keeps_both() {
    let (system, anchor) = empty_anchor();
    let files = settings_files(&anchor);
    let rules_a = RuleSet {
        allow: Vec::new(),
        deny: vec![String::from("Edit(///r/a/**)")],
    };
    let rules_b = RuleSet {
        allow: Vec::new(),
        deny: vec![String::from("Edit(///r/b/**)")],
    };
    apply_rules(&system, &anchor, "/r/a", &rules_a, &files, "now").unwrap();
    apply_rules(&system, &anchor, "/r/b", &rules_b, &files, "now").unwrap();

    let value = read_settings(&system, &files[0]);
    let deny = value["permissions"]["deny"].as_array().unwrap();
    assert!(deny.iter().any(|v| v == "Edit(///r/a/**)"));
    assert!(deny.iter().any(|v| v == "Edit(///r/b/**)"));

    let sidecar = sidecar::load(&system, &anchor).unwrap();
    assert_eq!(sidecar.entries.len(), 2);
}

/// Scenario 10: clean revert restores the settings + sidecar to the
/// pre-apply state.
#[test]
fn revert_after_apply_restores_clean_state() {
    let (system, anchor) = empty_anchor();
    let files = settings_files(&anchor);
    let local = files[0].clone();
    let pre_apply_local = json!({ "env": { "PRESERVE": "true" } });
    system.create_dir_all(local.parent().unwrap()).unwrap();
    system
        .write(&local, pre_apply_local.to_string().as_bytes())
        .unwrap();

    let rules = small_rule_set();
    apply_rules(&system, &anchor, "/r/secret", &rules, &files, "now").unwrap();
    let report = revert_rules(&system, &anchor, "/r/secret").unwrap();
    assert!(report.warnings.is_empty(), "{:#?}", report.warnings);

    let after = read_settings(&system, &local);
    let deny = after["permissions"]["deny"].as_array().unwrap();
    assert!(deny.is_empty(), "{after:#?}");
    let allow = after["permissions"]["allow"].as_array().unwrap();
    assert!(allow.is_empty());
    assert_eq!(after["env"]["PRESERVE"], json!("true"));

    let sidecar = sidecar::load(&system, &anchor).unwrap();
    assert!(sidecar.entries.is_empty());
}

/// Scenario 11: a manually-deleted rule between apply and revert
/// surfaces as a warning but does NOT fail the revert.
#[test]
fn revert_warns_on_manually_deleted_rules() {
    let (system, anchor) = empty_anchor();
    let local = anchor.join(".claude/settings.local.json");
    let rules = small_rule_set();
    apply_rules(
        &system,
        &anchor,
        "/r/secret",
        &rules,
        from_ref(&local),
        "now",
    )
    .unwrap();

    // Hand-edit the settings: drop one of the deny rules.
    let mut value = read_settings(&system, &local);
    let deny = value["permissions"]["deny"].as_array_mut().unwrap();
    deny.retain(|v| v.as_str() != Some("Edit(///r/secret/**)"));
    let body = serde_json::to_string_pretty(&value).unwrap();
    system.write(&local, body.as_bytes()).unwrap();

    let report = revert_rules(&system, &anchor, "/r/secret").unwrap();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("Edit(///r/secret/**)") && w.contains("manually removed")),
        "expected manual-removal warning, got: {:#?}",
        report.warnings
    );
}

/// Scenario 12: revert when the sidecar has no entry for `target_path`
/// returns an empty report (no warnings).
#[test]
fn revert_empty_when_no_sidecar_entry() {
    let (system, anchor) = empty_anchor();
    let report = revert_rules(&system, &anchor, "/r/never-tracked").unwrap();
    assert!(report.warnings.is_empty());
    assert!(report.touched_files.is_empty());
}

/// Scenario 18: settings files with unrelated top-level keys (env,
/// hooks, etc.) preserve those keys verbatim across apply.
#[test]
fn apply_preserves_top_level_keys() {
    let (system, anchor) = empty_anchor();
    let local = anchor.join(".claude/settings.local.json");
    system.create_dir_all(local.parent().unwrap()).unwrap();
    let prior = json!({
        "env": { "DEBUG": "true" },
        "hooks": { "stop": ["echo done"] }
    });
    system.write(&local, prior.to_string().as_bytes()).unwrap();

    apply_rules(
        &system,
        &anchor,
        "/r/secret",
        &small_rule_set(),
        from_ref(&local),
        "now",
    )
    .unwrap();

    let value = read_settings(&system, &local);
    assert_eq!(value["env"]["DEBUG"], json!("true"));
    assert_eq!(value["hooks"]["stop"][0], json!("echo done"));
}

/// Sidecar contains the canonical settings-file paths the apply ran
/// against, so a later revert can reach exactly the same files even
/// when the caller's notion of "user-scope" changes (e.g. HOME moves).
#[test]
fn sidecar_records_resolved_settings_file_paths() {
    let (system, anchor) = empty_anchor();
    let files = settings_files(&anchor);
    apply_rules(
        &system,
        &anchor,
        "/r/secret",
        &small_rule_set(),
        &files,
        "now",
    )
    .unwrap();
    let sidecar = sidecar::load(&system, &anchor).unwrap();
    assert_eq!(sidecar.entries["/r/secret"].added_to_files, files);
    let _path = sidecar_path(&anchor);
}

// ---------------------------------------------------------------------
// rule_shape: PathGlob / RuleShape / overlap (rem-aovx)
// ---------------------------------------------------------------------

/// `PathGlob` #1: canonical recursive glob.
#[test]
fn path_glob_parse_canonical_recursive() {
    let p = PathGlob::parse("/foo/**");
    assert_eq!(p.components, vec![String::from("foo")]);
    assert!(p.recursive);
}

/// `PathGlob` #2: extra leading slashes collapse — the rem-em33 case.
#[test]
fn path_glob_parse_collapses_runs_of_slash() {
    let p = PathGlob::parse("///foo/**");
    assert_eq!(p.components, vec![String::from("foo")]);
    assert!(p.recursive);
}

/// `PathGlob` #3: trailing slash strips, no recursive flag.
#[test]
fn path_glob_parse_trailing_slash_is_not_recursive() {
    let p = PathGlob::parse("/foo/");
    assert_eq!(p.components, vec![String::from("foo")]);
    assert!(!p.recursive);
}

/// `PathGlob` #4: dot-prefixed components are kept verbatim.
#[test]
fn path_glob_parse_keeps_dot_prefixed_components() {
    let p = PathGlob::parse("/foo/.bar/baz");
    assert_eq!(
        p.components,
        vec![
            String::from("foo"),
            String::from(".bar"),
            String::from("baz")
        ]
    );
    assert!(!p.recursive);
}

/// `PathGlob` #5: lexical resolution of `..`.
#[test]
fn path_glob_parse_resolves_parent_dir_lexically() {
    let p = PathGlob::parse("/foo/../bar");
    assert_eq!(p.components, vec![String::from("bar")]);
    assert!(!p.recursive);
}

/// `PathGlob` overlap #6: identical recursive globs overlap (Exact).
#[test]
fn path_glob_overlap_exact_recursive() {
    let a = PathGlob::parse("/foo/**");
    let b = PathGlob::parse("/foo/**");
    assert!(a.overlaps(&b));
    assert_eq!(a.classify_overlap(&b), Some(OverlapKind::Exact));
}

/// `PathGlob` overlap #7: prefix recursive shadows the longer path.
#[test]
fn path_glob_overlap_prefix_recursive() {
    let broad = PathGlob::parse("/foo/**");
    let specific = PathGlob::parse("/foo/sub");
    assert!(broad.overlaps(&specific));
    assert!(specific.overlaps(&broad));
    assert_eq!(
        broad.classify_overlap(&specific),
        Some(OverlapKind::DenyShadowedByBroaderAllow)
    );
    assert_eq!(
        specific.classify_overlap(&broad),
        Some(OverlapKind::AllowShadowedByBroaderDeny)
    );
}

/// `PathGlob` overlap #8: same-prefix neither recursive — only equal
/// paths overlap. `/foo` vs `/foo/sub` (both non-recursive) → no
/// overlap.
#[test]
fn path_glob_overlap_neither_recursive_disjoint_lengths() {
    let a = PathGlob::parse("/foo");
    let b = PathGlob::parse("/foo/sub");
    assert!(!a.overlaps(&b));
    assert!(!b.overlaps(&a));
    assert_eq!(a.classify_overlap(&b), None);
}

/// `PathGlob` overlap #9: disjoint paths never overlap.
#[test]
fn path_glob_overlap_disjoint() {
    let a = PathGlob::parse("/foo");
    let b = PathGlob::parse("/bar");
    assert!(!a.overlaps(&b));
    assert_eq!(a.classify_overlap(&b), None);
}

/// `PathGlob` overlap #10: component-confusion guard — `/foo` does NOT
/// overlap `/foobar`.
#[test]
fn path_glob_overlap_component_confusion_rejected() {
    let a = PathGlob::parse("/foo/**");
    let b = PathGlob::parse("/foobar/**");
    assert!(!a.overlaps(&b));
    assert_eq!(a.classify_overlap(&b), None);
}

/// `RuleShape` #11: canonical Read.
#[test]
fn rule_shape_parse_read_tool() {
    let shape = RuleShape::parse("Read(/foo/**)");
    let expected = RuleShape::Tool {
        path_glob: PathGlob {
            components: vec![String::from("foo")],
            recursive: true,
        },
        tool: String::from("Read"),
    };
    assert_eq!(shape, expected);
}

/// `RuleShape` #12: Bash with cmd tokens preserved verbatim.
#[test]
fn rule_shape_parse_bash_with_cmd_tokens() {
    let shape = RuleShape::parse("Bash(curl * /foo/**)");
    let expected = RuleShape::Bash {
        cmd_tokens: vec![String::from("curl"), String::from("*")],
        path_glob: PathGlob {
            components: vec![String::from("foo")],
            recursive: true,
        },
    };
    assert_eq!(shape, expected);
}

/// `RuleShape` #13: `mcp__remargin__*` is opaque (no parens).
#[test]
fn rule_shape_parse_mcp_remargin_is_opaque() {
    let shape = RuleShape::parse("mcp__remargin__*");
    assert!(matches!(shape, RuleShape::Opaque(_)));
}

/// `RuleShape` #14: `WebFetch(domain:…)` is opaque (not a path body).
#[test]
fn rule_shape_parse_webfetch_is_opaque() {
    // `WebFetch` is not a known editor tool; the parser falls through
    // to Opaque rather than misinterpreting the domain literal as a
    // path glob.
    let shape = RuleShape::parse("WebFetch(domain:github.com)");
    assert!(matches!(shape, RuleShape::Opaque(_)));
}

/// `RuleShape` #15: cross-tool no overlap — `Read(/foo)` allow vs
/// `Edit(/foo)` deny does not fire.
#[test]
fn rules_overlap_cross_tool_returns_none() {
    let allow = RuleShape::parse("Read(/foo)");
    let deny = RuleShape::parse("Edit(/foo)");
    assert_eq!(rules_overlap(&allow, &deny), None);
}

/// Format-drift tolerance: legacy `///` deny vs single-slash allow
/// canonicalize to the same path-glob and overlap (Exact).
#[test]
fn rules_overlap_handles_legacy_triple_slash_prefix() {
    let allow = RuleShape::parse("Read(/foo/**)");
    let deny = RuleShape::parse("Read(///foo/**)");
    assert_eq!(rules_overlap(&allow, &deny), Some(OverlapKind::Exact));
}

/// Whitespace tolerance inside the rule body.
#[test]
fn rules_overlap_handles_internal_whitespace() {
    let allow = RuleShape::parse("Read( /foo/** )");
    let deny = RuleShape::parse("Read(/foo/**)");
    assert_eq!(rules_overlap(&allow, &deny), Some(OverlapKind::Exact));
}

/// Bash overlap: identical cmd tokens + overlapping path glob fires.
#[test]
fn rules_overlap_bash_identical_cmd_tokens_overlap() {
    let allow = RuleShape::parse("Bash(curl * /foo/**)");
    let deny = RuleShape::parse("Bash(curl * /foo/sub/**)");
    assert_eq!(
        rules_overlap(&allow, &deny),
        Some(OverlapKind::DenyShadowedByBroaderAllow)
    );
}

/// Bash overlap: different cmd tokens never overlap, even with
/// matching path glob.
#[test]
fn rules_overlap_bash_different_cmd_tokens_no_overlap() {
    let allow = RuleShape::parse("Bash(cp * /foo/**)");
    let deny = RuleShape::parse("Bash(mv * /foo/**)");
    assert_eq!(rules_overlap(&allow, &deny), None);
}
