//! Tests for downward session discovery over temp trees.

use core::time::Duration;
use std::path::Path;

use os_shim::mock::MockSystem;

use super::backend::{ClaudeBackend, SessionBackend as _, resolve_backend};
use super::discovery::{DiscoveredSession, discover_sessions};
use super::manifest::resolve_fleet;
use super::spec::build_launch_spec;

/// Build a `demo-remargin`-shaped tree: a root that declares its own
/// identity and a system prompt, five child realms each declaring their
/// own identity, and a `session:` block on `finance`.
fn demo_tree() -> MockSystem {
    MockSystem::new()
        .with_file(
            Path::new("/demo/.remargin.yaml"),
            b"identity: eburgos_notes_agent\nsystem_prompt:\n  name: root\n  prompt: root body\n",
        )
        .unwrap()
        .with_file(
            Path::new("/demo/audience/.remargin.yaml"),
            b"identity: audience\n",
        )
        .unwrap()
        .with_file(
            Path::new("/demo/content/.remargin.yaml"),
            b"identity: content\n",
        )
        .unwrap()
        .with_file(
            Path::new("/demo/coordinator/.remargin.yaml"),
            b"identity: coordinator\n",
        )
        .unwrap()
        .with_file(
            Path::new("/demo/finance/.remargin.yaml"),
            b"identity: finance\nsession:\n  loop: 30s\n  goal: process pending work\n",
        )
        .unwrap()
        .with_file(Path::new("/demo/ops/.remargin.yaml"), b"identity: ops\n")
        .unwrap()
}

#[test]
fn six_identities_over_demo_shaped_tree() {
    let system = demo_tree();

    let sessions = discover_sessions(&system, Path::new("/demo")).unwrap();

    let ids: Vec<&str> = sessions.iter().map(|s| s.identity.as_str()).collect();
    assert_eq!(
        ids,
        [
            "eburgos_notes_agent",
            "audience",
            "content",
            "coordinator",
            "finance",
            "ops",
        ]
    );

    assert_eq!(sessions[0].folder.as_path(), Path::new("/demo"));
    assert_eq!(sessions[4].folder.as_path(), Path::new("/demo/finance"));
    assert!(sessions[4].session.is_some());
    // Every other realm has no session: block of its own.
    assert!(sessions[1].session.is_none());
}

#[test]
fn each_session_carries_its_resolved_system_prompt() {
    let system = demo_tree();

    let sessions = discover_sessions(&system, Path::new("/demo")).unwrap();

    // The root declares the prompt; children inherit it via the walk-up.
    assert_eq!(sessions[0].system_prompt.name, "root");
    assert_eq!(sessions[0].system_prompt.prompt, "root body");
    assert_eq!(sessions[3].system_prompt.prompt, "root body");
    assert!(!sessions[3].system_prompt.is_default);
}

#[test]
fn inherit_only_subfolder_is_not_emitted() {
    let system = MockSystem::new()
        .with_file(Path::new("/demo/.remargin.yaml"), b"identity: root_agent\n")
        .unwrap()
        .with_file(
            Path::new("/demo/sub/.remargin.yaml"),
            b"system_prompt:\n  name: sub\n  prompt: sub body\n",
        )
        .unwrap();

    let sessions = discover_sessions(&system, Path::new("/demo")).unwrap();

    let ids: Vec<&str> = sessions.iter().map(|s| s.identity.as_str()).collect();
    assert_eq!(ids, ["root_agent"]);
}

#[test]
fn nested_realm_boundary_yields_two_scoped_sessions() {
    let system = MockSystem::new()
        .with_file(Path::new("/tree/a/.remargin.yaml"), b"identity: a_id\n")
        .unwrap()
        .with_file(Path::new("/tree/a/b/.remargin.yaml"), b"identity: b_id\n")
        .unwrap();

    let sessions = discover_sessions(&system, Path::new("/tree")).unwrap();

    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].identity, "a_id");
    assert_eq!(sessions[0].folder.as_path(), Path::new("/tree/a"));
    assert_eq!(sessions[0].scope_root.as_path(), Path::new("/tree/a"));
    assert_eq!(sessions[1].identity, "b_id");
    assert_eq!(sessions[1].folder.as_path(), Path::new("/tree/a/b"));
    assert_eq!(sessions[1].scope_root.as_path(), Path::new("/tree/a/b"));
}

#[test]
fn same_identity_in_sibling_folders_stays_distinct() {
    let system = MockSystem::new()
        .with_file(Path::new("/tree/bar/.remargin.yaml"), b"identity: x\n")
        .unwrap()
        .with_file(Path::new("/tree/foo/.remargin.yaml"), b"identity: x\n")
        .unwrap();

    let sessions = discover_sessions(&system, Path::new("/tree")).unwrap();

    assert_eq!(sessions.len(), 2);
    assert!(sessions.iter().all(|s| s.identity == "x"));
    assert_eq!(sessions[0].folder.as_path(), Path::new("/tree/bar"));
    assert_eq!(sessions[1].folder.as_path(), Path::new("/tree/foo"));
}

#[test]
fn no_identity_anywhere_yields_zero_sessions() {
    let system = MockSystem::new()
        .with_file(Path::new("/tree/.remargin.yaml"), b"mode: open\n")
        .unwrap();

    let sessions = discover_sessions(&system, Path::new("/tree")).unwrap();

    assert!(sessions.is_empty());
}

#[test]
fn root_identity_inherited_from_ancestor_uses_cwd_as_folder() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/vault/.remargin.yaml"),
            b"identity: vault_agent\nsession:\n  loop: 5min\n  goal: x\n",
        )
        .unwrap()
        .with_file(
            Path::new("/vault/proj/.remargin.yaml"),
            b"system_prompt:\n  name: proj\n  prompt: proj body\n",
        )
        .unwrap();

    let sessions = discover_sessions(&system, Path::new("/vault/proj")).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].identity, "vault_agent");
    assert_eq!(sessions[0].folder.as_path(), Path::new("/vault/proj"));
    assert_eq!(sessions[0].scope_root.as_path(), Path::new("/vault/proj"));
    // The session: block travels with the identity's declaring config.
    assert!(sessions[0].session.is_some());
}

#[test]
fn dot_directories_are_skipped() {
    let system = MockSystem::new()
        .with_file(Path::new("/tree/.remargin.yaml"), b"identity: root\n")
        .unwrap()
        .with_file(
            Path::new("/tree/.git/.remargin.yaml"),
            b"identity: should_not_appear\n",
        )
        .unwrap();

    let sessions = discover_sessions(&system, Path::new("/tree")).unwrap();

    let ids: Vec<&str> = sessions.iter().map(|s| s.identity.as_str()).collect();
    assert_eq!(ids, ["root"]);
}

// --- Launch-spec builder (task 84) ---------------------------------------

/// Tree with two launchable realms: `finance` carries a full `session:`
/// block (loop + goal + claude + budget) and its own system prompt; `ops`
/// carries loop + goal only (no claude, no budget).
fn launch_demo_tree() -> MockSystem {
    MockSystem::new()
        .with_file(Path::new("/demo/.remargin.yaml"), b"identity: root_agent\n")
        .unwrap()
        .with_file(
            Path::new("/demo/finance/.remargin.yaml"),
            b"identity: finance\n\
              system_prompt:\n  name: finance\n  prompt: You are the finance agent\n\
              session:\n  loop: 30s\n  \
              goal: 'process pending work; stop when the sandbox is empty'\n  \
              claude:\n    model: claude-opus-4-8\n    effort: high\n  \
              budget:\n    max_turns: 40\n",
        )
        .unwrap()
        .with_file(
            Path::new("/demo/ops/.remargin.yaml"),
            b"identity: ops\n\
              system_prompt:\n  name: ops\n  prompt: You are the ops agent\n\
              session:\n  loop: 1h\n  goal: keep the queue empty\n",
        )
        .unwrap()
}

fn discovered(system: &MockSystem, identity: &str) -> DiscoveredSession {
    discover_sessions(system, Path::new("/demo"))
        .unwrap()
        .into_iter()
        .find(|s| s.identity == identity)
        .unwrap()
}

#[test]
fn build_launch_spec_composes_full_session() {
    let system = launch_demo_tree();
    let finance = discovered(&system, "finance");

    let spec = build_launch_spec(&finance).unwrap();

    assert_eq!(spec.identity, "finance");
    assert_eq!(spec.cwd.as_path(), Path::new("/demo/finance"));
    assert_eq!(spec.loop_interval, Duration::from_secs(30));
    assert_eq!(
        spec.goal,
        "process pending work; stop when the sandbox is empty"
    );
    assert!(spec.prompt.contains("You are the finance agent"));
    assert!(spec.prompt.contains("/loop 30s"));
    assert!(
        spec.prompt
            .contains("/goal process pending work; stop when the sandbox is empty")
    );
    assert!(spec.prompt.contains("Remargin operating rules"));
    assert_eq!(spec.model.as_deref(), Some("claude-opus-4-8"));
    assert_eq!(spec.effort.as_deref(), Some("high"));
    assert_eq!(spec.budget.as_ref().unwrap().max_turns, Some(40));
}

#[test]
fn build_launch_spec_missing_loop_defaults_to_5m() {
    let system = launch_demo_tree();
    let mut goal_only = discovered(&system, "finance");
    goal_only.session.as_mut().unwrap().loop_interval = None;

    let spec = build_launch_spec(&goal_only).unwrap();

    assert_eq!(spec.loop_interval, Duration::from_mins(5));
    assert!(
        spec.prompt.contains("/loop 5m"),
        "defaulted cadence framed as 5m: {}",
        spec.prompt
    );
}

#[test]
fn build_launch_spec_bad_loop_is_hard_error() {
    let system = launch_demo_tree();
    let mut broken = discovered(&system, "finance");
    broken.session.as_mut().unwrap().loop_interval = Some("not-a-duration".to_owned());

    let err = build_launch_spec(&broken).unwrap_err().to_string();

    assert!(err.contains("finance"), "error names the identity: {err}");
    assert!(
        err.contains("bad `loop`"),
        "error flags the bad value: {err}"
    );
}

#[test]
fn build_launch_spec_missing_goal_is_hard_error() {
    let system = launch_demo_tree();
    let mut broken = discovered(&system, "finance");
    broken.session.as_mut().unwrap().goal = None;

    let err = build_launch_spec(&broken).unwrap_err().to_string();

    assert!(err.contains("finance"), "error names the identity: {err}");
    assert!(
        err.contains("`goal` is required"),
        "error names goal: {err}"
    );
}

#[test]
fn build_launch_spec_missing_session_block_is_hard_error() {
    let system = launch_demo_tree();
    let root = discovered(&system, "root_agent");
    assert!(root.session.is_none());

    let err = build_launch_spec(&root).unwrap_err().to_string();

    assert!(
        err.contains("root_agent"),
        "error names the identity: {err}"
    );
    assert!(
        err.contains("`goal` is required"),
        "an absent block is an empty block: goal is the one hard requirement: {err}"
    );
}

#[test]
fn build_launch_spec_without_budget_or_claude_has_no_caps() {
    let system = launch_demo_tree();
    let ops = discovered(&system, "ops");

    let spec = build_launch_spec(&ops).unwrap();

    assert!(spec.budget.is_none());
    assert!(spec.model.is_none());
    assert!(spec.effort.is_none());
    assert_eq!(spec.loop_interval, Duration::from_hours(1));
}

#[test]
fn build_launch_spec_infers_claude_backend_from_declared_block() {
    let system = launch_demo_tree();
    let spec = build_launch_spec(&discovered(&system, "finance")).unwrap();

    assert_eq!(spec.backend, "claude");
}

#[test]
fn build_launch_spec_infers_claude_backend_without_declared_block() {
    // `ops` carries a `session:` block but no `claude:` params block: a
    // declared block and no block at all both infer the claude backend.
    let system = launch_demo_tree();
    let spec = build_launch_spec(&discovered(&system, "ops")).unwrap();

    assert_eq!(spec.backend, "claude");
}

#[test]
fn mcp_server_spec_scopes_to_cwd_and_identity() {
    let system = launch_demo_tree();
    let finance = discovered(&system, "finance");

    let spec = build_launch_spec(&finance).unwrap();

    assert_eq!(spec.mcp.base_dir, spec.cwd);
    assert_eq!(spec.mcp.base_dir.as_path(), Path::new("/demo/finance"));
    assert_eq!(spec.mcp.identity, "finance");
    assert_eq!(spec.mcp.argv, ["remargin", "mcp"]);
}

// --- Claude backend (task 85) --------------------------------------------

/// The argv value immediately following `flag`, if present.
fn flag_value<'argv>(argv: &'argv [String], flag: &str) -> Option<&'argv str> {
    argv.iter()
        .position(|arg| arg == flag)
        .and_then(|index| argv.get(index + 1))
        .map(String::as_str)
}

#[test]
fn claude_launch_command_uses_task81_invocation() {
    let system = launch_demo_tree();
    let spec = build_launch_spec(&discovered(&system, "finance")).unwrap();

    let argv = ClaudeBackend.launch_command(&spec).unwrap();

    assert_eq!(argv.first().map(String::as_str), Some("claude"));
    assert_eq!(
        flag_value(&argv, "--append-system-prompt"),
        Some(spec.prompt.as_str())
    );
    assert!(argv.iter().any(|arg| arg == "--strict-mcp-config"));
    assert_eq!(flag_value(&argv, "--model"), Some("claude-opus-4-8"));
    assert_eq!(flag_value(&argv, "--effort"), Some("high"));
    assert_eq!(flag_value(&argv, "-n"), Some("finance"));
    assert_eq!(flag_value(&argv, "--permission-mode"), Some("auto"));
    // Interactive launch only -- never headless `claude -p`/`--print`.
    assert!(!argv.iter().any(|arg| arg == "-p" || arg == "--print"));
    // Budget has no interactive claude flag; none is invented.
    assert!(
        !argv
            .iter()
            .any(|arg| arg == "--max-turns" || arg == "--max-budget-usd")
    );
}

#[test]
fn claude_launch_command_carries_scoped_mcp_config() {
    let system = launch_demo_tree();
    let spec = build_launch_spec(&discovered(&system, "finance")).unwrap();

    let argv = ClaudeBackend.launch_command(&spec).unwrap();
    let mcp = flag_value(&argv, "--mcp-config").unwrap();
    let parsed: serde_json::Value = serde_json::from_str(mcp).unwrap();

    assert_eq!(parsed["mcpServers"]["remargin"]["command"], "remargin");
    assert_eq!(parsed["mcpServers"]["remargin"]["args"][0], "mcp");
}

#[test]
fn claude_launch_command_omits_model_and_effort_when_absent() {
    let system = launch_demo_tree();
    let spec = build_launch_spec(&discovered(&system, "ops")).unwrap();

    let argv = ClaudeBackend.launch_command(&spec).unwrap();

    assert!(!argv.iter().any(|arg| arg == "--model"));
    assert!(!argv.iter().any(|arg| arg == "--effort"));
    assert_eq!(flag_value(&argv, "-n"), Some("ops"));
}

#[test]
fn claude_seed_inputs_fold_max_turns_into_goal() {
    let system = launch_demo_tree();
    let spec = build_launch_spec(&discovered(&system, "finance")).unwrap();

    let seeds = ClaudeBackend.seed_inputs(&spec);

    assert_eq!(
        seeds,
        [
            "/loop 30s".to_owned(),
            "/goal process pending work; stop when the sandbox is empty \
             or stop after 40 turns"
                .to_owned(),
        ]
    );
}

#[test]
fn claude_seed_inputs_without_budget_is_plain_goal() {
    let system = launch_demo_tree();
    let spec = build_launch_spec(&discovered(&system, "ops")).unwrap();

    let seeds = ClaudeBackend.seed_inputs(&spec);

    assert_eq!(
        seeds,
        [
            "/loop 1h".to_owned(),
            "/goal keep the queue empty".to_owned()
        ]
    );
}

#[test]
fn claude_seed_inputs_uses_5m_default_when_loop_absent() {
    let system = launch_demo_tree();
    let mut goal_only = discovered(&system, "ops");
    goal_only.session.as_mut().unwrap().loop_interval = None;

    let spec = build_launch_spec(&goal_only).unwrap();
    let seeds = ClaudeBackend.seed_inputs(&spec);

    assert_eq!(seeds[0], "/loop 5m");
}

#[test]
fn resolve_backend_known_and_unknown() {
    assert_eq!(resolve_backend("claude").unwrap().name(), "claude");

    let err = resolve_backend("bogus").err().unwrap().to_string();
    assert!(err.contains("bogus"), "names the offender: {err}");
    assert!(err.contains("claude"), "lists known backends: {err}");
}

// --- Manifest fleet resolution (task 93) ---------------------------------

/// Workspace whose `.remargin.yaml` carries only a `sessions:` block (no
/// identity of its own, so downward discovery from `/ws` is empty) plus two
/// out-of-tree agent folders that entries point at by absolute path.
fn manifest_workspace(root_yaml: &str) -> MockSystem {
    MockSystem::new()
        .with_file(Path::new("/ws/.remargin.yaml"), root_yaml.as_bytes())
        .unwrap()
        .with_file(
            Path::new("/agents/a/.remargin.yaml"),
            b"identity: a_id\nsession:\n  goal: base a goal\n  loop: 9s\n",
        )
        .unwrap()
        .with_file(Path::new("/agents/b/.remargin.yaml"), b"identity: b_id\n")
        .unwrap()
}

fn identities(fleet: &[DiscoveredSession]) -> Vec<&str> {
    fleet.iter().map(|s| s.identity.as_str()).collect()
}

#[test]
fn resolve_fleet_without_manifest_matches_discovery() {
    let system = demo_tree();

    let via_fleet = resolve_fleet(&system, Path::new("/demo"), None).unwrap();
    let via_discovery = discover_sessions(&system, Path::new("/demo")).unwrap();

    let project = |fleet: &[DiscoveredSession]| {
        fleet
            .iter()
            .map(|s| (s.identity.clone(), s.folder.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(project(&via_fleet), project(&via_discovery));
    // The finance realm's session: block still rides along untouched.
    assert!(via_fleet[4].session.is_some());
}

#[test]
fn resolve_fleet_named_without_manifest_errors() {
    let system = demo_tree();

    let err = resolve_fleet(&system, Path::new("/demo"), Some("ghost"))
        .unwrap_err()
        .to_string();

    assert!(err.contains("ghost"), "names the requested session: {err}");
}

#[test]
fn select_default_session_when_declared() {
    let system = manifest_workspace(
        "sessions:\n  default: alpha\n  \
         alpha:\n    agents:\n      - path: /agents/a\n  \
         beta:\n    agents:\n      - path: /agents/b\n",
    );

    let fleet = resolve_fleet(&system, Path::new("/ws"), None).unwrap();

    assert_eq!(identities(&fleet), ["a_id"]);
}

#[test]
fn select_single_session_when_no_default() {
    let system = manifest_workspace("sessions:\n  alpha:\n    agents:\n      - path: /agents/a\n");

    let fleet = resolve_fleet(&system, Path::new("/ws"), None).unwrap();

    assert_eq!(identities(&fleet), ["a_id"]);
}

#[test]
fn select_ambiguous_without_default_errors() {
    let system = manifest_workspace(
        "sessions:\n  \
         alpha:\n    agents:\n      - path: /agents/a\n  \
         beta:\n    agents:\n      - path: /agents/b\n",
    );

    let err = resolve_fleet(&system, Path::new("/ws"), None)
        .unwrap_err()
        .to_string();

    assert!(err.contains("alpha"), "lists alpha: {err}");
    assert!(err.contains("beta"), "lists beta: {err}");
}

#[test]
fn unknown_requested_name_errors_listing_defined() {
    let system = manifest_workspace(
        "sessions:\n  \
         alpha:\n    agents:\n      - path: /agents/a\n  \
         beta:\n    agents:\n      - path: /agents/b\n",
    );

    let err = resolve_fleet(&system, Path::new("/ws"), Some("gamma"))
        .unwrap_err()
        .to_string();

    assert!(err.contains("gamma"), "names the unknown request: {err}");
    assert!(
        err.contains("alpha") && err.contains("beta"),
        "lists the defined names: {err}"
    );
}

#[test]
fn relative_and_tilde_paths_resolve_against_manifest_dir() {
    let system = MockSystem::new()
        .with_env("HOME", "/home/user")
        .unwrap()
        .with_file(
            Path::new("/ws/.remargin.yaml"),
            b"sessions:\n  main:\n    agents:\n      \
              - path: ./product\n      - path: ~/agents/r\n",
        )
        .unwrap()
        .with_file(
            Path::new("/ws/product/.remargin.yaml"),
            b"identity: product\n",
        )
        .unwrap()
        .with_file(
            Path::new("/home/user/agents/r/.remargin.yaml"),
            b"identity: r_id\n",
        )
        .unwrap()
        .with_dir(Path::new("/ws/deep/sub"))
        .unwrap();

    // Launch from a subfolder: paths must anchor on the manifest dir (/ws),
    // never on cwd.
    let fleet = resolve_fleet(&system, Path::new("/ws/deep/sub"), None).unwrap();

    assert_eq!(identities(&fleet), ["product", "r_id"]);
    assert_eq!(fleet[0].folder.as_path(), Path::new("/ws/product"));
    assert_eq!(fleet[1].folder.as_path(), Path::new("/home/user/agents/r"));
}

#[test]
fn missing_entry_folder_is_hard_error() {
    let system =
        manifest_workspace("sessions:\n  main:\n    agents:\n      - path: /agents/ghost\n");

    let err = resolve_fleet(&system, Path::new("/ws"), None)
        .unwrap_err()
        .to_string();

    assert!(err.contains("main"), "names the session: {err}");
    assert!(err.contains("/agents/ghost"), "names the entry path: {err}");
}

#[test]
fn entry_folder_without_config_is_hard_error() {
    let system =
        manifest_workspace("sessions:\n  main:\n    agents:\n      - path: /agents/empty\n")
            .with_dir(Path::new("/agents/empty"))
            .unwrap();

    let err = resolve_fleet(&system, Path::new("/ws"), None)
        .unwrap_err()
        .to_string();

    assert!(err.contains("main"), "names the session: {err}");
    assert!(err.contains("/agents/empty"), "names the entry path: {err}");
}

#[test]
fn entry_config_without_identity_is_hard_error() {
    let system =
        manifest_workspace("sessions:\n  main:\n    agents:\n      - path: /agents/noident\n")
            .with_file(Path::new("/agents/noident/.remargin.yaml"), b"mode: open\n")
            .unwrap();

    let err = resolve_fleet(&system, Path::new("/ws"), None)
        .unwrap_err()
        .to_string();

    assert!(err.contains("main"), "names the session: {err}");
    assert!(
        err.contains("/agents/noident"),
        "names the entry path: {err}"
    );
    assert!(err.contains("identity"), "names the missing field: {err}");
}

#[test]
fn bad_entry_yields_no_partial_fleet() {
    // A good entry precedes a broken one; the whole resolution must fail
    // rather than return the good half.
    let system = manifest_workspace(
        "sessions:\n  mixed:\n    agents:\n      \
         - path: /agents/a\n      - path: /agents/ghost\n",
    );

    let result = resolve_fleet(&system, Path::new("/ws"), None);

    assert!(
        result.is_err(),
        "all-or-nothing: one bad entry fails the fleet"
    );
}

#[test]
fn entry_overrides_win_per_field() {
    // Target declares goal + loop + claude; entry overrides goal and adds a
    // budget. Each field replaces as a whole value, entry wins.
    let system = MockSystem::new()
        .with_file(
            Path::new("/ws/.remargin.yaml"),
            b"sessions:\n  main:\n    agents:\n      \
              - path: /agents/a\n        goal: entry goal\n        \
              budget:\n          max_turns: 5\n",
        )
        .unwrap()
        .with_file(
            Path::new("/agents/a/.remargin.yaml"),
            b"identity: a_id\nsession:\n  goal: base goal\n  loop: 30s\n  \
              claude:\n    model: base-model\n",
        )
        .unwrap();

    let fleet = resolve_fleet(&system, Path::new("/ws"), None).unwrap();
    let session = fleet[0].session.as_ref().unwrap();

    assert_eq!(session.goal.as_deref(), Some("entry goal"));
    assert_eq!(session.loop_interval.as_deref(), Some("30s"));
    assert_eq!(
        session.claude.as_ref().unwrap().model.as_deref(),
        Some("base-model")
    );
    assert_eq!(session.budget.as_ref().unwrap().max_turns, Some(5));
}

#[test]
fn union_dedups_by_identity_and_folder_entry_wins() {
    // The entry points at a folder discovery also finds, overriding its loop.
    let system = MockSystem::new()
        .with_file(
            Path::new("/ws/.remargin.yaml"),
            b"identity: ws_root\nsessions:\n  main:\n    agents:\n      \
              - path: ./child\n        loop: 99s\n",
        )
        .unwrap()
        .with_file(
            Path::new("/ws/child/.remargin.yaml"),
            b"identity: child_id\nsession:\n  goal: child goal\n  loop: 1s\n",
        )
        .unwrap();

    let fleet = resolve_fleet(&system, Path::new("/ws"), None).unwrap();

    let children: Vec<&DiscoveredSession> =
        fleet.iter().filter(|s| s.identity == "child_id").collect();
    assert_eq!(children.len(), 1, "deduped by (identity, folder)");
    assert_eq!(
        children[0]
            .session
            .as_ref()
            .unwrap()
            .loop_interval
            .as_deref(),
        Some("99s"),
        "the entry's override survives the dedup"
    );
    assert_eq!(fleet[0].identity, "child_id", "entries precede discovery");
    assert!(
        identities(&fleet).contains(&"ws_root"),
        "root discovery is unioned in: {:?}",
        identities(&fleet)
    );
    assert_eq!(fleet.len(), 2);
}

#[test]
fn entry_without_overrides_passes_target_session_through() {
    let system = MockSystem::new()
        .with_file(
            Path::new("/ws/.remargin.yaml"),
            b"sessions:\n  main:\n    agents:\n      - path: /agents/full\n",
        )
        .unwrap()
        .with_file(
            Path::new("/agents/full/.remargin.yaml"),
            b"identity: full_id\nsession:\n  goal: keep it\n  loop: 7s\n  \
              claude:\n    model: cm\n    effort: high\n  \
              budget:\n    max_turns: 3\n",
        )
        .unwrap();

    let fleet = resolve_fleet(&system, Path::new("/ws"), None).unwrap();
    let session = fleet[0].session.as_ref().unwrap();

    assert_eq!(session.goal.as_deref(), Some("keep it"));
    assert_eq!(session.loop_interval.as_deref(), Some("7s"));
    let claude = session.claude.as_ref().unwrap();
    assert_eq!(claude.model.as_deref(), Some("cm"));
    assert_eq!(claude.effort.as_deref(), Some("high"));
    assert_eq!(session.budget.as_ref().unwrap().max_turns, Some(3));
}
