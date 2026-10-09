//! Unit tests for [`crate::permissions::claude_sync::hook_covered_rules`].
//!
//! Pure-data round-trips: every test feeds a hand-rolled
//! [`ResolvedTrustedRoot`] in and asserts the returned rule strings.
//!
//! `Bash(remargin *)` is not part of the set: CLI denial is enforced by the `PreToolUse` hook
//! through the folder-level `cli_allowed` field in `.remargin.yaml`.

use core::slice::from_ref;
use std::path::{Path, PathBuf};

use os_shim::System as _;
use os_shim::mock::MemorySystem;
use serde_json::{Value, json};

use crate::config::permissions::resolve::{ResolvedTrustedRoot, TrustedRootPath};
use crate::permissions::claude_sync::rule_shape::{
    OverlapKind, PathGlob, RuleShape, rules_overlap,
};
use crate::permissions::claude_sync::{
    BASH_MUTATORS, RuleSet, apply_rules, hook_covered_rules, revert_rules,
};
use crate::permissions::sidecar::{self, sidecar_path};

fn restrict_subpath(path: &str, also_deny_bash: &[&str], cli_allowed: bool) -> ResolvedTrustedRoot {
    ResolvedTrustedRoot {
        also_deny_bash: also_deny_bash.iter().copied().map(String::from).collect(),
        cli_allowed,
        path: TrustedRootPath::Absolute(PathBuf::from(path)),
        source_file: PathBuf::from("/r/.remargin.yaml"),
    }
}

fn restrict_wildcard(realm: &str, cli_allowed: bool) -> ResolvedTrustedRoot {
    ResolvedTrustedRoot {
        also_deny_bash: Vec::new(),
        cli_allowed,
        path: TrustedRootPath::Wildcard {
            realm_root: PathBuf::from(realm),
        },
        source_file: PathBuf::from(format!("{realm}/.remargin.yaml")),
    }
}

/// `Bash(remargin *)` is not emitted: CLI denial is hook-enforced.
#[test]
fn subpath_no_extras_emits_full_default_set() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules = hook_covered_rules(&entry, Path::new("/a"), &[]);

    let expected = 5 + 5 + BASH_MUTATORS.len() + 3 + 3;
    assert_eq!(rules.deny.len(), expected, "{:#?}", rules.deny);

    assert_eq!(rules.deny[0], "Edit(/a/b/**)");
    assert_eq!(rules.deny[1], "Write(/a/b/**)");
    assert_eq!(rules.deny[2], "Read(/a/b/**)");
    assert_eq!(rules.deny[3], "NotebookEdit(/a/b/**)");
    assert_eq!(rules.deny[4], "MultiEdit(/a/b/**)");

    assert_eq!(rules.deny[5], "Edit(/a/b/.*/**)");
    assert_eq!(rules.deny[6], "Write(/a/b/.*/**)");
    assert_eq!(rules.deny[7], "Read(/a/b/.*/**)");
    assert_eq!(rules.deny[8], "NotebookEdit(/a/b/.*/**)");
    assert_eq!(rules.deny[9], "MultiEdit(/a/b/.*/**)");

    assert_eq!(rules.deny[10], "Bash(cp * /a/b/**)");
    assert_eq!(rules.deny[11], "Bash(mv * /a/b/**)");
    assert_eq!(rules.deny[12], "Bash(tee /a/b/**)");

    let must_contain = [
        "Bash(sed * /a/b/**)",
        "Bash(rm /a/b/**)",
        "Bash(rm * /a/b/**)",
        "Bash(rmdir /a/b/**)",
        "Bash(rmdir * /a/b/**)",
        "Bash(unlink /a/b/**)",
        "Bash(unlink * /a/b/**)",
        "Bash(shred /a/b/**)",
        "Bash(shred * /a/b/**)",
        "Bash(mkdir * /a/b/**)",
        "Bash(ln * /a/b/**)",
        "Bash(install * /a/b/**)",
        "Bash(chmod * /a/b/**)",
        "Bash(chown * /a/b/**)",
        "Bash(setfacl * /a/b/**)",
        "Bash(vim * /a/b/**)",
        "Bash(nvim * /a/b/**)",
        "Bash(nano * /a/b/**)",
        "Bash(awk * /a/b/**)",
        "Bash(perl * /a/b/**)",
        "Bash(python * /a/b/**)",
        "Bash(ruby * /a/b/**)",
        "Bash(node * /a/b/**)",
        "Bash(tar * /a/b/**)",
        "Bash(zip * /a/b/**)",
        "Bash(gzip * /a/b/**)",
        "Bash(7z * /a/b/**)",
        "Bash(rsync * /a/b/**)",
        "Bash(scp * /a/b/**)",
        "Bash(curl * /a/b/**)",
        "Bash(wget * /a/b/**)",
        "Bash(xargs * /a/b/**)",
        "Bash(find * /a/b/**)",
        "Bash(bash * /a/b/**)",
        "Bash(sh * /a/b/**)",
        "Bash(git * /a/b/**)",
        "Bash(make * /a/b/**)",
        "Bash(dd * /a/b/**)",
        "Bash(cd /a/b/**)",
        "Bash(cd * /a/b/**)",
        "Bash(pushd /a/b/**)",
        "Bash(pushd * /a/b/**)",
        "Bash(attrib /a/b/**)",
        "Bash(attrib * /a/b/**)",
        "Bash(copy /a/b/**)",
        "Bash(copy * /a/b/**)",
        "Bash(del /a/b/**)",
        "Bash(del * /a/b/**)",
        "Bash(erase /a/b/**)",
        "Bash(erase * /a/b/**)",
        "Bash(fc * /a/b/**)",
        "Bash(move /a/b/**)",
        "Bash(move * /a/b/**)",
        "Bash(rd /a/b/**)",
        "Bash(rd * /a/b/**)",
        "Bash(ren /a/b/**)",
        "Bash(ren * /a/b/**)",
        "Bash(rename /a/b/**)",
        "Bash(rename * /a/b/**)",
        "Bash(robocopy * /a/b/**)",
        "Bash(type * /a/b/**)",
        "Bash(xcopy * /a/b/**)",
        "Bash(Add-Content /a/b/**)",
        "Bash(Add-Content * /a/b/**)",
        "Bash(Clear-Content /a/b/**)",
        "Bash(Clear-Content * /a/b/**)",
        "Bash(Copy-Item /a/b/**)",
        "Bash(Copy-Item * /a/b/**)",
        "Bash(Move-Item /a/b/**)",
        "Bash(Move-Item * /a/b/**)",
        "Bash(New-Item /a/b/**)",
        "Bash(New-Item * /a/b/**)",
        "Bash(Out-File /a/b/**)",
        "Bash(Out-File * /a/b/**)",
        "Bash(Remove-Item /a/b/**)",
        "Bash(Remove-Item * /a/b/**)",
        "Bash(Rename-Item /a/b/**)",
        "Bash(Rename-Item * /a/b/**)",
        "Bash(Set-Content /a/b/**)",
        "Bash(Set-Content * /a/b/**)",
        "Bash(mv /a/b/**)",
        "Bash(mv /a/b/** *)",
        "Bash(mv /a/b/** /a/b/**)",
        "Bash(cp /a/b/**)",
        "Bash(cp /a/b/** *)",
        "Bash(cp /a/b/** /a/b/**)",
    ];
    for needle in must_contain {
        assert!(
            rules.deny.iter().any(|rule| rule == needle),
            "default deny list missing {needle:?}\nfull deny: {:#?}",
            rules.deny
        );
    }

    assert!(
        !rules.deny.iter().any(|r| r.starts_with("Bash(remargin")),
        "Bash(remargin *) must not appear in projection: {:#?}",
        rules.deny
    );

    assert!(rules.allow.is_empty(), "{:#?}", rules.allow);
    assert!(
        !rules.allow.iter().any(|r| r.contains(".remargin")),
        "no implicit .remargin/ re-allow expected, got: {:#?}",
        rules.allow
    );
    assert!(
        !rules.allow.iter().any(|r| r.contains("mcp__remargin__")),
        "no implicit mcp__remargin__* allow expected, got: {:#?}",
        rules.allow
    );
}

#[test]
fn wildcard_uses_realm_root_for_glob() {
    let entry = restrict_wildcard("/r", false);
    let rules = hook_covered_rules(&entry, Path::new("/r"), &[]);

    assert_eq!(rules.deny[0], "Edit(/r/**)");
    assert_eq!(rules.deny[5], "Edit(/r/.*/**)");
    assert!(
        rules.deny.iter().all(|rule| rule.contains("/r/")),
        "every projected rule should be path-anchored with /r/: {:#?}",
        rules.deny
    );
}

#[test]
fn projection_never_emits_remargin_cli_deny_regardless_of_cli_allowed() {
    for cli_allowed in [true, false] {
        let entry = restrict_subpath("/a/b", &[], cli_allowed);
        let rules = hook_covered_rules(&entry, Path::new("/a"), &[]);

        assert!(
            !rules
                .deny
                .iter()
                .any(|rule| rule.starts_with("Bash(remargin")),
            "cli_allowed={cli_allowed}: Bash(remargin *) must never be projected, got: {:#?}",
            rules.deny
        );
        let expected = 5 + 5 + BASH_MUTATORS.len() + 3 + 3;
        assert_eq!(rules.deny.len(), expected, "cli_allowed={cli_allowed}");
    }
}

#[test]
fn no_remargin_cli_deny_emitted_in_any_configuration() {
    for cli_allowed in [true, false] {
        for also_deny_bash in [&[] as &[&str], &["curl", "nc"]] {
            let entry = restrict_subpath("/a/b", also_deny_bash, cli_allowed);
            let rules = hook_covered_rules(&entry, Path::new("/a"), &[]);
            assert!(
                !rules.deny.iter().any(|r| r.starts_with("Bash(remargin")),
                "cli_allowed={cli_allowed}, also_deny_bash={also_deny_bash:?}: \
                 Bash(remargin *) must never appear in the projected deny set: {:#?}",
                rules.deny
            );
        }
    }
    for cli_allowed in [true, false] {
        let entry = restrict_wildcard("/r", cli_allowed);
        let rules = hook_covered_rules(&entry, Path::new("/r"), &[]);
        assert!(
            !rules.deny.iter().any(|r| r.starts_with("Bash(remargin")),
            "wildcard cli_allowed={cli_allowed}: Bash(remargin *) must not appear: {:#?}",
            rules.deny
        );
    }
}

/// Uses commands outside `BASH_MUTATORS`, so the extras path is what emits them.
#[test]
fn also_deny_bash_extras_appended() {
    let entry = restrict_subpath("/a/b", &["aria2c", "nc"], false);
    let rules = hook_covered_rules(&entry, Path::new("/a"), &[]);

    assert!(
        rules.deny.iter().any(|r| r == "Bash(aria2c * /a/b/**)"),
        "aria2c extra deny missing from: {:#?}",
        rules.deny
    );
    assert!(
        rules.deny.iter().any(|r| r == "Bash(nc * /a/b/**)"),
        "nc extra deny missing from: {:#?}",
        rules.deny
    );
    assert!(
        !rules.deny.iter().any(|r| r.starts_with("Bash(remargin")),
        "Bash(remargin *) must not appear in projection: {:#?}",
        rules.deny
    );
}

/// `.remargin/` is not auto-allowed.
#[test]
fn allow_dot_folders_emits_re_allows() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules = hook_covered_rules(&entry, Path::new("/a"), &[String::from(".github")]);

    let github_allows: Vec<&String> = rules
        .allow
        .iter()
        .filter(|rule| rule.contains(".github"))
        .collect();
    assert_eq!(
        github_allows.len(),
        5,
        "expected one .github re-allow per editor tool, got: {github_allows:#?}"
    );
    let remargin_allow_count = rules
        .allow
        .iter()
        .filter(|rule| rule.contains(".remargin"))
        .count();
    assert_eq!(
        remargin_allow_count, 0,
        ".remargin must NOT be auto-allowed unless explicitly listed"
    );
}

#[test]
fn explicit_remargin_in_allow_list_emits_re_allows() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules = hook_covered_rules(&entry, Path::new("/a"), &[String::from(".remargin")]);

    let count = rules
        .allow
        .iter()
        .filter(|rule| rule.contains(".remargin"))
        .count();
    assert_eq!(count, 5, "{:#?}", rules.allow);
}

/// `rm <path>` with no flag token is denied alongside `rm -rf <path>`.
#[test]
fn deletion_family_emits_bare_and_flagged_forms() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules = hook_covered_rules(&entry, Path::new("/a"), &[]);

    for cmd in ["rm", "rmdir", "unlink", "shred"] {
        let bare = format!("Bash({cmd} /a/b/**)");
        let with_flags = format!("Bash({cmd} * /a/b/**)");
        assert!(
            rules.deny.iter().any(|rule| rule == &bare),
            "missing bare deletion rule {bare:?} in {:#?}",
            rules.deny
        );
        assert!(
            rules.deny.iter().any(|rule| rule == &with_flags),
            "missing flagged deletion rule {with_flags:?} in {:#?}",
            rules.deny
        );
    }
}

#[test]
fn windows_cmd_mutators_projected() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules = hook_covered_rules(&entry, Path::new("/a"), &[]);

    let bare_or_flagged = [
        "attrib", "del", "erase", "move", "rd", "ren", "rename", "copy",
    ];
    for cmd in bare_or_flagged {
        let bare = format!("Bash({cmd} /a/b/**)");
        let flagged = format!("Bash({cmd} * /a/b/**)");
        assert!(
            rules.deny.iter().any(|rule| rule == &bare),
            "missing Windows bare rule {bare:?}"
        );
        assert!(
            rules.deny.iter().any(|rule| rule == &flagged),
            "missing Windows flagged rule {flagged:?}"
        );
    }

    let flagged_only = ["fc", "robocopy", "type", "xcopy"];
    for cmd in flagged_only {
        let flagged = format!("Bash({cmd} * /a/b/**)");
        assert!(
            rules.deny.iter().any(|rule| rule == &flagged),
            "missing Windows flagged-only rule {flagged:?}"
        );
    }
}

#[test]
fn powershell_cmdlet_mutators_projected() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules = hook_covered_rules(&entry, Path::new("/a"), &[]);

    let cmdlets = [
        "Add-Content",
        "Clear-Content",
        "Copy-Item",
        "Move-Item",
        "New-Item",
        "Out-File",
        "Remove-Item",
        "Rename-Item",
        "Set-Content",
    ];
    for cmd in cmdlets {
        let bare = format!("Bash({cmd} /a/b/**)");
        let flagged = format!("Bash({cmd} * /a/b/**)");
        assert!(
            rules.deny.iter().any(|rule| rule == &bare),
            "missing PowerShell bare rule {bare:?}"
        );
        assert!(
            rules.deny.iter().any(|rule| rule == &flagged),
            "missing PowerShell flagged rule {flagged:?}"
        );
    }
}

#[test]
fn xargs_and_find_projected() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules = hook_covered_rules(&entry, Path::new("/a"), &[]);

    assert!(
        rules
            .deny
            .iter()
            .any(|rule| rule == "Bash(xargs * /a/b/**)"),
        "xargs deny missing"
    );
    assert!(
        rules.deny.iter().any(|rule| rule == "Bash(find * /a/b/**)"),
        "find deny missing"
    );
}

#[test]
fn no_implicit_remargin_native_allows_emitted() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules = hook_covered_rules(&entry, Path::new("/a"), &[]);

    for tool in ["Edit", "Write", "Read", "NotebookEdit", "MultiEdit"] {
        let needle = format!("{tool}(/a/b/.remargin/**)");
        assert!(
            !rules.allow.iter().any(|r| r == &needle),
            "{needle} must not appear in allow, got: {:#?}",
            rules.allow
        );
    }
    assert!(
        !rules.allow.iter().any(|r| r.contains("mcp__remargin__")),
        "no implicit mcp__remargin__* allow expected, got: {:#?}",
        rules.allow
    );
}

/// The sidecar persists a `RuleSet` as JSON.
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

/// The anchor argument is unused: the same entry yields the same `RuleSet` for any anchor.
#[test]
fn anchor_argument_does_not_affect_output() {
    let entry = restrict_subpath("/a/b", &[], false);
    let rules_a = hook_covered_rules(&entry, Path::new("/a"), &[]);
    let rules_b = hook_covered_rules(&entry, Path::new("/somewhere/else"), &[]);
    assert_eq!(rules_a, rules_b);
}

fn empty_anchor() -> (MemorySystem, PathBuf) {
    let anchor = PathBuf::from("/r");
    let system = MemorySystem::new().with_dir(&anchor).unwrap();
    (system, anchor)
}

fn small_rule_set() -> RuleSet {
    RuleSet {
        allow: Vec::new(),
        deny: vec![
            String::from("Edit(/r/secret/**)"),
            String::from("Write(/r/secret/**)"),
        ],
    }
}

fn settings_files(anchor: &Path) -> Vec<PathBuf> {
    vec![
        anchor.join(".claude/settings.local.json"),
        PathBuf::from("/home/u/.claude/settings.json"),
    ]
}

fn read_settings(system: &MemorySystem, path: &Path) -> Value {
    let body = system.read_to_string(path).unwrap();
    serde_json::from_str(&body).unwrap()
}

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
        assert!(allow.is_empty(), "{file:?} -> {value:#?}");
    }

    let sidecar = sidecar::load(&system, &anchor).unwrap();
    let entry = &sidecar.entries["/r/secret"];
    assert_eq!(entry.deny, rules.deny);
    assert_eq!(entry.allow, rules.allow);
    assert_eq!(entry.added_at, "2026-04-26T10:00:00Z");

    let gitignore = system.read_to_string(&anchor.join(".gitignore")).unwrap();
    assert!(gitignore.contains(".claude/.remargin-restrictions.json"));
}

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
            .any(|v| v.as_str() == Some("Edit(/r/secret/**)"))
    );
    assert_eq!(
        value["env"]["FOO"],
        json!("bar"),
        "unrelated keys must be preserved"
    );
}

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
                "Edit(/r/secret/**)",
                "Edit(/r/secret/**)"
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
        .filter(|v| v.as_str() == Some("Edit(/r/secret/**)"))
        .count();
    assert_eq!(edit_count, 2, "{value:#?}");
}

#[test]
fn apply_two_different_entries_keeps_both() {
    let (system, anchor) = empty_anchor();
    let files = settings_files(&anchor);
    let rules_a = RuleSet {
        allow: Vec::new(),
        deny: vec![String::from("Edit(/r/a/**)")],
    };
    let rules_b = RuleSet {
        allow: Vec::new(),
        deny: vec![String::from("Edit(/r/b/**)")],
    };
    apply_rules(&system, &anchor, "/r/a", &rules_a, &files, "now").unwrap();
    apply_rules(&system, &anchor, "/r/b", &rules_b, &files, "now").unwrap();

    let value = read_settings(&system, &files[0]);
    let deny = value["permissions"]["deny"].as_array().unwrap();
    assert!(deny.iter().any(|v| v == "Edit(/r/a/**)"));
    assert!(deny.iter().any(|v| v == "Edit(/r/b/**)"));

    let sidecar = sidecar::load(&system, &anchor).unwrap();
    assert_eq!(sidecar.entries.len(), 2);
}

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
    assert_eq!(allow.as_slice(), [] as [Value; 0]);
    assert_eq!(after["env"]["PRESERVE"], json!("true"));

    let sidecar = sidecar::load(&system, &anchor).unwrap();
    assert!(sidecar.entries.is_empty());
}

/// A rule deleted by hand between apply and revert is a warning, not a failure.
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

    let mut value = read_settings(&system, &local);
    let deny = value["permissions"]["deny"].as_array_mut().unwrap();
    deny.retain(|v| v.as_str() != Some("Edit(/r/secret/**)"));
    let body = serde_json::to_string_pretty(&value).unwrap();
    system.write(&local, body.as_bytes()).unwrap();

    let report = revert_rules(&system, &anchor, "/r/secret").unwrap();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("Edit(/r/secret/**)") && w.contains("manually removed")),
        "expected manual-removal warning, got: {:#?}",
        report.warnings
    );
}

#[test]
fn revert_empty_when_no_sidecar_entry() {
    let (system, anchor) = empty_anchor();
    let report = revert_rules(&system, &anchor, "/r/never-tracked").unwrap();
    assert_eq!(report.warnings, [] as [String; 0]);
    assert_eq!(report.touched_files, [] as [PathBuf; 0]);
}

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

/// A later revert reaches the same files even when the caller's user scope has moved.
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

#[test]
fn canonicalize_rule_collapses_triple_slash() {
    use crate::permissions::claude_sync::canonicalize_rule;
    assert_eq!(canonicalize_rule("Read(///foo/**)"), "Read(/foo/**)");
}

#[test]
fn canonicalize_rule_collapses_double_slash() {
    use crate::permissions::claude_sync::canonicalize_rule;
    assert_eq!(canonicalize_rule("Read(//foo/**)"), "Read(/foo/**)");
}

#[test]
fn canonicalize_rule_is_noop_on_canonical_form() {
    use crate::permissions::claude_sync::canonicalize_rule;
    assert_eq!(canonicalize_rule("Read(/foo/**)"), "Read(/foo/**)");
}

#[test]
fn simulate_apply_rules_membership_collapses_legacy_double_slash() {
    use crate::permissions::claude_sync::simulate_apply_rules;
    let (system, anchor) = empty_anchor();
    let local = anchor.join(".claude/settings.local.json");
    system.create_dir_all(local.parent().unwrap()).unwrap();
    let prior = json!({
        "permissions": {
            "deny": ["Edit(///r/secret/**)", "Write(//r/secret/**)"],
            "allow": []
        }
    });
    system.write(&local, prior.to_string().as_bytes()).unwrap();

    let rules = small_rule_set();
    let sims = simulate_apply_rules(&system, from_ref(&local), &rules).unwrap();
    let sim = &sims[0];
    assert!(
        sim.deny_rules_to_add.is_empty(),
        "legacy double/triple-slash should collapse to already-present: to_add={:?}",
        sim.deny_rules_to_add
    );
    assert_eq!(sim.deny_rules_already_present.len(), 2);
}

#[test]
fn apply_rules_does_not_duplicate_legacy_double_slash_rules() {
    let (system, anchor) = empty_anchor();
    let local = anchor.join(".claude/settings.local.json");
    system.create_dir_all(local.parent().unwrap()).unwrap();
    let prior = json!({
        "permissions": {
            "deny": ["Edit(//r/secret/**)"],
            "allow": []
        }
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
    let deny = value["permissions"]["deny"].as_array().unwrap();
    let edit_rules: Vec<&str> = deny
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|s| s.contains("Edit(") && s.contains("r/secret"))
        .collect();
    assert_eq!(
        edit_rules.len(),
        1,
        "legacy double-slash + canonical projected rule must not duplicate: {edit_rules:?}",
    );
    assert_eq!(edit_rules[0], "Edit(//r/secret/**)");
}

#[test]
fn revert_rules_strips_legacy_double_slash_rule() {
    let (system, anchor) = empty_anchor();
    let local = anchor.join(".claude/settings.local.json");
    system.create_dir_all(local.parent().unwrap()).unwrap();
    let prior = json!({
        "permissions": {
            "deny": ["Edit(//r/secret/**)", "Write(//r/secret/**)"],
            "allow": []
        }
    });
    system.write(&local, prior.to_string().as_bytes()).unwrap();

    let rules = small_rule_set();
    let entry = sidecar::SidecarEntry {
        added_at: String::from("now"),
        added_to_files: vec![local.clone()],
        allow: rules.allow.clone(),
        deny: rules.deny,
    };
    sidecar::add_entry(&system, &anchor, "/r/secret", entry).unwrap();

    let report = revert_rules(&system, &anchor, "/r/secret").unwrap();
    assert!(report.warnings.is_empty(), "{:#?}", report.warnings);

    let value = read_settings(&system, &local);
    let deny = value["permissions"]["deny"].as_array().unwrap();
    assert!(
        !deny.iter().any(|v| {
            v.as_str()
                .is_some_and(|s| s.contains("Edit(") || s.contains("Write("))
        }),
        "legacy rules should be scrubbed: {deny:?}"
    );
}

#[test]
fn path_glob_parse_canonical_recursive() {
    let p = PathGlob::parse("/foo/**");
    assert_eq!(p.components, vec![String::from("foo")]);
    assert!(p.recursive);
}

#[test]
fn path_glob_parse_collapses_runs_of_slash() {
    let p = PathGlob::parse("///foo/**");
    assert_eq!(p.components, vec![String::from("foo")]);
    assert!(p.recursive);
}

#[test]
fn path_glob_parse_trailing_slash_is_not_recursive() {
    let p = PathGlob::parse("/foo/");
    assert_eq!(p.components, vec![String::from("foo")]);
    assert!(!p.recursive);
}

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

#[test]
fn path_glob_parse_resolves_parent_dir_lexically() {
    let p = PathGlob::parse("/foo/../bar");
    assert_eq!(p.components, vec![String::from("bar")]);
    assert!(!p.recursive);
}

#[test]
fn path_glob_overlap_exact_recursive() {
    let a = PathGlob::parse("/foo/**");
    let b = PathGlob::parse("/foo/**");
    assert!(a.overlaps(&b));
    assert_eq!(a.classify_overlap(&b), Some(OverlapKind::Exact));
}

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

/// `/foo` and `/foo/sub`, neither recursive, do not overlap.
#[test]
fn path_glob_overlap_neither_recursive_disjoint_lengths() {
    let a = PathGlob::parse("/foo");
    let b = PathGlob::parse("/foo/sub");
    assert!(!a.overlaps(&b));
    assert!(!b.overlaps(&a));
    assert_eq!(a.classify_overlap(&b), None);
}

#[test]
fn path_glob_overlap_disjoint() {
    let a = PathGlob::parse("/foo");
    let b = PathGlob::parse("/bar");
    assert!(!a.overlaps(&b));
    assert_eq!(a.classify_overlap(&b), None);
}

#[test]
fn path_glob_overlap_component_confusion_rejected() {
    let a = PathGlob::parse("/foo/**");
    let b = PathGlob::parse("/foobar/**");
    assert!(!a.overlaps(&b));
    assert_eq!(a.classify_overlap(&b), None);
}

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

/// No parentheses, so there is no path body to parse.
#[test]
fn rule_shape_parse_mcp_remargin_is_opaque() {
    let shape = RuleShape::parse("mcp__remargin__*");
    assert!(matches!(shape, RuleShape::Opaque(_)));
}

#[test]
fn rule_shape_parse_webfetch_is_opaque() {
    let shape = RuleShape::parse("WebFetch(domain:github.com)");
    assert!(matches!(shape, RuleShape::Opaque(_)));
}

#[test]
fn rules_overlap_cross_tool_returns_none() {
    let allow = RuleShape::parse("Read(/foo)");
    let deny = RuleShape::parse("Edit(/foo)");
    assert_eq!(rules_overlap(&allow, &deny), None);
}

/// A `///` deny and a single-slash allow canonicalize to the same path glob.
#[test]
fn rules_overlap_handles_legacy_triple_slash_prefix() {
    let allow = RuleShape::parse("Read(/foo/**)");
    let deny = RuleShape::parse("Read(///foo/**)");
    assert_eq!(rules_overlap(&allow, &deny), Some(OverlapKind::Exact));
}

#[test]
fn rules_overlap_handles_internal_whitespace() {
    let allow = RuleShape::parse("Read( /foo/** )");
    let deny = RuleShape::parse("Read(/foo/**)");
    assert_eq!(rules_overlap(&allow, &deny), Some(OverlapKind::Exact));
}

#[test]
fn rules_overlap_bash_identical_cmd_tokens_overlap() {
    let allow = RuleShape::parse("Bash(curl * /foo/**)");
    let deny = RuleShape::parse("Bash(curl * /foo/sub/**)");
    assert_eq!(
        rules_overlap(&allow, &deny),
        Some(OverlapKind::DenyShadowedByBroaderAllow)
    );
}

#[test]
fn rules_overlap_bash_different_cmd_tokens_no_overlap() {
    let allow = RuleShape::parse("Bash(cp * /foo/**)");
    let deny = RuleShape::parse("Bash(mv * /foo/**)");
    assert_eq!(rules_overlap(&allow, &deny), None);
}
